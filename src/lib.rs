//! Shared measurement code for the latency experiments on the RP2350A
//! (Pico 2, feature `rp2350`, default) and the RP2040 (Pico, feature `rp2040`).
//!
//! * [`chip`]     – the few names and registers that differ between the chips.
//! * [`measure`]  – PWM slice used as event source *and* clock, ISR/task
//!   instrumentation points, static sample buffer.
//! * [`board`]    – clock, pins and NVIC priorities.
//! * [`report`]   – USB CDC-ACM output (`META` / `SAMPLE` / `DONE` lines).
//! * [`tasks`]    – Embassy tasks: measured task, CPU load, extra timer tasks.
//! * [`scenario`] – batch loop used by the Embassy binaries.
//!
//! See README.md for the method and the design choices.

#![no_std]

pub mod board;
pub mod chip;
pub mod measure;
pub mod report;
pub mod scenario;
pub mod tasks;
