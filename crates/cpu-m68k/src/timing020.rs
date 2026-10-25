//! 68020 instruction cache and instruction timing.
//!
//! **Cache** (MC68020/MC68EC020 User's Manual, section 4 "On-Chip Cache
//! Memory"): 64 long-word entries, direct mapped by A7–A2, tagged with
//! A31–A8 and FC2 (supervisor/user program space) plus a valid bit. Only
//! instruction prefetches go through it (data accesses are never cached).
//! A miss fetches the aligned long word and, unless CACR.F (freeze) is set,
//! writes it into the entry. CACR.E enables it (clearing E does not flush
//! it), writing CACR with C set clears every valid bit, with CE set the one
//! entry CAAR's index field (A7–A2) selects; C and CE read as zero. Reset
//! clears the cache and E/F. The cache is not snooped: code that modifies
//! itself must clear it (or the entry), as on the chip.
//!
//! **Timing** (section 8 "Instruction Execution Timing", tables 8.2.1 to
//! 8.2.18). The manual gives three numbers per instruction: best case
//! (cached, maximal overlap), cache case (cached, no overlap) and worst
//! case (not cached, no overlap), with the counts of operand reads,
//! instruction accesses and writes. This module keeps the cache-case
//! column, which has no instruction accesses, and splits each entry into
//! its bus cycles (3 clocks each at zero wait states) and the rest, the
//! sequencer's internal clocks: `internal = clocks - 3 * (reads + writes)`.
//! The CPU then runs the instruction's real bus cycles (with the wait
//! states the machine's bus inserts) next to these internal clocks:
//!
//! - an instruction costs the internal clocks of its operation entry plus
//!   those of its effective address entry (fetch, fetch immediate,
//!   calculate, calculate immediate or jump effective address, as the
//!   manual's footnotes say), or of the MOVE table's source/destination
//!   pair;
//! - the effective address part runs before the first operand read (the
//!   address is calculated before it is used), the rest before the first
//!   write or change of flow, or at the end;
//! - reads stall the sequencer until the data arrives; writes are handed
//!   to the bus controller, which runs them while the sequencer goes on
//!   (one cycle in flight, section 8.1.3);
//! - instruction words come from the cache (no delay, no bus cycle) or
//!   from a long-word bus cycle the bus controller runs in the background
//!   (section 8.1.1); the sequencer waits only when it needs a word that
//!   has not arrived. The second word of a fetched long word is taken from
//!   the fetch buffer without another cycle.
//!
//! With every word cached and no wait states an instruction thus takes its
//! cache-case time; without the cache the prefetches add their cycles as
//! far as they do not overlap the internal clocks, which gives the worst
//! case for flow changes (Bcc, JMP, DBcc) and lies between the cache and
//! worst cases elsewhere — the overlap the manual's examples in section
//! 8.1.5 show. Exceptions use the "exception-related instructions" table
//! (8.2.17): 5 internal clocks for a four-word frame, 7 for a six-word one,
//! 8 for an interrupt (11 with the throwaway frame on the master stack).
//!
//! Approximations: the manual's tables are bounds, not exact timings (it
//! says so in 8.2); the placement of the internal clocks between the bus
//! cycles is this model's; operation times that depend on data (MUL, DIV)
//! use the table's maximum; the pipeline's three stages are reduced to the
//! prefetch queue (IRD/IRC) plus one long-word buffer; a few rows the
//! manual leaves out (CIEA with -(An), FIEA with ([d16,B],I,d32)) take the
//! neighbouring row. Bit field memory operands are accessed byte by byte,
//! so they take more bus cycles than the table's one or two.
//!
//! Source: Motorola, MC68020/MC68EC020 Microprocessors User's Manual
//! (MC68020UM/AD, first edition, 1992), sections 4 and 8.

use crate::M68000;

// ---------------------------------------------------------------- cache

/// Valid bit of a tag.
const VALID: u32 = 1;
/// FC2 in a tag (supervisor program space).
const FC2: u32 = 4;

/// The 256-byte instruction cache (section 4.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ICache {
    /// A31–A8, FC2 (bit 2) and the valid bit (bit 0) of each entry.
    tag: [u32; 64],
    data: [u32; 64],
}

impl Default for ICache {
    fn default() -> Self {
        Self {
            tag: [0; 64],
            data: [0; 64],
        }
    }
}

/// CACR bits (figure 4-2).
pub mod cacr {
    /// Enable cache.
    pub const E: u32 = 1;
    /// Freeze cache.
    pub const F: u32 = 2;
    /// Clear entry (the one CAAR selects).
    pub const CE: u32 = 4;
    /// Clear cache.
    pub const C: u32 = 8;
}

impl ICache {
    fn index(addr: u32) -> usize {
        ((addr >> 2) & 63) as usize
    }

    fn tag_of(addr: u32, fc: u8) -> u32 {
        let fc2 = if fc & 4 != 0 { FC2 } else { 0 };
        (addr & 0xffff_ff00) | fc2 | VALID
    }

    /// The long word at `addr` (long aligned) in program space `fc`, if the
    /// cache is enabled and holds it.
    pub(crate) fn lookup(&self, cacr: u32, addr: u32, fc: u8) -> Option<u32> {
        let i = Self::index(addr);
        (cacr & cacr::E != 0 && self.tag[i] == Self::tag_of(addr, fc)).then_some(self.data[i])
    }

    /// After a miss: replace the entry unless the cache is disabled or
    /// frozen.
    pub(crate) fn fill(&mut self, cacr: u32, addr: u32, fc: u8, data: u32) {
        if cacr & (cacr::E | cacr::F) == cacr::E {
            let i = Self::index(addr);
            self.tag[i] = Self::tag_of(addr, fc);
            self.data[i] = data;
        }
    }

    pub(crate) fn clear(&mut self) {
        for t in &mut self.tag {
            *t &= !VALID;
        }
    }

    /// Clear the entry of CAAR's index field (A7–A2).
    pub(crate) fn clear_entry(&mut self, caar: u32) {
        self.tag[Self::index(caar)] &= !VALID;
    }

    pub(crate) fn save(&self, w: &mut emu_core::StateWriter) {
        for (t, d) in self.tag.iter().zip(&self.data) {
            w.u32(*t);
            w.u32(*d);
        }
    }

