//! PWM slice 0 as event source and clock.
//!
//! The slice free-runs from 0 to TOP. The wrap (counter back to 0) is the
//! event and raises `PWM_IRQ_WRAP_0`. Because the counter restarts at the
//! event, *reading the counter gives the time elapsed since the event*, in
//! ticks of `clk_sys / DIV`. Three reads per event:
//!
//! * `t_isr_start` – first useful instruction of the ISR  -> interrupt latency
//! * `t_isr_end`   – last instruction of the ISR          -> ISR duration
//! * `t_task`      – first instruction of the woken task  -> scheduling latency
//!
//! Nothing here allocates, logs or blocks.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

use embassy_rp::pac;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;

use crate::chip;

/// PWM slice used for the measurement (channel A is GPIO0).
pub const SLICE: usize = 0;
/// Samples per batch.
pub const N_SAMPLES: usize = 2000;
/// The first `WARMUP` samples of a batch are flagged, not dropped.
pub const WARMUP: usize = 50;
/// GPIO2: high while the ISR runs.
pub const PIN_ISR_MARK: u32 = 2;
/// GPIO3: toggles every time the measured task runs.
pub const PIN_TASK_MARK: u32 = 3;

/// One event: the three raw counter reads, in PWM ticks.
#[derive(Clone, Copy)]
pub struct Sample {
    pub t_isr_start: u16,
    pub t_isr_end: u16,
    pub t_task: u16,
}

/// Divider and TOP of the PWM slice. Period = `(top + 1) * div` clk_sys cycles.
#[derive(Clone, Copy)]
pub struct PwmProfile {
    pub div: u8,
    pub top: u16,
}

impl PwmProfile {
    /// Smallest integer divider (best resolution) that fits `period_us` in the
    /// 16-bit counter.
    pub fn for_period_us(period_us: u32, clk_sys_hz: u32) -> Self {
        let cycles = (period_us as u64 * clk_sys_hz as u64 / 1_000_000) as u32;
        let div = cycles.div_ceil(65_536).clamp(1, 255);
        let top = (cycles / div).clamp(2, 65_536) - 1;
        Self {
            div: div as u8,
            top: top as u16,
        }
    }
}

// ---------------------------------------------------------------- hot path

/// Time since the last event, in PWM ticks. Single register read.
#[inline(always)]
pub fn now_ticks() -> u32 {
    pac::PWM.ch(SLICE).ctr().read().0
}

/// ISR entry: read the clock first, then raise the marker and clear the flag.
#[inline(always)]
pub fn isr_begin() -> u32 {
    let t_isr_start = now_ticks(); // <- measurement point 1
    pac::SIO
        .gpio_out(0)
        .value_set()
        .write_value(1 << PIN_ISR_MARK);
    pac::PWM
        .intr()
        .write_value(pac::pwm::regs::Intr(1 << SLICE)); // write 1 to clear
    t_isr_start
}

/// ISR exit: read the clock after the task has been signalled, publish it,
/// drop the marker.
#[inline(always)]
pub fn isr_end() {
    let t_isr_end = now_ticks(); // <- measurement point 2
    T_ISR_END.store(t_isr_end, Relaxed);
    pac::SIO
        .gpio_out(0)
        .value_clr()
        .write_value(1 << PIN_ISR_MARK);
}

/// Sequence number of the event being handled. Only the ISR writes `SEQ`.
#[inline(always)]
pub fn isr_next_seq() -> u32 {
    let seq = SEQ.load(Relaxed).wrapping_add(1);
    SEQ.store(seq, Relaxed);
    seq
}

/// Complete ISR body for the Embassy scenarios: wake the measured task through
/// an `embassy_sync` `Signal` carrying `(t_isr_start, seq)`.
///
/// `t_isr_end` cannot travel in the signal because it is read *after*
/// signalling, so it is published through an atomic instead. The task cannot
/// observe it too early: every executor runs below this interrupt.
#[inline(always)]
pub fn on_wrap_irq() {
    let t_isr_start = isr_begin();
    EVENT.signal((t_isr_start, isr_next_seq()));
    isr_end();
}

/// First thing the measured task does when it wakes: read the clock, then
/// toggle the task marker.
#[inline(always)]
pub fn task_begin() -> u32 {
    let t_task = now_ticks(); // <- measurement point 3
    pac::SIO
        .gpio_out(0)
        .value_xor()
        .write_value(1 << PIN_TASK_MARK);
    t_task
}

// ------------------------------------------------------------ shared state

/// ISR -> measured task: `(t_isr_start, seq)`.
pub static EVENT: Signal<CriticalSectionRawMutex, (u32, u32)> = Signal::new();
/// Measured task -> batch loop: the buffer is full.
pub static BATCH_DONE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

static SEQ: AtomicU32 = AtomicU32::new(0);
static LAST_SEQ: AtomicU32 = AtomicU32::new(0);
static T_ISR_END: AtomicU32 = AtomicU32::new(0);
static COLLECTING: AtomicBool = AtomicBool::new(false);
static LEN: AtomicU32 = AtomicU32::new(0);
static DISCARDED: AtomicU32 = AtomicU32::new(0);
static GUARD_TICKS: AtomicU32 = AtomicU32::new(0);

