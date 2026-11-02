//! 6845 CRT controller as fitted to the CPC, in its three common variants
//! (cpcwiki, "CRTC"):
//!
//! | Type | Chip                 | VSYNC width       | HSYNC width 0 | Readable           | &BExx  |
//! | ---- | -------------------- | ----------------- | ------------- | ------------------ | ------ |
//! | 0    | HD6845S / UM6845     | R3 bits 7–4, 0=16 | none          | R12–R17            | –      |
//! | 1    | UM6845R              | 16 lines          | none          | R14–R17, R31 = &FF | status |
//! | 2    | MC6845               | 16 lines          | 16            | R14–R17            | –      |
//!
//! | Reg | Function                       | Reg | Function                         |
//! | --- | ------------------------------ | --- | -------------------------------- |
//! | R0  | horizontal total − 1           | R7  | vertical sync position (rows)    |
//! | R1  | horizontal displayed           | R8  | interlace and skew               |
//! | R2  | horizontal sync position       | R9  | scan lines per row − 1           |
//! | R3  | sync widths (VSYNC 7–4, HSYNC 3–0) | R10/R11 | cursor start / end       |
//! | R4  | vertical total − 1 (rows)      | R12/R13 | start address (MA13–0)       |
//! | R5  | vertical total adjust (lines)  | R14/R15 | cursor address               |
//! | R6  | vertical displayed (rows)      | R16/R17 | light pen (read only)        |
//!
//! Ports: &BCxx selects a register, &BDxx writes it, &BExx reads the type
//! 1 status (bit 5: vertical blanking, set while the row counter is at or
//! past R6, cleared at row 0; bit 6: light pen strobe, set at power-on and
//! cleared by reading R16 or R17, Arnold acid test `type1ctrl`), &BFxx
//! reads the selected register. R16/R17 hold the light-pen address: a
//! strobe on LPSTB (the CPC's expansion port pin 47, LPEN) latches the
//! refresh address MA13–MA8 into R16 and MA7–MA0 into R17 ([`Crtc::light_pen`];
//! Motorola MC6845 data sheet, "Light pen registers"; cpcwiki,
//! "Connector:Expansion port"). Which character's address the strobe
//! catches (the 6845 synchronises it to the character clock) is taken as
//! the character being shown — not verified per CRTC type.
//!
//! [`Crtc::tick`] is one character clock (1 µs on the CPC). Type 1 reloads
//! the start address at the start of every line of row 0; types 0 and 2
//! only at the start of the frame. Type 0's R8 DISPTMG skew value 3 turns
//! the display off; the smaller skews are not modelled.
//!
//! Type 1 interlaces (R8 bit 0; Arnold acid test `crtctest`, "R8 - count
//! lines" with its table for the UM6845R): every second field starts VSYNC
//! half a line late and ends with one extra line, so VSYNC comes every
//! N + ½ lines. With bits 1 and 0 set (interlace sync and video) the scan
//! line counter steps by 2, from 0 or 1 by field. A row ends one short of
//! R9 too, and with an odd number of lines per row the next row starts on
//! the other parity, so rows of 5 and 4 lines alternate at R9 = 8.
//!
//! Type 0 (same test, its table): only R8 = 3 changes anything. The counter
//! steps by 2 until it reaches R9 and the next row starts on the other
//! parity of the last value: 5 lines a row at R9 = 8, rows of 5 and 4 at
//! R9 = 7. No field is half a line late. Type 2 is not interlaced here (the
//! test's table for it is type 3's, its notes say type 2 hangs).
//!
//! In the vertical total adjust the row counter goes on every R9 + 1 lines
//! and VSYNC can start there (`crtctest`, "minimum VADJ value"); type 0
//! counts once on entering it, so only R7 = R4 + 1 still starts one.
//!
//! Type 1 ends a row when the line counter has reached R9 or passed it, and
//! a frame when the row counter has reached R4 or passed it: registers
//! lowered below a running counter take effect at the next line end instead
//! of letting it run round. Inferred, not from a data sheet: the demo "From
//! Scratch" (CNGSOFT, 2014) writes R9 = 0 at line 7 and then R4 = 0 at row
//! 8; with equality the row counter runs on through R7 = 61, a second VSYNC
//! halves the field and the music runs at twice its speed.
//!
//! Type 2 starts no VSYNC while HSYNC is still on in the line's last
//! character (`crtctest`, "hsync length measure (type 2)": R2 + width must
//! stay below R0 + 1).

