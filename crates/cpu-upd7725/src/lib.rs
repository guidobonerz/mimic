//! NEC µPD77C25, the fixed-point signal processor in the SNES DSP-1,
//! DSP-1B, DSP-2, DSP-3 and DSP-4 cartridges.
//!
//! **Memory** (Harvard): 2048 instructions of 24 bits (program ROM), 1024
//! words of 16 bits (data ROM, read through RP), 256 words of data RAM
//! (through DP), a 4-level return stack. The user's ROM file holds the
//! program (3 bytes per instruction, little-endian) followed by the data
//! ROM (2 bytes per word, little-endian): 8192 bytes.
//!
//! **Registers:** accumulators A and B with their flags (S1 S0 C Z OV1
//! OV0), TR, TRB, the multiplier inputs K and L and its outputs M (sign
//! and the upper 15 bits of K × L) and N (the lower 15 bits, shifted
//! left), DP (8 bits: DPL bits 0–3, DPH bits 4–7), RP (10 bits), DR (the
//! data register the host reads and writes) and SR (status: RQM 15, USF1
//! 14, USF0 13, DRS 12, DMA 11, DRC 10 (8-bit transfers), SOC 9, SIC 8, EI
//! 7, P1 1, P0 0). The multiplier works every instruction: what K and L
//! hold at its end is in M and N for the next one.
//!
//! **Instructions** (bits 23–22): 00 OP and 01 RT (OP, then return) move
//! one source onto the internal bus (bits 7–4) into one destination (bits
//! 3–0) while the ALU (19–16) works on A or B (bit 15) with RAM at DP, the
//! bus, M or N (21–20), DP's low nibble steps (14–13), DPH is XORed (12–9)
//! and RP counts down (8); 10 JP (condition 21–13, target 12–2); 11 LD
//! (16-bit immediate 21–6 into a destination). SBB, ADC and SHL1 take
//! the carry of the other accumulator; OV1 and S1 follow overflows that
//! have not been undone (SGN reads $7FFF or $8000 by S1 of A).
//!
//! **Host side:** SR's upper byte reads at its status address; DR moves 16
//! bits as two bytes (low first, DRS between them) or, with DRC set, one.
//! RQM is set when the DSP reads or writes DR and cleared when the host
//! has moved the last byte.
//!
//! Sources: NEC µPD77C25 data sheet (instruction set, flags); Martin
//! Korth, fullsnes, "SNES Cart DSP-n/ST010/ST011 - NEC uPD77C25 /
//! uPD96050" (opcode encoding, SNES mapping; read 2026-11-09). The
//! implementation is this project's own.

#![forbid(unsafe_code)]

use emu_core::{Register, RegisterSet, StateError, StateReader, StateWriter};

pub const PROGRAM_WORDS: usize = 2048;
pub const DATA_WORDS: usize = 1024;
pub const RAM_WORDS: usize = 256;
/// Size of a ROM file: program then data ROM.
pub const ROM_BYTES: usize = PROGRAM_WORDS * 3 + DATA_WORDS * 2;

pub const SR_RQM: u16 = 0x8000;
pub const SR_DRS: u16 = 0x1000;
pub const SR_DRC: u16 = 0x0400;
/// SR bits the program cannot write.
const SR_READ_ONLY: u16 = 0x907c;

/// The flags of one accumulator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub s1: bool,
    pub s0: bool,
    pub c: bool,
    pub z: bool,
    pub ov1: bool,
    pub ov0: bool,
}

impl Flags {
    pub fn bits(self) -> u8 {
        u8::from(self.ov0)
            | u8::from(self.ov1) << 1
            | u8::from(self.z) << 2
            | u8::from(self.c) << 3
            | u8::from(self.s0) << 4
            | u8::from(self.s1) << 5
    }

