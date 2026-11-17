//! S0 / interrupt executor: measured task on an `InterruptExecutor` driven by
//! `SWI_IRQ_1`, system idle. Everything else stays on the thread executor.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_rp::executor::InterruptExecutor;
use embassy_rp::interrupt;
use panic_halt as _;
use rp2350_rust_latency::scenario::Batch;
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

    let high = EXECUTOR_HIGH.start(interrupt::SWI_IRQ_1);
    high.spawn(tasks::measured_task().unwrap());

    scenario::run(
        "s0_irq_idle",
        "irq",
        &[Batch::idle()],
        b.clk_sys_hz,
        b.led,
        serial,
    )
    .await
}
