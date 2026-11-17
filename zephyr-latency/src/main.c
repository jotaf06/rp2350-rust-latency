/*
 * Zephyr port of the latency measurement. Same method, pins and output
 * protocol as the Rust firmware (src/measure.rs, src/scenario.rs,
 * src/report.rs), so tools/analyze.py reads it unchanged.
 *
 * PWM slice 0 free-runs and is both the event source and the clock: its wrap
 * raises PWM_IRQ_WRAP(_0), and the counter restarts at the event, so reading
 * it gives the time since the event in clk_sys ticks. Three reads per event:
 *   t_isr_start  first useful instruction of the ISR     -> irq_lat
 *   t_isr_end    last instruction of the ISR             -> isr_dur
 *   t_task       first instruction of the woken thread   -> sched_lat, total
 *
 * The ISR wakes the measured thread with k_sem_give(), the usual Zephyr
 * pattern. Two variants (CONFIG_LAT_COOP):
 *   preempt  measured thread preemptive, higher priority than the load
 *            (counterpart of RTIC / Embassy InterruptExecutor)
 *   coop     measured thread and load cooperative, same priority
 *            (counterpart of Embassy's thread executor)
 *
 * One binary runs the idle case and the three loads (L = 0, 10, 100,
 * 1000 us), like the Rust rtic_latency binary.
 *
 * Pins: GPIO0 PWM output (rises at the event), GPIO2 high during the ISR,
 * GPIO3 toggles when the measured thread runs, GPIO25 LED during a batch.
 */

#include <zephyr/kernel.h>
#include <zephyr/device.h>
#include <zephyr/devicetree.h>
#include <zephyr/drivers/gpio.h>
#include <zephyr/drivers/uart.h>
#include <zephyr/irq.h>
#include <zephyr/sys/printk.h>
#include <zephyr/version.h>

#include <hardware/gpio.h>
#include <hardware/resets.h>
#include <hardware/structs/pwm.h>
#include <hardware/structs/sio.h>
#include <pico/bootrom.h>

/* ------------------------------------------------------------------ chip */

#if defined(CONFIG_SOC_SERIES_RP2040)
#define CHIP_NAME "rp2040"
#define EVENT_IRQ 4 /* PWM_IRQ_WRAP */
#elif defined(CONFIG_SOC_SERIES_RP2350)
#define CHIP_NAME "rp2350"
#define EVENT_IRQ 8 /* PWM_IRQ_WRAP_0 */
#else
#error "unsupported SoC"
#endif

#define CLK_SYS_HZ DT_PROP(DT_NODELABEL(clk_sys), clock_frequency)

#define SLICE         0
#define N_SAMPLES     2000
#define WARMUP        50
#define PIN_PWM       0
#define PIN_ISR_MARK  2
#define PIN_TASK_MARK 3
#define PIN_LED       25

#define EVENT_IRQ_PRIO 0 /* most urgent; USB and timer keep their defaults */

#if defined(CONFIG_LAT_COOP)
#define EXEC_NAME     "zephyr_coop"
#define PRIO_MEASURED K_PRIO_COOP(2)
#define PRIO_LOAD     K_PRIO_COOP(2)
#else
#define EXEC_NAME     "zephyr_preempt"
#define PRIO_MEASURED K_PRIO_PREEMPT(1)
#define PRIO_LOAD     K_PRIO_PREEMPT(10)
#endif

/* ------------------------------------------------------------- batches */

struct batch {
	uint32_t period_us; /* > 2x the longest latency, coprime with L */
	uint32_t load_us;   /* 0 = no load */
};

/* Same as IDLE_AND_LOAD_BATCHES in src/scenario.rs. */
static const struct batch batches[] = {
	{103, 0},
	{103, 10},
	{257, 100},
	{2503, 1000},
};

struct pwm_profile {
	uint32_t div;
	uint32_t top;
};

/* Smallest integer divider that fits the period in the 16-bit counter. */
static struct pwm_profile pwm_profile_for(uint32_t period_us)
{
	uint32_t cycles = (uint32_t)((uint64_t)period_us * CLK_SYS_HZ / 1000000u);
	uint32_t div = CLAMP((cycles + 65535u) / 65536u, 1u, 255u);
	uint32_t top = CLAMP(cycles / div, 2u, 65536u) - 1u;

	return (struct pwm_profile){.div = div, .top = top};
}

/* ------------------------------------------------------------ shared state */

