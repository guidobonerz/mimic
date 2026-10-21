//! The custom chip set as one unit: Agnus' beam counters and DMA slot
//! allocation, the bitplane fetch, the Copper, the Blitter, Denise and
//! Paula, advanced one colour clock (one DMA slot) per [`Custom::tick`].
//!
//! **Beam.** PAL: 227 colour clocks per line, 313 lines in a long field and
//! 312 in a short one (non-interlaced fields stay long); NTSC: lines
//! alternate 228/227 clocks, 263/262 lines. LOF toggles every field with
//! BPLCON0 LACE. VPOSR: bit 15 LOF, bits 14–8 the Agnus id (0 on the OCS
//! 8367/8361), bit 0 vertical bit 8; VHPOSR: vertical bits 7–0, horizontal
//! position in colour clocks.
//!
//! **DMA slots** (HRM chapter 7, "DMA time slot allocation"): odd slots
//! $01–$07 refresh, $09–$0D disk, $0F–$15 audio, $17–$35 sprites; bitplanes
//! in their fetch window; the Copper takes even free slots, the Blitter any
//! free slot, the CPU what is left. Without BLTPRI the Blitter leaves every
//! fourth slot to a waiting CPU. Disk, audio and sprite DMA follow in later
//! milestones; their slots stay free until then.
//!
//! **Bitplane fetch.** Inside the vertical display window and between
//! DDFSTRT and DDFSTOP, eight-slot groups fetch the planes in the order
//! (lowres) `- 4 6 2 - 3 5 1` and (hires) `4 2 3 1 4 2 3 1`; groups start
//! at DDFSTRT + 8n while ≤ DDFSTOP. After the line's last group the
//! modulos are added (BPL1MOD odd planes, BPL2MOD even planes).
//!
//! **Picture.** [`Custom::frame`] is the field in 12-bit colours as RGB, 720
//! high-resolution pixels wide from colour clock $38 (low-resolution pixel
//! $70) and, PAL, 286 lines from line 26 (NTSC: 241 from line 21).

use emu_core::frame::Rgb8;
use emu_core::{Savestate, StateError, StateReader, StateWriter};

use crate::blitter::Blitter;
use crate::chipram::ChipRam;
use crate::copper::Copper;
use crate::denise::{self, Denise};
use crate::paula::{set_clr, Paula};
use crate::{dma, int, reg};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standard {
    Pal,
    Ntsc,
}

impl Standard {
    fn lines(self, long: bool) -> u16 {
        match self {
            Standard::Pal => 312 + u16::from(long),
            Standard::Ntsc => 262 + u16::from(long),
        }
    }

    /// First displayed line (end of vertical blank).
    pub fn top(self) -> u16 {
        match self {
            Standard::Pal => 26,
            Standard::Ntsc => 21,
        }
    }

    /// Displayed lines.
    pub fn height(self) -> u32 {
        match self {
            Standard::Pal => 286,
            Standard::Ntsc => 241,
        }
    }
}

/// Picture width in high-resolution pixels.
pub const WIDTH: u32 = 720;
/// Colour clock of the first picture column.
pub const FIRST_CCK: u32 = 0x38;
/// Colour clocks per PAL line.
pub const PAL_LINE: u16 = 227;

/// What happened in one colour clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickOut {
    /// The slot was free and the waiting CPU got it.
    pub cpu_slot: bool,
    /// A line began (HSYNC, CIA-B TOD).
    pub hsync: bool,
    /// A field began (VSYNC, CIA-A TOD).
    pub vsync: bool,
    /// The last colour clock of a field.
    pub field_done: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Fixed,
    Plane(usize),
    /// Sprite n, first (POS / DATA) or second (CTL / DATB) slot.
    Sprite(usize, bool),
    Free,
}