    pub(crate) fn load(
        &mut self,
        r: &mut emu_core::StateReader<'_>,
    ) -> Result<(), emu_core::StateError> {
        for i in 0..64 {
            self.tag[i] = r.u32()?;
            self.data[i] = r.u32()?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- pipeline state

/// The 68020's bus controller and prefetch state next to the sequencer's
/// clock (`M68000::t`). Times count clocks from the start of the current
/// step; between steps they are kept relative to the next one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pipe {
    /// Clock at which the bus controller is free again.
    pub bus_free: u32,
    /// Clocks at which the words in IRD and IRC are available.
    pub ird_ready: u32,
    pub irc_ready: u32,
    /// The last fetched long word (the second word of an aligned fetch).
    pub buf_valid: bool,
    pub buf_addr: u32,
    pub buf_fc: u8,
    pub buf_data: u32,
    pub buf_ready: u32,
    /// The current instruction's timing.
    pub plan: Plan,
    /// Effective address classes the instruction has calculated, in order.
    pub eas: [u8; 2],
    pub n_eas: u8,
    /// Internal clocks already spent, the address part done, the
    /// instruction has written, the plan is finished (or there is none:
    /// exceptions, reset).
    pub spent: u16,
    pub lead_done: bool,
    pub wrote: bool,
    pub plan_done: bool,
}

impl Default for Pipe {
    fn default() -> Self {
        Self {
            bus_free: 0,
            ird_ready: 0,
            irc_ready: 0,
            buf_valid: false,
            buf_addr: 0,
            buf_fc: 0,
            buf_data: 0,
            buf_ready: 0,
            plan: Plan::default(),
            eas: [class::DN; 2],
            n_eas: 0,
            spent: 0,
            lead_done: true,
            wrote: false,
            plan_done: true,
        }
    }
}

impl Pipe {
    /// Start an instruction with `plan`.
    pub(crate) fn begin(&mut self, plan: Plan) {
        self.plan = plan;
        self.n_eas = 0;
        self.spent = 0;
        self.lead_done = false;
        self.wrote = false;
        self.plan_done = false;
    }

    pub(crate) fn note_ea(&mut self, class: u8) {
        if usize::from(self.n_eas) < self.eas.len() {
            self.eas[usize::from(self.n_eas)] = class;
            self.n_eas += 1;
        }
    }

    fn ea(&self, i: usize) -> Option<u8> {
        (usize::from(self.n_eas) > i).then(|| self.eas[i])
    }

    /// Internal clocks of the effective address part.
    pub(crate) fn lead(&self) -> u16 {
        let p = &self.plan;
        match p.kind {
            Kind::Move => self
                .ea(0)
                .map_or(0, |s| FEA[usize::from(s)].internal())
                .min(self.total()),
            _ => ea_internal(p.kind, p.long_imm, self.ea(0)),
        }
    }

    /// All internal clocks of the instruction.
    pub(crate) fn total(&self) -> u16 {
        let p = &self.plan;
        let op = match p.on_write {
            Some(w) if self.wrote => w,
            _ => p.op,
        };
        match p.kind {
            Kind::Move => move_internal(
                self.ea(0).unwrap_or(class::DN),
                self.ea(1).unwrap_or(class::DN),
            ),
            k => op + ea_internal(k, p.long_imm, self.ea(0)),
        }
    }

    /// Make the times relative to the next step, which starts `t` clocks
    /// after this one.
    pub(crate) fn rebase(&mut self, t: u32) {
        for v in [
            &mut self.bus_free,
            &mut self.ird_ready,
            &mut self.irc_ready,
            &mut self.buf_ready,
        ] {
            *v = v.saturating_sub(t);
        }
    }

    pub(crate) fn save(&self, w: &mut emu_core::StateWriter) {
        for v in [
            self.bus_free,
            self.ird_ready,
            self.irc_ready,
            self.buf_addr,
            self.buf_data,
            self.buf_ready,
        ] {
            w.u32(v);
        }
        w.bool(self.buf_valid);
        w.u8(self.buf_fc);
    }

    pub(crate) fn load(
        &mut self,
        r: &mut emu_core::StateReader<'_>,
    ) -> Result<(), emu_core::StateError> {
        let mut n = Self::default();
        for v in [
            &mut n.bus_free,
            &mut n.ird_ready,
            &mut n.irc_ready,
            &mut n.buf_addr,
            &mut n.buf_data,
            &mut n.buf_ready,
        ] {
            *v = r.u32()?;
        }
        n.buf_valid = r.bool()?;
        n.buf_fc = r.u8()? & 7;
        *self = n;
        Ok(())
    }
}

// ---------------------------------------------------------------- table entries

/// One cache-case entry: total clocks, operand reads and writes (the
/// instruction accesses are zero in this column).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct E {
    pub clocks: u16,
    pub reads: u8,
    pub writes: u8,
}

const fn e(clocks: u16, reads: u8, writes: u8) -> E {
    E {
        clocks,
        reads,
        writes,
    }
}

/// An addressing mode the table has no row for.
const NA: E = e(0, 0, 0);

impl E {
    /// Clocks besides the 3-clock bus cycles.
    pub(crate) const fn internal(self) -> u16 {
        self.clocks
            .saturating_sub(3 * (self.reads as u16 + self.writes as u16))
    }
}

/// Effective address classes: the rows of the effective address tables.
/// The 68020's full-format extension words fall into (B), (d16,B), (d32,B)
/// and the nine memory indirect rows by their base and outer displacement
/// sizes (index or not, pre- or post-indexed do not change the timing).
pub(crate) mod class {
    pub const DN: u8 = 0;
    pub const AN: u8 = 1;
    /// (An)
    pub const IND: u8 = 2;
    /// (An)+
    pub const INC: u8 = 3;
    /// -(An)
    pub const DEC: u8 = 4;
    /// (d16,An), (d16,PC)
    pub const D16: u8 = 5;
    pub const ABS_W: u8 = 6;
    pub const ABS_L: u8 = 7;
    /// #<data>.B, #<data>.W
    pub const IMM_W: u8 = 8;
    pub const IMM_L: u8 = 9;
    /// (d8,An,Xn), (d8,PC,Xn): the brief extension word.
    pub const BRIEF: u8 = 10;
    /// Full format with a word base displacement, base and index register:
    /// (d16,An,Xn), (d16,PC,Xn).
    pub const D16_INDEX: u8 = 11;
    /// (B): full format without base displacement.
    pub const BASE: u8 = 12;
    /// (d16,B): a word base displacement with base or index suppressed.
    pub const BASE_D16: u8 = 13;
    /// (d32,B)
    pub const BASE_D32: u8 = 14;
    /// ([bd,B],I,od): `MEM + 3 * bd + od`, bd and od 0 (null), 1 (word)
    /// or 2 (long).
    pub const MEM: u8 = 15;
    pub const COUNT: usize = 24;
}

/// The class of a full-format extension word (M68000PRM 2.2.3: BS bit 7,
/// IS bit 6, BD SIZE bits 5–4, I/IS bits 2–0).
pub(crate) fn full_class(ext: u16) -> u8 {
    let size = |s: u16| match s & 3 {
        2 => 1,
        3 => 2,
        _ => 0,
    };
    let bd = size(ext >> 4);
    let iis = ext & 7;
    if iis == 0 {
        return match bd {
            0 => class::BASE,
            1 if ext & 0xc0 == 0 => class::D16_INDEX,
            1 => class::BASE_D16,
            _ => class::BASE_D32,
        };
    }
    class::MEM + 3 * bd + size(iis)
}

/// The class of mode `m`, register `r` (the indexed modes as the brief
/// format; their extension word refines it).
pub(crate) fn mode_class(m: u16, r: usize, long: bool) -> u8 {
    match (m, r) {
        (0, _) => class::DN,
        (1, _) => class::AN,
        (2, _) => class::IND,
        (3, _) => class::INC,
        (4, _) => class::DEC,
        (5, _) | (7, 2) => class::D16,
        (7, 0) => class::ABS_W,
        (7, 1) => class::ABS_L,
        (7, 4) if long => class::IMM_L,
        (7, 4) => class::IMM_W,
        _ => class::BRIEF,
    }
}

/// 8.2.1 Fetch Effective Address, cache case.
const FEA: [E; class::COUNT] = [
    e(0, 0, 0),  // Dn
    e(0, 0, 0),  // An
    e(4, 1, 0),  // (An)
    e(4, 1, 0),  // (An)+
    e(5, 1, 0),  // -(An)
    e(5, 1, 0),  // (d16,An), (d16,PC)
    e(4, 1, 0),  // (xxx).W
    e(4, 1, 0),  // (xxx).L
    e(2, 0, 0),  // #<data>.B, .W
    e(4, 0, 0),  // #<data>.L
    e(7, 1, 0),  // (d8,An,Xn)
    e(7, 1, 0),  // (d16,An,Xn)
    e(7, 1, 0),  // (B)
    e(9, 1, 0),  // (d16,B)
    e(13, 1, 0), // (d32,B)
    e(12, 2, 0), // ([B],I)
    e(14, 2, 0), // ([B],I,d16)
    e(14, 2, 0), // ([B],I,d32)
    e(14, 2, 0), // ([d16,B],I)
    e(16, 2, 0), // ([d16,B],I,d16)
    e(16, 2, 0), // ([d16,B],I,d32)
    e(18, 2, 0), // ([d32,B],I)
    e(20, 2, 0), // ([d32,B],I,d16)
    e(20, 2, 0), // ([d32,B],I,d32)
];

/// 8.2.2 Fetch Immediate Effective Address, cache case: [#<data>.W,
/// #<data>.L] with the destination of each row. No An row; the
/// ([d16,B],I,d32) row is missing in the manual and takes ([d16,B],I,d16)'s.
const FIEA: [[E; 2]; class::COUNT] = [
    [e(2, 0, 0), e(4, 0, 0)],   // Dn
    [NA, NA],                   // An
    [e(4, 1, 0), e(4, 1, 0)],   // (An)
    [e(6, 1, 0), e(8, 1, 0)],   // (An)+
    [e(5, 1, 0), e(7, 1, 0)],   // -(An)
    [e(5, 1, 0), e(7, 1, 0)],   // (bd,An)
    [e(5, 1, 0), e(7, 1, 0)],   // xxx.W
    [e(6, 1, 0), e(8, 1, 0)],   // xxx.L
    [e(4, 0, 0), e(6, 0, 0)],   // #<data>.B,W
    [e(6, 0, 0), e(8, 0, 0)],   // #<data>.L
    [e(9, 1, 0), e(11, 1, 0)],  // (d8,An,Xn)
    [e(9, 1, 0), e(11, 1, 0)],  // (d16,An,Xn)
    [e(9, 1, 0), e(11, 1, 0)],  // (B)
    [e(11, 1, 0), e(13, 1, 0)], // (d16,B)
    [e(15, 1, 0), e(17, 1, 0)], // (d32,B)
    [e(14, 2, 0), e(16, 2, 0)], // ([B],I)
    [e(16, 2, 0), e(18, 2, 0)], // ([B],I,d16)
    [e(16, 2, 0), e(18, 2, 0)], // ([B],I,d32)
    [e(16, 2, 0), e(18, 2, 0)], // ([d16,B],I)
    [e(18, 2, 0), e(20, 2, 0)], // ([d16,B],I,d16)
    [e(18, 2, 0), e(20, 2, 0)], // ([d16,B],I,d32) (not in the manual)
    [e(20, 2, 0), e(22, 2, 0)], // ([d32,B],I)
    [e(22, 2, 0), e(24, 2, 0)], // ([d32,B],I,d16)
    [e(22, 2, 0), e(24, 2, 0)], // ([d32,B],I,d32)
];

/// 8.2.3 Calculate Effective Address, cache case (the "<data>.W/.L" rows
/// are the absolute addresses).
const CEA: [E; class::COUNT] = [
    e(0, 0, 0),  // Dn
    e(0, 0, 0),  // An
    e(2, 0, 0),  // (An)
    e(2, 0, 0),  // (An)+
    e(2, 0, 0),  // -(An)
    e(2, 0, 0),  // (d16,An), (d16,PC)
    e(2, 0, 0),  // xxx.W
    e(4, 0, 0),  // xxx.L
    NA,          // #<data>
    NA,          // #<data>.L
    e(4, 0, 0),  // (d8,An,Xn)
    e(6, 0, 0),  // (d16,An,Xn)
    e(6, 0, 0),  // (B)
    e(8, 0, 0),  // (d16,B)
    e(12, 0, 0), // (d32,B)
    e(11, 1, 0), // ([B],I)
    e(13, 1, 0), // ([B],I,d16)
    e(13, 1, 0), // ([B],I,d32)
    e(13, 1, 0), // ([d16,B],I)
    e(15, 1, 0), // ([d16,B],I,d16)
    e(15, 1, 0), // ([d16,B],I,d32)
    e(17, 1, 0), // ([d32,B],I)
    e(19, 1, 0), // ([d32,B],I,d16)
    e(19, 1, 0), // ([d32,B],I,d32)
];

/// 8.2.4 Calculate Immediate Effective Address, cache case: [#<data>.W,
/// #<data>.L]. The manual has no -(An) row; it takes (An)+'s.
const CIEA: [[E; 2]; class::COUNT] = [
    [e(2, 0, 0), e(4, 0, 0)],   // Dn
    [NA, NA],                   // An
    [e(2, 0, 0), e(4, 0, 0)],   // (An)
    [e(4, 0, 0), e(6, 0, 0)],   // (An)+
    [e(4, 0, 0), e(6, 0, 0)],   // -(An) (not in the manual)
    [e(4, 0, 0), e(6, 0, 0)],   // (bd,An)
    [e(4, 0, 0), e(6, 0, 0)],   // xxx.W
    [e(4, 0, 0), e(8, 0, 0)],   // xxx.L
    [NA, NA],                   // #<data>
    [NA, NA],                   // #<data>.L
    [e(6, 0, 0), e(8, 0, 0)],   // (d8,An,Xn)
    [e(8, 0, 0), e(10, 0, 0)],  // (d16,An,Xn)
    [e(8, 0, 0), e(10, 0, 0)],  // (B)
    [e(10, 0, 0), e(12, 0, 0)], // (d16,B)
    [e(14, 0, 0), e(16, 0, 0)], // (d32,B)
    [e(13, 1, 0), e(15, 1, 0)], // ([B],I)
    [e(15, 1, 0), e(17, 1, 0)], // ([B],I,d16)
    [e(15, 1, 0), e(17, 1, 0)], // ([B],I,d32)
    [e(15, 1, 0), e(17, 1, 0)], // ([d16,B],I)
    [e(17, 1, 0), e(19, 1, 0)], // ([d16,B],I,d16)
    [e(17, 1, 0), e(19, 1, 0)], // ([d16,B],I,d32)
    [e(19, 1, 0), e(21, 1, 0)], // ([d32,B],I)
    [e(21, 1, 0), e(23, 1, 0)], // ([d32,B],I,d16)
    [e(21, 1, 0), e(23, 1, 0)], // ([d32,B],I,d32)
];

/// 8.2.5 Jump Effective Address, cache case.
const JEA: [E; class::COUNT] = [
    NA,          // Dn
    NA,          // An
    e(2, 0, 0),  // (An)
    NA,          // (An)+
    NA,          // -(An)
    e(4, 0, 0),  // (d16,An), (d16,PC)
    e(2, 0, 0),  // (xxx).W
    e(2, 0, 0),  // (xxx).L
    NA,          // #<data>
    NA,          // #<data>.L
    e(6, 0, 0),  // (d8,An,Xn)
    e(6, 0, 0),  // (d16,An,Xn)
    e(6, 0, 0),  // (B)
    e(8, 0, 0),  // (B,d16)
    e(12, 0, 0), // (B,d32)
    e(11, 1, 0), // ([B],I)
    e(13, 1, 0), // ([B],I,d16)
    e(13, 1, 0), // ([B],I,d32)
    e(13, 1, 0), // ([d16,B],I)
    e(15, 1, 0), // ([d16,B],I,d16)
    e(15, 1, 0), // ([d16,B],I,d32)
    e(17, 1, 0), // ([d32,B],I)
    e(19, 1, 0), // ([d32,B],I,d16)
    e(19, 1, 0), // ([d32,B],I,d32)
];

/// 8.2.6 MOVE Instruction, cache case: total clocks by source row (Rn,
/// #<data>.B/W, #<data>.L, (An), (An)+, -(An), (d16,An), (xxx).W, (xxx).L,
/// (d8,An,Xn), (d16,An,Xn), (B), (d16,B), (d32,B), the nine memory
/// indirect rows) and destination column (An, Dn, (An), (An)+, -(An),
/// (d16,An), (xxx).W, (xxx).L, (d8,An,Xn), (d16,An,Xn), (B), (d16,B),
/// (d32,B), the nine memory indirect columns). The reads and writes
/// follow from the modes.
#[rustfmt::skip]
const MOVE: [[u8; 22]; 23] = [
    [2, 2, 4, 4, 5, 5, 4, 6, 7, 9, 8, 10, 14, 12, 14, 15, 14, 16, 17, 18, 20, 21],
    [4, 4, 6, 6, 7, 7, 6, 8, 7, 9, 8, 10, 14, 12, 14, 15, 14, 16, 17, 18, 20, 21],
    [6, 6, 8, 8, 9, 9, 8, 10, 9, 11, 10, 12, 16, 14, 16, 17, 16, 18, 19, 20, 22, 23],
    [6, 6, 7, 7, 7, 7, 7, 9, 9, 11, 10, 12, 16, 14, 16, 17, 16, 18, 19, 20, 22, 23],
    [6, 6, 7, 7, 7, 7, 7, 9, 9, 11, 10, 12, 16, 14, 16, 17, 16, 18, 19, 20, 22, 23],
    [7, 7, 8, 8, 8, 8, 8, 10, 10, 12, 11, 13, 17, 15, 17, 18, 17, 19, 20, 21, 23, 24],
    [7, 7, 8, 8, 8, 8, 8, 10, 10, 12, 11, 13, 17, 15, 17, 18, 17, 19, 20, 21, 23, 24],
    [6, 6, 7, 7, 7, 7, 7, 9, 9, 11, 10, 12, 16, 14, 16, 17, 16, 18, 19, 20, 22, 23],
    [6, 6, 7, 7, 7, 7, 7, 9, 9, 11, 10, 12, 16, 14, 16, 17, 16, 18, 19, 20, 22, 23],
    [9, 9, 10, 10, 10, 10, 10, 12, 12, 14, 13, 15, 19, 17, 19, 20, 19, 21, 22, 23, 25, 26],
    [9, 9, 10, 10, 10, 10, 10, 12, 12, 14, 13, 15, 19, 17, 19, 20, 19, 21, 22, 23, 25, 26],
    [9, 9, 10, 10, 10, 10, 10, 12, 12, 14, 13, 15, 19, 17, 19, 20, 19, 21, 22, 23, 25, 26],
    [11, 11, 12, 12, 12, 12, 12, 14, 14, 16, 15, 17, 21, 19, 21, 22, 21, 23, 24, 25, 27, 28],
    [15, 15, 16, 16, 16, 16, 16, 18, 18, 20, 19, 21, 25, 23, 25, 26, 25, 27, 28, 29, 31, 32],
    [14, 14, 15, 15, 15, 15, 15, 17, 17, 19, 18, 20, 24, 22, 24, 25, 24, 26, 27, 28, 30, 31],
    [16, 16, 17, 17, 17, 17, 17, 19, 19, 21, 20, 22, 26, 24, 26, 27, 26, 28, 29, 30, 32, 33],
    [16, 16, 17, 17, 17, 17, 17, 19, 19, 21, 20, 22, 26, 24, 26, 27, 26, 28, 29, 30, 32, 33],
    [16, 16, 17, 17, 17, 17, 17, 19, 19, 21, 20, 22, 26, 24, 26, 27, 26, 28, 29, 30, 32, 33],
    [18, 18, 19, 19, 19, 19, 19, 21, 21, 23, 22, 24, 28, 26, 28, 29, 28, 30, 31, 32, 34, 35],
    [18, 18, 19, 19, 19, 19, 19, 21, 21, 23, 22, 24, 28, 26, 28, 29, 28, 30, 31, 32, 34, 35],
    [20, 20, 21, 21, 21, 21, 21, 23, 23, 25, 24, 26, 30, 28, 30, 31, 30, 32, 33, 34, 36, 37],
    [22, 22, 23, 23, 23, 23, 23, 25, 25, 27, 26, 28, 32, 30, 32, 33, 32, 34, 35, 36, 38, 39],
    [22, 22, 23, 23, 23, 23, 23, 25, 25, 27, 26, 28, 32, 30, 32, 33, 32, 34, 35, 36, 38, 39],
];

/// The MOVE table's internal clocks for source class `s`, destination `d`.
fn move_internal(s: u8, d: u8) -> u16 {
    let row = match s {
        class::DN | class::AN => 0,
        class::IMM_W => 1,
        class::IMM_L => 2,
        c @ class::IND..=class::ABS_L => usize::from(c) + 1,
        c => usize::from(c) - 1,
    };
    let col = match d {
        class::AN => 0,
        class::DN | class::IMM_W | class::IMM_L => 1,
        c @ class::IND..=class::ABS_L => usize::from(c),
        c => usize::from(c) - 2,
    };
    let reads = FEA[usize::from(s)].reads + CEA[usize::from(d)].reads;
    let writes = u8::from(!matches!(d, class::DN | class::AN));
    e(u16::from(MOVE[row][col]), reads, writes).internal()
}

/// Which effective address table an instruction adds (the footnotes of
/// the instruction tables).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Complete as given (or no instruction: an exception).
    #[default]
    Op,
    /// Fetch effective address (8.2.1).
    Fea,
    /// Fetch immediate effective address (8.2.2).
    Fiea,
    /// Calculate effective address (8.2.3).
    Cea,
    /// Calculate immediate effective address (8.2.4).
    Ciea,
    /// Jump effective address (8.2.5).
    Jea,
    /// The MOVE table (8.2.6).
    Move,
}