struct sample {
	uint16_t t_isr_start;
	uint16_t t_isr_end;
	uint16_t t_task;
};

static struct sample buf[N_SAMPLES]; /* written only by the measured thread */

static volatile uint32_t seq;          /* written only by the ISR */
static volatile uint32_t ev_t_isr_start;
static volatile uint32_t ev_seq;
static volatile uint32_t t_isr_end;
static volatile bool collecting;
static volatile uint32_t len;
static volatile uint32_t discarded;
static volatile uint32_t last_seq;
static volatile uint32_t guard_ticks;
static volatile uint32_t load_us;

K_SEM_DEFINE(event_sem, 0, 1);  /* ISR -> measured thread */
K_SEM_DEFINE(batch_done, 0, 1); /* measured thread -> batch loop */
K_SEM_DEFINE(load_gate, 0, 1);  /* wakes the parked load */

/* ------------------------------------------------------------- hot path */

static ALWAYS_INLINE uint32_t now_ticks(void)
{
	return pwm_hw->slice[SLICE].ctr;
}

static void event_isr(const void *arg)
{
	ARG_UNUSED(arg);

	uint32_t t0 = now_ticks(); /* <- measurement point 1 */

	sio_hw->gpio_set = BIT(PIN_ISR_MARK);
	pwm_hw->intr = BIT(SLICE); /* write 1 to clear */
	uint32_t s = seq + 1u;

	seq = s;
	ev_t_isr_start = t0;
	ev_seq = s;
	k_sem_give(&event_sem);
	t_isr_end = now_ticks(); /* <- measurement point 2 */
	sio_hw->gpio_clr = BIT(PIN_ISR_MARK);
}

static void pwm_irq_enable(bool on)
{
	/* `inte` is INTE on the RP2040 and IRQ0_INTE on the RP2350. */
	pwm_hw->inte = on ? BIT(SLICE) : 0u;
}

/* Same discard rules as measure::record() in the Rust firmware. */
static void record(uint32_t t_task)
{
	if (!collecting) {
		return;
	}

	unsigned int key = irq_lock(); /* consistent snapshot; after t_task */
	uint32_t t0 = ev_t_isr_start;
	uint32_t s = ev_seq;
	uint32_t t1 = t_isr_end;
	bool fresh = (seq == s);

	irq_unlock(key);

	/* No new event since the last record: a wake-up without an event.
	 * k_sem_reset() in batch_start() releases the waiting thread with
	 * -EAGAIN; in coop mode it runs only after collecting is set. */
	if (s == last_seq) {
		return;
	}

	uint32_t skipped = s - last_seq - 1u;

	last_seq = s;
	if (skipped != 0u) {
		discarded += skipped;
	}

	if (!fresh || t_task < t1 || t1 < t0 || t_task >= guard_ticks) {
		discarded++;
		return;
	}

	uint32_t i = len;

	if (i < N_SAMPLES) {
		buf[i] = (struct sample){(uint16_t)t0, (uint16_t)t1, (uint16_t)t_task};
		len = i + 1u;
	}
	if (i + 1u >= N_SAMPLES) {
		collecting = false;
		pwm_irq_enable(false);
		load_us = 0; /* park the load: in coop mode main cannot run otherwise */
		k_sem_give(&batch_done);
	}
}

static void measured_thread(void *a, void *b, void *c)
{
	ARG_UNUSED(a);
	ARG_UNUSED(b);
	ARG_UNUSED(c);

	for (;;) {
		k_sem_take(&event_sem, K_FOREVER);
		uint32_t t_task = now_ticks(); /* <- measurement point 3 */

		sio_hw->gpio_togl = BIT(PIN_TASK_MARK);
		record(t_task);
	}
}

/* CPU-bound stretch of L us without blocking, then one yield, forever.
 * k_busy_wait() is timed against the system timer, so L is wall-clock time. */
static void load_thread(void *a, void *b, void *c)
{
	ARG_UNUSED(a);
	ARG_UNUSED(b);
	ARG_UNUSED(c);

	for (;;) {
		uint32_t l = load_us;

		if (l == 0u) {
			k_sem_take(&load_gate, K_FOREVER);
			continue;
		}
		k_busy_wait(l);
		k_yield();
	}
}

K_THREAD_DEFINE(measured_tid, 1024, measured_thread, NULL, NULL, NULL, PRIO_MEASURED, 0, 0);
K_THREAD_DEFINE(load_tid, 1024, load_thread, NULL, NULL, NULL, PRIO_LOAD, 0, 0);

