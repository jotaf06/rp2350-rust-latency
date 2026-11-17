//! Baseline: the same event, ISR instrumentation and clock, but **no
//! executor**. Plain `cortex-m-rt` entry point; the "task" is the main loop
//! polling a value written by the ISR.
//!
//! This gives the hardware floor for the interrupt latency, and the cheapest
//! possible "scheduling" (exception return + one loop iteration).
//!
//! embassy-rp is still used as HAL (clock setup, USB driver), and the USB
//! output is driven by a busy-polling `block_on` between batches.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

use embassy_futures::join::join;
use embassy_futures::yield_now;
use embassy_time::{Duration, Instant};
use panic_halt as _;
use rp2350_rust_latency::measure::{self, PwmProfile};
use rp2350_rust_latency::report::Meta;
use rp2350_rust_latency::{board, report, scenario};

static EVENT_T_ISR_START: AtomicU32 = AtomicU32::new(0);
static EVENT_SEQ: AtomicU32 = AtomicU32::new(0);

// The event: PWM slice 0 wrapped. Same instrumentation as the Embassy
// scenarios, but "signalling the task" is just two atomic stores.
rp2350_rust_latency::event_irq! {
    let t_isr_start = measure::isr_begin();
    EVENT_T_ISR_START.store(t_isr_start, Relaxed);
    EVENT_SEQ.store(measure::isr_next_seq(), Relaxed);
    measure::isr_end();
}

/// Collect one batch by polling. Nothing else runs meanwhile (USB included).
fn collect(profile: PwmProfile) {
    let mut last = EVENT_SEQ.load(Relaxed);
    let mut spins = 0u32;
    measure::batch_start(profile);
    while measure::collecting() && spins < 400_000_000 {
        let seq = EVENT_SEQ.load(Relaxed);
        if seq != last {
            let t_task = measure::task_begin(); // first instruction of the "task"
            measure::record(EVENT_T_ISR_START.load(Relaxed), seq, t_task);
            last = seq;
        }
        spins += 1;
    }
    measure::batch_stop();
}

#[cortex_m_rt::entry]
fn main() -> ! {
    let mut b = board::init();
    let (mut usb, mut serial) = report::usb_init(b.usb);
    let profile = PwmProfile::for_period_us(scenario::IDLE_PERIOD_US, b.clk_sys_hz);

    let batches = async {
        let mut batch_no = 0u32;
        loop {
            // Send only batches during which the port was already open: opening
            // it causes USB interrupts that would disturb the polling loop.
            let was_connected = serial.connected();
            b.led.set_high();
            collect(profile);
            b.led.set_low();

            if was_connected && serial.connected() {
                let meta = Meta {
                    bin: "baseline_bare_irq",
                    exec: "bare",
                    clk_sys_hz: b.clk_sys_hz,
                    pwm_div: profile.div as u32,
                    pwm_top: profile.top as u32,
                    load_us: 0,
                    ntasks: 0,
                    batch: batch_no,
                };
                serial.send_batch(&meta).await;
            }
            batch_no = batch_no.wrapping_add(1);

            // Pause. No async timers here: they need an Embassy executor.
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                serial.reboot_if_requested();
                yield_now().await;
            }
        }
    };

    embassy_futures::block_on(join(usb.run(), batches)).1
}
