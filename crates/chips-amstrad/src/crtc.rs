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
//! reads the selected register. No light pen is fitted, so R16/R17 stay 0.
//!
//! [`Crtc::tick`] is one character clock (1 µs on the CPC). Type 1 reloads
//! the start address at the start of every line of row 0; types 0 and 2
//! only at the start of the frame. Type 0's R8 DISPTMG skew value 3 turns
//! the display off; the smaller skews and interlace are not modelled.

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
        if self.hsync > 0 {
            self.hsync -= 1;
        }
        if self.hcc == self.regs[0] {
            self.hcc = 0;
            self.h_disp = true;
            self.end_of_line();
        } else {
            self.hcc = self.hcc.wrapping_add(1);
        }
        if self.hcc == self.regs[1] {
            self.h_disp = false;
            if self.vlc == self.regs[9] {
                self.ma_next = self.ma_row.wrapping_add(u16::from(self.regs[1])) & 0x3fff;
            }
        }
        if self.hcc == self.regs[2] && self.hsync == 0 {
            self.hsync = self.hsync_width();
        }
        out
    }

    fn end_of_line(&mut self) {
        if self.vsync > 0 {
            self.vsync -= 1;
        }
        let mut new_row = false;
        if self.adjusting {
            self.adj += 1;
            self.vlc = (self.vlc + 1) & 0x1f;
            if self.adj >= self.regs[5] {
                self.new_frame();
                new_row = true;
            }
        } else if self.vlc == self.regs[9] {
            self.vlc = 0;
            self.ma_row = self.ma_next;
            if self.vcc == self.regs[4] {
                if self.regs[5] > 0 {
                    self.adjusting = true;
                    self.adj = 0;
                    self.vcc = (self.vcc + 1) & 0x7f;
                } else {
                    self.new_frame();
                }
            } else {
                self.vcc = (self.vcc + 1) & 0x7f;
            }
            new_row = true;
        } else {
            self.vlc = (self.vlc + 1) & 0x1f;
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
        if new_row && !self.adjusting && self.vcc == self.regs[7] && self.vsync == 0 {
            self.vsync = self.vsync_width();
        }
    }

    fn new_frame(&mut self) {
        self.adjusting = false;
        self.adj = 0;
        self.vcc = 0;
        self.vlc = 0;
        self.ma_row = self.start_address();
        self.ma_next = self.ma_row;
        self.v_disp = true;
        self.vblank = false;
    }
}

const TAG: [u8; 4] = *b"CRTC";
const VERSION: u16 = 2;

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
            ] {
                w.bool(v);
            }
            w.u16(self.ma_row);
            w.u16(self.ma_next);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
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