fn ea_internal(kind: Kind, long: bool, ea: Option<u8>) -> u16 {
    let Some(c) = ea.map(usize::from) else {
        return 0;
    };
    let l = usize::from(long);
    match kind {
        Kind::Op | Kind::Move => 0,
        Kind::Fea => FEA[c].internal(),
        Kind::Fiea => FIEA[c][l].internal(),
        Kind::Cea => CEA[c].internal(),
        Kind::Ciea => CIEA[c][l].internal(),
        Kind::Jea => JEA[c].internal(),
    }
}

/// An instruction's timing: the table it adds, the internal clocks of its
/// own entry, and those when it writes (CAS/CAS2 succeeding).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    pub kind: Kind,
    pub op: u16,
    pub on_write: Option<u16>,
    /// The immediate effective address tables' #<data>.L column.
    pub long_imm: bool,
}

fn plan(kind: Kind, entry: E) -> Plan {
    Plan {
        kind,
        op: entry.internal(),
        on_write: None,
        long_imm: false,
    }
}

fn fea(entry: E) -> Plan {
    plan(Kind::Fea, entry)
}

fn cea(entry: E) -> Plan {
    plan(Kind::Cea, entry)
}

fn ciea(entry: E) -> Plan {
    plan(Kind::Ciea, entry)
}

