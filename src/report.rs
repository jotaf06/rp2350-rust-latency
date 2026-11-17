//! Output over USB CDC-ACM (a `/dev/ttyACM*` port on the host).
//!
//! Text protocol, one batch:
//!
//! ```text
//! META,chip=...,bin=...,exec=...,profile=...,clk_sys_hz=...,pwm_div=...,pwm_top=...,load_us=...,...
//! SAMPLE,<idx>,<irq_lat>,<isr_dur>,<sched_lat>,<total>,<warmup 0|1>
//! DONE
//! ```
//!
//! All values are in PWM ticks; the host converts with `pwm_div` and
//! `clk_sys_hz`. Nothing is ever written while a batch is being collected.
//! Formatting is hand-rolled (no `core::fmt`, no heap).

use embassy_futures::select::{Either, select};
use embassy_rp::peripherals::USB;
use embassy_rp::usb::{Driver, InterruptHandler};
use embassy_rp::{Peri, bind_interrupts};
use embassy_usb::class::cdc_acm::{CdcAcmClass, ControlChanged, Receiver, Sender, State};
use embassy_usb::driver::EndpointError;
use embassy_usb::{Builder, Config, UsbDevice};
use static_cell::StaticCell;

use crate::chip;
use crate::measure::{self, WARMUP};

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => InterruptHandler<USB>;
});

pub type UsbDriver = Driver<'static, USB>;

/// Build-time facts (see build.rs).
pub const PROFILE: &str = env!("LAT_PROFILE");
pub const OPT_LEVEL: &str = env!("LAT_OPT_LEVEL");
pub const GIT_REV: &str = env!("LAT_GIT");

/// The serial port, as seen by the batch loop.
pub struct Serial {
    tx: Sender<'static, UsbDriver>,
    _rx: Receiver<'static, UsbDriver>,
    ctl: ControlChanged<'static>,
}

/// Per-batch metadata that is not known at build time.
pub struct Meta<'a> {
    pub bin: &'a str,
    pub exec: &'a str,
    pub clk_sys_hz: u32,
    pub pwm_div: u32,
    pub pwm_top: u32,
    pub load_us: u32,
    pub ntasks: u32,
    pub batch: u32,
}

/// Create the USB device with one CDC-ACM interface.
pub fn usb_init(usb: Peri<'static, USB>) -> (UsbDevice<'static, UsbDriver>, Serial) {
    let driver = Driver::new(usb, Irqs);

    let mut config = Config::new(0xc0de, 0xcafe);
    config.manufacturer = Some("STR-UFAL");
    config.product = Some(if cfg!(feature = "rp2040") {
        "rp2040-rust-latency"
    } else {
        "rp2350-rust-latency"
    });
    config.serial_number = Some("0001");
    config.max_power = 100;
    config.max_packet_size_0 = 64;

    static CONFIG_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    static STATE: StaticCell<State> = StaticCell::new();

    let mut builder = Builder::new(
        driver,
        config,
        CONFIG_DESCRIPTOR.init([0; 256]),
        BOS_DESCRIPTOR.init([0; 256]),
        &mut [], // no msos descriptors
        CONTROL_BUF.init([0; 64]),
    );
    let class = CdcAcmClass::new(&mut builder, STATE.init(State::new()), 64);
    let (tx, rx, ctl) = class.split_with_control();

    (builder.build(), Serial { tx, _rx: rx, ctl })
}

/// Runs the USB stack. Lives on the thread executor (lowest priority).
#[embassy_executor::task]
pub async fn usb_task(mut dev: UsbDevice<'static, UsbDriver>) -> ! {
    dev.run().await
}

impl Serial {
    /// A terminal has the port open (DTR asserted).
    pub fn connected(&self) -> bool {
        self.tx.dtr()
    }

    /// Resolves when the host changes DTR/RTS or the line coding.
    pub async fn control_changed(&self) {
        self.ctl.control_changed().await
    }

