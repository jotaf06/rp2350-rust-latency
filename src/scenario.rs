//! Batch loop of the Embassy scenarios. Runs on the thread executor.
//!
//! For every batch of the scenario: start the load, collect `N_SAMPLES`
//! events in silence, stop everything, then send the batch over USB (if a
//! terminal is listening) and pause. Repeats forever, so the port can be
//! opened at any time.

use embassy_futures::select::{Either, Either3, select, select3};
use embassy_rp::gpio::Output;
use embassy_time::{Duration, Instant, Timer};

use crate::measure::{self, PwmProfile};
use crate::report::{Meta, Serial};
use crate::tasks;

/// Event period for the idle scenarios: ~100 µs, 1-cycle resolution at 150 MHz.
pub const IDLE_PERIOD_US: u32 = 103;

/// One batch configuration.
#[derive(Clone, Copy)]
pub struct Batch {
    /// Event period. Must be > 2x the longest expected latency, and should not
    /// be a multiple of the load length (all values used here are primes).
    pub period_us: u32,
    /// Length of the load task's stretch without `.await`; 0 = no load.
    pub load_us: u32,
    /// Extra timer tasks running; 0 = none.
    pub ntasks: u32,
}

impl Batch {
    pub const fn idle() -> Self {
        Self {
            period_us: IDLE_PERIOD_US,
            load_us: 0,
            ntasks: 0,
        }
    }

    pub const fn load(period_us: u32, load_us: u32) -> Self {
        Self {
            period_us,
            load_us,
            ntasks: 0,
        }
    }

    pub const fn extra_tasks(ntasks: u32) -> Self {
        Self {
            period_us: IDLE_PERIOD_US,
            load_us: 0,
            ntasks,
        }
    }
}

/// The three load lengths of scenario S1 (10, 100 and 1000 µs), each with an
/// event period P >= 2 L that is coprime with L.
pub const LOAD_BATCHES: [Batch; 3] = [
    Batch::load(103, 10),
    Batch::load(257, 100),
    Batch::load(2503, 1000),
];

/// RTIC binary: the idle case (S0) followed by the three loads (S1).
pub const IDLE_AND_LOAD_BATCHES: [Batch; 4] = [
    Batch::idle(),
    Batch::load(103, 10),
    Batch::load(257, 100),
    Batch::load(2503, 1000),
];

/// The task counts of scenario S2.
pub const NTASK_BATCHES: [Batch; 5] = [
    Batch::extra_tasks(1),
    Batch::extra_tasks(2),
    Batch::extra_tasks(4),
    Batch::extra_tasks(8),
    Batch::extra_tasks(16),
];

/// Give up on a batch after this long (only happens if every event is discarded).
const BATCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Pause between batches.
const PAUSE: Duration = Duration::from_secs(2);

/// How the batch loop waits.
#[derive(Clone, Copy, PartialEq)]
enum Waiting {
    /// `embassy_time` timers (needs an Embassy executor).
    Timers,
    /// Busy-polling the clock with cooperative yields. For executors that
    /// cannot drive `embassy_time` timers (RTIC). Used only between batches.
    Polling,
}

async fn sleep_until(deadline: Instant, how: Waiting) {
    match how {
        Waiting::Timers => Timer::at(deadline).await,
        Waiting::Polling => {
            while Instant::now() < deadline {
                embassy_futures::yield_now().await;
            }
        }
    }
}

/// Batch loop for the Embassy binaries.
pub async fn run(
    bin: &'static str,
    exec: &'static str,
    batches: &[Batch],
    clk_sys_hz: u32,
    led: Output<'static>,
    serial: Serial,
) -> ! {
    run_inner(bin, exec, batches, clk_sys_hz, led, serial, Waiting::Timers).await
}

/// Batch loop for executors without `embassy_time` timer support (RTIC).
pub async fn run_polling(
    bin: &'static str,
    exec: &'static str,
    batches: &[Batch],
    clk_sys_hz: u32,
    led: Output<'static>,
    serial: Serial,
) -> ! {
    run_inner(
        bin,
        exec,
        batches,
        clk_sys_hz,
        led,
        serial,
        Waiting::Polling,
    )
    .await
}

async fn run_inner(
    bin: &'static str,
    exec: &'static str,
    batches: &[Batch],
    clk_sys_hz: u32,
    mut led: Output<'static>,
    mut serial: Serial,
    how: Waiting,
) -> ! {
    let mut batch_no = 0u32;
    loop {
        for b in batches {
            let profile = PwmProfile::for_period_us(b.period_us, clk_sys_hz);

            // Collect. If the host touches the port meanwhile (USB traffic
            // would disturb the measurement), throw the batch away and redo it.
            loop {
                tasks::set_load_us(b.load_us);
                tasks::set_extra_tasks(b.ntasks);
                // Let the load reach steady state.
                sleep_until(Instant::now() + Duration::from_millis(50), how).await;

                led.set_high();
                measure::batch_start(profile);
                // The system must stay undisturbed here, so the give-up
                // timeout exists only where it costs nothing (a pending timer).
                let timeout = async {
                    match how {
                        Waiting::Timers => Timer::after(BATCH_TIMEOUT).await,
                        Waiting::Polling => core::future::pending().await,
                    }
                };
                let outcome = select3(
                    measure::BATCH_DONE.wait(),
                    serial.control_changed(),
                    timeout,
                )
                .await;
                measure::batch_stop();
                tasks::set_load_us(0);
                tasks::set_extra_tasks(0);
                led.set_low();

                match outcome {
                    Either3::Second(()) => serial.reboot_if_requested(),
                    Either3::First(()) | Either3::Third(()) => break,
                }
            }

            // Report (only now: nothing is written during collection).
            if serial.connected() {
                let meta = Meta {
                    bin,
                    exec,
                    clk_sys_hz,
                    pwm_div: profile.div as u32,
                    pwm_top: profile.top as u32,
                    load_us: b.load_us,
                    ntasks: b.ntasks,
                    batch: batch_no,
                };
                serial.send_batch(&meta).await;
                serial.reboot_if_requested(); // the host may have asked while we were sending
            }
            batch_no = batch_no.wrapping_add(1);

            // Pause, still answering the "reboot to BOOTSEL" request.
            let deadline = Instant::now() + PAUSE;
            while let Either::Second(()) =
                select(sleep_until(deadline, how), serial.control_changed()).await
            {
                serial.reboot_if_requested();
            }
            serial.reboot_if_requested();
        }
    }
}
