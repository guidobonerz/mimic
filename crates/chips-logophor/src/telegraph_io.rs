//! Telegraph's I/O units (SPEC.md 10.2 and 10.3): four timers, the
//! real-time clock, the keyboard FIFO, the two mouse/joystick ports and
//! the SD card controller. The interrupt controller is in
//! [`crate::telegraph`], the network in [`crate::telegraph_net`].
//!
//! **[S]** (decided here, recorded in SPEC.md 10): the timers count at
//! 1 MHz (every 12th CPU cycle); a running timer at 0 underflows on its
//! next count and reloads TnLATCH (bit 1) or stops, so it underflows every
//! TnLATCH + 1 counts. The clock's seconds turn every 12 000 000 CPU
//! cycles; a write to RTCHI waits for the write to RTCLO, which sets the
//! clock and starts the second anew; reading RTCHI latches RTCLO for the
//! next read. The clock starts from the machine's `rtc` option (never the
//! host's time inside the machine). The SD card moves data through
//! Telegraph's fixed-DMA slots, 8 a line (32 bytes), so a block of 512
//! bytes takes 16 lines (about 0.42 ms); errors (no card, a block beyond
//! the card, a buffer beyond chip RAM, a write to a protected card) end
//! the command with the error and done bits and the interrupt.

use emu_core::{StateError, StateReader, StateWriter};

use crate::blitter::ChipRamMut;

pub const T0CNT: u16 = 0x4200;
pub const TSTAT: u16 = 0x4220;
pub const RTCHI: u16 = 0x4224;
pub const RTCLO: u16 = 0x4226;
pub const RTCALARM: u16 = 0x4228;
pub const KBDDATA: u16 = 0x4240;
pub const KBDSTAT: u16 = 0x4242;
pub const PORTCFG: u16 = 0x4248;
pub const MOUSE0X: u16 = 0x424a;
pub const MOUSE0Y: u16 = 0x424c;
pub const MOUSE1X: u16 = 0x424e;
pub const MOUSE1Y: u16 = 0x4250;
pub const PORT0: u16 = 0x4252;
pub const PORT1: u16 = 0x4254;
pub const SDSTAT: u16 = 0x4260;
pub const SDCMD: u16 = 0x4262;
pub const SDBLKH: u16 = 0x4264;
pub const SDBLKL: u16 = 0x4266;
pub const SDCOUNT: u16 = 0x4268;
pub const SDSEG: u16 = 0x426a;
pub const SDOFS: u16 = 0x426c;
pub const SDSIZEH: u16 = 0x426e;
pub const SDSIZEL: u16 = 0x4270;

pub const TCTL_RUN: u16 = 1 << 0;
pub const TCTL_RELOAD: u16 = 1 << 1;
pub const TCTL_CASCADE: u16 = 1 << 2;
pub const TCTL_IRQ: u16 = 1 << 3;

/// CPU cycles per timer count (12 MHz / 1 MHz) and per second.
const TIMER_DIV: u32 = 12;
const CYCLES_PER_SECOND: u32 = 12_000_000;
const CHIP_END: u32 = 0x0040_0000;
/// SD bytes per line (8 slots of 4 bytes).
const SD_BYTES_PER_LINE: u32 = 32;
pub const SD_SLOTS_PER_LINE: u16 = 8;
const KBD_FIFO: usize = 16;

/// What the timers and the clock raised in a cycle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Raised {
    pub timer: bool,
    pub rtc: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Timers {
    cnt: [u16; 4],
    latch: [u16; 4],
    ctl: [u16; 4],
    stat: u16,
    prescale: u32,
    /// Seconds since 2000-01-01, the cycles into the current second.
    seconds: u32,
    sub: u32,
    hi_pending: u16,
    lo_latched: Option<u16>,
    alarm: u16,
}

impl Timers {
    pub fn new(seconds: u32) -> Self {
        Self {
            seconds,
            ..Self::default()
        }
    }

    pub fn seconds(&self) -> u32 {
        self.seconds
    }

    pub fn peek(&self, off: u16) -> u16 {
        match off & !1 {
            o @ 0x4200..=0x421f => {
                let n = usize::from((o - T0CNT) >> 3);
                match o & 7 {
                    0 => self.cnt[n],
                    2 => self.latch[n],
                    4 => self.ctl[n],
                    _ => 0,
                }
            }
            TSTAT => self.stat,
            RTCHI => (self.seconds >> 16) as u16,
            RTCLO => self.lo_latched.unwrap_or(self.seconds as u16),
            RTCALARM => self.alarm,
            _ => 0,
        }
    }