fn fiea(entry: E, long: bool) -> Plan {
    Plan {
        long_imm: long,
        ..plan(Kind::Fiea, entry)
    }
}

/// No instruction time: illegal opcodes, TRAP, line A/F (the exception
/// brings its own).
const NONE: Plan = Plan {
    kind: Kind::Op,
    op: 0,
    on_write: None,
    long_imm: false,
};

// 8.2.7 Special-Purpose MOVE Instruction.
const EXG: E = e(2, 0, 0);
const MOVEC_TO_RN: E = e(6, 0, 0);
const MOVEC_TO_CR: E = e(12, 0, 0);
const MOVE_PSW_RN: E = e(4, 0, 0);
const MOVE_PSW_MEM: E = e(5, 0, 1);
const MOVE_TO_CCR: E = e(4, 0, 0);
const MOVE_TO_SR: E = e(8, 0, 0);
/// MOVEM EA,RL: 8 + 4n (n/0/0), n added at classification.
const MOVEM_TO_REGS: E = e(8, 0, 0);
/// MOVEM RL,EA: 4 + 3n (0/0/n).
const MOVEM_TO_MEM: E = e(4, 0, 0);
const MOVEP_W_TO_MEM: E = e(11, 0, 2);
const MOVEP_L_TO_MEM: E = e(17, 0, 4);
const MOVEP_W_TO_REG: E = e(12, 2, 0);
const MOVEP_L_TO_REG: E = e(18, 4, 0);
const MOVES_TO_REG: E = e(7, 1, 0);
const MOVES_TO_MEM: E = e(5, 0, 1);
const MOVE_USP: E = e(2, 0, 0);
const SWAP: E = e(4, 0, 0);