use emu_core::{Savestate, StateError, StateReader, StateWriter};

/// The CRTC variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrtcType {
    T0,
    T1,
    T2,
}

impl CrtcType {
    pub fn from_number(n: u8) -> Option<Self> {
        match n {
            0 => Some(Self::T0),
            1 => Some(Self::T1),
            2 => Some(Self::T2),
            _ => None,
        }
    }

    pub fn number(self) -> u8 {
        match self {
            Self::T0 => 0,
            Self::T1 => 1,
            Self::T2 => 2,
        }
    }
}

/// Writable bits per register.
const MASKS: [u8; 18] = [
    0xff, 0xff, 0xff, 0xff, 0x7f, 0x1f, 0x7f, 0x7f, 0xff, 0x1f, 0x7f, 0x1f, 0x3f, 0xff, 0x3f, 0xff,
    0x3f, 0xff,
];

/// What one character clock puts out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CrtcOut {
    /// Display enable (DISPTMG).
    pub disp: bool,
    /// Memory address (14 bits) and row address of the character.
    pub ma: u16,
    pub ra: u8,
    pub hsync: bool,
    pub vsync: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crtc {
    pub kind: CrtcType,
    pub regs: [u8; 18],
    /// Selected register (5 bits).
    pub addr: u8,
    /// Horizontal character counter, scan line in the row, row counter.
    hcc: u8,
    vlc: u8,
    vcc: u8,
    /// In the vertical total adjust lines, and how many are done.
    adjusting: bool,
    adj: u8,
    /// Characters / lines of sync left.
    hsync: u8,
    vsync: u8,
    h_disp: bool,
    v_disp: bool,
    /// MA of the current row's first character, and of the next row's.
    ma_row: u16,
    ma_next: u16,
    /// Type 1 status bit 5.
    vblank: bool,
    /// Type 1 status bit 6 (light pen register full).
    lpen_full: bool,
    /// The second field of an interlaced frame.
    odd_field: bool,
    /// That field's extra line has begun.
    extra_line: bool,
    /// VSYNC runs half a line late, and has yet to start.
    vsync_half: bool,
    vsync_pending: bool,
}

impl Crtc {
    pub fn new(kind: CrtcType) -> Self {
        Self {
            kind,
            regs: [0; 18],
            addr: 0,
            hcc: 0,
            vlc: 0,
            vcc: 0,
            adjusting: false,
            adj: 0,
            hsync: 0,
            vsync: 0,
            h_disp: true,
            v_disp: true,
            ma_row: 0,
            ma_next: 0,
            vblank: false,
            lpen_full: true,
            odd_field: false,
            extra_line: false,
            vsync_half: false,
            vsync_pending: false,
        }
    }

    fn interlaced(&self) -> bool {
        self.kind == CrtcType::T1 && self.regs[8] & 1 != 0
    }

    fn video_interlaced(&self) -> bool {
        match self.kind {
            CrtcType::T0 => self.regs[8] & 3 == 3,
            _ => self.interlaced() && self.regs[8] & 2 != 0,
        }
    }

    /// The scan line counter is at the row's last line.
    fn last_line(&self) -> bool {
        if self.video_interlaced() {
            let last = match self.kind {
                CrtcType::T0 => self.regs[9],
                _ => self.regs[9].saturating_sub(1),
            };
            self.vlc >= last
        } else if self.kind == CrtcType::T1 {
            self.vlc >= self.regs[9]
        } else {
            self.vlc == self.regs[9]
        }
    }