static void set_load_us(uint32_t us)
{
	load_us = us;
	if (us != 0u) {
		k_sem_give(&load_gate);
	}
}

/* ------------------------------------------------------- batch control */

static void batch_stop(void)
{
	collecting = false;
	pwm_irq_enable(false);
	pwm_hw->slice[SLICE].csr = 0;
	pwm_hw->intr = BIT(SLICE);
}

/* Program the stopped slice (divider, TOP, 50 % duty on A) and start it. */
static void batch_start(struct pwm_profile p)
{
	batch_stop();
	pwm_hw->slice[SLICE].div = p.div << PWM_CH0_DIV_INT_LSB;
	pwm_hw->slice[SLICE].top = p.top;
	pwm_hw->slice[SLICE].cc = MAX(p.top / 2u, 1u); /* channel A */
	pwm_hw->slice[SLICE].ctr = 0;
	pwm_hw->intr = BIT(SLICE);

	uint32_t period = p.top + 1u;

	guard_ticks = period - period / 8u;
	len = 0;
	discarded = 0;
	last_seq = seq;
	k_sem_reset(&event_sem);
	k_sem_reset(&batch_done);
	collecting = true;
	pwm_irq_enable(true);
	pwm_hw->slice[SLICE].csr = PWM_CH0_CSR_EN_BITS;
}

/* ------------------------------------------------------------- output */

static const struct device *const uart = DEVICE_DT_GET_ONE(zephyr_cdc_acm_uart);

static uint32_t line_ctrl(uint32_t what)
{
	uint32_t v = 0;

	(void)uart_line_ctrl_get(uart, what, &v);
	return v;
}

/* Arduino-style "1200 baud touch": reboot into BOOTSEL (tools/flash.sh). */
static void reboot_if_requested(void)
{
	if (line_ctrl(UART_LINE_CTRL_BAUD_RATE) == 1200u) {
		k_msleep(50); /* let the USB stack finish the control transfer */
		reset_usb_boot(0, 0);
	}
}

static void put(const char *s, int n)
{
	for (int i = 0; i < n; i++) {
		uart_poll_out(uart, (unsigned char)s[i]);
	}
}

/* Diagnostics, only in builds with -DEXTRA_CFLAGS=-DLAT_DEBUG. */
#ifdef LAT_DEBUG
#define DBG(...)                                                                                   \
	do {                                                                                       \
		char _l[160];                                                                      \
		int _n = snprintk(_l, sizeof(_l), "# " __VA_ARGS__);                               \
		if (line_ctrl(UART_LINE_CTRL_DTR)) {                                               \
			put(_l, _n);                                                               \
		}                                                                                  \
	} while (0)
#else
#define DBG(...) do { } while (0)
#endif

static void send_batch(const struct batch *b, struct pwm_profile p, uint32_t batch_no)
{
	char line[256];
	int n;

	n = snprintk(line, sizeof(line),
		     "META,chip=%s,bin=%s,exec=%s,rtos=zephyr-%s,profile=%s,opt=%s,"
		     "clk_sys_hz=%u,pwm_div=%u,pwm_top=%u,load_us=%u,ntasks=0,n=%u,"
		     "discarded=%u,warmup=%u,batch=%u,git=unknown\r\n",
		     CHIP_NAME, EXEC_NAME, EXEC_NAME, KERNEL_VERSION_STRING,
		     CONFIG_LAT_PROFILE, CONFIG_LAT_OPT, (unsigned int)CLK_SYS_HZ,
		     (unsigned int)p.div, (unsigned int)p.top, (unsigned int)b->load_us,
		     (unsigned int)len, (unsigned int)discarded, WARMUP,
		     (unsigned int)batch_no);
	put(line, n);

	for (uint32_t i = 0; i < len; i++) {
		uint32_t t0 = buf[i].t_isr_start, t1 = buf[i].t_isr_end, t2 = buf[i].t_task;

		n = snprintk(line, sizeof(line), "SAMPLE,%u,%u,%u,%u,%u,%u\r\n",
			     (unsigned int)i, (unsigned int)t0, (unsigned int)(t1 - t0),
			     (unsigned int)(t2 - t1), (unsigned int)t2, i < WARMUP ? 1u : 0u);
		put(line, n);
	}
	put("DONE\r\n", 6);
}

/* ---------------------------------------------------------------- main */

