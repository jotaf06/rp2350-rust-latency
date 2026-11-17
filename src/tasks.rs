//! Embassy tasks shared by the scenarios.

use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer, block_for};

use crate::measure;

/// The task under measurement. It can be spawned on the thread executor or on
/// an `InterruptExecutor`; the code is the same.
#[embassy_executor::task]
pub async fn measured_task() -> ! {
    loop {
        let (t_isr_start, seq) = measure::EVENT.wait().await;
        let t_task = measure::task_begin(); // first instruction after waking
        measure::record(t_isr_start, seq, t_task);
    }
}

// ---------------------------------------------------------------- CPU load

static LOAD_US: AtomicU32 = AtomicU32::new(0);
static LOAD_GATE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Length of the load's CPU-bound section; 0 parks the load.
pub fn set_load_us(us: u32) {
    LOAD_US.store(us, Relaxed);
    if us != 0 {
        LOAD_GATE.signal(());
    }
}

/// Low-priority load (scenario S1): a CPU-bound stretch of `L` µs **without
/// any `.await`**, then a single cooperative yield, forever.
///
/// The stretch is timed against the hardware timer (`block_for`), so `L` is
/// wall-clock time: being preempted does not make the stretch longer.
/// While `L == 0` the load is parked on a signal and never wakes up.
///
/// Executor-agnostic: also used by the RTIC binary.
pub async fn load_loop() -> ! {
    loop {
        let l = LOAD_US.load(Relaxed);
        if l == 0 {
            LOAD_GATE.wait().await;
            continue;
        }
        block_for(Duration::from_micros(l as u64));
        embassy_futures::yield_now().await;
    }
}

/// [`load_loop`] as an Embassy task.
#[embassy_executor::task]
pub async fn load_task() -> ! {
    load_loop().await
}

// ------------------------------------------------------- extra timer tasks

/// Maximum number of extra tasks (scenario S2).
pub const MAX_EXTRA_TASKS: usize = 16;

/// Sleep time of each extra task, in µs. Distinct primes around 1-2.5 ms so the
/// wake-ups drift against each other and against the event period.
const EXTRA_SLEEP_US: [u64; MAX_EXTRA_TASKS] = [
    1009, 1103, 1201, 1301, 1409, 1511, 1601, 1709, 1801, 1901, 2003, 2111, 2203, 2309, 2411, 2503,
];

static ACTIVE_EXTRA: AtomicU32 = AtomicU32::new(0);
static GATES: [Signal<CriticalSectionRawMutex, ()>; MAX_EXTRA_TASKS] =
    [const { Signal::new() }; MAX_EXTRA_TASKS];

/// Number of extra tasks that are running (the others are parked).
pub fn set_extra_tasks(n: u32) {
    ACTIVE_EXTRA.store(n, Relaxed);
    for gate in GATES.iter().take(n as usize) {
        gate.signal(());
    }
}

/// Extra task `idx` (scenario S2): `Timer::after_micros(k).await` in a loop,
/// with a different `k` per task. Parked (no wake-ups at all) while
/// `idx >= active`.
#[embassy_executor::task(pool_size = MAX_EXTRA_TASKS)]
pub async fn extra_task(idx: usize) -> ! {
    loop {
        if (idx as u32) < ACTIVE_EXTRA.load(Relaxed) {
            Timer::after_micros(EXTRA_SLEEP_US[idx]).await;
        } else {
            GATES[idx].wait().await;
        }
    }
}
