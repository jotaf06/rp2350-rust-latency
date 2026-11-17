//! S1 / interrupt executor: measured task on an `InterruptExecutor`
//! (`SWI_IRQ_1`), CPU load on the thread executor.
//!
//! The interrupt executor preempts thread mode, so the load's stretch without
//! `.await` no longer delays the measured task. Hypothesis: the maximum stays
//! close to the idle case (S0).

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_rp::executor::InterruptExecutor;
use embassy_rp::interrupt;
use panic_halt as _;
use rp2350_rust_latency::{board, measure, report, scenario, tasks};

static EXECUTOR_HIGH: InterruptExecutor = InterruptExecutor::new();

// The event: PWM slice 0 wrapped (`PWM_IRQ_WRAP_0` / `PWM_IRQ_WRAP`).
rp2350_rust_latency::event_irq!(measure::on_wrap_irq(););

/// Software interrupt that runs the high-priority executor.
#[interrupt]
unsafe fn SWI_IRQ_1() {
    unsafe { EXECUTOR_HIGH.on_interrupt() }
}

#[embassy_executor::main(
    executor = "embassy_rp::executor::Executor",
    entry = "cortex_m_rt::entry"
)]
async fn main(spawner: Spawner) {
    let b = board::init(); // sets the SWI_IRQ_1 priority (below the event ISR)
    let (usb, serial) = report::usb_init(b.usb);
    spawner.spawn(report::usb_task(usb).unwrap());
    spawner.spawn(tasks::load_task().unwrap());

    let high = EXECUTOR_HIGH.start(interrupt::SWI_IRQ_1);
    high.spawn(tasks::measured_task().unwrap());

    scenario::run(
        "s1_irq_load",
        "irq",
        &scenario::LOAD_BATCHES,
        b.clk_sys_hz,
        b.led,
        serial,
    )
    .await
}
