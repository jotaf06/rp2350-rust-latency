//! S0 / thread: measured task on the thread-mode executor, system idle.
//! Corresponds to Table 3 of the paper (Embassy rows).

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use panic_halt as _;
use rp2350_rust_latency::scenario::Batch;
use rp2350_rust_latency::{board, measure, report, scenario, tasks};

// The event: PWM slice 0 wrapped (`PWM_IRQ_WRAP_0` / `PWM_IRQ_WRAP`).
rp2350_rust_latency::event_irq!(measure::on_wrap_irq(););

#[embassy_executor::main(
    executor = "embassy_rp::executor::Executor",
    entry = "cortex_m_rt::entry"
)]
async fn main(spawner: Spawner) {
    let b = board::init();
    let (usb, serial) = report::usb_init(b.usb);
    spawner.spawn(report::usb_task(usb).unwrap());
    spawner.spawn(tasks::measured_task().unwrap());

    scenario::run(
        "s0_thread_idle",
        "thread",
        &[Batch::idle()],
        b.clk_sys_hz,
        b.led,
        serial,
    )
    .await
}