    /// The row is the frame's last.
    fn last_row(&self) -> bool {
        if self.kind == CrtcType::T1 {
            self.vcc >= self.regs[4]
        } else {
            self.vcc == self.regs[4]
        }
    }

    fn start_address(&self) -> u16 {
        (u16::from(self.regs[12]) << 8 | u16::from(self.regs[13])) & 0x3fff
    }

    /// Horizontal character, scan line in row, row.
    pub fn counters(&self) -> (u8, u8, u8) {
        (self.hcc, self.vlc, self.vcc)
    }

    /// Port &BCxx.
    pub fn select(&mut self, v: u8) {
        self.addr = v & 0x1f;
    }

    /// Port &BDxx.
    pub fn write(&mut self, v: u8) {
        if let Some(&mask) = MASKS.get(usize::from(self.addr)) {
            if self.addr < 16 {
                self.regs[usize::from(self.addr)] = v & mask;
            }
        }
    }

    /// Port &BFxx (no side effects).
    pub fn read(&self) -> u8 {
        let r = usize::from(self.addr);
        let readable = match self.kind {
            CrtcType::T0 => (12..=17).contains(&r),
            CrtcType::T1 | CrtcType::T2 => (14..=17).contains(&r),
        };
        if readable {
            self.regs[r]
        } else if self.kind == CrtcType::T1 && r == 31 {
            0xff
        } else {
            0
        }
    }

    /// What a CPU read of &BFxx changes besides returning [`Crtc::read`]:
    /// reading R16 or R17 clears the light pen strobe.
    pub fn after_read(&mut self) {
        if matches!(self.addr, 16 | 17) {
            self.lpen_full = false;
        }
    }

    /// A light-pen strobe (LPSTB rising edge) while the CRTC shows the
    /// character at refresh address `ma`: R16 = MA13–8, R17 = MA7–0, and
    /// the type 1 status bit 6 (register full) is set.
    pub fn light_pen(&mut self, ma: u16) {
        self.regs[16] = (ma >> 8) as u8 & 0x3f;
        self.regs[17] = ma as u8;
        self.lpen_full = true;
    }

    /// Port &BExx (no side effects).
    pub fn status(&self) -> u8 {
        match self.kind {
            CrtcType::T1 => u8::from(self.vblank) << 5 | u8::from(self.lpen_full) << 6,
            _ => 0xff,
        }
    }

    fn hsync_width(&self) -> u8 {
        match (self.regs[3] & 0x0f, self.kind) {
            (0, CrtcType::T2) => 16,
            (w, _) => w,
        }
    }

    fn vsync_width(&self) -> u8 {
        match self.kind {
            CrtcType::T0 => match self.regs[3] >> 4 {
                0 => 16,
                w => w,
            },
            _ => 16,
        }
    }

    /// One character clock: what this character shows, then the counters
    /// advance.
    pub fn tick(&mut self) -> CrtcOut {
        let skew_off = self.kind == CrtcType::T0 && self.regs[8] & 0x30 == 0x30;
        let out = CrtcOut {
            disp: self.h_disp && self.v_disp && !skew_off,
            ma: self.ma_row.wrapping_add(u16::from(self.hcc)) & 0x3fff,
            ra: self.vlc,
            hsync: self.hsync > 0,
            vsync: self.vsync > 0,
        };
        let hsync_on = self.hsync > 0;
        if hsync_on {
            self.hsync -= 1;
        }
        if self.hcc == self.regs[0] {
            self.hcc = 0;
            self.h_disp = true;
            self.end_of_line(hsync_on);
        } else {
            self.hcc = self.hcc.wrapping_add(1);
        }
        if self.hcc == self.regs[1] {
            self.h_disp = false;
            if self.last_line() {
                self.ma_next = self.ma_row.wrapping_add(u16::from(self.regs[1])) & 0x3fff;
            }
        }
        if self.hcc == self.regs[2] && self.hsync == 0 {
            self.hsync = self.hsync_width();
        }
        if self.vsync_half && self.hcc == (self.regs[0] >> 1).wrapping_add(1) {
            self.half_line();
        }
        out
    }