    pub fn from_bits(b: u8) -> Self {
        Self {
            ov0: b & 1 != 0,
            ov1: b & 2 != 0,
            z: b & 4 != 0,
            c: b & 8 != 0,
            s0: b & 16 != 0,
            s1: b & 32 != 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upd7725 {
    program: Vec<u32>,
    data: Vec<u16>,
    pub ram: Vec<u16>,
    pub pc: u16,
    pub stack: [u16; 4],
    pub rp: u16,
    pub dp: u16,
    pub a: u16,
    pub b: u16,
    pub fa: Flags,
    pub fb: Flags,
    pub tr: u16,
    pub trb: u16,
    pub k: u16,
    pub l: u16,
    pub m: u16,
    pub n: u16,
    pub dr: u16,
    pub sr: u16,
    pub so: u16,
}

impl Upd7725 {
    /// A processor running `rom` (program then data ROM, [`ROM_BYTES`]).
    pub fn new(rom: &[u8]) -> Result<Self, String> {
        if rom.len() != ROM_BYTES {
            return Err(format!(
                "uPD77C25 ROM must be {ROM_BYTES} bytes, got {}",
                rom.len()
            ));
        }
        let (p, d) = rom.split_at(PROGRAM_WORDS * 3);
        let program = p
            .chunks(3)
            .map(|c| u32::from(c[0]) | u32::from(c[1]) << 8 | u32::from(c[2]) << 16)
            .collect();
        let data = d
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let mut s = Self {
            program,
            data,
            ram: vec![0; RAM_WORDS],
            pc: 0,
            stack: [0; 4],
            rp: 0,
            dp: 0,
            a: 0,
            b: 0,
            fa: Flags::default(),
            fb: Flags::default(),
            tr: 0,
            trb: 0,
            k: 0,
            l: 0,
            m: 0,
            n: 0,
            dr: 0,
            sr: 0,
            so: 0,
        };
        s.reset();
        Ok(s)
    }

    /// The RESET line: PC, stack, flags and SR clear, RP is set (RAM and
    /// the other registers keep their values).
    pub fn reset(&mut self) {
        self.pc = 0;
        self.stack = [0; 4];
        self.rp = 0x3ff;
        self.fa = Flags::default();
        self.fb = Flags::default();
        self.sr = 0;
    }

    pub fn program_word(&self, addr: u16) -> u32 {
        self.program[usize::from(addr) % PROGRAM_WORDS]
    }

    pub fn data_word(&self, addr: u16) -> u16 {
        self.data[usize::from(addr) % DATA_WORDS]
    }

    // ---- host side

    /// The status byte (SR's upper half).
    pub fn read_sr(&self) -> u8 {
        (self.sr >> 8) as u8
    }

    /// DR as the next host read would see it.
    pub fn peek_dr(&self) -> u8 {
        if self.sr & SR_DRC == 0 && self.sr & SR_DRS != 0 {
            (self.dr >> 8) as u8
        } else {
            self.dr as u8
        }
    }

    pub fn read_dr(&mut self) -> u8 {
        let v = self.peek_dr();
        if self.sr & SR_DRC != 0 {
            self.sr &= !SR_RQM;
        } else if self.sr & SR_DRS == 0 {
            self.sr |= SR_DRS;
        } else {
            self.sr &= !(SR_RQM | SR_DRS);
        }
        v
    }

    pub fn write_dr(&mut self, v: u8) {
        let v = u16::from(v);
        if self.sr & SR_DRC != 0 {
            self.dr = (self.dr & 0xff00) | v;
            self.sr &= !SR_RQM;
        } else if self.sr & SR_DRS == 0 {
            self.dr = (self.dr & 0xff00) | v;
            self.sr |= SR_DRS;
        } else {
            self.dr = (self.dr & 0x00ff) | v << 8;
            self.sr &= !(SR_RQM | SR_DRS);
        }
    }

    // ---- execution

    fn push(&mut self, v: u16) {
        self.stack.copy_within(0..3, 1);
        self.stack[0] = v;
    }

    fn pop(&mut self) -> u16 {
        let v = self.stack[0];
        self.stack.copy_within(1..4, 0);
        self.stack[3] = 0;
        v
    }

    /// Execute one instruction.
    pub fn step(&mut self) {
        let op = self.program_word(self.pc);
        self.pc = (self.pc + 1) & 0x7ff;
        match op >> 22 {
            0 => self.exec_op(op),
            1 => {
                self.exec_op(op);
                self.pc = self.pop();
            }
            2 => self.exec_jp(op),
            _ => self.store((op & 15) as u8, (op >> 6) as u16),
        }
        let product = i32::from(self.k as i16) * i32::from(self.l as i16);
        self.m = (product >> 15) as u16;
        self.n = (product << 1) as u16;
    }

    fn source(&mut self, src: u8) -> u16 {
        match src {
            0 => self.trb,
            1 => self.a,
            2 => self.b,
            3 => self.tr,
            4 => self.dp,
            5 => self.rp,
            6 => self.data_word(self.rp),
            7 => {
                if self.fa.s1 {
                    0x8000
                } else {
                    0x7fff
                }
            }
            8 => {
                self.sr |= SR_RQM;
                self.dr
            }
            9 => self.dr,
            10 => self.sr,
            // Serial input (SIM, SIL): nothing is connected.
            11 | 12 => 0,
            13 => self.k,
            14 => self.l,
            _ => self.ram[usize::from(self.dp) % RAM_WORDS],
        }
    }

    fn store(&mut self, dst: u8, v: u16) {
        match dst {
            0 => {}
            1 => self.a = v,
            2 => self.b = v,
            3 => self.tr = v,
            4 => self.dp = v & 0xff,
            5 => self.rp = v & 0x3ff,
            6 => {
                self.dr = v;
                self.sr |= SR_RQM;
            }
            7 => self.sr = (self.sr & SR_READ_ONLY) | (v & !SR_READ_ONLY),
            8 | 9 => self.so = v,
            10 => self.k = v,
            11 => {
                self.k = v;
                self.l = self.data_word(self.rp);
            }
            12 => {
                self.l = v;
                self.k = self.ram[usize::from(self.dp | 0x40) % RAM_WORDS];
            }
            13 => self.l = v,
            14 => self.trb = v,
            _ => self.ram[usize::from(self.dp) % RAM_WORDS] = v,
        }
    }

    fn exec_op(&mut self, op: u32) {
        let pselect = (op >> 20) & 3;
        let alu = (op >> 16) & 15;
        let asl = (op >> 15) & 1;
        let dpl = (op >> 13) & 3;
        let dphm = ((op >> 9) & 15) as u16;
        let rpdcr = (op >> 8) & 1 != 0;
        let src = ((op >> 4) & 15) as u8;
        let dst = (op & 15) as u8;

        let bus = self.source(src);
        if alu != 0 {
            let p = match pselect {
                0 => self.ram[usize::from(self.dp) % RAM_WORDS],
                1 => bus,
                2 => self.m,
                _ => self.n,
            };
            if asl == 0 {
                let (r, f) = alu_op(alu, self.a, p, self.fa, self.fb.c);
                self.a = r;
                self.fa = f;
            } else {
                let (r, f) = alu_op(alu, self.b, p, self.fb, self.fa.c);
                self.b = r;
                self.fb = f;
            }
        }
        self.store(dst, bus);
        let lo = self.dp & 0x0f;
        let lo = match dpl {
            1 => (lo + 1) & 0x0f,
            2 => lo.wrapping_sub(1) & 0x0f,
            3 => 0,
            _ => lo,
        };
        self.dp = ((self.dp & 0xf0) ^ (dphm << 4)) | lo;
        if rpdcr {
            self.rp = self.rp.wrapping_sub(1) & 0x3ff;
        }
    }

    fn condition(&self, brch: u32) -> Option<bool> {
        let (fa, fb) = (self.fa, self.fb);
        match brch {
            0x100 => Some(true),
            0x080..=0x0af if brch & 1 == 0 => {
                let g = (brch >> 2) & 0x0f;
                let fl = if g & 1 == 0 { fa } else { fb };
                let bit = match g >> 1 {
                    0 => fl.c,
                    1 => fl.z,
                    2 => fl.ov0,
                    3 => fl.ov1,
                    4 => fl.s0,
                    _ => fl.s1,
                };
                Some(bit == (brch & 2 != 0))
            }
            0x0b0 => Some(self.dp & 0x0f == 0),
            0x0b1 => Some(self.dp & 0x0f != 0),
            0x0b2 => Some(self.dp & 0x0f == 0x0f),
            0x0b3 => Some(self.dp & 0x0f != 0x0f),
            // Serial acknowledges (SIAK, SOAK): never given.
            0x0b4 | 0x0b8 => Some(true),
            0x0b6 | 0x0ba => Some(false),
            0x0bc => Some(self.sr & SR_RQM == 0),
            0x0be => Some(self.sr & SR_RQM != 0),
            _ => None,
        }
    }

    fn exec_jp(&mut self, op: u32) {
        let brch = (op >> 13) & 0x1ff;
        let na = ((op >> 2) & 0x7ff) as u16;
        if brch == 0x140 {
            self.push(self.pc);
            self.pc = na;
        } else if self.condition(brch) == Some(true) {
            self.pc = na;
        }
    }

    // ---- debugger

    pub fn regs(&self) -> RegisterSet {
        let reg = |name, value: u32, bits| Register { name, value, bits };
        RegisterSet(vec![
            reg("PC", u32::from(self.pc), 11),
            reg("A", u32::from(self.a), 16),
            reg("B", u32::from(self.b), 16),
            reg("FLAGA", u32::from(self.fa.bits()), 6),
            reg("FLAGB", u32::from(self.fb.bits()), 6),
            reg("TR", u32::from(self.tr), 16),
            reg("TRB", u32::from(self.trb), 16),
            reg("K", u32::from(self.k), 16),
            reg("L", u32::from(self.l), 16),
            reg("M", u32::from(self.m), 16),
            reg("N", u32::from(self.n), 16),
            reg("DP", u32::from(self.dp), 8),
            reg("RP", u32::from(self.rp), 10),
            reg("DR", u32::from(self.dr), 16),
            reg("SR", u32::from(self.sr), 16),
        ])
    }

    pub fn set_reg(&mut self, name: &str, v: u32) {
        let w = v as u16;
        match name.to_ascii_uppercase().as_str() {
            "PC" => self.pc = w & 0x7ff,
            "A" => self.a = w,
            "B" => self.b = w,
            "FLAGA" => self.fa = Flags::from_bits(v as u8),
            "FLAGB" => self.fb = Flags::from_bits(v as u8),
            "TR" => self.tr = w,
            "TRB" => self.trb = w,
            "K" => self.k = w,
            "L" => self.l = w,
            "M" => self.m = w,
            "N" => self.n = w,
            "DP" => self.dp = w & 0xff,
            "RP" => self.rp = w & 0x3ff,
            "DR" => self.dr = w,
            "SR" => self.sr = w,
            _ => {}
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"D77C", 1, |w| {
            for &v in &self.ram {
                w.u16(v);
            }
            w.u16(self.pc);
            for v in self.stack {
                w.u16(v);
            }
            for v in [
                self.rp, self.dp, self.a, self.b, self.tr, self.trb, self.k, self.l, self.m,
                self.n, self.dr, self.sr, self.so,
            ] {
                w.u16(v);
            }
            w.u8(self.fa.bits());
            w.u8(self.fb.bits());
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"D77C", 1)?;
        let mut n = self.clone();
        for v in n.ram.iter_mut() {
            *v = c.u16()?;
        }
        n.pc = c.u16()? & 0x7ff;
        for v in n.stack.iter_mut() {
            *v = c.u16()? & 0x7ff;
        }
        for v in [
            &mut n.rp, &mut n.dp, &mut n.a, &mut n.b, &mut n.tr, &mut n.trb, &mut n.k, &mut n.l,
            &mut n.m, &mut n.n, &mut n.dr, &mut n.sr, &mut n.so,
        ] {
            *v = c.u16()?;
        }
        n.rp &= 0x3ff;
        n.dp &= 0xff;
        n.fa = Flags::from_bits(c.u8()?);
        n.fb = Flags::from_bits(c.u8()?);
        *self = n;
        Ok(())
    }
}

/// One ALU operation on accumulator `q` with operand `p`; `c` is the other
/// accumulator's carry. Returns the result and the new flags.
fn alu_op(alu: u32, q: u16, p: u16, old: Flags, c: bool) -> (u16, Flags) {
    let ci = u32::from(c);
    let (q32, p32) = (u32::from(q), u32::from(p));
    let mut f = old;
    // The operand as the overflow sees it (INC and DEC add or take 1).
    let (r, arith, carry, p_eff) = match alu {
        1 => (q | p, false, false, p),
        2 => (q & p, false, false, p),
        3 => (q ^ p, false, false, p),
        4 => (q.wrapping_sub(p), true, q32 < p32, p),
        5 => (q.wrapping_add(p), true, q32 + p32 > 0xffff, p),
        6 => (
            q.wrapping_sub(p).wrapping_sub(c.into()),
            true,
            q32 < p32 + ci,
            p,
        ),
        7 => (
            q.wrapping_add(p).wrapping_add(c.into()),
            true,
            q32 + p32 + ci > 0xffff,
            p,
        ),
        8 => (q.wrapping_sub(1), true, q == 0, 1),
        9 => (q.wrapping_add(1), true, q == 0xffff, 1),
        10 => (!q, false, false, p),
        11 => ((q >> 1) | (q & 0x8000), false, q & 1 != 0, p),
        12 => ((q << 1) | u16::from(c), false, q & 0x8000 != 0, p),
        13 => ((q << 2) | 3, false, false, p),
        14 => ((q << 4) | 15, false, false, p),
        _ => (q.rotate_left(8), false, false, p),
    };
    f.s0 = r & 0x8000 != 0;
    f.z = r == 0;
    f.c = carry;
    if arith {
        // Additions are the odd codes (ADD 5, ADC 7, INC 9).
        f.ov0 = if alu & 1 == 1 {
            (q ^ r) & (p_eff ^ r) & 0x8000 != 0
        } else {
            (q ^ r) & (q ^ p_eff) & 0x8000 != 0
        };
        if f.ov0 {
            f.s1 = f.ov1 ^ !f.s0;
            f.ov1 = !f.ov1;
        }
    } else {
        f.ov0 = false;
        f.ov1 = false;
    }
    (r, f)
}

const SOURCES: [&str; 16] = [
    "TRB", "A", "B", "TR", "DP", "RP", "RO", "SGN", "DR", "DRNF", "SR", "SIM", "SIL", "K", "L",
    "MEM",
];
const DESTS: [&str; 16] = [
    "NON", "A", "B", "TR", "DP", "RP", "DR", "SR", "SOL", "SOM", "K", "KLR", "KLM", "L", "TRB",
    "MEM",
];
const ALU: [&str; 16] = [
    "NOP", "OR", "AND", "XOR", "SUB", "ADD", "SBB", "ADC", "DEC", "INC", "CMP", "SHR1", "SHL1",
    "SHL2", "SHL4", "XCHG",
];

/// Text of one instruction: mnemonic, operand, jump target.
pub fn disassemble(op: u32) -> (String, String, Option<u16>) {
    match op >> 22 {
        0 | 1 => {
            let mut parts = Vec::new();
            let alu = ((op >> 16) & 15) as usize;
            if alu != 0 {
                let acc = if op & 0x8000 == 0 { "A" } else { "B" };
                let p = ["RAM", "IDB", "M", "N"][((op >> 20) & 3) as usize];
                parts.push(if (1..=7).contains(&alu) {
                    format!("{} {acc},{p}", ALU[alu])
                } else {
                    format!("{} {acc}", ALU[alu])
                });
            }
            let (src, dst) = (((op >> 4) & 15) as usize, (op & 15) as usize);
            if dst != 0 || src != 0 {
                parts.push(format!("MOV {},{}", DESTS[dst], SOURCES[src]));
            }
            match (op >> 13) & 3 {
                1 => parts.push("DPINC".into()),
                2 => parts.push("DPDEC".into()),
                3 => parts.push("DPCLR".into()),
                _ => {}
            }
            let dphm = (op >> 9) & 15;
            if dphm != 0 {
                parts.push(format!("M{dphm:X}"));
            }
            if op & 0x100 != 0 {
                parts.push("RPDEC".into());
            }
            let m = if op >> 22 == 1 { "RT" } else { "OP" };
            (m.into(), parts.join(" | "), None)
        }
        2 => {
            let brch = (op >> 13) & 0x1ff;
            let na = ((op >> 2) & 0x7ff) as u16;
            let name = match brch {
                0x100 => "JMP".to_string(),
                0x140 => "CALL".to_string(),
                0x080..=0x0af if brch & 1 == 0 => {
                    let g = (brch >> 2) & 0x0f;
                    let flag = ["C", "Z", "OV0", "OV1", "S0", "S1"][(g >> 1) as usize];
                    let acc = if g & 1 == 0 { "A" } else { "B" };
                    let n = if brch & 2 == 0 { "N" } else { "" };
                    format!("J{n}{flag}{acc}")
                }
                0x0b0 => "JDPL0".into(),
                0x0b1 => "JDPLN0".into(),
                0x0b2 => "JDPLF".into(),
                0x0b3 => "JDPLNF".into(),
                0x0b4 => "JNSIAK".into(),
                0x0b6 => "JSIAK".into(),
                0x0b8 => "JNSOAK".into(),
                0x0ba => "JSOAK".into(),
                0x0bc => "JNRQM".into(),
                0x0be => "JRQM".into(),
                _ => format!("J?{brch:03X}"),
            };
            (name, format!("${na:03X}"), Some(na))
        }
        _ => (
            "LD".into(),
            format!("{},#${:04X}", DESTS[(op & 15) as usize], (op >> 6) & 0xffff),
            None,
        ),
    }
}

#[cfg(test)]
mod tests;
