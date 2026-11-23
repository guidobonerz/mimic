//! The ITU ("interrupt, timer, UART") at I/O `$000000`–`$0000FF`: the
//! register side of the interrupt controller ([`Irq`]), the 5 µs
//! countdown, the periodic interrupt timer that is the firmware's only
//! system tick, the millisecond counter, the buttons, the FPGA's version
//! and capability word, the console UART, the busy lamps and the enables
//! of the high interrupt sources.
//!
//! Decoding: bits 5–4 of the address select a part (0 interrupts and
//! timers, 1 UART, 2 millisecond counter/lamps/high interrupts, 3
//! nothing), bits 3–0 the register; the UART looks at bits 1–0 only, so
//! its four registers repeat through `$10`–`$1F`. This model repeats the
//! 64 bytes through its whole range (whether the board does is not
//! known; the firmware stays below `$30`).
//!
//! | Addr | Access | Register |
//! |---|---|---|
//! | `$00` | R/W | bit 0: global interrupt enable (1 after power-on) |
//! | `$01` | R/W | write: set bits of the low enables; read: the enables |
//! | `$02` | W | clear bits of the low enables |
//! | `$03` | R | which low sources are edges (`$85`, fixed) |
//! | `$04` | W | clear latched edges |
//! | `$05` | R | low sources active and enabled |
//! | `$06` | R/W | countdown: minus one every 5 µs, stays at 0 |
//! | `$07` | R/W | periodic timer: bit 0 run, bit 1 count external pulses |
//! | `$08` | R/W | write: reload value, high byte; read: counter bits 15–8 |
//! | `$09` | R/W | write: reload value, low byte; read: counter bits 7–0 |
//! | `$0A` | R | buttons: bits 7–5 buttons 2–0, bit 6 also the menu button |
//! | `$0B` | R | FPGA version |
//! | `$0C`–`$0F` | R | capabilities, most significant byte first |
//! | `$10` | R/W | UART: write sends a byte, read shows the oldest received |
//! | `$11` | W | UART: drop the oldest received byte |
//! | `$12` | R/W | UART flags (see [`Itu::uart_input`]); write bit 0 = 1 clears the overrun |
//! | `$13` | – | UART interrupt control: not built in, reads 0 |
//! | `$22`, `$23` | R | millisecond counter, high and low byte |
//! | `$24`, `$25`, `$29` | W | bit 0: USB, SD card, printer busy (one lamp) |
//! | `$26` | W | eight output pins without a known use |
//! | `$27` | R/W | enables of the high interrupt sources |
//! | `$28` | R | high sources active and enabled |
//!
//! Everything else reads 0 and ignores writes; that includes `$1F`, where
//! the firmware's earliest trap handler leaves a mark.
//!
//! **Time.** All three time bases follow the board's clock alone
//! ([`IoCtx::now`], 100 MHz):
//! - the millisecond counter is the clock divided by 100 000, 16 bits;
//!   two reads in a row agree unless a millisecond passed (the firmware
//!   reads low/high twice until they agree);
//! - the countdown steps on a divider that runs free (every 500 clocks),
//!   so the first step after a write comes after 0 to 5 µs;
//! - the periodic timer is a 24-bit counter that counts down with every
//!   clock. At 0 it gives a pulse to low interrupt source 0 and loads
//!   reload·256 + 255, so the period is (reload + 1)·256 clocks: with the
//!   firmware's `$07A0`, 499 968 clocks (4.99968 ms). A write to `$07`
//!   while the timer stands loads the counter the same way, so the first
//!   pulse comes one full period after the start. Stopping freezes the
//!   counter. Reload after power-on: `$8000`.
//!
//! Source 0 is an edge: the pulse sets a one-bit flag in [`Irq`] whether
//! the source is enabled or not. Pulses that arrive while the flag is
//! still set merge into it, on the board as here; several periods that
//! pass between two calls therefore set the flag once and lose nothing
//! the board would keep, because every write to this block first brings
//! the timer up to the moment of the write. A pulse that falls on the
//! clock of a clear survives it.
//!
//! With bit 1 of `$07` set the counter waits for pulses on an input that
//! the board is not known to drive; here they never come, so the counter
//! stands (the firmware never sets the bit).
//!
//! **UART.** Sent bytes go to [`IoCtx::console`] unchanged (the firmware
//! ends lines with CR LF) and at once: the "transmit FIFO almost full"
//! flag, which the firmware waits on before every byte, never shows. The
//! receive side is a FIFO of 15 bytes filled by [`Itu::uart_input`]; the
//! firmware in question never reads it.
//!
//! Sources: UE2-C64U-Emulator `docs/hw/02-itu-uart.md` (register table,
//! hazards H1–H14, functional model) and `00-memory-map.md` (section 1b,
//! boot phases A–C, "Interrupts", contradictions C2 and C12); 1541
//! Ultimate `fpga/io/itu/vhdl_source/itu.vhd` (counter reload and order
//! of clear and set, reset values, button bits),
//! `fpga/io/uart_lite/vhdl_source/uart_peripheral_io.vhd` (flag bits,
//! overrun rule) and `fpga/ip/srl_fifo/vhdl_source/srl_fifo-xilinx.vhd`
//! (FIFO depth 15, "almost full" from 14 bytes).

use std::collections::VecDeque;

use crate::io::{IoCtx, IoDevice};
use crate::irq::{Irq, EDGE_MASK};
use mimic_core::{StateError, StateReader, StateWriter};

/// First and last I/O address of the block.
pub const ITU_FIRST: u32 = 0x00_0000;
pub const ITU_LAST: u32 = 0x00_00FF;

/// Clocks per step of the countdown at `$06` (5 µs).
pub const TIMER_STEP: u64 = 500;
/// Clocks per step of the millisecond counter.
pub const MS_STEP: u64 = 100_000;
/// The reload value of the periodic timer after power-on.
pub const IRQ_TIMER_RELOAD: u16 = 0x8000;
/// The FPGA version this model reports unless told otherwise: the default
/// of the open top levels. The firmware only prints it.
pub const FPGA_VERSION: u8 = 0x25;