    /// The middle of a line, in place of the line end for a late VSYNC.
    fn half_line(&mut self) {
        if self.vsync_pending {
            self.vsync_pending = false;
            self.vsync = self.vsync_width();
        } else {
            self.vsync = self.vsync.saturating_sub(1);
            self.vsync_half = self.vsync > 0;
        }
    }

    /// `hsync_on`: HSYNC was on in the last character.
    fn end_of_line(&mut self, hsync_on: bool) {
        if self.vsync > 0 && !self.vsync_half {
            self.vsync -= 1;
        }
        let mut new_row = false;
        if self.adjusting {
            self.adj += 1;
            self.vlc = (self.vlc + 1) & 0x1f;
            if self.adj >= self.regs[5] {
                new_row = self.end_of_frame();
            } else if self.kind != CrtcType::T0 && self.adj.is_multiple_of(self.regs[9] + 1) {
                self.vcc = (self.vcc + 1) & 0x7f;
                new_row = true;
            }
        } else if self.last_line() {
            self.vlc = if self.kind == CrtcType::T0 && self.video_interlaced() {
                (self.vlc + 1) & 1
            } else if self.video_interlaced() {
                (self.vlc + 2).wrapping_sub(self.regs[9].wrapping_add(1)) & 1
            } else {
                0
            };
            self.ma_row = self.ma_next;
            if self.last_row() {
                if self.regs[5] > 0 {
                    self.adjusting = true;
                    self.adj = 0;
                    self.vcc = (self.vcc + 1) & 0x7f;
                } else if !self.end_of_frame() {
                    self.vcc = (self.vcc + 1) & 0x7f;
                }
            } else {
                self.vcc = (self.vcc + 1) & 0x7f;
            }
            new_row = true;
        } else {
            let step = if self.video_interlaced() { 2 } else { 1 };
            self.vlc = (self.vlc + step) & 0x1f;
        }
        if self.kind == CrtcType::T1 && self.vcc == 0 && !self.adjusting {
            // Type 1 reads R12/R13 again on every line of row 0.
            self.ma_row = self.start_address();
            self.ma_next = self.ma_row;
        }
        if self.vcc == self.regs[6] {
            self.v_disp = false;
            self.vblank = true;
        }
        let held_off = self.kind == CrtcType::T2 && hsync_on;
        if new_row
            && self.vcc == self.regs[7]
            && self.vsync == 0
            && !self.vsync_pending
            && !held_off
        {
            if self.interlaced() && self.odd_field {
                self.vsync_half = true;
                self.vsync_pending = true;
            } else {
                self.vsync = self.vsync_width();
            }
        }
    }

    /// The last line of a frame is over: the next frame starts (true), or
    /// the extra line of an interlaced second field comes first.
    fn end_of_frame(&mut self) -> bool {
        if self.interlaced() && self.odd_field && !self.extra_line {
            self.extra_line = true;
            self.adjusting = true;
            self.adj = self.regs[5];
            return false;
        }
        self.new_frame();
        true
    }

    fn new_frame(&mut self) {
        self.adjusting = false;
        self.adj = 0;
        self.vcc = 0;
        self.extra_line = false;
        self.odd_field = self.interlaced() && !self.odd_field;
        self.vlc = u8::from(self.video_interlaced() && self.odd_field);
        self.ma_row = self.start_address();
        self.ma_next = self.ma_row;
        self.v_disp = true;
        self.vblank = false;
    }
}

const TAG: [u8; 4] = *b"CRTC";
const VERSION: u16 = 3;

