//! S1 / thread: measured task and CPU load on the **same** thread executor.
//!
//! The executor is cooperative: while the load runs its stretch without
//! `.await` (L = 10, 100, 1000 µs, one batch each), the measured task cannot
//! run. Hypothesis: the maximum total latency grows to about L.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use panic_halt as _;
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
    spawner.spawn(tasks::load_task().unwrap());

    scenario::run(
        "s1_thread_load",
        "thread",
        &scenario::LOAD_BATCHES,
        b.clk_sys_hz,
        b.led,
        serial,
    )
    .await
}
