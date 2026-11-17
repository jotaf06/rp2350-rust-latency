//! Board bring-up: clock, observation pins, PWM pin and NVIC priorities.
//!
//! Pins (all outputs, so erratum RP2350-E9 does not apply):
//! * GPIO0  – PWM slice 0 channel A, rises at the event
//! * GPIO2  – high during the ISR
//! * GPIO3  – toggles when the measured task runs
//! * GPIO25 – LED, on while a batch is being collected

use embassy_rp::gpio::{Level, Output};
use embassy_rp::interrupt::{self, InterruptExt, Priority};
use embassy_rp::peripherals::USB;
use embassy_rp::{Peri, pwm};

use crate::chip;

/// NVIC priorities (lower value = more urgent; embassy-rp exposes four levels,
/// which is also all the RP2040's Cortex-M0+ implements). Both chips reset
/// every interrupt to priority 0, so the order has to be set
/// explicitly:
/// event ISR > interrupt executor > time driver > USB > thread executor.
pub const PRIO_EVENT: Priority = Priority::P0;
pub const PRIO_IRQ_EXECUTOR: Priority = Priority::P1;
pub const PRIO_TIME_DRIVER: Priority = Priority::P2;
pub const PRIO_USB: Priority = Priority::P3;

pub struct Board {
    pub usb: Peri<'static, USB>,
    pub led: Output<'static>,
    /// Real clk_sys frequency, read back from the clock tree.
    pub clk_sys_hz: u32,
}

fn clock_config() -> embassy_rp::config::Config {
    #[cfg(feature = "clk-64mhz")]
    {
        // 64 MHz, the clock of the paper's nRF52840 (PLL from the 12 MHz crystal).
        let clocks = embassy_rp::clocks::ClockConfig::system_freq(64_000_000).unwrap();
        embassy_rp::config::Config::new(clocks)
    }
    #[cfg(not(feature = "clk-64mhz"))]
    {
        // Default: 150 MHz (RP2350) or 125 MHz (RP2040) from the 12 MHz crystal.
        embassy_rp::config::Config::default()
    }
}

/// Full bring-up for the Embassy binaries: hardware plus NVIC priorities.
pub fn init() -> Board {
    let board = init_hw();
    chip::PWM_WRAP_IRQ.set_priority(PRIO_EVENT);
    interrupt::SWI_IRQ_1.set_priority(PRIO_IRQ_EXECUTOR);
    chip::TIMER_IRQ.set_priority(PRIO_TIME_DRIVER);
    interrupt::USBCTRL_IRQ.set_priority(PRIO_USB);
    board
}

/// Clock and pins only. NVIC priorities are left to the caller (RTIC assigns
/// its own).
pub fn init_hw() -> Board {
    let p = embassy_rp::init(clock_config());

    // Marker pins: configured through the HAL, then driven with single SIO
    // register writes from the hot path (see `measure`).
    core::mem::forget(Output::new(p.PIN_2, Level::Low));
    core::mem::forget(Output::new(p.PIN_3, Level::Low));

    // GPIO0 = PWM0 A. The HAL only does the pin muxing; the slice itself is
    // programmed through the PAC in `measure::batch_start`.
    let mut cfg = pwm::Config::default();
    cfg.enable = false;
    core::mem::forget(pwm::Pwm::new_output_a(p.PWM_SLICE0, p.PIN_0, cfg));

    // The NVIC line stays enabled; events are gated by the slice's IRQ0_INTE bit.
    // SAFETY: the handler is defined by every measurement binary.
    unsafe { chip::PWM_WRAP_IRQ.enable() };

    Board {
        usb: p.USB,
        led: Output::new(p.PIN_25, Level::Low),
        clk_sys_hz: embassy_rp::clocks::clk_sys_freq(),
    }
}