/// Sprite DMA state of one sprite.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SprDma {
    /// Before the first sprite line of a field.
    #[default]
    Off,
    /// Fetch POS and CTL on this line.
    Control,
    /// Wait for VSTART.
    Wait,
    /// Fetch DATA and DATB every line until VSTOP.
    Data,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Custom {
    pub standard: Standard,
    /// Address bits Agnus' pointers keep (512 KB: $7FFFE).
    pub ptr_mask: u32,
    pub dmacon: u16,
    pub v: u16,
    pub h: u16,
    pub lof: bool,
    /// NTSC: this line is a long one (228 colour clocks).
    pub lol: bool,
    /// Fields since power-on.
    pub fields: u64,
    pub diwstrt: u16,
    pub diwstop: u16,
    pub ddfstrt: u16,
    pub ddfstop: u16,
    pub bplpt: [u32; 6],
    pub bpl1mod: u16,
    pub bpl2mod: u16,
    pub sprpt: [u32; 8],
    pub clxcon: u16,
    /// JOY0DAT/JOY1DAT, supplied by the machine's game ports.
    pub joydat: [u16; 2],
    /// POT pins pulled low by the game ports (POTGOR data bit positions).
    pub pot_low: u16,
    pub copper: Copper,
    pub blitter: Blitter,
    pub denise: Denise,
    pub paula: Paula,
    /// Bitplane DMA vertical window flip-flop.
    bpl_vertical: bool,
    /// Planes were fetched on this line (modulos are due).
    fetched: bool,
    /// Consecutive slots the Blitter took while the CPU waited.
    blit_streak: u8,
    spr_dma: [SprDma; 8],
    frame: Vec<Rgb8>,
    lut: Vec<Rgb8>,
}

impl Custom {
    pub fn new(standard: Standard, ptr_mask: u32) -> Self {
        Self {
            standard,
            ptr_mask,
            dmacon: 0,
            v: 0,
            h: 0,
            lof: true,
            lol: false,
            fields: 0,
            diwstrt: 0,
            diwstop: 0,
            ddfstrt: 0,
            ddfstop: 0,
            bplpt: [0; 6],
            bpl1mod: 0,
            bpl2mod: 0,
            sprpt: [0; 8],
            clxcon: 0,
            joydat: [0; 2],
            pot_low: 0,
            copper: Copper::new(),
            blitter: Blitter::new(),
            denise: Denise::new(),
            paula: Paula::new(),
            bpl_vertical: false,
            fetched: false,
            blit_streak: 0,
            spr_dma: [SprDma::Off; 8],
            frame: vec![Rgb8::default(); (WIDTH * standard.height()) as usize],
            lut: (0..4096u16).map(denise::rgb12).collect(),
        }
    }

    /// Power-on / RESET: registers to their reset values; the beam keeps
    /// running.
    pub fn reset(&mut self) {
        let mut n = Self::new(self.standard, self.ptr_mask);
        n.v = self.v;
        n.h = self.h;
        n.lof = self.lof;
        n.lol = self.lol;
        n.fields = self.fields;
        n.frame = std::mem::take(&mut self.frame);
        *self = n;
    }

    pub fn line_len(&self) -> u16 {
        match self.standard {
            Standard::Pal => PAL_LINE,
            Standard::Ntsc => PAL_LINE + u16::from(self.lol),
        }
    }

    pub fn field_lines(&self) -> u16 {
        self.standard.lines(self.lof)
    }

    pub fn frame(&self) -> &[Rgb8] {
        &self.frame
    }

    pub fn interlaced(&self) -> bool {
        self.denise.bplcon0 & 0x0004 != 0
    }

    fn dma_on(&self, bit: u16) -> bool {
        self.dmacon & dma::DMAEN != 0 && self.dmacon & bit != 0
    }

    fn slot(&self, h: u16) -> Slot {
        if h & 1 == 1 && h <= 7 {
            return Slot::Fixed;
        }
        if let Some(p) = self.plane_at(h) {
            return Slot::Plane(p);
        }
        if (0x17..=0x35).contains(&h) && h & 1 == 1 && self.dma_on(dma::SPREN) {
            let n = usize::from((h - 0x17) / 4);
            if matches!(self.spr_dma[n], SprDma::Control | SprDma::Data) {
                return Slot::Sprite(n, (h - 0x17).is_multiple_of(4));
            }
        }
        Slot::Free
    }