    pub fn read(&mut self, off: u16) -> u16 {
        let v = self.peek(off);
        match off & !1 {
            TSTAT => self.stat = 0,
            RTCHI => self.lo_latched = Some(self.seconds as u16),
            RTCLO => self.lo_latched = None,
            _ => {}
        }
        v
    }

    pub fn write(&mut self, off: u16, v: u16) {
        match off & !1 {
            o @ 0x4200..=0x421f => {
                let n = usize::from((o - T0CNT) >> 3);
                match o & 7 {
                    0 => self.cnt[n] = v,
                    2 => self.latch[n] = v,
                    4 => self.ctl[n] = v & 0xf,
                    _ => {}
                }
            }
            RTCHI => self.hi_pending = v,
            RTCLO => {
                self.seconds = u32::from(self.hi_pending) << 16 | u32::from(v);
                self.sub = 0;
                self.lo_latched = None;
            }
            RTCALARM => self.alarm = v,
            _ => {}
        }
    }

    /// One CPU cycle.
    pub fn cycle(&mut self) -> Raised {
        let mut raised = Raised::default();
        self.sub += 1;
        if self.sub == CYCLES_PER_SECOND {
            self.sub = 0;
            self.seconds = self.seconds.wrapping_add(1);
            raised.rtc = self.seconds as u16 == self.alarm;
        }
        if self.ctl.iter().all(|c| c & TCTL_RUN == 0) {
            self.prescale = 0;
            return raised;
        }
        self.prescale += 1;
        if self.prescale < TIMER_DIV {
            return raised;
        }
        self.prescale = 0;
        let mut under = false;
        for n in 0..4 {
            let ctl = self.ctl[n];
            if ctl & TCTL_RUN == 0 {
                under = false;
                continue;
            }
            let count = if ctl & TCTL_CASCADE != 0 && n > 0 {
                under
            } else {
                true
            };
            under = false;
            if !count {
                continue;
            }
            if self.cnt[n] == 0 {
                under = true;
                self.stat |= 1 << n;
                if ctl & TCTL_IRQ != 0 {
                    raised.timer = true;
                }
                if ctl & TCTL_RELOAD != 0 {
                    self.cnt[n] = self.latch[n];
                } else {
                    self.ctl[n] &= !TCTL_RUN;
                }
            } else {
                self.cnt[n] -= 1;
            }
        }
        raised
    }

    pub fn save(&self, w: &mut StateWriter) {
        for n in 0..4 {
            w.u16(self.cnt[n]);
            w.u16(self.latch[n]);
            w.u16(self.ctl[n]);
        }
        w.u16(self.stat);
        w.u32(self.prescale);
        w.u32(self.seconds);
        w.u32(self.sub);
        w.u16(self.hi_pending);
        w.u32(self.lo_latched.map_or(0x1_0000, u32::from));
        w.u16(self.alarm);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut t = Timers::default();
        for n in 0..4 {
            t.cnt[n] = r.u16()?;
            t.latch[n] = r.u16()?;
            t.ctl[n] = r.u16()? & 0xf;
        }
        t.stat = r.u16()? & 0xf;
        t.prescale = r.u32()? % TIMER_DIV;
        t.seconds = r.u32()?;
        t.sub = r.u32()? % CYCLES_PER_SECOND;
        t.hi_pending = r.u16()?;
        let lo = r.u32()?;
        t.lo_latched = (lo <= 0xffff).then_some(lo as u16);
        t.alarm = r.u16()?;
        *self = t;
        Ok(())
    }
}

/// What is plugged into a port (PORTCFG, set by the machine).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PortDev {
    #[default]
    None,
    Mouse,
    Joystick,
}

impl PortDev {
    fn code(self) -> u16 {
        match self {
            PortDev::None => 0,
            PortDev::Mouse => 1,
            PortDev::Joystick => 2,
        }
    }
}

/// Keyboard FIFO and the two ports.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Input {
    fifo: Vec<u16>,
    overflow: bool,
    pub ports: [PortDev; 2],
    mouse: [(u16, u16); 2],
    /// PORTn bits: joystick directions and buttons, mouse buttons.
    state: [u16; 2],
}

impl Input {
    pub fn new(ports: [PortDev; 2]) -> Self {
        Self {
            ports,
            ..Self::default()
        }
    }

