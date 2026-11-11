//! The DMAgic (F018-compatible DMA controller, part of the 45GS02 on the
//! MEGA65).
//!
//! **Registers** (`$D700`–`$D70F`, C65 and MEGA65 personalities): `$D700`
//! list address LSB and trigger (F018 list), `$D701` MSB, `$D702` bank
//! (bits 16–22; writing it clears `$D704`), `$D703` bit 0 F018B format,
//! bit 1 no wrap at megabyte boundaries, `$D704` megabyte (bits 20–27),
//! `$D705` LSB and trigger of an enhanced job (list at the 28-bit
//! address), `$D706` the same with the list in the CPU's memory map,
//! `$D707` an enhanced job inline after the instruction (PC continues
//! after the list), `$D70E` LSB without trigger.
//!
//! **Lists.** F018: command, count (16 bits, 0 = 65536), source (16 bits,
//! bank and flags), destination (the same), modulo; F018B adds the
//! command's high byte before the modulo. Command bits 0–1 the operation
//! (copy, mix, swap, fill), 2 chain, 3 yield; high byte bits 8–9 and 10–11
//! the source's and destination's addressing (linear, modulo, hold, XY).
//! Bank and flags: bits 0–3 the bank, 4 hold, 5 modulo, 6 decrement, 7 I/O
//! visible at `$D000`–`$DFFF`. Fill takes the source's low byte as the
//! value. An enhanced job begins with option tokens up to `$00`: `$06`/`$07`
//! transparency off/on, `$0A`/`$0B` the F018/F018B format, `$80`/`$81` the
//! source/destination megabyte, `$82`–`$85` the source/destination step
//! (fraction, whole bytes), `$86` the transparent value; unknown tokens
//! (with an argument when bit 7 is set) are skipped. Options hold through a
//! chain and are reset after its last job.
//!
//! **Time.** The CPU waits until the job is done: a master clock a byte to
//! fill, two to copy (the guide's 40.5 MB/s fill, 20.25 MB/s copy).
//!
//! Not yet: mix (minterms; done as copy, as the MEGA65 does), modulo and XY
//! addressing, line drawing and texture options, audio DMA.
//!
//! Sources: the MEGA65 user guide, `appendix-dmagic.tex`; the `@IO`
//! register descriptions of `gs4510.vhdl` (`$D700`–`$D70E`). The
//! implementation is this project's own.

use cpu_45gs02::{Map, ADDR_MASK};

use crate::board::Board;

/// How a job was started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// `$D700`: an F018/F018B list without options.
    F018,
    /// `$D705`: enhanced, the list at its 28-bit address.
    Enhanced,
    /// `$D706`: enhanced, the list in the CPU's map.
    Mapped,
    /// `$D707`: enhanced, the list after the instruction.
    Inline,
}

/// Options of an enhanced job.
#[derive(Clone, Copy, Debug)]
struct Options {
    f018b: bool,
    src_mb: u8,
    dst_mb: u8,
    src_step: u32,
    dst_step: u32,
    transparent: Option<u8>,
    transparent_value: u8,
}

impl Options {
    fn new(f018b: bool) -> Self {
        Self {
            f018b,
            src_mb: 0,
            dst_mb: 0,
            src_step: 0x100,
            dst_step: 0x100,
            transparent: None,
            transparent_value: 0,
        }
    }
}

/// Where the list bytes come from.
enum ListSource<'a> {
    Flat(u32),
    Cpu(&'a Map, u16),
}

impl ListSource<'_> {
    fn next(&mut self, b: &mut Board) -> u8 {
        match self {
            ListSource::Flat(a) => {
                let v = b.read(*a & ADDR_MASK);
                *a = a.wrapping_add(1);
                v
            }
            ListSource::Cpu(map, pc) => {
                let v = b.read(map.translate(*pc));
                *pc = pc.wrapping_add(1);
                v
            }
        }
    }
}

/// One end of a transfer.
#[derive(Clone, Copy, Debug)]
struct End {
    /// Address in 1/256 bytes (28.8 fixed point), within its megabyte.
    pos: u64,
    mb: u32,
    hold: bool,
    down: bool,
    io: bool,
    step: u32,
}

impl End {
    fn new(addr16: u16, bank: u8, mb: u8, step: u32, mode: u8) -> Self {
        let a = (u32::from(bank & 0x0f) << 16) | u32::from(addr16);
        Self {
            pos: u64::from(a) << 8,
            mb: u32::from(mb),
            hold: bank & 0x10 != 0 || mode == 2,
            down: bank & 0x40 != 0,
            io: bank & 0x80 != 0,
            step,
        }
    }