    /// (first slot, first slot after) of the bitplane fetch window.
    fn fetch_window(&self) -> (u16, u16) {
        let start = (self.ddfstrt & 0xfc).max(0x18);
        let stop = (self.ddfstop & 0xfc).min(0xd8);
        if stop < start {
            return (start, start);
        }
        (start, start + ((stop - start) / 8 + 1) * 8)
    }

    fn plane_at(&self, h: u16) -> Option<usize> {
        // No bitplane DMA during vertical blank, even with the window open.
        if !self.bpl_vertical || self.v < self.standard.top() - 1 || !self.dma_on(dma::BPLEN) {
            return None;
        }
        let planes = self.denise.planes();
        if planes == 0 {
            return None;
        }
        let (start, end) = self.fetch_window();
        if h < start || h >= end {
            return None;
        }
        const LORES: [usize; 8] = [0, 4, 6, 2, 0, 3, 5, 1];
        const HIRES: [usize; 8] = [4, 2, 3, 1, 4, 2, 3, 1];
        let off = usize::from((h - start) & 7);
        let p = if self.denise.hires() {
            HIRES[off]
        } else {
            LORES[off]
        };
        (p != 0 && p <= planes).then(|| p - 1)
    }

    /// Denise line colour clock of Agnus clock `h`.
    fn denise_h(&self, h: u16) -> u32 {
        if h >= denise::LINE_START {
            u32::from(h)
        } else {
            u32::from(h) + u32::from(PAL_LINE)
        }
    }

    /// Run one colour clock. `cpu_wants`: the CPU waits for a chip bus
    /// slot; [`TickOut::cpu_slot`] says whether it got this one.
    pub fn tick(&mut self, chip: &mut ChipRam, cpu_wants: bool) -> TickOut {
        let (v, h) = (self.v, self.h);
        let mut out = TickOut::default();
        if h == 0 {
            out.hsync = true;
            if v == 0 {
                out.vsync = true;
                self.paula.request(int::VERTB);
                self.copper.jump(false);
                self.bpl_vertical = false;
            }
            let vstart = self.diwstrt >> 8;
            let vstop = (self.diwstop >> 8) | if self.diwstop & 0x8000 == 0 { 0x100 } else { 0 };
            if v == vstart {
                self.bpl_vertical = true;
            }
            if v == vstop {
                self.bpl_vertical = false;
            }
            self.fetched = false;
            self.sprite_lines(v);
        }
        if h == denise::LINE_START {
            self.denise.start_line();
        }
        self.blitter.tick();
        self.copper.check_wait(v, h, self.blitter.busy);

        let mut blitter_slot = false;
        match self.slot(h) {
            Slot::Fixed => {}
            Slot::Sprite(n, first) => {
                let w = chip.word(self.sprpt[n]);
                self.sprpt[n] = self.sprpt[n].wrapping_add(2) & self.ptr_mask;
                let control = self.spr_dma[n] == SprDma::Control;
                let reg = usize::from(!first) + if control { 0 } else { 2 };
                self.denise.write_sprite(n, reg, w);
                if control && !first {
                    self.spr_dma[n] = SprDma::Wait;
                }
            }
            Slot::Plane(p) => {
                let w = chip.word(self.bplpt[p]);
                self.bplpt[p] = self.bplpt[p].wrapping_add(2) & self.ptr_mask;
                let dh = self.denise_h(h);
                self.denise.write_bpldat(p, w, dh);
                self.fetched = true;
            }
            Slot::Free => {
                if h & 1 == 0 && self.copper.wants_slot() && self.dma_on(dma::COPEN) {
                    if let Some((r, val)) = self.copper.cycle(chip, v, h, self.blitter.busy) {
                        self.write(r, val);
                    }
                } else if self.blitter.wants_slot() && self.dma_on(dma::BLTEN) {
                    if cpu_wants && self.dmacon & dma::BLTPRI == 0 && self.blit_streak >= 3 {
                        out.cpu_slot = true;
                    } else {
                        blitter_slot = true;
                        if self.blitter.cycle(chip) {
                            self.paula.request(int::BLIT);
                        }
                    }
                } else if cpu_wants {
                    out.cpu_slot = true;
                }
            }
        }
        self.blit_streak = if blitter_slot && cpu_wants {
            self.blit_streak + 1
        } else {
            0
        };
        if self.fetched && h == self.fetch_window().1 {
            self.add_modulos();
            self.fetched = false;
        }
        self.render(v, h);

        self.h += 1;
        if self.h >= self.line_len() {
            self.h = 0;
            if self.standard == Standard::Ntsc {
                self.lol = !self.lol;
            }
            self.v += 1;
            if self.v >= self.field_lines() {
                self.v = 0;
                self.fields += 1;
                out.field_done = true;
                if self.interlaced() {
                    self.lof = !self.lof;
                }
            }
        }
        out
    }

