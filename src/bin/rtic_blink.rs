//! RTIC 2 blink: toggles the LED (GPIO25) every 250 ms using the SysTick
//! monotonic. Used only for the memory comparison (Table 4 of the paper).
//!
//! embassy-rp provides clock setup and the GPIO driver; no Embassy executor.

#![no_std]
#![no_main]

use panic_halt as _;
use rtic_monotonics::systick::prelude::*;

systick_monotonic!(Mono, 1000);

#[rtic::app(device = embassy_rp::pac, peripherals = false, dispatchers = [SWI_IRQ_0])]
mod app {
    use super::*;
    use embassy_rp::gpio::{Level, Output};

    #[shared]
    struct Shared {}

    #[local]
    struct Local {
        led: Output<'static>,
    }

    #[init]
    fn init(cx: init::Context) -> (Shared, Local) {
        let p = embassy_rp::init(Default::default());
        Mono::start(cx.core.SYST, embassy_rp::clocks::clk_sys_freq());
        blink::spawn().ok();
        (
            Shared {},
            Local {
                led: Output::new(p.PIN_25, Level::Low),
            },
        )
    }

    #[task(priority = 1, local = [led])]
    async fn blink(cx: blink::Context) {
        loop {
            cx.local.led.toggle();
            Mono::delay(250.millis()).await;
        }
    }
}