    /// The bus address now: I/O for `$D000`–`$DFFF` with the I/O flag.
    fn addr(&self, b: &Board, no_wrap: bool) -> u32 {
        let within = (self.pos >> 8) as u32;
        let a = if no_wrap {
            (self.mb << 20).wrapping_add(within)
        } else {
            (self.mb << 20) | (within & 0xf_ffff)
        };
        if self.io && (a & 0xf000) == 0xd000 && a < 0x10_0000 {
            // The I/O area in the personality now.
            0x0ffd_0000 | (u32::from(b.vic.personality.index()) << 12) | (a & 0x0fff)
        } else {
            a & ADDR_MASK
        }
    }

    fn advance(&mut self) {
        if self.hold {
            return;
        }
        let s = u64::from(self.step);
        self.pos = if self.down {
            self.pos.wrapping_sub(s)
        } else {
            self.pos.wrapping_add(s)
        } & 0x000f_ffff_ffff;
    }
}

/// Run a job (and its chain); the master clocks it takes. `pc` is the CPU's
/// PC for inline jobs (moved past the list).
pub fn run(b: &mut Board, trigger: Trigger, map: &Map, pc: &mut u16) -> u32 {
    let regs = &b.devices[0x100..0x110];
    let list_addr = (u32::from(regs[4]) << 20)
        | (u32::from(regs[2] & 0x7f) << 16)
        | (u32::from(regs[1]) << 8)
        | u32::from(regs[0]);
    let f018b = regs[3] & 1 != 0;
    let no_wrap = regs[3] & 2 != 0;
    let mut src = match trigger {
        Trigger::F018 | Trigger::Enhanced => ListSource::Flat(list_addr),
        Trigger::Mapped => ListSource::Cpu(map, list_addr as u16),
        Trigger::Inline => ListSource::Cpu(map, *pc),
    };
    let enhanced = trigger != Trigger::F018;
    let mut opts = Options::new(f018b);
    let mut clocks = 0u32;
    // A runaway chain in guest memory stops after this many jobs.
    for _ in 0..4096 {
        if enhanced {
            loop {
                let t = src.next(b);
                match t {
                    0x00 => break,
                    0x06 => opts.transparent = None,
                    0x07 => opts.transparent = Some(opts.transparent_value),
                    0x0a => opts.f018b = false,
                    0x0b => opts.f018b = true,
                    _ if t & 0x80 != 0 => {
                        let arg = src.next(b);
                        match t {
                            0x80 => opts.src_mb = arg,
                            0x81 => opts.dst_mb = arg,
                            0x82 => opts.src_step = (opts.src_step & !0xff) | u32::from(arg),
                            0x83 => opts.src_step = (opts.src_step & 0xff) | u32::from(arg) << 8,
                            0x84 => opts.dst_step = (opts.dst_step & !0xff) | u32::from(arg),
                            0x85 => opts.dst_step = (opts.dst_step & 0xff) | u32::from(arg) << 8,
                            0x86 => {
                                opts.transparent_value = arg;
                                if opts.transparent.is_some() {
                                    opts.transparent = Some(arg);
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut byte = || src.next(b);
        let cmd_lo = byte();
        let count = u16::from_le_bytes([byte(), byte()]);
        let s16 = u16::from_le_bytes([byte(), byte()]);
        let s_bank = byte();
        let d16 = u16::from_le_bytes([byte(), byte()]);
        let d_bank = byte();
        let cmd_hi = if opts.f018b { byte() } else { 0 };
        let _modulo = u16::from_le_bytes([byte(), byte()]);
        let n = if count == 0 {
            0x1_0000
        } else {
            u32::from(count)
        };
        let op = cmd_lo & 3;
        let mut s = End::new(s16, s_bank, opts.src_mb, opts.src_step, cmd_hi & 3);
        let mut d = End::new(d16, d_bank, opts.dst_mb, opts.dst_step, cmd_hi >> 2 & 3);
        let fill = s16 as u8;
        for _ in 0..n {
            let da = d.addr(b, no_wrap);
            match op {
                3 => {
                    if opts.transparent != Some(fill) {
                        b.write(da, fill);
                    }
                }
                2 => {
                    let sa = s.addr(b, no_wrap);
                    let x = b.read(sa);
                    let y = b.read(da);
                    b.write(sa, y);
                    b.write(da, x);
                    s.advance();
                }
                _ => {
                    let sa = s.addr(b, no_wrap);
                    let v = b.read(sa);
                    if opts.transparent != Some(v) {
                        b.write(da, v);
                    }
                    s.advance();
                }
            }
            d.advance();
        }
        clocks += n * if op == 3 { 1 } else { 2 };
        if cmd_lo & 4 == 0 {
            break;
        }
    }
    if let ListSource::Cpu(_, end) = src {
        if trigger == Trigger::Inline {
            *pc = end;
        }
    }
    clocks
}