    /// A key event (USB HID usage); `true`: the keyboard interrupt.
    pub fn key(&mut self, usage: u8, down: bool) -> bool {
        if usage == 0 {
            return false;
        }
        if self.fifo.len() == KBD_FIFO {
            self.overflow = true;
            return false;
        }
        self.fifo
            .push(u16::from(usage) | if down { 0 } else { 0x8000 });
        true
    }

    /// Mouse motion and buttons (bits 0–2) in port `p`; `true`: the ports
    /// interrupt.
    pub fn mouse(&mut self, p: usize, dx: i16, dy: i16, buttons: u8) -> bool {
        if p > 1 || self.ports[p] != PortDev::Mouse {
            return false;
        }
        let m = &mut self.mouse[p];
        m.0 = m.0.wrapping_add(dx as u16);
        m.1 = m.1.wrapping_add(dy as u16);
        self.state[p] = u16::from(buttons & 7) << 4;
        true
    }

    /// Joystick bits (up, down, left, right, buttons 1–3) in port `p`.
    pub fn joystick(&mut self, p: usize, bits: u16) -> bool {
        if p > 1 || self.ports[p] != PortDev::Joystick || self.state[p] == bits & 0x7f {
            return false;
        }
        self.state[p] = bits & 0x7f;
        true
    }

    pub fn peek(&self, off: u16) -> u16 {
        match off & !1 {
            KBDDATA => self.fifo.first().copied().unwrap_or(0),
            KBDSTAT => u16::from(!self.fifo.is_empty()) | u16::from(self.overflow) << 1,
            PORTCFG => self.ports[0].code() | self.ports[1].code() << 2,
            MOUSE0X => self.mouse[0].0,
            MOUSE0Y => self.mouse[0].1,
            MOUSE1X => self.mouse[1].0,
            MOUSE1Y => self.mouse[1].1,
            PORT0 => self.state[0],
            PORT1 => self.state[1],
            _ => 0,
        }
    }

