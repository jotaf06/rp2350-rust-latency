//! S2 / thread: latency versus number of extra tasks (Figure 4 of the paper).
//!
//! The paper does not say what its extra tasks did. Here each extra task runs
//! `Timer::after_micros(k).await` in a loop, with a different `k` per task
//! (primes between 1009 and 2503 µs). One batch each for N = 1, 2, 4, 8, 16.

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
    for idx in 0..tasks::MAX_EXTRA_TASKS {
        spawner.spawn(tasks::extra_task(idx).unwrap()); // parked until a batch needs them
    }

    scenario::run(
        "s2_thread_ntasks",
        "thread",
        &scenario::NTASK_BATCHES,
        b.clk_sys_hz,
        b.led,
        serial,
    )
    .await
}