    /// Sprite DMA at the start of line `v`: control words on the first
    /// sprite line (end of vertical blank), data from VSTART, new control
    /// words at VSTOP.
    fn sprite_lines(&mut self, v: u16) {
        if v == 0 {
            self.spr_dma = [SprDma::Off; 8];
        }
        if v == self.standard.top() - 1 {
            self.spr_dma = [SprDma::Control; 8];
            return;
        }
        for (n, st) in self.spr_dma.iter_mut().enumerate() {
            let s = &self.denise.sprites[n];
            match *st {
                SprDma::Wait if v == s.vstart() => *st = SprDma::Data,
                SprDma::Data if v == s.vstop() => *st = SprDma::Control,
                _ => {}
            }
        }
    }

    fn add_modulos(&mut self) {
        for p in 0..self.denise.planes() {
            let m = if p & 1 == 0 {
                self.bpl1mod
            } else {
                self.bpl2mod
            };
            self.bplpt[p] = self.bplpt[p].wrapping_add(m as i16 as i32 as u32) & self.ptr_mask;
        }
    }

    fn render(&mut self, v: u16, h: u16) {
        let dh = self.denise_h(h);
        let mut px = [0u16; 4];
        self.denise.output(dh, &mut px);
        let row = if h >= denise::LINE_START {
            i32::from(v)
        } else {
            i32::from(v) - 1
        };
        let y = row - i32::from(self.standard.top());
        if y < 0 || y as u32 >= self.standard.height() {
            return;
        }
        let Some(x) = (4 * dh).checked_sub(4 * FIRST_CCK) else {
            return;
        };
        if x + 4 > WIDTH {
            return;
        }
        let base = (y as u32 * WIDTH + x) as usize;
        for (i, c) in px.iter().enumerate() {
            self.frame[base + i] = self.lut[usize::from(*c & 0xfff)];
        }
    }

    fn set_ptr(pt: &mut u32, high: bool, v: u16, mask: u32) {
        *pt = if high {
            (*pt & 0xffff) | u32::from(v) << 16
        } else {
            (*pt & 0xffff_0000) | u32::from(v & 0xfffe)
        } & mask;
    }