#ifdef LAT_DEBUG
/* Debug builds: a fatal error reboots straight into BOOTSEL, so a crash is
 * visible from the host (the board shows up as 2e8a:000f) and re-flashable. */
void k_sys_fatal_error_handler(unsigned int reason, const struct arch_esf *esf)
{
	ARG_UNUSED(reason);
	ARG_UNUSED(esf);
	reset_usb_boot(0, 0);
}
#endif

static void hw_init(void)
{
	const struct device *gpio0 = DEVICE_DT_GET(DT_NODELABEL(gpio0));

	DBG("hw_init: gpio\r\n");
	gpio_pin_configure(gpio0, PIN_ISR_MARK, GPIO_OUTPUT_INACTIVE);
	gpio_pin_configure(gpio0, PIN_TASK_MARK, GPIO_OUTPUT_INACTIVE);
	gpio_pin_configure(gpio0, PIN_LED, GPIO_OUTPUT_INACTIVE);

	DBG("hw_init: pwm unreset\r\n");
	unreset_block_num_wait_blocking(RESET_PWM);
	batch_stop();
	DBG("hw_init: pwm pin\r\n");
	gpio_set_function(PIN_PWM, GPIO_FUNC_PWM);

	DBG("hw_init: irq %d\r\n", EVENT_IRQ);
	IRQ_CONNECT(EVENT_IRQ, EVENT_IRQ_PRIO, event_isr, NULL, 0);
	irq_enable(EVENT_IRQ); /* events are gated by the slice's INTE bit */
	DBG("hw_init: done\r\n");
}

int main(void)
{
	/* The batch loop shares the load's priority, like the Rust firmware
	 * (same executor / same RTIC priority): the load never blocks, and
	 * k_yield() only hands the CPU to threads of equal or higher priority,
	 * so a lower-priority main would starve as soon as a load starts. */
	k_thread_priority_set(k_current_get(), PRIO_LOAD);

#ifdef LAT_DEBUG
	while (!line_ctrl(UART_LINE_CTRL_DTR)) {
		reboot_if_requested();
		k_msleep(100);
	}
	k_msleep(200);
#endif
	hw_init();
	DBG("boot chip=%s exec=%s clk=%u\r\n", CHIP_NAME, EXEC_NAME, (unsigned int)CLK_SYS_HZ);

	uint32_t batch_no = 0;

	for (;;) {
		for (size_t k = 0; k < ARRAY_SIZE(batches); k++) {
			const struct batch *b = &batches[k];
			struct pwm_profile p = pwm_profile_for(b->period_us);
			bool ok;

			/* Collect. If the host touched the port meanwhile (USB
			 * traffic disturbs the measurement), redo the batch. */
			do {
				uint32_t dtr = line_ctrl(UART_LINE_CTRL_DTR);
				uint32_t baud = line_ctrl(UART_LINE_CTRL_BAUD_RATE);

				set_load_us(b->load_us);
				k_msleep(50); /* let the load reach steady state */

				sio_hw->gpio_set = BIT(PIN_LED);
				DBG("batch_start L=%u div=%u top=%u\r\n", (unsigned int)b->load_us,
				    (unsigned int)p.div, (unsigned int)p.top);
#ifdef LAT_DEBUG
				k_msleep(20); /* let the line above leave before the batch */
#endif
				batch_start(p);
				ok = k_sem_take(&batch_done, K_SECONDS(20)) == 0;
				batch_stop();
				DBG("batch L=%u ok=%d len=%u disc=%u seq=%u\r\n", (unsigned int)b->load_us,
				    ok, (unsigned int)len, (unsigned int)discarded, (unsigned int)seq);
				set_load_us(0);
				sio_hw->gpio_clr = BIT(PIN_LED);

				reboot_if_requested();
				if (dtr != line_ctrl(UART_LINE_CTRL_DTR) ||
				    baud != line_ctrl(UART_LINE_CTRL_BAUD_RATE)) {
					ok = false;
					continue;
				}
				ok = true; /* a timed-out batch is reported as is */
			} while (!ok);

			if (line_ctrl(UART_LINE_CTRL_DTR)) {
				send_batch(b, p, batch_no);
			}
			batch_no++;

			/* Pause, still answering the "reboot to BOOTSEL" request. */
			for (int i = 0; i < 40; i++) {
				reboot_if_requested();
				k_msleep(50);
			}
		}
	}
	return 0;
}