impl Savestate for Crtc {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.u8(self.kind.number());
            w.raw(&self.regs);
            for v in [
                self.addr, self.hcc, self.vlc, self.vcc, self.adj, self.hsync, self.vsync,
            ] {
                w.u8(v);
            }
            for v in [
                self.adjusting,
                self.h_disp,
                self.v_disp,
                self.vblank,
                self.lpen_full,
                self.odd_field,
                self.extra_line,
                self.vsync_half,
                self.vsync_pending,
            ] {
                w.bool(v);
            }
            w.u16(self.ma_row);
            w.u16(self.ma_next);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut c) = r.chunk(TAG, VERSION)?;
        let mut n = self.clone();
        if c.u8()? != self.kind.number() {
            return Err(StateError::Invalid("CRTC type differs"));
        }
        c.raw_into(&mut n.regs)?;
        for v in [
            &mut n.addr,
            &mut n.hcc,
            &mut n.vlc,
            &mut n.vcc,
            &mut n.adj,
            &mut n.hsync,
            &mut n.vsync,
        ] {
            *v = c.u8()?;
        }
        for v in [
            &mut n.adjusting,
            &mut n.h_disp,
            &mut n.v_disp,
            &mut n.vblank,
            &mut n.lpen_full,
        ] {
            *v = c.bool()?;
        }
        (n.odd_field, n.extra_line, n.vsync_half, n.vsync_pending) = (false, false, false, false);
        if version >= 3 {
            for v in [
                &mut n.odd_field,
                &mut n.extra_line,
                &mut n.vsync_half,
                &mut n.vsync_pending,
            ] {
                *v = c.bool()?;
            }
        }
        n.ma_row = c.u16()? & 0x3fff;
        n.ma_next = c.u16()? & 0x3fff;
        n.addr &= 0x1f;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CPC firmware's settings.
    fn cpc(kind: CrtcType) -> Crtc {
        let mut c = Crtc::new(kind);
        for (r, v) in [63, 40, 46, 0x8e, 38, 0, 25, 30, 0, 7, 0, 0, 0x30, 0]
            .into_iter()
            .enumerate()
        {
            c.select(r as u8);
            c.write(v);
        }
        c
    }

    /// Lines between VSYNC starts, in half lines, over `n` frames.
    fn vsync_periods(c: &mut Crtc, n: usize) -> Vec<u32> {
        let (mut at, mut last, mut prev, mut out) = (0u32, None, false, Vec::new());
        while out.len() < n {
            let v = c.tick().vsync;
            if v && !prev {
                if let Some(l) = last {
                    out.push((at - l) / 32);
                }
                last = Some(at);
            }
            prev = v;
            at += 1;
        }
        out
    }

    fn set(c: &mut Crtc, r: u8, v: u8) {
        c.select(r);
        c.write(v);
    }

    #[test]
    fn interlaced_type_1_has_vsync_every_n_and_a_half_lines() {
        let mut c = cpc(CrtcType::T1);
        set(&mut c, 8, 1);
        // 39 rows of 8 lines: 312.5 lines from VSYNC to VSYNC.
        assert_eq!(vsync_periods(&mut c, 6)[2..], [625, 625, 625, 625]);
        let mut c = cpc(CrtcType::T0);
        set(&mut c, 8, 1);
        assert_eq!(vsync_periods(&mut c, 4)[2..], [624, 624]);
    }

    #[test]
    fn video_interlace_steps_the_scan_lines_by_field() {
        let mut c = cpc(CrtcType::T1);
        set(&mut c, 8, 3);
        let mut rows = std::collections::BTreeSet::new();
        for _ in 0..64 * 700 {
            rows.insert(c.tick().ra);
        }
        assert_eq!(rows.len(), 8, "both fields' lines appear: {rows:?}");
        while c.counters() != (0, 0, 0) {
            c.tick();
        }
        let first: Vec<u8> = (0..64 * 4)
            .step_by(64)
            .map(|_| {
                let ra = c.tick().ra;
                for _ in 0..63 {
                    c.tick();
                }
                ra
            })
            .collect();
        assert_eq!(first, [0, 2, 4, 6]);
    }

    #[test]
    fn vsync_can_start_in_the_vertical_adjust() {
        let mut c = cpc(CrtcType::T1);
        set(&mut c, 7, 40);
        set(&mut c, 5, 8);
        assert!(
            vsync_periods_none(&mut c),
            "row 40 is out of reach with 8 lines"
        );
        set(&mut c, 5, 9);
        assert_eq!(vsync_periods(&mut c, 2)[1], 2 * (312 + 9));
    }

    fn vsync_periods_none(c: &mut Crtc) -> bool {
        (0..64 * 2000).all(|_| !c.tick().vsync)
    }

    /// Run one frame from its start; returns the characters per line, the
    /// lines per frame, displayed characters and the line VSYNC starts on.
    fn frame(c: &mut Crtc) -> (u32, u32, u32, Option<u32>) {
        // Get to the frame start.
        while c.counters() != (0, 0, 0) {
            c.tick();
        }
        let mut chars = 0;
        let mut disp = 0;
        let mut vsync_line = None;
        let mut prev_vsync = false;
        loop {
            let o = c.tick();
            if o.vsync && !prev_vsync {
                vsync_line = Some(chars / 64);
            }
            prev_vsync = o.vsync;
            disp += u32::from(o.disp);
            chars += 1;
            if c.counters() == (0, 0, 0) {
                break;
            }
        }
        (64, chars / 64, disp, vsync_line)
    }

    /// R4, R9 for a frame of a known length, VSYNC at row 0.
    fn periods(kind: CrtcType, r4: u8, r9: u8, r8: u8) -> Vec<u32> {
        let mut c = cpc(kind);
        set(&mut c, 7, 0);
        set(&mut c, 4, r4);
        set(&mut c, 9, r9);
        set(&mut c, 8, r8);
        vsync_periods(&mut c, 3)
    }

    #[test]
    fn type_0_interlaces_only_with_both_r8_bits() {
        // Lines from `crtctest`, "R8 - count lines", in half lines.
        for r8 in [0, 1, 2] {
            assert_eq!(periods(CrtcType::T0, 16, 8, r8)[2], 0x99 * 2, "R8 = {r8}");
        }
        assert_eq!(periods(CrtcType::T0, 16, 8, 3)[2], 0x55 * 2);
        assert_eq!(periods(CrtcType::T0, 15, 8, 3)[2], 0x50 * 2);
        assert_eq!(periods(CrtcType::T0, 16, 7, 3)[2], 0x4d * 2);
        assert_eq!(periods(CrtcType::T0, 15, 7, 3)[2], 0x48 * 2);
    }

    #[test]
    fn type_0_counts_one_row_in_the_vertical_adjust() {
        let vsync_in_adjust = |kind, r7| {
            let mut c = cpc(kind);
            set(&mut c, 7, r7);
            set(&mut c, 5, 31);
            (0..3 * 64 * 400).any(|_| c.tick().vsync)
        };
        assert!(vsync_in_adjust(CrtcType::T0, 39));
        assert!(!vsync_in_adjust(CrtcType::T0, 40));
        assert!(vsync_in_adjust(CrtcType::T1, 40));
        assert!(vsync_in_adjust(CrtcType::T2, 40));
    }

    #[test]
    fn type_2_starts_no_vsync_with_hsync_on_at_the_line_end() {
        let vsync = |r2| {
            let mut c = cpc(CrtcType::T2);
            set(&mut c, 3, 0x0e);
            set(&mut c, 2, r2);
            (0..3 * 64 * 400).any(|_| c.tick().vsync)
        };
        assert!(
            vsync(64 - 14 - 1),
            "ends in the line's last character but one"
        );
        assert!(!vsync(64 - 14), "still on in the last character");
    }

    #[test]
    fn type_1_ends_rows_and_frames_when_the_counters_are_past_the_registers() {
        // Two rows of eight lines, then R9 and R4 go to 0 in row 1, line 3.
        let lowered = |kind| {
            let mut c = cpc(kind);
            set(&mut c, 7, 61);
            while c.counters() != (0, 3, 1) {
                c.tick();
            }
            set(&mut c, 9, 0);
            set(&mut c, 4, 0);
            (0..64 * 2).for_each(|_| {
                c.tick();
            });
            c.counters()
        };
        assert_eq!(lowered(CrtcType::T1).2, 0, "a new frame at once");
        assert_eq!(lowered(CrtcType::T0).1, 5, "line counter runs on");
    }

    #[test]
    fn standard_cpc_frame() {
        for kind in [CrtcType::T0, CrtcType::T1, CrtcType::T2] {
            let mut c = cpc(kind);
            frame(&mut c);
            let (_, lines, disp, vsync) = frame(&mut c);
            assert_eq!(lines, 312, "{kind:?}");
            assert_eq!(disp, 40 * 200, "{kind:?}");
            assert_eq!(vsync, Some(240), "{kind:?}");
        }
    }

    #[test]
    fn video_addresses_follow_rows() {
        let mut c = cpc(CrtcType::T1);
        frame(&mut c);
        // First character of the frame: MA = R12/R13 = &3000.
        let o = c.tick();
        assert_eq!((o.ma, o.ra, o.disp), (0x3000, 0, true));
        // Row 1 starts 40 characters further on.
        for _ in 0..(64 * 8 - 1) {
            c.tick();
        }
        let o = c.tick();
        assert_eq!((o.ma, o.ra), (0x3000 + 40, 0));
    }

    #[test]
    fn sync_widths_per_type() {
        let mut c = cpc(CrtcType::T0);
        c.select(3);
        c.write(0x00);
        assert_eq!((c.hsync_width(), c.vsync_width()), (0, 16));
        c.kind = CrtcType::T2;
        assert_eq!(c.hsync_width(), 16);
        c.write(0x24);
        c.kind = CrtcType::T0;
        assert_eq!((c.hsync_width(), c.vsync_width()), (4, 2));
        c.kind = CrtcType::T1;
        assert_eq!(c.vsync_width(), 16);
    }

    #[test]
    fn readable_registers_per_type() {
        let mut c = cpc(CrtcType::T0);
        c.select(12);
        assert_eq!(c.read(), 0x30);
        c.kind = CrtcType::T1;
        assert_eq!(c.read(), 0);
        c.select(31);
        assert_eq!(c.read(), 0xff);
        assert_eq!(c.status() & 0x20, 0);
    }

    #[test]
    fn light_pen_strobe_latches_the_refresh_address() {
        for kind in [CrtcType::T0, CrtcType::T1, CrtcType::T2] {
            let mut c = Crtc::new(kind);
            c.light_pen(0x3123);
            for (r, v) in [(16, 0x31), (17, 0x23)] {
                c.select(r);
                assert_eq!(c.read(), v, "{kind:?} R{r}");
            }
            // R16/R17 are read only.
            c.select(16);
            c.write(0);
            assert_eq!(c.read(), 0x31);
            c.after_read();
            if kind == CrtcType::T1 {
                assert_eq!(c.status() & 0x40, 0);
                c.light_pen(0x0001);
                assert_ne!(c.status() & 0x40, 0);
            }
        }
    }

    #[test]
    fn type1_light_pen_strobe_until_r16_is_read() {
        let mut c = cpc(CrtcType::T1);
        assert_eq!(c.status() & 0x40, 0x40, "set at power-on");
        c.select(15);
        c.after_read();
        assert_eq!(c.status() & 0x40, 0x40);
        c.select(16);
        assert_eq!(c.read(), 0);
        c.after_read();
        assert_eq!(c.status() & 0x40, 0);
    }

    #[test]
    fn savestate_round_trip() {
        let mut c = cpc(CrtcType::T0);
        for _ in 0..12345 {
            c.tick();
        }
        let mut w = StateWriter::new();
        c.save(&mut w);
        let bytes = w.into_bytes();
        let mut d = Crtc::new(CrtcType::T0);
        d.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(c, d);
    }
}