// 8.2.8 Arithmetic/Logical Instructions (ADD, AND, EOR, OR, SUB share
// their rows).
const ALU_TO_REG: E = e(2, 0, 0);
const ALU_TO_MEM: E = e(4, 0, 1);
const CMPA: E = e(4, 0, 0);
const CMP2: E = e(18, 1, 0);
const MUL_W: E = e(27, 0, 0);
const MUL_L: E = e(43, 0, 0);
const DIVU_W: E = e(44, 0, 0);
const DIVU_L: E = e(78, 0, 0);
const DIVS_W: E = e(56, 0, 0);
const DIVS_L: E = e(90, 0, 0);

// 8.2.9 Immediate Arithmetic/Logical Instructions.
const MOVEQ: E = e(2, 0, 0);
const QUICK_REG: E = e(2, 0, 0);
const QUICK_MEM: E = e(4, 0, 1);
const IMM_DN: E = e(2, 0, 0);
const IMM_MEM: E = e(4, 0, 1);
const CMPI: E = e(2, 0, 0);

// 8.2.10 Binary-Coded Decimal Operations.
const BCD_REG: E = e(4, 0, 0);
const BCD_MEM: E = e(16, 2, 1);
const ADDX_REG: E = e(2, 0, 0);
const ADDX_MEM: E = e(12, 2, 1);
const CMPM: E = e(9, 2, 0);
const PACK_REG: E = e(6, 0, 0);
const PACK_MEM: E = e(13, 1, 1);
const UNPK_REG: E = e(8, 0, 0);
const UNPK_MEM: E = e(13, 1, 1);

