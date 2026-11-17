//! Blink: toggles the on-board LED (GPIO25) every 250 ms with `embassy_time`.
//!
//! Used only for the memory comparison with Table 4 of the paper
//! (`tools/sizes.sh`). It deliberately does not link the measurement library.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_rp::gpio::{Level, Output};
use embassy_time::Timer;
use panic_halt as _;

#[embassy_executor::main(
    executor = "embassy_rp::executor::Executor",
    entry = "cortex_m_rt::entry"
)]
async fn main(_spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    let mut led = Output::new(p.PIN_25, Level::Low);

    loop {
        led.toggle();
        Timer::after_millis(250).await;
    }
}
