//! Everything that differs between the two supported chips. The rest of the
//! firmware is the same source for both.
//!
//! | | RP2350A (Pico 2), feature `rp2350` | RP2040 (Pico), feature `rp2040` |
//! | --- | --- | --- |
//! | core | Cortex-M33 (Armv8-M), `thumbv8m.main-none-eabihf` | Cortex-M0+ (Armv6-M), `thumbv6m-none-eabi` |
//! | default clk_sys | 150 MHz | 125 MHz |
//! | PWM wrap IRQ | `PWM_IRQ_WRAP_0` | `PWM_IRQ_WRAP` |
//! | timer IRQ (time driver) | `TIMER0_IRQ_0` | `TIMER_IRQ_0` |
//! | PWM IRQ enable register | `IRQ0_INTE` | `INTE` |
//! | NVIC priority bits | 4 | 2 |
//! | boot block | `IMAGE_DEF` (embassy-rp) | `.boot2` (embassy-rp, W25Q080) |

use embassy_rp::pac;
use embassy_rp::pac::Interrupt;

#[cfg(all(feature = "rp2350", feature = "rp2040"))]
compile_error!("features `rp2350` and `rp2040` are mutually exclusive (use --no-default-features)");
#[cfg(not(any(feature = "rp2350", feature = "rp2040")))]
compile_error!("enable one chip feature: `rp2350` or `rp2040`");

/// Chip name, sent in the `chip=` field of every META line.
#[cfg(feature = "rp2350")]
pub const NAME: &str = "rp2350";
#[cfg(feature = "rp2040")]
pub const NAME: &str = "rp2040";

/// The event interrupt: PWM slice wrap.
#[cfg(feature = "rp2350")]
pub const PWM_WRAP_IRQ: Interrupt = Interrupt::PWM_IRQ_WRAP_0;
#[cfg(feature = "rp2040")]
pub const PWM_WRAP_IRQ: Interrupt = Interrupt::PWM_IRQ_WRAP;

/// Alarm interrupt of the timer used by the embassy-rp time driver.
#[cfg(feature = "rp2350")]
pub const TIMER_IRQ: Interrupt = Interrupt::TIMER0_IRQ_0;
#[cfg(feature = "rp2040")]
pub const TIMER_IRQ: Interrupt = Interrupt::TIMER_IRQ_0;

/// Implemented NVIC priority bits (for raw NVIC priority bytes under RTIC).
pub const NVIC_PRIO_BITS: u8 = pac::NVIC_PRIO_BITS;

/// Write the PWM interrupt-enable register (one bit per slice). Written whole:
/// this firmware uses no other PWM slice.
#[inline(always)]
pub fn pwm_irq_enable(mask: u32) {
    #[cfg(feature = "rp2350")]
    pac::PWM
        .irq0_inte()
        .write_value(pac::pwm::regs::Irq0inte(mask));
    #[cfg(feature = "rp2040")]
    pac::PWM.inte().write_value(pac::pwm::regs::Inte(mask));
}

/// Defines the event ISR (the PWM wrap handler) with the given body, under
/// the interrupt name of the selected chip. Use once per binary:
///
/// ```ignore
/// rp2350_rust_latency::event_irq!(measure::on_wrap_irq());
/// ```
///
/// The body sees the binary's top-level items (`use super::*`).
#[macro_export]
macro_rules! event_irq {
    ($($body:tt)*) => {
        mod __event_irq {
            #[allow(unused_imports)]
            use super::*;
            use ::embassy_rp::interrupt;

            /// The event: PWM slice 0 wrapped.
            #[cfg(feature = "rp2350")]
            #[interrupt]
            fn PWM_IRQ_WRAP_0() {
                $($body)*
            }

            /// The event: PWM slice 0 wrapped.
            #[cfg(feature = "rp2040")]
            #[interrupt]
            fn PWM_IRQ_WRAP() {
                $($body)*
            }
        }
    };
}