// 8.2.11 Single-Operand Instructions (NEG, NEGX, NOT share their rows).
const CLR_REG: E = e(2, 0, 0);
const CLR_MEM: E = e(4, 0, 1);
const UNARY_REG: E = e(2, 0, 0);
const UNARY_MEM: E = e(4, 0, 1);
const EXT: E = e(4, 0, 0);
const NBCD_REG: E = e(6, 0, 0);
/// Not in the manual: NBCD Dn's operation plus the write.
const NBCD_MEM: E = e(9, 0, 1);
const SCC_REG: E = e(4, 0, 0);
const SCC_MEM: E = e(6, 0, 1);
const TAS_REG: E = e(4, 0, 0);
const TAS_MEM: E = e(12, 1, 1);
const TST: E = e(2, 0, 0);

// 8.2.12 Shift/Rotate Instructions.
const LS_STATIC: E = e(4, 0, 0);
const LS_DYNAMIC: E = e(6, 0, 0);
const LS_MEM: E = e(5, 0, 1);
const ASL_REG: E = e(8, 0, 0);
const ASR_REG: E = e(6, 0, 0);
const ASL_MEM: E = e(6, 0, 1);
const ASR_MEM: E = e(5, 0, 1);
const RO_REG: E = e(8, 0, 0);
const RO_MEM: E = e(7, 0, 1);
const ROX_REG: E = e(12, 0, 0);
const ROX_MEM: E = e(5, 0, 1);

// 8.2.13 Bit Manipulation Instructions.
const BIT_REG: E = e(4, 0, 0);
const BTST_MEM: E = e(4, 0, 0);
const BCHG_MEM: E = e(4, 0, 1);

// 8.2.14 Bit Field Manipulation Instructions: [Dn, Mem < 5 bytes, Mem 5
// bytes] by type (BFTST, BFEXTU, BFCHG, BFEXTS, BFCLR, BFFFO, BFSET,
// BFINS).
const BITFIELD: [[E; 3]; 8] = [
    [e(6, 0, 0), e(11, 1, 0), e(15, 2, 0)],  // BFTST
    [e(8, 0, 0), e(13, 1, 0), e(18, 2, 0)],  // BFEXTU
    [e(12, 0, 0), e(16, 1, 1), e(24, 2, 2)], // BFCHG
    [e(8, 0, 0), e(13, 1, 0), e(18, 2, 0)],  // BFEXTS
    [e(12, 0, 0), e(16, 1, 1), e(24, 2, 2)], // BFCLR
    [e(18, 0, 0), e(24, 1, 0), e(32, 2, 0)], // BFFFO
    [e(12, 0, 0), e(16, 1, 1), e(24, 2, 2)], // BFSET
    [e(10, 0, 0), e(14, 1, 1), e(20, 2, 2)], // BFINS
];

// 8.2.15 Conditional Branch Instructions.
const BCC_TAKEN: E = e(6, 0, 0);
const BCC_B_NOT_TAKEN: E = e(4, 0, 0);
const BCC_W_NOT_TAKEN: E = e(6, 0, 0);
const BCC_L_NOT_TAKEN: E = e(6, 0, 0);
const DBCC_LOOP: E = e(6, 0, 0);
const DBCC_EXPIRED: E = e(10, 0, 0);
const DBCC_TRUE: E = e(6, 0, 0);

// 8.2.16 Control Instructions.
const IMM_TO_SR: E = e(12, 0, 0);
const BSR: E = e(7, 0, 1);
const CAS_OK: E = e(15, 1, 1);
const CAS_FAIL: E = e(12, 1, 0);
const CAS2_OK: E = e(25, 2, 2);
const CAS2_FAIL: E = e(22, 2, 0);
const CHK: E = e(8, 0, 0);
const CHK2: E = e(18, 2, 0);
const JMP: E = e(4, 0, 0);
const JSR: E = e(5, 0, 1);
const LEA: E = e(2, 0, 0);
const LINK_W: E = e(5, 0, 1);
const LINK_L: E = e(6, 0, 1);
const NOP: E = e(2, 0, 0);
const PEA: E = e(5, 0, 1);
const RTD: E = e(10, 1, 0);
const RTR: E = e(14, 2, 0);
const RTS: E = e(10, 1, 0);
const UNLK: E = e(6, 1, 0);

// 8.2.17 Exception-Related Instructions.
const RESET: E = e(518, 0, 0);
const STOP: E = e(8, 0, 0);
const TRAPCC: E = e(4, 0, 0);
const TRAPCC_W: E = e(6, 0, 0);
const TRAPCC_L: E = e(8, 0, 0);
const TRAPV: E = e(4, 0, 0);
/// TRAP #n, illegal instruction, line A/F, privilege violation: 20(1/0/4).
pub(crate) const EXCEPTION_SHORT: u16 = e(20, 1, 4).internal();
/// Trace, TRAPcc/TRAPV taking the trap (the six-word frame): 25(1/0/5).
pub(crate) const EXCEPTION_LONG: u16 = e(25, 1, 5).internal();
/// Interrupt on the interrupt stack: 26(2/0/4).
pub(crate) const INTERRUPT: u16 = e(26, 2, 4).internal();
/// Interrupt with the throwaway frame (from the master stack): 41(2/0/8).
pub(crate) const INTERRUPT_MASTER: u16 = e(41, 2, 8).internal();

// 8.2.18 Save and Restore Operations.
const RTE: E = e(21, 4, 0);