/// Static sample buffer (no heap). Protocol instead of a lock, to keep the
/// measured task free of critical sections:
/// * written only by the measured task, only while `COLLECTING` is set;
/// * read only after `batch_stop()`.
struct SampleBuf(UnsafeCell<[Sample; N_SAMPLES]>);
// SAFETY: see the access protocol above.
unsafe impl Sync for SampleBuf {}
static BUF: SampleBuf = SampleBuf(UnsafeCell::new(
    [Sample {
        t_isr_start: 0,
        t_isr_end: 0,
        t_task: 0,
    }; N_SAMPLES],
));

/// Store one event. Called by the measured task right after [`task_begin`].
///
/// An event is discarded (and counted) when the counter may have wrapped
/// between the event and the task read, which would make `t_task` meaningless:
/// * a newer event already happened (`SEQ` moved on), or events were skipped;
/// * `t_task` is in the last 1/8 of the period (too close to the wrap).
pub fn record(t_isr_start: u32, seq: u32, t_task: u32) {
    if !COLLECTING.load(Relaxed) {
        return;
    }
    let t_isr_end = T_ISR_END.load(Relaxed);
    let fresh = SEQ.load(Relaxed) == seq;

    // Events that fired but never reached the task (signal overwritten).
    let skipped = seq.wrapping_sub(LAST_SEQ.load(Relaxed)).wrapping_sub(1);
    LAST_SEQ.store(seq, Relaxed);
    if skipped != 0 {
        DISCARDED.store(DISCARDED.load(Relaxed).wrapping_add(skipped), Relaxed);
    }

    if !fresh
        || t_task < t_isr_end
        || t_isr_end < t_isr_start
        || t_task >= GUARD_TICKS.load(Relaxed)
    {
        DISCARDED.store(DISCARDED.load(Relaxed).wrapping_add(1), Relaxed);
        return;
    }

    let i = LEN.load(Relaxed) as usize;
    if i < N_SAMPLES {
        // SAFETY: single writer while COLLECTING, index in bounds.
        unsafe {
            BUF.0.get().cast::<Sample>().add(i).write(Sample {
                t_isr_start: t_isr_start as u16,
                t_isr_end: t_isr_end as u16,
                t_task: t_task as u16,
            });
        }
        LEN.store((i + 1) as u32, Relaxed);
    }
    if i + 1 >= N_SAMPLES {
        COLLECTING.store(false, Relaxed);
        irq_disable();
        BATCH_DONE.signal(());
    }
}

// ------------------------------------------------------------ batch control

#[inline(always)]
fn irq_disable() {
    chip::pwm_irq_enable(0);
}

/// Program the slice (stopped): divider, TOP, 50 % duty on channel A so that
/// GPIO0 rises exactly at the event.
fn pwm_configure(p: PwmProfile) {
    let ch = pac::PWM.ch(SLICE);
    ch.csr().modify(|w| w.set_en(false));
    ch.div().write(|w| {
        w.set_int(p.div);
        w.set_frac(0);
    });
    ch.top().write(|w| w.set_top(p.top));
    ch.cc().write(|w| w.set_a((p.top / 2).max(1)));
    ch.ctr().write(|w| w.set_ctr(0));
    pac::PWM
        .intr()
        .write_value(pac::pwm::regs::Intr(1 << SLICE));
}

/// Start a new batch with the given PWM profile.
pub fn batch_start(p: PwmProfile) {
    batch_stop();
    pwm_configure(p);
    let period = p.top as u32 + 1;
    GUARD_TICKS.store(period - period / 8, Relaxed);
    LEN.store(0, Relaxed);
    DISCARDED.store(0, Relaxed);
    LAST_SEQ.store(SEQ.load(Relaxed), Relaxed);
    EVENT.reset();
    BATCH_DONE.reset();
    COLLECTING.store(true, Relaxed);
    chip::pwm_irq_enable(1 << SLICE);
    pac::PWM.ch(SLICE).csr().modify(|w| w.set_en(true));
}

/// Stop collecting: no more events, the buffer may now be read.
pub fn batch_stop() {
    COLLECTING.store(false, Relaxed);
    irq_disable();
    pac::PWM.ch(SLICE).csr().modify(|w| w.set_en(false));
    pac::PWM
        .intr()
        .write_value(pac::pwm::regs::Intr(1 << SLICE));
}

/// True while a batch is being collected.
pub fn collecting() -> bool {
    COLLECTING.load(Relaxed)
}

/// Samples of the last batch. Call only after [`batch_stop`].
pub fn samples() -> &'static [Sample] {
    let n = (LEN.load(Relaxed) as usize).min(N_SAMPLES);
    // SAFETY: collection is stopped, so there is no writer.
    unsafe { core::slice::from_raw_parts(BUF.0.get().cast::<Sample>(), n) }
}

/// Events discarded in the last batch.
pub fn discarded() -> u32 {
    DISCARDED.load(Relaxed)
}