    /// A register write by the CPU (word) or the Copper.
    pub fn write(&mut self, r: u16, v: u16) {
        let r = r & 0x1fe;
        let mask = self.ptr_mask;
        match r {
            reg::DSKPTH | reg::DSKPTL => {
                Self::set_ptr(&mut self.paula.dskpt, r == reg::DSKPTH, v, mask)
            }
            reg::DSKLEN => self.paula.dsklen = v,
            reg::VPOSW => {
                self.lof = v & 0x8000 != 0;
                self.v = (self.v & 0xff) | (v & 1) << 8;
            }
            reg::VHPOSW => {
                self.v = (self.v & 0x100) | v >> 8;
                self.h = (v & 0xff).min(self.line_len() - 1);
            }
            reg::COPCON => self.copper.danger = v & 2 != 0,
            reg::SERDAT => self.paula.request(int::TBE),
            reg::SERPER => self.paula.serper = v,
            reg::POTGO => self.paula.potgo = v,
            reg::JOYTEST => self.joydat = [v & 0xfcfc; 2],
            reg::BLTCON0 => self.blitter.con0 = v,
            reg::BLTCON1 => self.blitter.con1 = v,
            reg::BLTAFWM => self.blitter.afwm = v,
            reg::BLTALWM => self.blitter.alwm = v,
            reg::BLTCPTH..=reg::BLTDPTL => {
                // C, B, A, D pointers in that order.
                let i = usize::from((r - reg::BLTCPTH) / 4);
                let ch = [2, 1, 0, 3][i];
                Self::set_ptr(&mut self.blitter.pt[ch], r & 2 == 0, v, mask);
            }
            reg::BLTSIZE => self.blitter.start(v),
            reg::BLTCMOD..=reg::BLTDMOD => {
                let ch = [2, 1, 0, 3][usize::from((r - reg::BLTCMOD) / 2)];
                self.blitter.modulo[ch] = v & 0xfffe;
            }
            reg::BLTCDAT => self.blitter.cdat = v,
            reg::BLTBDAT => self.blitter.bdat = v,
            reg::BLTADAT => self.blitter.adat = v,
            reg::DSKSYNC => self.paula.dsksync = v,
            reg::COP1LCH | reg::COP1LCL => {
                Self::set_ptr(&mut self.copper.cop1lc, r == reg::COP1LCH, v, mask)
            }
            reg::COP2LCH | reg::COP2LCL => {
                Self::set_ptr(&mut self.copper.cop2lc, r == reg::COP2LCH, v, mask)
            }
            reg::COPJMP1 => self.copper.jump(false),
            reg::COPJMP2 => self.copper.jump(true),
            reg::DIWSTRT => {
                self.diwstrt = v;
                self.denise.diwstrt = v;
            }
            reg::DIWSTOP => {
                self.diwstop = v;
                self.denise.diwstop = v;
            }
            reg::DDFSTRT => self.ddfstrt = v & 0xfc,
            reg::DDFSTOP => self.ddfstop = v & 0xfc,
            reg::DMACON => set_clr(&mut self.dmacon, v & 0x87ff),
            reg::CLXCON => self.clxcon = v,
            reg::INTENA => self.paula.write_intena(v),
            reg::INTREQ => self.paula.write_intreq(v),
            reg::ADKCON => self.paula.write_adkcon(v),
            reg::BPL1PTH..=reg::BPL6PTL => {
                let p = usize::from((r - reg::BPL1PTH) / 4);
                Self::set_ptr(&mut self.bplpt[p], r & 2 == 0, v, mask);
            }
            reg::BPLCON0 => self.denise.bplcon0 = v,
            reg::BPLCON1 => self.denise.bplcon1 = v,
            reg::BPLCON2 => self.denise.bplcon2 = v,
            reg::BPL1MOD => self.bpl1mod = v & 0xfffe,
            reg::BPL2MOD => self.bpl2mod = v & 0xfffe,
            reg::BPL1DAT..=reg::BPL6DAT => {
                let dh = self.denise_h(self.h);
                self.denise
                    .write_bpldat(usize::from((r - reg::BPL1DAT) / 2), v, dh);
            }
            0x120..=0x13e => {
                let s = usize::from((r - reg::SPR0PTH) / 4);
                Self::set_ptr(&mut self.sprpt[s], r & 2 == 0, v, mask);
            }
            0x140..=0x17e => {
                let off = usize::from(r - 0x140);
                self.denise.write_sprite(off / 8, (off % 8) / 2, v);
            }
            reg::COLOR00..=reg::COLOR31 => {
                self.denise.colors[usize::from((r - reg::COLOR00) / 2)] = v & 0xfff;
            }
            _ => {}
        }
    }

    /// A register read by the CPU.
    pub fn read(&mut self, r: u16) -> u16 {
        self.peek(r)
    }