/// Capability bits the firmware tests (`$0C`–`$0F`). A set bit makes the
/// firmware use that part of the board and enable its interrupt, so only
/// bits of modelled blocks should be given.
pub mod capab {
    pub const UART: u32 = 1 << 0;
    pub const DRIVE_1541_1: u32 = 1 << 1;
    pub const DRIVE_1541_2: u32 = 1 << 2;
    pub const HARDWARE_IEC: u32 = 1 << 5;
    pub const BUS_MEASURE: u32 = 1 << 6;
    pub const C2N_STREAMER: u32 = 1 << 7;
    pub const C2N_RECORDER: u32 = 1 << 8;
    /// Without it the firmware has no C64 and no menu.
    pub const CARTRIDGE: u32 = 1 << 9;
    pub const MM_DRIVE: u32 = 1 << 11;
    pub const COMMAND_INTF: u32 = 1 << 18;
    pub const SAMPLER: u32 = 1 << 21;
    pub const EEPROM: u32 = 1 << 22;
    pub const USB_HOST2: u32 = 1 << 23;
    pub const ETH_RMII: u32 = 1 << 24;
    /// Must stay clear on this board.
    pub const ULTIMATE2PLUS: u32 = 1 << 25;
    /// Must be set on this board.
    pub const ULTIMATE64: u32 = 1 << 26;
    pub const ACIA: u32 = 1 << 27;
    /// Bits 29–28: the FPGA type; 2 and 3 are the two sizes of this
    /// board's FPGA and select the layout of the flash (3: the larger).
    pub const FPGA_TYPE_SHIFT: u32 = 28;
}

/// UART flag bits (`$12`).
pub mod uart_flag {
    /// A byte arrived while the receive FIFO was almost full.
    pub const OVERRUN: u8 = 0x01;
    /// Transmit FIFO almost full; never set here.
    pub const TX_FULL: u8 = 0x10;
    /// Receive FIFO almost full (14 or 15 bytes).
    pub const RX_FULL: u8 = 0x20;
    /// The transmitter is idle; always set here.
    pub const TX_IDLE: u8 = 0x40;
    /// A received byte waits.
    pub const RX_AVAILABLE: u8 = 0x80;
}

/// Bytes the receive FIFO holds.
const RX_DEPTH: usize = 15;
/// From this fill on the receive FIFO counts as almost full.
const RX_ALMOST_FULL: usize = 14;

/// The menu button in the button register.
const BUTTON_MENU: u8 = 0x40;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Itu {
    capabilities: u32,
    version: u8,
    /// The three button inputs (bits 2–0).
    buttons: u8,
    menu: bool,
    /// `$06` as written, and the count of 5 µs steps since power-on at
    /// that moment.
    timer: u8,
    timer_step: u64,
    /// `$07` bit 0 and bit 1.
    irq_timer_en: bool,
    irq_timer_select: bool,
    /// The reload value (`$08`/`$09`).
    irq_timer_val: u16,
    /// The 24-bit counter at the clock `irq_timer_at`; while the timer
    /// runs it has gone down by one per clock since.
    irq_timer_cnt: u32,
    irq_timer_at: u64,
    /// The timer's output stands high (only when it reloads 0 while
    /// waiting for external pulses): no new edge.
    irq_timer_high: bool,
    rx: VecDeque<u8>,
    overrun: bool,
    usb_busy: bool,
    sd_busy: bool,
    printer_busy: bool,
    misc_io: u8,
    /// The clock and the interrupt state at the last access or tick, for
    /// [`IoDevice::peek`], which gets neither.
    seen_now: u64,
    seen_irq: Irq,
}

impl Itu {
    /// `capabilities`: the word at `$0C`–`$0F`.
    pub fn new(capabilities: u32) -> Self {
        Self {
            capabilities,
            version: FPGA_VERSION,
            buttons: 0,
            menu: false,
            timer: 0,
            timer_step: 0,
            irq_timer_en: false,
            irq_timer_select: false,
            irq_timer_val: IRQ_TIMER_RELOAD,
            irq_timer_cnt: 0,
            irq_timer_at: 0,
            irq_timer_high: false,
            rx: VecDeque::new(),
            overrun: false,
            usb_busy: false,
            sd_busy: false,
            printer_busy: false,
            misc_io: 0,
            seen_now: 0,
            seen_irq: Irq::default(),
        }
    }

    pub fn capabilities(&self) -> u32 {
        self.capabilities
    }

    pub fn set_capabilities(&mut self, capabilities: u32) {
        self.capabilities = capabilities;
    }

    pub fn fpga_version(&self) -> u8 {
        self.version
    }

    pub fn set_fpga_version(&mut self, version: u8) {
        self.version = version;
    }

    /// The menu button (bit 6 of `$0A`). The firmware looks every 15 ms
    /// for a change from released to pressed; held for a second when the
    /// menu opens means "swap disk" instead. It must read released at
    /// power-on.
    pub fn set_menu_button(&mut self, pressed: bool) {
        self.menu = pressed;
    }

    pub fn menu_button(&self) -> bool {
        self.menu
    }

    /// The three button inputs, bits 2–0, which read as bits 7–5 of
    /// `$0A` (button 1 is the same bit as the menu button). This board's
    /// firmware reads only bit 6.
    pub fn set_buttons(&mut self, buttons: u8) {
        self.buttons = buttons & 7;
    }

    /// A byte arriving on the UART's receive line. The FIFO holds 15;
    /// from 14 on it counts as almost full, a byte arriving then sets the
    /// overrun flag, and a byte arriving at 15 is lost as well.
    pub fn uart_input(&mut self, byte: u8) {
        if self.rx.len() >= RX_ALMOST_FULL {
            self.overrun = true;
        }
        if self.rx.len() < RX_DEPTH {
            self.rx.push_back(byte);
        }
    }

    /// The lamp that the three busy registers share.
    pub fn busy_led(&self) -> bool {
        self.usb_busy || self.sd_busy || self.printer_busy
    }

    pub fn usb_busy(&self) -> bool {
        self.usb_busy
    }

    pub fn sd_busy(&self) -> bool {
        self.sd_busy
    }

    pub fn printer_busy(&self) -> bool {
        self.printer_busy
    }

    /// The output pins of `$26`.
    pub fn misc_io(&self) -> u8 {
        self.misc_io
    }