/// The timing of instruction `op` (decoded like the execution units; the
/// extension word in IRC and the registers decide the variants known
/// before execution).
pub(crate) fn classify(cpu: &M68000, op: u16) -> Plan {
    let m = (op >> 3) & 7;
    let r = usize::from(op & 7);
    let ext = cpu.prefetch_queue()[1];
    match op >> 12 {
        0x0 => line0(op, m, r, ext),
        0x1..=0x3 => plan(Kind::Move, NA),
        0x4 => line4(op, m, ext),
        0x5 => line5(cpu, op, m, r),
        0x6 => {
            let cc = (op >> 8) & 15;
            op_entry(match (cc, op as u8) {
                (1, _) => BSR,
                (0, _) => BCC_TAKEN,
                _ if cpu.condition(cc) => BCC_TAKEN,
                (_, 0) => BCC_W_NOT_TAKEN,
                (_, 0xff) => BCC_L_NOT_TAKEN,
                _ => BCC_B_NOT_TAKEN,
            })
        }
        0x7 => op_entry(MOVEQ),
        0x8 | 0xc => match op & 0x1f0 {
            0x100 => op_entry(if op & 8 != 0 { BCD_MEM } else { BCD_REG }),
            0x140 if op & 0xf000 == 0x8000 => {
                op_entry(if op & 8 != 0 { PACK_MEM } else { PACK_REG })
            }
            0x180 if op & 0xf000 == 0x8000 => {
                op_entry(if op & 8 != 0 { UNPK_MEM } else { UNPK_REG })
            }
            0x140 | 0x180
                if op & 0xf000 == 0xc000 && matches!(op & 0x1f8, 0x140 | 0x148 | 0x188) =>
            {
                op_entry(EXG)
            }
            _ if op & 0xc0 == 0xc0 => fea(match (op >> 12, op & 0x100 != 0) {
                (0x8, false) => DIVU_W,
                (0x8, true) => DIVS_W,
                _ => MUL_W,
            }),
            _ => fea(if op & 0x100 != 0 {
                ALU_TO_MEM
            } else {
                ALU_TO_REG
            }),
        },
        0x9 | 0xd => {
            if op & 0xc0 == 0xc0 {
                fea(ALU_TO_REG)
            } else if op & 0x130 == 0x100 {
                op_entry(if op & 8 != 0 { ADDX_MEM } else { ADDX_REG })
            } else {
                fea(if op & 0x100 != 0 {
                    ALU_TO_MEM
                } else {
                    ALU_TO_REG
                })
            }
        }
        0xb => {
            if op & 0xc0 == 0xc0 {
                fea(CMPA)
            } else if op & 0x100 == 0 {
                fea(ALU_TO_REG)
            } else if m == 1 {
                op_entry(CMPM)
            } else {
                fea(if m == 0 { ALU_TO_REG } else { ALU_TO_MEM })
            }
        }
        0xe => line_e(cpu, op, m, ext),
        // Line A/F: the exception.
        _ => NONE,
    }
}

fn op_entry(entry: E) -> Plan {
    plan(Kind::Op, entry)
}

fn line0(op: u16, m: u16, r: usize, ext: u16) -> Plan {
    if op & 0x0100 != 0 {
        if m == 1 {
            return op_entry(match (op >> 6) & 3 {
                0 => MOVEP_W_TO_REG,
                1 => MOVEP_L_TO_REG,
                2 => MOVEP_W_TO_MEM,
                _ => MOVEP_L_TO_MEM,
            });
        }
        // Dynamic bit number.
        return match (m, (op >> 6) & 3) {
            (0, _) => op_entry(BIT_REG),
            (_, 0) => fea(BTST_MEM),
            _ => fea(BCHG_MEM),
        };
    }
    let kind = (op >> 9) & 7;
    if op & 0xc0 == 0xc0 {
        return match kind {
            0..=2 => fiea(if ext & 0x0800 != 0 { CHK2 } else { CMP2 }, false),
            5..=7 if op & 0x3f == 0x3c && kind != 5 => Plan {
                on_write: Some(CAS2_OK.internal()),
                ..op_entry(CAS2_FAIL)
            },
            5..=7 => Plan {
                on_write: Some(CAS_OK.internal()),
                ..ciea(CAS_FAIL)
            },
            _ => NONE,
        };
    }
    if kind == 4 {
        // Static bit number in the extension word.
        return match (m, (op >> 6) & 3) {
            (0, _) => op_entry(BIT_REG),
            (_, 0) => fiea(BTST_MEM, false),
            _ => fiea(BCHG_MEM, false),
        };
    }
    if kind == 7 {
        return ciea(if ext & 0x0800 != 0 {
            MOVES_TO_MEM
        } else {
            MOVES_TO_REG
        });
    }
    if m == 7 && r == 4 {
        return op_entry(IMM_TO_SR);
    }
    let long = (op >> 6) & 3 == 2;
    let entry = if kind == 6 {
        CMPI
    } else if m == 0 {
        IMM_DN
    } else {
        IMM_MEM
    };
    fiea(entry, long)
}

fn line4(op: u16, m: u16, ext: u16) -> Plan {
    match op {
        0x4e70 => return op_entry(RESET),
        0x4e71 => return op_entry(NOP),
        0x4e72 => return op_entry(STOP),
        0x4e73 => return op_entry(RTE),
        0x4e74 => return op_entry(RTD),
        0x4e75 => return op_entry(RTS),
        0x4e76 => return op_entry(TRAPV),
        0x4e77 => return op_entry(RTR),
        0x4e7a => return op_entry(MOVEC_TO_RN),
        0x4e7b => return op_entry(MOVEC_TO_CR),
        _ => {}
    }
    match op & 0xfff0 {
        0x4e40 => return NONE,
        0x4e60 => return op_entry(MOVE_USP),
        _ => {}
    }
    match op & 0xfff8 {
        0x4e50 => return op_entry(LINK_W),
        0x4e58 => return op_entry(UNLK),
        0x4808 => return op_entry(LINK_L),
        0x4848 => return NONE,
        0x4840 => return op_entry(SWAP),
        0x4880 | 0x48c0 | 0x49c0 => return op_entry(EXT),
        _ => {}
    }
    if op & 0xf1c0 == 0x41c0 {
        return cea(LEA);
    }
    if op & 0xf140 == 0x4100 {
        return fea(CHK);
    }
    match op & 0xffc0 {
        0x40c0 | 0x42c0 if m == 0 => op_entry(MOVE_PSW_RN),
        0x40c0 | 0x42c0 => cea(MOVE_PSW_MEM),
        0x44c0 => fea(MOVE_TO_CCR),
        0x46c0 => fea(MOVE_TO_SR),
        0x4800 if m == 0 => op_entry(NBCD_REG),
        0x4800 => fea(NBCD_MEM),
        0x4840 => cea(PEA),
        0x4880 | 0x48c0 => ciea(MOVEM_TO_MEM),
        0x4c80 | 0x4cc0 => Plan {
            op: MOVEM_TO_REGS.internal() + ext.count_ones() as u16,
            ..ciea(MOVEM_TO_REGS)
        },
        0x4ac0 if op == 0x4afc => NONE,
        0x4ac0 if m == 0 => op_entry(TAS_REG),
        0x4ac0 => cea(TAS_MEM),
        0x4c00 => fiea(MUL_L, false),
        0x4c40 => fiea(if ext & 0x0800 != 0 { DIVS_L } else { DIVU_L }, false),
        0x4e80 => plan(Kind::Jea, JSR),
        0x4ec0 => plan(Kind::Jea, JMP),
        _ => match op & 0xff00 {
            0x4200 if m == 0 => op_entry(CLR_REG),
            0x4200 => cea(CLR_MEM),
            0x4a00 => fea(TST),
            0x4000 | 0x4400 | 0x4600 if m == 0 => op_entry(UNARY_REG),
            0x4000 | 0x4400 | 0x4600 => fea(UNARY_MEM),
            _ => NONE,
        },
    }
}