    /// Side-effect-free register read; write-only registers read $FFFF.
    pub fn peek(&self, r: u16) -> u16 {
        match r & 0x1fe {
            reg::DMACONR => {
                (self.dmacon & 0x07ff)
                    | if self.blitter.busy { dma::BBUSY } else { 0 }
                    | if self.blitter.zero { dma::BZERO } else { 0 }
            }
            reg::VPOSR => u16::from(self.lof) << 15 | self.v >> 8,
            reg::VHPOSR => (self.v & 0xff) << 8 | self.h,
            reg::DSKDATR => 0,
            reg::JOY0DAT => self.joydat[0],
            reg::JOY1DAT => self.joydat[1],
            reg::CLXDAT => 0x8000,
            reg::ADKCONR => self.paula.adkcon,
            reg::POT0DAT | reg::POT1DAT => 0,
            reg::POTGOR => self.paula.potgor(self.pot_low),
            // TBE and TSRE: the transmitter is idle.
            reg::SERDATR => 0x3000,
            reg::DSKBYTR => 0,
            reg::INTENAR => self.paula.intena,
            reg::INTREQR => self.paula.requests(),
            _ => 0xffff,
        }
    }

    /// The interrupt level Paula drives on IPL0–2.
    pub fn ipl(&self) -> u8 {
        self.paula.ipl()
    }
}

const TAG: [u8; 4] = *b"OCS ";

impl Savestate for Custom {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, 1, |w| {
            w.u8(match self.standard {
                Standard::Pal => 0,
                Standard::Ntsc => 1,
            });
            for v in [
                self.dmacon,
                self.v,
                self.h,
                self.diwstrt,
                self.diwstop,
                self.ddfstrt,
                self.ddfstop,
                self.bpl1mod,
                self.bpl2mod,
                self.clxcon,
                self.pot_low,
            ] {
                w.u16(v);
            }
            w.u16(self.joydat[0]);
            w.u16(self.joydat[1]);
            w.bool(self.lof);
            w.bool(self.lol);
            w.u64(self.fields);
            for p in self.bplpt.iter().chain(&self.sprpt) {
                w.u32(*p);
            }
            w.bool(self.bpl_vertical);
            w.bool(self.fetched);
            w.u8(self.blit_streak);
            for s in self.spr_dma {
                w.u8(s as u8);
            }
            self.copper.save(w);
            self.blitter.save(w);
            self.denise.save(w);
            self.paula.save(w);
        });
    }

    /// Atomic; the standard and pointer width are configuration.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, 1)?;
        let standard = match c.u8()? {
            0 => Standard::Pal,
            1 => Standard::Ntsc,
            _ => return Err(StateError::Invalid("video standard")),
        };
        if standard != self.standard {
            return Err(StateError::Invalid("video standard differs"));
        }
        let mut n = Custom::new(self.standard, self.ptr_mask);
        for v in [
            &mut n.dmacon,
            &mut n.v,
            &mut n.h,
            &mut n.diwstrt,
            &mut n.diwstop,
            &mut n.ddfstrt,
            &mut n.ddfstop,
            &mut n.bpl1mod,
            &mut n.bpl2mod,
            &mut n.clxcon,
            &mut n.pot_low,
        ] {
            *v = c.u16()?;
        }
        n.joydat = [c.u16()?, c.u16()?];
        n.lof = c.bool()?;
        n.lol = c.bool()?;
        n.fields = c.u64()?;
        for p in n.bplpt.iter_mut().chain(n.sprpt.iter_mut()) {
            *p = c.u32()?;
        }
        n.bpl_vertical = c.bool()?;
        n.fetched = c.bool()?;
        n.blit_streak = c.u8()?;
        for s in &mut n.spr_dma {
            *s = match c.u8()? {
                0 => SprDma::Off,
                1 => SprDma::Control,
                2 => SprDma::Wait,
                3 => SprDma::Data,
                _ => return Err(StateError::Invalid("sprite DMA state")),
            };
        }
        n.copper = Copper::load(&mut c)?;
        n.blitter = crate::blitter::Blitter::load(&mut c)?;
        n.denise = Denise::load(&mut c)?;
        n.paula = Paula::load(&mut c)?;
        if n.h >= 228 || n.v >= n.field_lines() {
            return Err(StateError::Invalid("beam position"));
        }
        n.frame = std::mem::take(&mut self.frame);
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