    /// What a read of `addr` returns at the clock `now` with the
    /// interrupt state `irq`, without side effects and exact (unlike
    /// [`IoDevice::peek`], which knows only the last access). `now` must
    /// not lie before the last access or tick.
    pub fn peek_at(&self, addr: u32, irq: &Irq, now: u64) -> u8 {
        let a = (addr & 0x3f) as u8;
        match a {
            0x00 => u8::from(irq.global),
            0x01 => irq.mask,
            0x03 => EDGE_MASK,
            0x05 => irq.active(),
            0x06 => self.timer_at(now),
            0x07 => u8::from(self.irq_timer_en) | u8::from(self.irq_timer_select) << 1,
            0x08 => (self.counter_at(now) >> 8) as u8,
            0x09 => self.counter_at(now) as u8,
            0x0a => self.buttons << 5 | if self.menu { BUTTON_MENU } else { 0 },
            0x0b => self.version,
            0x0c..=0x0f => self.capabilities.to_be_bytes()[usize::from(a - 0x0c)],
            0x10..=0x1f => match a & 3 {
                0 => self.rx.front().copied().unwrap_or(0),
                2 => self.uart_flags(),
                _ => 0,
            },
            0x22 => (Self::ms_at(now) >> 8) as u8,
            0x23 => Self::ms_at(now) as u8,
            0x27 => irq.mask_high,
            0x28 => irq.high_active(),
            _ => 0,
        }
    }

    fn uart_flags(&self) -> u8 {
        let mut f = uart_flag::TX_IDLE;
        if self.overrun {
            f |= uart_flag::OVERRUN;
        }
        if self.rx.len() >= RX_ALMOST_FULL {
            f |= uart_flag::RX_FULL;
        }
        if !self.rx.is_empty() {
            f |= uart_flag::RX_AVAILABLE;
        }
        f
    }

    fn ms_at(now: u64) -> u16 {
        (now / MS_STEP) as u16
    }

    /// The countdown `$06` at `now`.
    fn timer_at(&self, now: u64) -> u8 {
        let steps = (now / TIMER_STEP).saturating_sub(self.timer_step);
        u8::try_from(steps).map_or(0, |s| self.timer.saturating_sub(s))
    }

    fn running(&self) -> bool {
        self.irq_timer_en && !self.irq_timer_select
    }

    fn reload(&self) -> u32 {
        u32::from(self.irq_timer_val) << 8 | 0xff
    }

    /// The periodic timer's counter at `now` (all pulses up to `now`
    /// taken, or `now` before the next one).
    fn counter_at(&self, now: u64) -> u32 {
        if !self.running() {
            return self.irq_timer_cnt;
        }
        let gone = now.saturating_sub(self.irq_timer_at);
        u32::try_from(gone).map_or(0, |g| self.irq_timer_cnt.saturating_sub(g))
    }

    /// The clock of the running timer's next pulse: the counter is 0 one
    /// clock before and loaded anew at this clock.
    fn next_pulse(&self) -> u64 {
        self.irq_timer_at
            .saturating_add(u64::from(self.irq_timer_cnt))
            .saturating_add(1)
    }

    /// Bring the periodic timer up to the clock `to`: every pulse due by
    /// then goes to interrupt source 0.
    fn advance(&mut self, to: u64, irq: &mut Irq) {
        if !self.irq_timer_en {
            self.irq_timer_high = false;
            return;
        }
        if self.irq_timer_select {
            // No external pulses: only a counter that stands at 0 acts.
            // It reloads the bare value; with a reload of 0 it stays at 0
            // and the output stays high, which is one edge.
            if self.irq_timer_cnt == 0 {
                if !self.irq_timer_high {
                    irq.pulse(0);
                }
                self.irq_timer_cnt = u32::from(self.irq_timer_val);
                self.irq_timer_high = self.irq_timer_val == 0;
            } else {
                self.irq_timer_high = false;
            }
            return;
        }
        let first = self.next_pulse();
        if first > to {
            return;
        }
        // The reload value cannot have changed between `first` and `to`
        // (a write brings the timer up to its own clock first), so the
        // later pulses follow at the full period.
        let period = u64::from(self.reload()) + 1;
        let more = (to - first) / period;
        if more > 0 || !self.irq_timer_high {
            irq.pulse(0);
        }
        self.irq_timer_high = false;
        self.irq_timer_cnt = self.reload();
        self.irq_timer_at = first + more * period;
    }

    fn seen(&mut self, ctx: &IoCtx<'_>) {
        self.seen_now = ctx.now;
        self.seen_irq = ctx.irq.clone();
    }
}

impl IoDevice for Itu {
    fn read(&mut self, addr: u32, ctx: &mut IoCtx<'_>) -> u8 {
        // No read here has a side effect of its own (the UART's data
        // register keeps its byte until `$11` is written).
        self.advance(ctx.now, ctx.irq);
        self.seen(ctx);
        self.peek_at(addr, ctx.irq, ctx.now)
    }

    fn peek(&self, addr: u32) -> u8 {
        self.peek_at(addr, &self.seen_irq, self.seen_now)
    }

    fn write(&mut self, addr: u32, v: u8, ctx: &mut IoCtx<'_>) {
        let now = ctx.now;
        // Pulses before this clock first; one that falls on this clock
        // comes after the write, so that it survives a clear.
        if let Some(before) = now.checked_sub(1) {
            self.advance(before, ctx.irq);
        }
        let a = (addr & 0x3f) as u8;
        match a {
            0x00 => ctx.irq.global = v & 1 != 0,
            0x01 => ctx.irq.mask |= v,
            0x02 => ctx.irq.mask &= !v,
            0x04 => ctx.irq.clear(v),
            0x06 => {
                self.timer = v;
                self.timer_step = now / TIMER_STEP;
            }
            0x07 => {
                // A timer that stood is loaded, one that ran keeps its
                // count (and keeps it when it is stopped).
                let cnt = if self.irq_timer_en {
                    self.counter_at(now)
                } else {
                    self.reload()
                };
                self.irq_timer_en = v & 1 != 0;
                self.irq_timer_select = v & 2 != 0;
                self.irq_timer_cnt = cnt;
                self.irq_timer_at = now;
            }
            0x08 => self.irq_timer_val = self.irq_timer_val & 0x00ff | u16::from(v) << 8,
            0x09 => self.irq_timer_val = self.irq_timer_val & 0xff00 | u16::from(v),
            0x10..=0x1f => match a & 3 {
                0 => ctx.console.push(v),
                1 => {
                    self.rx.pop_front();
                }
                2 => self.overrun &= v & 1 == 0,
                _ => {}
            },
            0x24 => self.usb_busy = v & 1 != 0,
            0x25 => self.sd_busy = v & 1 != 0,
            0x26 => self.misc_io = v,
            0x27 => ctx.irq.mask_high = v,
            0x29 => self.printer_busy = v & 1 != 0,
            _ => {}
        }
        self.advance(now, ctx.irq);
        self.seen(ctx);
    }