    /// Arduino-style "1200 baud touch": opening the port at 1200 baud reboots
    /// into the BOOTSEL bootloader, so the board can be re-flashed without
    /// touching the button (see tools/flash.sh).
    pub fn reboot_if_requested(&self) {
        if self.tx.line_coding().data_rate() == 1200 {
            embassy_rp::rom_data::reset_to_usb_boot(0, 0);
        }
    }

    /// Send the last batch. Gives up (returns `false`) if the host closes the
    /// port or changes the line state in the middle of it.
    pub async fn send_batch(&mut self, meta: &Meta<'_>) -> bool {
        let Serial { tx, ctl, .. } = self;
        match select(write_batch(tx, meta), ctl.control_changed()).await {
            Either::First(r) => r.is_ok(),
            Either::Second(()) => false,
        }
    }
}

/// Small packet buffer: lines are packed into USB packets shorter than the
/// 64-byte endpoint size, so no zero-length packet is ever needed.
struct Out<'a> {
    tx: &'a mut Sender<'static, UsbDriver>,
    buf: [u8; 63],
    len: usize,
}

impl Out<'_> {
    async fn flush(&mut self) -> Result<(), EndpointError> {
        if self.len > 0 {
            self.tx.write_packet(&self.buf[..self.len]).await?;
            self.len = 0;
        }
        Ok(())
    }

    async fn bytes(&mut self, data: &[u8]) -> Result<(), EndpointError> {
        for &b in data {
            if self.len == self.buf.len() {
                self.flush().await?;
            }
            self.buf[self.len] = b;
            self.len += 1;
        }
        Ok(())
    }

    async fn str(&mut self, s: &str) -> Result<(), EndpointError> {
        self.bytes(s.as_bytes()).await
    }

    async fn num(&mut self, mut v: u32) -> Result<(), EndpointError> {
        let mut digits = [0u8; 10];
        let mut i = digits.len();
        loop {
            i -= 1;
            digits[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.bytes(&digits[i..]).await
    }

    async fn field(&mut self, key: &str, v: u32) -> Result<(), EndpointError> {
        self.str(key).await?;
        self.num(v).await
    }
}

async fn write_batch(
    tx: &mut Sender<'static, UsbDriver>,
    m: &Meta<'_>,
) -> Result<(), EndpointError> {
    let samples = measure::samples();
    let mut o = Out {
        tx,
        buf: [0; 63],
        len: 0,
    };

    o.str("META,chip=").await?;
    o.str(chip::NAME).await?;
    o.str(",bin=").await?;
    o.str(m.bin).await?;
    o.str(",exec=").await?;
    o.str(m.exec).await?;
    o.str(",profile=").await?;
    o.str(PROFILE).await?;
    o.str(",opt=").await?;
    o.str(OPT_LEVEL).await?;
    o.field(",clk_sys_hz=", m.clk_sys_hz).await?;
    o.field(",pwm_div=", m.pwm_div).await?;
    o.field(",pwm_top=", m.pwm_top).await?;
    o.field(",load_us=", m.load_us).await?;
    o.field(",ntasks=", m.ntasks).await?;
    o.field(",n=", samples.len() as u32).await?;
    o.field(",discarded=", measure::discarded()).await?;
    o.field(",warmup=", WARMUP as u32).await?;
    o.field(",batch=", m.batch).await?;
    o.str(",git=").await?;
    o.str(GIT_REV).await?;
    o.str("\r\n").await?;

    for (i, s) in samples.iter().enumerate() {
        let (t0, t1, t2) = (s.t_isr_start as u32, s.t_isr_end as u32, s.t_task as u32);
        o.field("SAMPLE,", i as u32).await?;
        o.field(",", t0).await?; // irq_lat   = t_isr_start
        o.field(",", t1 - t0).await?; // isr_dur   = t_isr_end - t_isr_start
        o.field(",", t2 - t1).await?; // sched_lat = t_task - t_isr_end
        o.field(",", t2).await?; // total     = t_task
        o.str(if i < WARMUP { ",1\r\n" } else { ",0\r\n" }).await?;
    }
    o.str("DONE\r\n").await?;
    o.flush().await
}