fn line5(cpu: &M68000, op: u16, m: u16, r: usize) -> Plan {
    if op & 0xc0 != 0xc0 {
        // ADDQ/SUBQ.
        return if m <= 1 {
            op_entry(QUICK_REG)
        } else {
            fea(QUICK_MEM)
        };
    }
    let cc = (op >> 8) & 15;
    if m == 1 {
        return op_entry(if cpu.condition(cc) {
            DBCC_TRUE
        } else if cpu.d[r] & 0xffff == 0 {
            DBCC_EXPIRED
        } else {
            DBCC_LOOP
        });
    }
    match (m, r) {
        (7, 2) => op_entry(TRAPCC_W),
        (7, 3) => op_entry(TRAPCC_L),
        (7, 4) => op_entry(TRAPCC),
        (0, _) => op_entry(SCC_REG),
        _ => cea(SCC_MEM),
    }
}

fn line_e(cpu: &M68000, op: u16, m: u16, ext: u16) -> Plan {
    let left = op & 0x100 != 0;
    if op & 0xc0 != 0xc0 {
        // Register shifts and rotates.
        return op_entry(match ((op >> 3) & 3, left) {
            (0, true) => ASL_REG,
            (0, false) => ASR_REG,
            (1, _) if op & 0x20 != 0 => LS_DYNAMIC,
            (1, _) => LS_STATIC,
            (2, _) => ROX_REG,
            _ => RO_REG,
        });
    }
    if op & 0x0800 == 0 {
        // Memory shifts by one.
        return fea(match ((op >> 9) & 3, left) {
            (0, true) => ASL_MEM,
            (0, false) => ASR_MEM,
            (1, _) => LS_MEM,
            (2, _) => ROX_MEM,
            _ => RO_MEM,
        });
    }
    // Bit fields: a field spreading over five bytes takes two operand
    // cycles.
    let row = &BITFIELD[usize::from((op >> 8) & 7)];
    if m == 0 {
        return op_entry(row[0]);
    }
    let offset = if ext & 0x0800 != 0 {
        cpu.d[usize::from((ext >> 6) & 7)]
    } else {
        u32::from((ext >> 6) & 31)
    };
    let width = match if ext & 0x0020 != 0 {
        cpu.d[usize::from(ext & 7)] & 31
    } else {
        u32::from(ext & 31)
    } {
        0 => 32,
        w => w,
    };
    let five = (offset & 7) + width > 32;
    ciea(row[if five { 2 } else { 1 }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_clocks_take_out_the_bus_cycles() {
        // ADD Dn,<ea>: 4(0/0/1) leaves one clock besides the write.
        assert_eq!(ALU_TO_MEM.internal(), 1);
        // (An): 4(1/0/0).
        assert_eq!(FEA[usize::from(class::IND)].internal(), 1);
        // MOVE (An),(An): 7(1/0/1).
        assert_eq!(move_internal(class::IND, class::IND), 1);
        // MOVE ([B],I),([d32,B],I,d32): 31(3/0/1) in the cache case.
        assert_eq!(move_internal(class::MEM, class::MEM + 8), 31 - 3 * 4);
        // MOVE #<data>.L,Dn: 6(0/0/0).
        assert_eq!(move_internal(class::IMM_L, class::DN), 6);
    }

    #[test]
    fn full_format_classes() {
        // (bd16,An,Xn) with base and index: (d16,An,Xn).
        assert_eq!(full_class(0x0120), class::D16_INDEX);
        // Base suppressed, bd16: (d16,B).
        assert_eq!(full_class(0x01a0), class::BASE_D16);
        // Null bd, no indirection: (B).
        assert_eq!(full_class(0x0110), class::BASE);
        // ([bd32,An],Xn,od16): memory indirect pre-indexed.
        assert_eq!(full_class(0x0132), class::MEM + 3 * 2 + 1);
        // ([An],Xn) post-indexed, null od.
        assert_eq!(full_class(0x0115), class::MEM);
    }

    #[test]
    fn cache_tags_include_fc2() {
        let mut c = ICache::default();
        c.fill(cacr::E, 0x1000, 6, 0x4e71_4e71);
        assert_eq!(c.lookup(cacr::E, 0x1000, 6), Some(0x4e71_4e71));
        // User program space misses the supervisor entry.
        assert_eq!(c.lookup(cacr::E, 0x1000, 2), None);
        // Another 256-byte line with the same index misses.
        assert_eq!(c.lookup(cacr::E, 0x1100, 6), None);
        // A31–A24 count (the 68EC020 keeps them in the tag).
        assert_eq!(c.lookup(cacr::E, 0xff00_1000, 6), None);
        // Disabled: no hits, the entry stays.
        assert_eq!(c.lookup(0, 0x1000, 6), None);
        assert_eq!(c.lookup(cacr::E, 0x1000, 6), Some(0x4e71_4e71));
        // Frozen: a miss does not replace.
        c.fill(cacr::E | cacr::F, 0x1100, 6, 1);
        assert_eq!(c.lookup(cacr::E, 0x1000, 6), Some(0x4e71_4e71));
        c.clear_entry(0x1000);
        assert_eq!(c.lookup(cacr::E, 0x1000, 6), None);
    }
}