    /// The periodic timer's next pulse while it runs, else never.
    fn next_event(&self) -> u64 {
        if self.running() {
            self.next_pulse()
        } else {
            u64::MAX
        }
    }

    fn tick(&mut self, ctx: &mut IoCtx<'_>) {
        self.advance(ctx.now, ctx.irq);
        self.seen(ctx);
    }

    /// Power-on state of the registers. What comes from outside stays:
    /// capabilities, version, the buttons and bytes on the way in. The
    /// millisecond counter and the countdown's divider follow the clock
    /// and start anew only if the machine starts the clock anew.
    fn reset(&mut self) {
        let fresh = Self {
            version: self.version,
            buttons: self.buttons,
            menu: self.menu,
            seen_now: self.seen_now,
            ..Self::new(self.capabilities)
        };
        *self = fresh;
    }

    fn save(&self, w: &mut StateWriter) {
        w.u32(self.capabilities);
        w.u8(self.version);
        w.u8(self.buttons);
        w.bool(self.menu);
        w.u8(self.timer);
        w.u64(self.timer_step);
        w.bool(self.irq_timer_en);
        w.bool(self.irq_timer_select);
        w.u16(self.irq_timer_val);
        w.u32(self.irq_timer_cnt);
        w.u64(self.irq_timer_at);
        w.bool(self.irq_timer_high);
        w.u8(self.rx.len() as u8);
        for &b in &self.rx {
            w.u8(b);
        }
        w.bool(self.overrun);
        w.bool(self.usb_busy);
        w.bool(self.sd_busy);
        w.bool(self.printer_busy);
        w.u8(self.misc_io);
        w.u64(self.seen_now);
        self.seen_irq.save(w);
    }