    pub fn read(&mut self, off: u16) -> u16 {
        let v = self.peek(off);
        match off & !1 {
            KBDDATA if !self.fifo.is_empty() => {
                self.fifo.remove(0);
            }
            KBDSTAT => self.overflow = false,
            _ => {}
        }
        v
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u8(self.fifo.len() as u8);
        for e in &self.fifo {
            w.u16(*e);
        }
        w.bool(self.overflow);
        for p in 0..2 {
            w.u8(self.ports[p].code() as u8);
            w.u16(self.mouse[p].0);
            w.u16(self.mouse[p].1);
            w.u16(self.state[p]);
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut s = Input::default();
        let n = usize::from(r.u8()?);
        if n > KBD_FIFO {
            return Err(StateError::Invalid("keyboard FIFO"));
        }
        for _ in 0..n {
            s.fifo.push(r.u16()?);
        }
        s.overflow = r.bool()?;
        for p in 0..2 {
            s.ports[p] = match r.u8()? {
                0 => PortDev::None,
                1 => PortDev::Mouse,
                2 => PortDev::Joystick,
                _ => return Err(StateError::Invalid("port device")),
            };
            s.mouse[p] = (r.u16()?, r.u16()?);
            s.state[p] = r.u16()? & 0x7f;
        }
        *self = s;
        Ok(())
    }
}

/// The card in the SD slot, as Telegraph's controller sees it.
pub trait SdCard {
    /// Size in 512-byte blocks (0: no card).
    fn blocks(&self) -> u32;
    fn protected(&self) -> bool;
    fn read_block(&mut self, block: u32, buf: &mut [u8; 512]) -> bool;
    fn write_block(&mut self, block: u32, buf: &[u8; 512]) -> bool;
}

pub const SD_READ: u16 = 1;
pub const SD_WRITE: u16 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sd {
    blk: (u16, u16),
    count: u16,
    buf_at: (u16, u16),
    busy: bool,
    error: bool,
    done: bool,
    cmd: u16,
    /// The running command: block, blocks left, chip address, offset in
    /// the block, the block's bytes.
    cur: u32,
    left: u32,
    at: u32,
    pos: u32,
    block: Vec<u8>,
    /// The card as last seen (SDSTAT bits 0 and 4, SDSIZE).
    present: bool,
    protected: bool,
    size: u32,
}

impl Default for Sd {
    fn default() -> Self {
        Self {
            blk: (0, 0),
            count: 0,
            buf_at: (0, 0),
            busy: false,
            error: false,
            done: false,
            cmd: 0,
            cur: 0,
            left: 0,
            at: 0,
            pos: 0,
            block: vec![0; 512],
            present: false,
            protected: false,
            size: 0,
        }
    }
}

impl Sd {
    /// The card changed (inserted, ejected): what SDSTAT and SDSIZE show.
    pub fn card(&mut self, card: &dyn SdCard) {
        self.size = card.blocks();
        self.present = self.size > 0;
        self.protected = card.protected();
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    pub fn peek(&self, off: u16) -> u16 {
        match off & !1 {
            SDSTAT => {
                u16::from(self.present)
                    | u16::from(self.busy) << 1
                    | u16::from(self.error) << 2
                    | u16::from(self.done) << 3
                    | u16::from(self.protected) << 4
            }
            SDCMD => self.cmd,
            SDBLKH => self.blk.0,
            SDBLKL => self.blk.1,
            SDCOUNT => self.count,
            SDSEG => self.buf_at.0,
            SDOFS => self.buf_at.1,
            SDSIZEH => (self.size >> 16) as u16,
            SDSIZEL => self.size as u16,
            _ => 0,
        }
    }

    pub fn read(&mut self, off: u16) -> u16 {
        let v = self.peek(off);
        if off & !1 == SDSTAT {
            self.done = false;
        }
        v
    }

    /// A register write; `true` when a command ended at once (an error,
    /// or no blocks): the SD interrupt.
    pub fn write(&mut self, off: u16, v: u16) -> bool {
        match off & !1 {
            SDCMD if !self.busy => {
                self.cmd = v;
                self.start();
                return !self.busy;
            }
            SDBLKH => self.blk.0 = v,
            SDBLKL => self.blk.1 = v,
            SDCOUNT => self.count = v,
            SDSEG => self.buf_at.0 = v,
            SDOFS => self.buf_at.1 = v,
            _ => {}
        }
        false
    }

    fn start(&mut self) {
        self.cur = u32::from(self.blk.0) << 16 | u32::from(self.blk.1);
        self.left = u32::from(self.count);
        self.at = u32::from(self.buf_at.0) << 16 | u32::from(self.buf_at.1);
        self.pos = 0;
        self.error = false;
        self.done = false;
        let fits = u64::from(self.cur) + u64::from(self.left) <= u64::from(self.size);
        let in_chip = u64::from(self.at) + 512 * u64::from(self.left) <= u64::from(CHIP_END);
        let ok = matches!(self.cmd, SD_READ | SD_WRITE)
            && self.present
            && fits
            && in_chip
            && !(self.cmd == SD_WRITE && self.protected);
        if !ok {
            self.error = true;
            self.done = true;
            self.busy = false;
            self.cmd = 0;
            return;
        }
        self.busy = self.left > 0;
        self.done = !self.busy;
        if !self.busy {
            self.cmd = 0;
        }
    }

    /// Slot 640: up to 32 bytes move; the slots taken and whether the
    /// command finished (the SD interrupt).
    pub fn line(
        &mut self,
        dma: bool,
        card: &mut dyn SdCard,
        mem: &mut dyn ChipRamMut,
    ) -> (u16, bool) {
        if !self.busy || !dma {
            return (0, false);
        }
        let mut buf = [0u8; 512];
        if self.pos == 0 && self.cmd == SD_READ {
            if !card.read_block(self.cur, &mut buf) {
                return self.fail();
            }
            self.block.copy_from_slice(&buf);
        }
        for _ in 0..SD_BYTES_PER_LINE {
            let p = self.pos as usize;
            if self.cmd == SD_READ {
                mem.set(self.at, self.block[p]);
            } else {
                self.block[p] = mem.get(self.at);
            }
            self.at += 1;
            self.pos += 1;
        }
        if self.pos == 512 {
            if self.cmd == SD_WRITE {
                buf.copy_from_slice(&self.block);
                if !card.write_block(self.cur, &buf) {
                    return self.fail();
                }
            }
            self.pos = 0;
            self.cur += 1;
            self.left -= 1;
            if self.left == 0 {
                self.busy = false;
                self.done = true;
                self.cmd = 0;
                return (SD_SLOTS_PER_LINE, true);
            }
        }
        (SD_SLOTS_PER_LINE, false)
    }

    fn fail(&mut self) -> (u16, bool) {
        self.busy = false;
        self.error = true;
        self.done = true;
        self.cmd = 0;
        (0, true)
    }

    pub fn save(&self, w: &mut StateWriter) {
        for v in [
            self.blk.0,
            self.blk.1,
            self.count,
            self.buf_at.0,
            self.buf_at.1,
            self.cmd,
        ] {
            w.u16(v);
        }
        w.bool(self.busy);
        w.bool(self.error);
        w.bool(self.done);
        w.u32(self.cur);
        w.u32(self.left);
        w.u32(self.at);
        w.u32(self.pos);
        w.raw(&self.block);
    }

    /// Loads the controller; the card's presence stays as the slot has
    /// it now (the machine calls [`Sd::card`] after loading).
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut s = Sd {
            present: self.present,
            protected: self.protected,
            size: self.size,
            ..Sd::default()
        };
        s.blk = (r.u16()?, r.u16()?);
        s.count = r.u16()?;
        s.buf_at = (r.u16()?, r.u16()?);
        s.cmd = r.u16()?;
        s.busy = r.bool()?;
        s.error = r.bool()?;
        s.done = r.bool()?;
        s.cur = r.u32()?;
        s.left = r.u32()?;
        s.at = r.u32()?;
        s.pos = r.u32()?;
        if s.pos >= 512 || !s.pos.is_multiple_of(SD_BYTES_PER_LINE) {
            return Err(StateError::Invalid("SD position"));
        }
        r.raw_into(&mut s.block)?;
        *self = s;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timers_count_reload_cascade_and_interrupt() {
        let mut t = Timers::new(0);
        t.write(T0CNT, 2);
        t.write(T0CNT + 2, 4);
        t.write(T0CNT + 4, TCTL_RUN | TCTL_RELOAD | TCTL_IRQ);
        // Timer 1 counts timer 0's underflows, once.
        t.write(T0CNT + 8, 1);
        t.write(T0CNT + 12, TCTL_RUN | TCTL_CASCADE);
        let mut irqs = Vec::new();
        for c in 0..12 * 20 {
            if t.cycle().timer {
                irqs.push(c / 12);
            }
        }
        // 2, 1, 0, underflow (count 3), then every 5 counts.
        assert_eq!(irqs, [2, 7, 12, 17]);
        assert_eq!(t.read(TSTAT), 0b11, "both underflowed");
        assert_eq!(t.read(TSTAT), 0, "cleared on read");
        assert_eq!(
            t.peek(T0CNT + 12) & TCTL_RUN,
            0,
            "timer 1 stopped (no reload)"
        );
    }

    #[test]
    fn the_clock_turns_sets_latches_and_rings() {
        let mut t = Timers::new(0x0001_fffe);
        t.write(RTCALARM, 0xffff);
        let mut rang = 0;
        for _ in 0..CYCLES_PER_SECOND {
            rang += u32::from(t.cycle().rtc);
        }
        assert_eq!((t.seconds(), rang), (0x0001_ffff, 1));
        assert_eq!(t.read(RTCHI), 1);
        for _ in 0..CYCLES_PER_SECOND {
            t.cycle();
        }
        assert_eq!(t.read(RTCLO), 0xffff, "latched when RTCHI was read");
        assert_eq!((t.read(RTCHI), t.read(RTCLO)), (2, 0));
        t.write(RTCHI, 0x1234);
        assert_eq!(t.seconds(), 0x0002_0000, "not before RTCLO");
        t.write(RTCLO, 0x5678);
        assert_eq!(t.seconds(), 0x1234_5678);
    }

    #[test]
    fn keyboard_fifo_and_ports() {
        let mut i = Input::new([PortDev::Mouse, PortDev::Joystick]);
        assert!(i.key(0x04, true));
        assert!(i.key(0x04, false));
        assert_eq!(i.read(KBDSTAT), 1);
        assert_eq!(i.read(KBDDATA), 0x0004);
        assert_eq!(i.read(KBDDATA), 0x8004);
        assert_eq!(i.read(KBDDATA), 0, "empty");
        for _ in 0..17 {
            i.key(0x05, true);
        }
        assert_eq!(i.read(KBDSTAT), 3, "waiting, overflow");
        assert_eq!(i.read(KBDSTAT), 1);
        assert_eq!(i.peek(PORTCFG), 1 | 2 << 2);
        assert!(i.mouse(0, -3, 5, 1));
        assert_eq!(
            (i.peek(MOUSE0X), i.peek(MOUSE0Y), i.peek(PORT0)),
            (0xfffd, 5, 0x10)
        );
        assert!(!i.mouse(1, 1, 1, 0), "no mouse in port 1");
        assert!(i.joystick(1, 0b1_0101));
        assert!(!i.joystick(1, 0b1_0101), "no change");
        assert_eq!(i.peek(PORT1), 0b1_0101);
        let mut w = StateWriter::new();
        i.save(&mut w);
        let bytes = w.into_bytes();
        let mut j = Input::default();
        j.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(i, j);
    }

    struct Card {
        data: Vec<u8>,
        protected: bool,
    }

    impl SdCard for Card {
        fn blocks(&self) -> u32 {
            (self.data.len() / 512) as u32
        }
        fn protected(&self) -> bool {
            self.protected
        }
        fn read_block(&mut self, b: u32, buf: &mut [u8; 512]) -> bool {
            buf.copy_from_slice(&self.data[b as usize * 512..][..512]);
            true
        }
        fn write_block(&mut self, b: u32, buf: &[u8; 512]) -> bool {
            self.data[b as usize * 512..][..512].copy_from_slice(buf);
            true
        }
    }

    struct Ram(Vec<u8>);
    impl ChipRamMut for Ram {
        fn get(&self, a: u32) -> u8 {
            self.0[a as usize]
        }
        fn set(&mut self, a: u32, v: u8) {
            self.0[a as usize] = v;
        }
    }

    fn command(sd: &mut Sd, card: &mut Card, ram: &mut Ram, regs: &[(u16, u16)]) -> (u32, u32) {
        for &(r, v) in regs {
            sd.write(r, v);
        }
        let (mut lines, mut slots) = (0, 0);
        while sd.busy() {
            let (s, _) = sd.line(true, card, ram);
            slots += u32::from(s);
            lines += 1;
        }
        (lines, slots)
    }

    #[test]
    fn sd_reads_and_writes_blocks_by_dma() {
        let mut card = Card {
            data: (0..4 * 512).map(|i| (i / 512 * 16 + i % 7) as u8).collect(),
            protected: false,
        };
        let mut ram = Ram(vec![0; 0x2_0000]);
        let mut sd = Sd::default();
        sd.card(&card);
        assert_eq!((sd.peek(SDSTAT), sd.peek(SDSIZEL)), (1, 4));
        let read = [
            (SDBLKL, 1),
            (SDCOUNT, 2),
            (SDSEG, 1),
            (SDOFS, 0x100),
            (SDCMD, SD_READ),
        ];
        let (lines, slots) = command(&mut sd, &mut card, &mut ram, &read);
        assert_eq!((lines, slots), (32, 256), "16 lines and 128 slots a block");
        assert_eq!(&ram.0[0x1_0100..0x1_0500], &card.data[512..1536]);
        assert_eq!(sd.read(SDSTAT), 1 | 8, "done");
        assert_eq!(sd.read(SDSTAT), 1, "cleared on read");
        // Write block 3 from chip RAM.
        ram.0[0x1_0000..0x1_0200].fill(0xab);
        let write = [(SDBLKL, 3), (SDCOUNT, 1), (SDOFS, 0), (SDCMD, SD_WRITE)];
        command(&mut sd, &mut card, &mut ram, &write);
        assert!(card.data[3 * 512..].iter().all(|b| *b == 0xab));
        // Errors: beyond the card, protected, beyond chip RAM.
        command(
            &mut sd,
            &mut card,
            &mut ram,
            &[(SDBLKL, 3), (SDCOUNT, 2), (SDCMD, SD_READ)],
        );
        assert_eq!(sd.read(SDSTAT), 1 | 4 | 8);
        card.protected = true;
        sd.card(&card);
        command(
            &mut sd,
            &mut card,
            &mut ram,
            &[(SDBLKL, 0), (SDCOUNT, 1), (SDCMD, SD_WRITE)],
        );
        assert_eq!(sd.read(SDSTAT), 1 | 4 | 8 | 16);
        command(
            &mut sd,
            &mut card,
            &mut ram,
            &[(SDSEG, 0x3f), (SDOFS, 0xff00), (SDCMD, SD_READ)],
        );
        assert_eq!(sd.read(SDSTAT) & 4, 4);
        // Savestate mid-command.
        sd.write(SDSEG, 1);
        sd.write(SDOFS, 0);
        sd.write(SDCOUNT, 1);
        assert!(!sd.write(SDCMD, SD_READ), "runs");
        sd.line(true, &mut card, &mut ram);
        let mut w = StateWriter::new();
        sd.save(&mut w);
        let bytes = w.into_bytes();
        let mut t = Sd::default();
        t.card(&card);
        t.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(sd, t);
    }
}