    /// Corrupt data gives an error and leaves the state as it was.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let capabilities = r.u32()?;
        let version = r.u8()?;
        let buttons = r.u8()?;
        if buttons > 7 {
            return Err(StateError::Invalid("ITU buttons"));
        }
        let menu = r.bool()?;
        let timer = r.u8()?;
        let timer_step = r.u64()?;
        let irq_timer_en = r.bool()?;
        let irq_timer_select = r.bool()?;
        let irq_timer_val = r.u16()?;
        let irq_timer_cnt = r.u32()?;
        if irq_timer_cnt > 0x00ff_ffff {
            return Err(StateError::Invalid("ITU interrupt timer counter"));
        }
        let irq_timer_at = r.u64()?;
        let irq_timer_high = r.bool()?;
        let n = usize::from(r.u8()?);
        if n > RX_DEPTH {
            return Err(StateError::Invalid("ITU UART receive FIFO length"));
        }
        let rx: VecDeque<u8> = r.raw(n)?.iter().copied().collect();
        let overrun = r.bool()?;
        let usb_busy = r.bool()?;
        let sd_busy = r.bool()?;
        let printer_busy = r.bool()?;
        let misc_io = r.u8()?;
        let seen_now = r.u64()?;
        let mut seen_irq = Irq::default();
        seen_irq.load(r)?;
        *self = Self {
            capabilities,
            version,
            buttons,
            menu,
            timer,
            timer_step,
            irq_timer_en,
            irq_timer_select,
            irq_timer_val,
            irq_timer_cnt,
            irq_timer_at,
            irq_timer_high,
            rx,
            overrun,
            usb_busy,
            sd_busy,
            printer_busy,
            misc_io,
            seen_now,
            seen_irq,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::NullC64;

    /// The firmware's tick: reload `$07A0`.
    const TICK: u64 = 499_968;

    struct Rig {
        itu: Itu,
        irq: Irq,
        console: Vec<u8>,
        c64: NullC64,
        ram: Vec<u8>,
        now: u64,
    }

    impl Rig {
        fn new(capabilities: u32) -> Self {
            Self {
                itu: Itu::new(capabilities),
                irq: Irq::default(),
                console: Vec::new(),
                c64: NullC64::default(),
                ram: vec![0; 16],
                now: 0,
            }
        }

        fn with<R>(&mut self, f: impl FnOnce(&mut Itu, &mut IoCtx<'_>) -> R) -> R {
            let mut ctx = IoCtx {
                now: self.now,
                ram: &mut self.ram,
                irq: &mut self.irq,
                console: &mut self.console,
                c64: &mut self.c64,
                storage: None,
                net: None,
            };
            f(&mut self.itu, &mut ctx)
        }

        fn rd(&mut self, addr: u32) -> u8 {
            self.with(|i, c| i.read(addr, c))
        }

        fn wr(&mut self, addr: u32, v: u8) {
            self.with(|i, c| i.write(addr, v, c));
        }

        /// Run to the clock `to` the way the machine does: tick at every
        /// event on the way.
        fn run_to(&mut self, to: u64) {
            loop {
                let ev = self.itu.next_event();
                if ev > to {
                    break;
                }
                self.now = ev;
                self.with(|i, c| i.tick(c));
            }
            self.now = to;
        }

        /// The firmware's set-up of its tick, in its order.
        fn start_tick(&mut self) {
            self.wr(0x27, 0x00);
            self.wr(0x07, 0x00);
            self.wr(0x02, 0xff);
            self.wr(0x04, 0xff);
            self.wr(0x08, 0x07);
            self.wr(0x09, 0xa0);
            self.wr(0x07, 0x01);
            self.wr(0x01, 0x01);
            self.wr(0x00, 0x01);
        }
    }

    #[test]
    fn power_on_values() {
        let mut r = Rig::new(0x3400_0200);
        assert_eq!(r.rd(0x00), 1, "global enable set");
        assert_eq!(r.rd(0x01), 0);
        assert_eq!(r.rd(0x03), 0x85);
        assert_eq!(r.rd(0x05), 0);
        assert_eq!(r.rd(0x06), 0);
        assert_eq!(r.rd(0x07), 0);
        assert_eq!(r.rd(0x0a), 0, "H12: no button held at boot");
        assert_eq!(r.rd(0x0b), FPGA_VERSION);
        assert_eq!(r.rd(0x27), 0);
        assert_eq!(r.rd(0x28), 0);
        assert_eq!(r.itu.next_event(), u64::MAX);
        assert!(!r.irq.line());
    }

    #[test]
    fn capabilities_read_most_significant_byte_first() {
        let mut r = Rig::new(0x3de4_0be7);
        let bytes = [r.rd(0x0c), r.rd(0x0d), r.rd(0x0e), r.rd(0x0f)];
        assert_eq!(bytes, [0x3d, 0xe4, 0x0b, 0xe7]);
        r.itu
            .set_capabilities(capab::ULTIMATE64 | capab::CARTRIDGE | 3 << 28);
        assert_eq!(r.rd(0x0c), 0x34);
        assert_eq!(r.rd(0x0e), 0x02);
        assert_eq!(r.itu.capabilities(), 0x3400_0200);
        r.itu.set_fpga_version(0x11);
        assert_eq!(r.rd(0x0b), 0x11);
    }

    #[test]
    fn h1_console_never_blocks_and_gets_the_bytes() {
        let mut r = Rig::new(0);
        for &b in b"-- Hi --\r\n" {
            assert_eq!(r.rd(0x12) & uart_flag::TX_FULL, 0);
            r.wr(0x10, b);
        }
        assert_eq!(r.rd(0x12), uart_flag::TX_IDLE);
        assert_eq!(r.console, b"-- Hi --\r\n");
    }

    #[test]
    fn h14_early_trap_mark_is_swallowed() {
        let mut r = Rig::new(0);
        r.wr(0x1f, 0x49);
        assert_eq!(r.rd(0x1f), 0);
        assert_eq!(r.rd(0x13), 0, "no interrupt control in the UART");
        assert!(r.console.is_empty());
        // The other registers of the UART repeat through $14-$1F.
        r.wr(0x1c, b'x');
        assert_eq!(r.console, b"x");
        assert_eq!(r.rd(0x16), uart_flag::TX_IDLE);
    }

    #[test]
    fn h2_countdown_follows_the_clock_without_interrupts() {
        let mut r = Rig::new(0);
        r.irq.global = false;
        r.now = 1_000;
        r.wr(0x06, 200);
        assert_eq!(r.rd(0x06), 200);
        r.now = 1_000 + 499;
        assert_eq!(r.rd(0x06), 200);
        r.now = 1_000 + 500;
        assert_eq!(r.rd(0x06), 199);
        r.now = 1_000 + 199 * 500;
        assert_eq!(r.rd(0x06), 1);
        r.now = 1_000 + 200 * 500;
        assert_eq!(r.rd(0x06), 0);
        // It stays at 0.
        r.now = 1_000_000_000_000;
        assert_eq!(r.rd(0x06), 0);
        assert_eq!(r.irq.flags, 0);
    }

    #[test]
    fn countdown_steps_on_a_free_running_divider() {
        let mut r = Rig::new(0);
        // Written one clock before the divider's step: the first step
        // comes at once.
        r.now = 499;
        r.wr(0x06, 3);
        r.now = 500;
        assert_eq!(r.rd(0x06), 2);
        r.now = 1_499;
        assert_eq!(r.rd(0x06), 1);
        r.now = 1_500;
        assert_eq!(r.rd(0x06), 0);
        // A new value starts a new countdown.
        r.wr(0x06, 20);
        r.now = 1_500 + 20 * 500 - 1;
        assert_eq!(r.rd(0x06), 1);
        r.now += 1;
        assert_eq!(r.rd(0x06), 0);
    }

    #[test]
    fn h3_budget_loop_of_one_millisecond_rounds() {
        // The firmware's pattern: when the countdown is 0, count one
        // round and start it again with 200.
        let mut r = Rig::new(0);
        r.now = 12_345;
        let start = r.now;
        let mut rounds = 0;
        r.wr(0x06, 200);
        while rounds < 25 {
            if r.rd(0x06) == 0 {
                rounds += 1;
                r.wr(0x06, 200);
            }
            r.now += 37;
        }
        let ms = (r.now - start) / MS_STEP;
        assert!((24..=25).contains(&ms), "25 rounds took {ms} ms");
    }

    #[test]
    fn h4_h5_millisecond_counter_is_stable_and_advances() {
        let mut r = Rig::new(0);
        r.now = 7 * MS_STEP + 99_999;
        let a = (r.rd(0x23), r.rd(0x22));
        let b = (r.rd(0x23), r.rd(0x22));
        assert_eq!(a, b);
        assert_eq!(a, (7, 0));
        r.now += 1;
        assert_eq!(r.rd(0x23), 8);
        r.now = 0x1234 * MS_STEP;
        assert_eq!((r.rd(0x22), r.rd(0x23)), (0x12, 0x34));
        // 16 bits.
        r.now = 0x1_0005 * MS_STEP;
        assert_eq!((r.rd(0x22), r.rd(0x23)), (0x00, 0x05));
    }

    #[test]
    fn h6_tick_every_499968_clocks_from_the_start_write() {
        let mut r = Rig::new(0);
        r.now = 3_000;
        r.start_tick();
        assert_eq!(r.rd(0x07), 1);
        assert_eq!(r.itu.next_event(), 3_000 + TICK);
        r.run_to(3_000 + TICK - 1);
        assert_eq!(r.rd(0x05), 0);
        assert!(!r.irq.line());
        r.run_to(3_000 + TICK);
        assert!(r.irq.line());
        assert_eq!(r.rd(0x05), 1);
        assert_eq!(r.itu.next_event(), 3_000 + 2 * TICK);
    }

    #[test]
    fn h7_active_then_clear_drops_the_line_until_the_next_tick() {
        let mut r = Rig::new(0);
        r.start_tick();
        let mut ticks = 0;
        for n in 1..=10 {
            r.run_to(n * TICK + 100);
            assert!(r.irq.line());
            let pending = r.rd(0x05);
            assert_eq!(pending, 1);
            r.wr(0x04, pending);
            assert_eq!(r.rd(0x05), 0);
            assert!(!r.irq.line());
            ticks += 1;
        }
        assert_eq!(ticks, 10);
    }

    #[test]
    fn tick_latches_while_disabled_and_fires_on_enable() {
        let mut r = Rig::new(0);
        r.start_tick();
        r.wr(0x02, 0x01);
        r.run_to(TICK + 5);
        assert_eq!(r.irq.flags, 1, "flag set regardless of the enable");
        assert_eq!(r.rd(0x05), 0, "ACTIVE shows enabled sources only");
        assert!(!r.irq.line());
        r.wr(0x01, 0x01);
        assert_eq!(r.rd(0x05), 1);
        assert!(r.irq.line());
        // The global enable gates the line, not the register.
        r.wr(0x00, 0);
        assert_eq!(r.rd(0x00), 0);
        assert!(!r.irq.line());
        assert_eq!(r.rd(0x05), 1);
    }

    #[test]
    fn several_periods_collapse_into_one_flag() {
        let mut lazy = Rig::new(0);
        lazy.start_tick();
        // Nobody looks for 7.5 periods (no tick calls at all).
        lazy.now = 7 * TICK + TICK / 2;
        assert_eq!(lazy.rd(0x05), 1);
        lazy.wr(0x04, 1);
        assert_eq!(lazy.rd(0x05), 0, "one flag, cleared once");
        // The phase is kept: the next pulse is the eighth.
        assert_eq!(lazy.itu.next_event(), 8 * TICK);

        // The same with a tick at every event gives the same state.
        let mut eager = Rig::new(0);
        eager.start_tick();
        eager.run_to(7 * TICK + TICK / 2);
        assert_eq!(eager.rd(0x05), 1);
        eager.wr(0x04, 1);
        assert_eq!(eager.rd(0x05), 0);
        assert_eq!(eager.itu, lazy.itu);
        assert_eq!(eager.irq, lazy.irq);
    }

    #[test]
    fn pulse_on_the_clock_of_a_clear_survives() {
        let mut r = Rig::new(0);
        r.start_tick();
        r.run_to(TICK + 10);
        // The handler is late and clears exactly when the next pulse
        // comes.
        r.now = 2 * TICK;
        r.wr(0x04, 1);
        assert_eq!(r.rd(0x05), 1);
        // One clock earlier the flag goes and comes back with the pulse.
        let mut r = Rig::new(0);
        r.start_tick();
        r.now = 2 * TICK - 1;
        r.wr(0x04, 1);
        assert_eq!(r.rd(0x05), 0);
        r.now = 2 * TICK;
        assert_eq!(r.rd(0x05), 1);
    }

    #[test]
    fn irq_timer_counter_reads_back() {
        let mut r = Rig::new(0);
        r.now = 50;
        // Standing timer: the write to $07 loads the power-on reload.
        r.wr(0x07, 0x00);
        assert_eq!((r.rd(0x08), r.rd(0x09)), (0x00, 0xff));
        r.wr(0x08, 0x07);
        r.wr(0x09, 0xa0);
        // The reload value itself cannot be read; the counter is still
        // the old one.
        assert_eq!((r.rd(0x08), r.rd(0x09)), (0x00, 0xff));
        r.wr(0x07, 0x01);
        // $07A0FF: bits 15-8 and 7-0.
        assert_eq!((r.rd(0x08), r.rd(0x09)), (0xa0, 0xff));
        r.now += 1;
        assert_eq!((r.rd(0x08), r.rd(0x09)), (0xa0, 0xfe));
        r.now += 0xff;
        assert_eq!((r.rd(0x08), r.rd(0x09)), (0x9f, 0xff));
        // One clock before the pulse the counter is 0, then loaded.
        r.now = 50 + TICK - 1;
        assert_eq!((r.rd(0x08), r.rd(0x09)), (0x00, 0x00));
        assert_eq!(r.irq.flags, 0);
        r.now = 50 + TICK;
        assert_eq!((r.rd(0x08), r.rd(0x09)), (0xa0, 0xff));
        assert_eq!(r.irq.flags, 1);
    }

    #[test]
    fn stopping_freezes_and_starting_reloads() {
        let mut r = Rig::new(0);
        r.wr(0x08, 0x00);
        r.wr(0x09, 0x01);
        r.wr(0x07, 0x01);
        assert_eq!(r.itu.next_event(), 512);
        r.now = 100;
        r.wr(0x07, 0x00);
        assert_eq!(r.itu.next_event(), u64::MAX);
        r.now = 10_000;
        assert_eq!((r.rd(0x08), r.rd(0x09)), (0x01, 0xff - 100));
        assert_eq!(r.irq.flags, 0);
        // A new reload value takes effect at the start.
        r.wr(0x09, 0x02);
        r.wr(0x07, 0x01);
        assert_eq!(r.itu.next_event(), 10_000 + 3 * 256);
        // Writing $07 while it runs does not load.
        r.now = 10_010;
        r.wr(0x07, 0x01);
        assert_eq!(r.itu.next_event(), 10_000 + 3 * 256);
        // A reload written while it runs counts from the next pulse.
        r.wr(0x09, 0x00);
        r.run_to(10_000 + 3 * 256);
        assert_eq!(r.irq.flags, 1);
        assert_eq!(r.itu.next_event(), 10_000 + 4 * 256);
    }

    #[test]
    fn external_pulse_mode_stands_still() {
        let mut r = Rig::new(0);
        r.wr(0x09, 0x05);
        r.wr(0x08, 0x00);
        r.wr(0x07, 0x03);
        assert_eq!(r.rd(0x07), 3);
        assert_eq!(r.itu.next_event(), u64::MAX);
        r.now = 1_000_000;
        assert_eq!((r.rd(0x08), r.rd(0x09)), (0x05, 0xff));
        assert_eq!(r.irq.flags, 0);
        // Back to the clock: it goes on from where it stood.
        r.wr(0x07, 0x01);
        assert_eq!(r.itu.next_event(), 1_000_000 + 0x5ff + 1);
    }

    #[test]
    fn external_pulse_mode_at_zero_gives_one_edge() {
        let mut r = Rig::new(0);
        r.wr(0x08, 0x00);
        r.wr(0x09, 0x00);
        r.wr(0x07, 0x01);
        // Switch over exactly when the counter is 0.
        r.now = 255;
        r.wr(0x07, 0x03);
        assert_eq!(r.irq.flags, 1);
        r.wr(0x04, 1);
        r.now = 5_000;
        assert_eq!(r.rd(0x09), 0);
        assert_eq!(r.irq.flags, 0, "the output stays high: no new edge");
        // Back on the clock the output falls after one more reload.
        r.wr(0x07, 0x01);
        r.run_to(5_001);
        assert_eq!(r.irq.flags, 0);
        r.run_to(5_001 + 256);
        assert_eq!(r.irq.flags, 1);
    }

    #[test]
    fn enable_and_disable_registers_set_and_clear_bits() {
        let mut r = Rig::new(0);
        r.wr(0x01, 0x81);
        r.wr(0x01, 0x10);
        assert_eq!(r.rd(0x01), 0x91);
        r.wr(0x02, 0x80);
        assert_eq!(r.rd(0x01), 0x11);
        assert_eq!(r.rd(0x02), 0);
        assert_eq!(r.rd(0x04), 0);
        // The edge register is fixed.
        r.wr(0x03, 0xff);
        assert_eq!(r.rd(0x03), 0x85);
        // Levels show in ACTIVE without a flag and need no clear.
        r.irq.set_level(4, true);
        assert_eq!(r.rd(0x05), 0x10);
        r.wr(0x04, 0xff);
        assert_eq!(r.rd(0x05), 0x10);
        r.irq.set_level(4, false);
        assert_eq!(r.rd(0x05), 0);
    }

    #[test]
    fn h8_high_sources_are_levels_behind_their_enables() {
        let mut r = Rig::new(0);
        // install_high_irq: read, set a bit, write back.
        let en = r.rd(0x27);
        r.wr(0x27, en | 1 << 5);
        let en = r.rd(0x27);
        r.wr(0x27, en | 1 << 6);
        assert_eq!(r.rd(0x27), 0x60);
        assert_eq!(r.rd(0x28), 0);
        assert!(!r.irq.line());
        r.irq.set_high(3, true);
        assert_eq!(r.rd(0x28), 0, "not enabled: not shown");
        r.irq.set_high(5, true);
        assert_eq!(r.rd(0x28), 0x20);
        assert!(r.irq.line());
        // No handler: the firmware clears the enable.
        r.wr(0x27, 0x40);
        assert_eq!(r.rd(0x28), 0);
        assert!(!r.irq.line());
    }

    #[test]
    fn h12_menu_button_is_bit_6() {
        let mut r = Rig::new(0);
        r.itu.set_menu_button(true);
        assert!(r.itu.menu_button());
        assert_eq!(r.rd(0x0a), 0x40);
        r.itu.set_menu_button(false);
        assert_eq!(r.rd(0x0a), 0x00);
        r.itu.set_buttons(0b101);
        assert_eq!(r.rd(0x0a), 0xa0);
        r.itu.set_buttons(0b010);
        assert_eq!(r.rd(0x0a), 0x40);
    }

    #[test]
    fn uart_receive_fifo_and_flags() {
        let mut r = Rig::new(0);
        assert_eq!(r.rd(0x10), 0);
        r.itu.uart_input(b'a');
        r.itu.uart_input(b'b');
        assert_eq!(r.rd(0x12), uart_flag::TX_IDLE | uart_flag::RX_AVAILABLE);
        // Reading does not take the byte; a write to $11 does.
        assert_eq!(r.rd(0x10), b'a');
        assert_eq!(r.rd(0x10), b'a');
        r.wr(0x11, 0);
        assert_eq!(r.rd(0x10), b'b');
        r.wr(0x11, 0);
        assert_eq!(r.rd(0x12), uart_flag::TX_IDLE);
        r.wr(0x11, 0);
        assert_eq!(r.rd(0x10), 0);

        for n in 0..13 {
            r.itu.uart_input(n);
        }
        assert_eq!(r.rd(0x12) & uart_flag::RX_FULL, 0);
        r.itu.uart_input(13);
        assert_eq!(
            r.rd(0x12),
            uart_flag::TX_IDLE | uart_flag::RX_AVAILABLE | uart_flag::RX_FULL
        );
        // The fifteenth still fits but counts as an overrun; the
        // sixteenth is lost.
        r.itu.uart_input(14);
        r.itu.uart_input(15);
        assert_ne!(r.rd(0x12) & uart_flag::OVERRUN, 0);
        r.wr(0x12, 0);
        assert_ne!(r.rd(0x12) & uart_flag::OVERRUN, 0);
        r.wr(0x12, 1);
        assert_eq!(r.rd(0x12) & uart_flag::OVERRUN, 0);
        for n in 0..15 {
            assert_eq!(r.rd(0x10), n);
            r.wr(0x11, 0);
        }
        assert_eq!(r.rd(0x12), uart_flag::TX_IDLE);
    }

    #[test]
    fn busy_lamp_and_output_pins() {
        let mut r = Rig::new(0);
        assert!(!r.itu.busy_led());
        r.wr(0x24, 1);
        assert!(r.itu.usb_busy() && r.itu.busy_led());
        r.wr(0x29, 0xff);
        r.wr(0x24, 0xfe);
        assert!(!r.itu.usb_busy() && r.itu.printer_busy() && r.itu.busy_led());
        r.wr(0x29, 0);
        r.wr(0x25, 1);
        assert!(r.itu.sd_busy() && r.itu.busy_led());
        r.wr(0x25, 0);
        assert!(!r.itu.busy_led());
        r.wr(0x26, 0x5a);
        assert_eq!(r.itu.misc_io(), 0x5a);
        // Write-only.
        for a in [0x24, 0x25, 0x26, 0x29] {
            assert_eq!(r.rd(a), 0);
        }
    }

    #[test]
    fn unlisted_addresses_read_zero_and_the_block_repeats() {
        let mut r = Rig::new(0x1122_3344);
        for a in (0x20..0x22).chain(0x2a..0x40) {
            r.wr(a, 0xff);
            assert_eq!(r.rd(a), 0, "address {a:#x}");
        }
        assert_eq!(r.rd(0x4c), 0x11);
        assert_eq!(r.rd(0xcf), 0x44);
        r.wr(0xd0, b'!');
        assert_eq!(r.console, b"!");
    }

    #[test]
    fn peek_matches_read_and_changes_nothing() {
        let mut r = Rig::new(0x3400_0200);
        r.start_tick();
        r.wr(0x06, 100);
        r.wr(0x27, 0x21);
        r.irq.set_high(0, true);
        r.itu.uart_input(b'k');
        r.itu.set_menu_button(true);
        r.run_to(3 * TICK + 12_345);
        let read: Vec<u8> = (0..0x100).map(|a| r.rd(a)).collect();
        let before = (r.itu.clone(), r.irq.clone());
        let peek: Vec<u8> = (0..0x100).map(|a| r.itu.peek(a)).collect();
        assert_eq!(read, peek);
        assert_eq!((r.itu.clone(), r.irq.clone()), before);
        // peek_at looks ahead without moving anything.
        let ahead = r.now + 2 * TICK;
        assert_eq!(r.itu.peek_at(0x06, &r.irq, ahead), 0);
        assert_eq!(r.itu.peek_at(0x23, &r.irq, ahead), (ahead / MS_STEP) as u8);
        assert_eq!((r.itu.clone(), r.irq.clone()), before);
    }

    #[test]
    fn reset_gives_power_on_registers() {
        let mut r = Rig::new(0x3400_0200);
        r.start_tick();
        r.wr(0x06, 9);
        r.wr(0x24, 1);
        r.wr(0x26, 3);
        r.itu.uart_input(1);
        r.itu.set_menu_button(true);
        r.itu.set_fpga_version(0x33);
        r.itu.reset();
        r.irq.reset();
        assert_eq!(r.itu.next_event(), u64::MAX);
        assert_eq!(r.rd(0x07), 0);
        assert_eq!(r.rd(0x06), 0);
        assert_eq!(r.rd(0x00), 1);
        assert_eq!(r.rd(0x01), 0);
        assert!(!r.itu.busy_led());
        assert_eq!(r.itu.misc_io(), 0);
        assert_eq!(r.rd(0x12), uart_flag::TX_IDLE);
        // The reload is the power-on one again.
        r.wr(0x07, 1);
        assert_eq!(r.itu.next_event(), r.now + 0x8001 * 256);
        // What comes from outside stays.
        assert_eq!(r.rd(0x0c), 0x34);
        assert_eq!(r.rd(0x0b), 0x33);
        assert_eq!(r.rd(0x0a), 0x40);
    }

    #[test]
    fn deterministic_whatever_the_pattern_of_calls() {
        // Two runs of the same accesses; the second gets extra ticks in
        // between. Flags and counter must agree at the end.
        let run = |extra: bool| {
            let mut r = Rig::new(0);
            r.start_tick();
            let mut seen = 0u32;
            let mut t = 0;
            while t < 20 * TICK {
                t += 123_457;
                if extra {
                    r.now = t - 60_000;
                    r.with(|i, c| i.tick(c));
                    r.run_to(t);
                } else {
                    r.now = t;
                }
                if r.rd(0x05) & 1 != 0 {
                    r.wr(0x04, 1);
                    seen += 1;
                }
            }
            (seen, r.rd(0x08), r.rd(0x09), r.itu.next_event())
        };
        assert_eq!(run(false), run(true));
        // The handler polls four times per period: no tick is lost.
        assert_eq!(run(false).0, 20);
    }

    #[test]
    fn savestate_round_trip() {
        let mut r = Rig::new(0x3de4_0be7);
        r.start_tick();
        r.wr(0x06, 77);
        r.wr(0x24, 1);
        r.wr(0x26, 0xc3);
        r.wr(0x27, 0x60);
        for b in 0..15 {
            r.itu.uart_input(b);
        }
        r.itu.set_menu_button(true);
        r.itu.set_buttons(4);
        r.itu.set_fpga_version(0x42);
        r.run_to(2 * TICK + 4_711);
        r.rd(0x05);

        let mut w = StateWriter::new();
        r.itu.save(&mut w);
        let bytes = w.into_bytes();
        let mut d = Itu::new(0);
        d.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(r.itu, d);
        assert_eq!(d.next_event(), 3 * TICK);

        // Cut anywhere: an error, no panic, and the state untouched.
        for cut in 0..bytes.len() {
            let mut e = Itu::new(5);
            assert!(e.load(&mut StateReader::new(&bytes[..cut])).is_err());
            assert_eq!(e, Itu::new(5));
        }
    }

    #[test]
    fn savestate_rejects_values_out_of_range() {
        let mut w = StateWriter::new();
        Itu::new(0).save(&mut w);
        let good = w.into_bytes();
        let mut d = Itu::new(0);
        d.load(&mut StateReader::new(&good)).unwrap();

        // Buttons (offset 5), counter (offset 20..24, top byte), FIFO
        // length (offset 33).
        for (at, v) in [(5, 8u8), (23, 1), (33, 16)] {
            let mut bad = good.clone();
            bad[at] = v;
            assert!(d.load(&mut StateReader::new(&bad)).is_err(), "offset {at}");
        }
        // Any byte pattern: no panic, and what loads behaves.
        for at in 0..good.len() {
            for v in [0x01, 0x7f, 0xff] {
                let mut odd = good.clone();
                odd[at] = v;
                let mut e = Itu::new(0);
                if e.load(&mut StateReader::new(&odd)).is_ok() {
                    let mut rig = Rig::new(0);
                    rig.itu = e;
                    rig.now = u64::MAX - 3;
                    rig.with(|i, c| i.tick(c));
                    for a in 0..0x40 {
                        rig.rd(a);
                        let _ = rig.itu.peek(a);
                    }
                    let _ = rig.itu.next_event();
                }
            }
        }
    }
}
