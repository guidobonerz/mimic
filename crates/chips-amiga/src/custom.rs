//! The custom chip set as one unit: Agnus' beam counters and DMA slot
//! allocation, the bitplane fetch, the Copper, the Blitter, Denise and
//! Paula, advanced one colour clock (one DMA slot) per [`Custom::tick`].
//!
//! **Beam.** PAL: 227 colour clocks per line, 313 lines in a long field and
//! 312 in a short one (non-interlaced fields stay long); NTSC: lines
//! alternate 228/227 clocks, 263/262 lines. LOF toggles every field with
//! BPLCON0 LACE. VPOSR: bit 15 LOF, bits 14–8 the Agnus id (0 on the OCS
//! 8367/8361; $20 PAL and $30 NTSC on the ECS 8372A/8375, HRM appendix C),
//! bit 7 LOL on the ECS Agnus, bits 2–0 vertical bits 10–8; VHPOSR:
//! vertical bits 7–0, horizontal position in colour clocks.
//!
//! **ECS Agnus** (8372A, 8375 with 2 MB chip RAM; [`Chipset`]): BEAMCON0
//! ($1DC) bit 5 PAL selects the PAL or NTSC beam (power-on value per the
//! chip's strap), bit 11 LOLDIS stops the NTSC long/short line toggle, bit
//! 7 VARBEAMEN makes the line HTOTAL + 1 colour clocks and the field
//! VTOTAL + 1 lines (plus one in the long field of an interlaced display);
//! HSSTOP … HCENTER ($1C2–$1E2) are kept for the debugger only (sync
//! shaping for monitors). DIWHIGH ($1E4) adds vertical window bits 10–8
//! (2–0 start, 10–8 stop) and, in Denise, horizontal bit 8 (5 start, 13
//! stop); a write to DIWSTRT or DIWSTOP restores the OCS defaults (start
//! bits 0, stop H8 = 1, V8 = the inverse of DIWSTOP bit 15). BLTSIZV/BLTSIZH
//! ($5C/$5E) start blits up to 2048 words by 32768 rows (see `blitter`).
//! With COPCON CDANG the Copper reaches every register, not just $40 up.
//! The 2 MB Agnus reports the same id as the 1 MB one (community
//! measurements, `docs/sources.md`).
//!
//! **AGA Alice** (`Chipset::aga`): id $22 PAL / $32 NTSC; BPL7PT/BPL8PT
//! ($F8–$FE) and BPL7DAT/BPL8DAT; FMODE ($1FC) bits 1–0 make a bitplane
//! fetch 32 or 64 bits wide (two or four words from consecutive
//! addresses) and bits 3–2 the sprite fetches. The fetch unit grows with
//! it: 8 colour clocks deliver 16 low-resolution pixels per plane in
//! FMODE 0, 16 clocks 32 pixels, 32 clocks 64; within a unit the first
//! eight slots fetch planes 8, 4, 6, 2, 7, 3, 5, 1 (the OCS order with
//! planes 7 and 8 in its empty slots), high resolution halves the unit
//! and super-hires halves it again (4-slot units fetch 4, 2, 3, 1;
//! 2-slot units 2, 1). The fetch window starts at DDFSTRT and runs in
//! whole units up to the one containing DDFSTOP — whether Alice aligns
//! DDFSTRT to the unit is not verified here. The modulo is added with a
//! plane's last fetch of the line as before.
//!
//! **DMA slots** (HRM chapter 7, "DMA time slot allocation"): odd slots
//! $01–$07 refresh, $09–$0D disk, $0F–$15 audio, $17–$35 sprites; bitplanes
//! in their fetch window; the Copper takes even free slots, the Blitter any
//! free slot, the CPU what is left — odd slots included. The HRM (chapter
//! 6, "Blitter DMA") says the 68000 uses only the even-numbered cycles; a
//! measurement on a cycle-exact A500 configuration (WinUAE, 2026-10-22,
//! `docs/sources.md`: bus-test boot block) shows a chip-RAM-bound loop
//! running at its full instruction time with bitplane DMA off, so a CPU
//! cycle whose second colour clock is odd is not delayed when that clock is
//! free. With even slots only it would lose a clock per 4n+2-cycle
//! instruction (10 % there). Without BLTPRI (BLITHOG) the Blitter leaves
//! a slot to the CPU once the CPU has been "unsatisfied for three
//! consecutive memory cycles" (HRM): colour clocks in which the CPU waits
//! for its data slot and the Blitter, the bitplane DMA or a Copper fetch
//! has the bus count; the CPU's request half-cycle ([`Custom::cpu_request`])
//! and the fixed channels' slots do not count and do not end the wait; a colour clock without a request starts over. Without
//! bitplanes a busy Blitter thus leaves the CPU every fourth of its bus
//! cycles' data slots (1 : 5 colour clocks). Measured with
//! `tools/amiga_bustest2.py` on a cycle-exact WinUAE A500 (2026-10-23,
//! `docs/sources.md`): blit time and the CPU's loop count with BLTPRI off
//! and on, with 5 lowres planes and a per-line Copper list, and for a
//! D-only clear. Sanity's "Interference" depends on it: its textured cube
//! fits its frame work into one field only with this share.
//!
//! **Bitplane fetch.** Inside the vertical display window and between
//! DDFSTRT and DDFSTOP, eight-slot groups fetch the planes in the order
//! (lowres) `- 4 6 2 - 3 5 1` and (hires) `4 2 3 1 4 2 3 1`; groups start
//! at DDFSTRT + 8n while ≤ DDFSTOP (hires: DDFSTOP + 4). Each plane's
//! modulo (BPL1MOD odd planes, BPL2MOD even planes) is added with its last
//! fetch of the line, in the last group (HRM chapter 3: the modulo is added
//! "at the end of the data fetch for each horizontal line"); a pointer
//! written after that (Desert Dream's cube: a Copper MOVE to BPL1PTL at
//! $E0, right after a window ending at $DF) is used unchanged on the next
//! line.
//!
//! **Picture.** [`Custom::frame`] is the field in 12-bit colours as RGB, 720
//! high-resolution pixels wide from colour clock $38 (low-resolution pixel
//! $70) and, PAL, 286 lines from line 26 (NTSC: 241 from line 21).

use emu_core::frame::Rgb8;
use emu_core::{Savestate, StateError, StateReader, StateWriter};

use crate::audio;
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

/// Which Agnus and Denise revisions are fitted: OCS (8371/8372 "Fat
/// Agnus" without the ECS registers, Denise 8362) or ECS (Agnus 8372A /
/// 8375, Denise 8373). The two are independent: many A500 rev 6A boards
/// have an ECS Agnus with an OCS Denise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Chipset {
    pub ecs_agnus: bool,
    pub ecs_denise: bool,
    /// Alice and Lisa (A1200/A4000); implies the ECS features.
    pub aga: bool,
}

impl Chipset {
    pub const OCS: Chipset = Chipset {
        ecs_agnus: false,
        ecs_denise: false,
        aga: false,
    };
    pub const ECS: Chipset = Chipset {
        ecs_agnus: true,
        ecs_denise: true,
        aga: false,
    };
    pub const AGA: Chipset = Chipset {
        ecs_agnus: true,
        ecs_denise: true,
        aga: true,
    };
}

/// BEAMCON0 bits (HRM appendix C).
pub mod beamcon0 {
    pub const HARDDIS: u16 = 0x4000;
    pub const LPENDIS: u16 = 0x2000;
    pub const VARVBEN: u16 = 0x1000;
    pub const LOLDIS: u16 = 0x0800;
    pub const CSCBEN: u16 = 0x0400;
    pub const VARVSYEN: u16 = 0x0200;
    pub const VARHSYEN: u16 = 0x0100;
    pub const VARBEAMEN: u16 = 0x0080;
    pub const DUAL: u16 = 0x0040;
    pub const PAL: u16 = 0x0020;
    pub const VARCSYEN: u16 = 0x0010;
    pub const BLANKEN: u16 = 0x0008;
}

/// The programmable beam registers $1C0–$1E2 ([`Custom::beam_regs`]).
pub const BEAM_REG_NAMES: [&str; 18] = [
    "HTOTAL", "HSSTOP", "HBSTRT", "HBSTOP", "VTOTAL", "VSSTOP", "VBSTRT", "VBSTOP", "SPRHSTRT",
    "SPRHSTOP", "BPLHSTRT", "BPLHSTOP", "HHPOSW", "HHPOSR", "BEAMCON0", "HSSTRT", "VSSTRT",
    "HCENTER",
];
const HTOTAL: usize = 0;
const VTOTAL: usize = 4;
const BEAMCON0: usize = 14;

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
/// Colour clocks Denise's pixel counter runs behind Agnus' horizontal
/// position (see [`Custom::denise_h`]).
const DENISE_LAG: u16 = 2;
/// Colour clocks per PAL line.
pub const PAL_LINE: u16 = 227;
/// Colour clocks from the vertical strobe to the VERTB request.
const VERTB_DELAY: u16 = 2;
/// DIWHIGH with DIWSTRT/DIWSTOP both 0: stop H8 set, stop V8 set.
const DIWHIGH_DEFAULT: u16 = 0x2100;
/// Smallest HTOTAL/VTOTAL the programmable beam accepts (a guest writing
/// zeros must not stall the line or field counters).
const MIN_HTOTAL: u16 = 0x40;
const MIN_VTOTAL: u16 = 0x40;
/// The light-pen latch is released at the end of vertical blanking.
const LPEN_RELEASE_LINE: u16 = 20;

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
    /// Audio output: left (channels 0 + 3) and right (1 + 2) levels.
    pub audio: [i32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Fixed,
    Plane(usize),
    Disk,
    Audio(usize),
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
    /// The beam's standard: the chip's strap, or on the ECS Agnus what
    /// BEAMCON0 PAL says.
    pub standard: Standard,
    /// The chip's PAL/NTSC strap (configuration).
    pub native: Standard,
    pub chipset: Chipset,
    /// Address bits Agnus' pointers keep (512 KB: $7FFFE).
    pub ptr_mask: u32,
    /// HTOTAL … HCENTER ($1C0–$1E2, [`BEAM_REG_NAMES`]); BEAMCON0 at 14.
    pub beam_regs: [u16; 18],
    /// DIWHIGH as Agnus keeps it (vertical bits; Denise has its own copy).
    pub diwhigh: u16,
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
    /// AGA: FMODE (fetch widths); 0 otherwise.
    pub fmode: u16,
    pub bplpt: [u32; 8],
    pub bpl1mod: u16,
    pub bpl2mod: u16,
    pub sprpt: [u32; 8],
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
    /// Colour clocks in a row the waiting CPU did not get the bus.
    cpu_starved: u8,
    /// The CPU asserted its bus request in the coming colour clock (it
    /// cannot take data yet; see [`Custom::cpu_request`]).
    cpu_pending: bool,
    spr_dma: [SprDma; 8],
    frame: Vec<Rgb8>,
    /// Light pen: the beam counters frozen by a trigger (or at the end of
    /// a field without one) while BPLCON0 LPEN is set, until line 20.
    lpen_latch: Option<(u16, u16)>,
    /// Where a light pen on game port 0 points (frame pixels) and the
    /// luma its photodiode needs; configuration from the port, not state.
    pen: Option<(u16, u16)>,
    pen_luma: u32,
    /// The debug overlays' record while one is switched on (`overlay`;
    /// not machine state, not saved).
    rec: Option<Box<overlay::OverlayRecord>>,
}

impl Custom {
    pub fn new(standard: Standard, ptr_mask: u32, chipset: Chipset) -> Self {
        let mut beam_regs = [0; 18];
        if chipset.ecs_agnus && standard == Standard::Pal {
            beam_regs[BEAMCON0] = beamcon0::PAL;
        }
        let mut denise = Denise::new();
        denise.ecs = chipset.ecs_denise;
        denise.set_aga(chipset.aga);
        Self {
            standard,
            native: standard,
            chipset,
            ptr_mask,
            beam_regs,
            diwhigh: DIWHIGH_DEFAULT,
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
            fmode: 0,
            bplpt: [0; 8],
            bpl1mod: 0,
            bpl2mod: 0,
            sprpt: [0; 8],
            joydat: [0; 2],
            pot_low: 0,
            copper: {
                let mut c = Copper::new();
                c.ecs = chipset.ecs_agnus;
                c
            },
            blitter: Blitter::new(),
            denise,
            paula: Paula::new(),
            bpl_vertical: false,
            fetched: false,
            cpu_starved: 0,
            cpu_pending: false,
            spr_dma: [SprDma::Off; 8],
            frame: vec![Rgb8::default(); (WIDTH * standard.height()) as usize],
            lpen_latch: None,
            pen: None,
            pen_luma: 0,
            rec: None,
        }
    }

    /// Power-on / RESET: registers to their reset values; the beam keeps
    /// running.
    pub fn reset(&mut self) {
        let mut n = Self::new(self.native, self.ptr_mask, self.chipset);
        n.paula.uart.loopback = self.paula.uart.loopback;
        n.v = self.v;
        n.h = self.h;
        n.lof = self.lof;
        n.lol = self.lol;
        n.fields = self.fields;
        n.frame = std::mem::take(&mut self.frame);
        n.frame
            .resize((WIDTH * n.standard.height()) as usize, Rgb8::default());
        n.rec = self.rec.take();
        n.denise.rec_sprites = self.denise.rec_sprites;
        n.pen = self.pen;
        n.pen_luma = self.pen_luma;
        *self = n;
    }

    /// A light pen on game port 0 points at frame pixel (`x`, `y`) of
    /// [`Custom::frame`] (`None`: no pen or off the picture); a pixel
    /// whose Rec. 601 luma reaches `luma` lights it. When the beam draws
    /// that pixel bright, the pen triggers Agnus' light-pen input (pin 6,
    /// LPENTG): with BPLCON0 LPEN (bit 3) set, VPOSR/VHPOSR freeze at the
    /// beam position of that colour clock until line 20; without a
    /// trigger they freeze at the end of the field (HRM, "Reading the light
    /// pen"; ECS BEAMCON0 LPENDIS makes reads show the running counters).
    /// The pen's response delay is not modelled.
    pub fn set_light_pen(&mut self, aim: Option<(u16, u16)>, luma: u32) {
        self.pen = aim;
        self.pen_luma = luma;
    }

    /// LPEN in BPLCON0 and the ECS LPENDIS: the latch shows in VPOSR/VHPOSR.
    fn lpen_shown(&self) -> Option<(u16, u16)> {
        let disabled = self.chipset.ecs_agnus && self.beamcon0() & beamcon0::LPENDIS != 0;
        self.lpen_latch
            .filter(|_| self.denise.bplcon0 & 0x0008 != 0 && !disabled)
    }

    /// The light-pen input triggered now.
    fn light_pen_strobe(&mut self) {
        if self.denise.bplcon0 & 0x0008 != 0 && self.lpen_latch.is_none() {
            self.lpen_latch = Some((self.v, self.h));
        }
    }

    pub fn beamcon0(&self) -> u16 {
        self.beam_regs[BEAMCON0]
    }

    /// The ECS Agnus' programmable beam is on.
    fn var_beam(&self) -> bool {
        self.chipset.ecs_agnus && self.beamcon0() & beamcon0::VARBEAMEN != 0
    }

    pub fn line_len(&self) -> u16 {
        if self.var_beam() {
            return (self.beam_regs[HTOTAL] & 0x1ff).max(MIN_HTOTAL) + 1;
        }
        match self.standard {
            Standard::Pal => PAL_LINE,
            Standard::Ntsc => PAL_LINE + u16::from(self.lol),
        }
    }

    pub fn field_lines(&self) -> u16 {
        if self.var_beam() {
            let long = self.interlaced() && self.lof;
            return (self.beam_regs[VTOTAL] & 0x7ff).max(MIN_VTOTAL) + 1 + u16::from(long);
        }
        self.standard.lines(self.lof)
    }

    /// The beam standard changed (BEAMCON0 PAL on the ECS Agnus): the
    /// picture takes the new height.
    fn set_standard(&mut self, standard: Standard) {
        if standard != self.standard {
            self.standard = standard;
            self.frame = vec![Rgb8::default(); (WIDTH * standard.height()) as usize];
        }
    }

    /// Vertical display window (start line, stop line) with DIWHIGH's bits.
    fn vertical_window(&self) -> (u16, u16) {
        let vstart = self.diwstrt >> 8 | (self.diwhigh & 7) << 8;
        let vstop = self.diwstop >> 8 | (self.diwhigh >> 8 & 7) << 8;
        (vstart, vstop)
    }

    /// The OCS meaning of DIWSTRT/DIWSTOP as DIWHIGH bits.
    fn diwhigh_default(diwstop: u16) -> u16 {
        0x2000 | if diwstop & 0x8000 == 0 { 0x100 } else { 0 }
    }

    pub fn frame(&self) -> &[Rgb8] {
        &self.frame
    }

    pub fn interlaced(&self) -> bool {
        self.denise.bplcon0 & 0x0004 != 0
    }

    /// The CPU starts a chip bus cycle: the next colour clock is the first
    /// half of it, in which the CPU already waits for the bus but cannot
    /// use it yet (its data comes in the second half at the earliest).
    pub fn cpu_request(&mut self) {
        self.cpu_pending = true;
    }

    fn dma_on(&self, bit: u16) -> bool {
        self.dmacon & dma::DMAEN != 0 && self.dmacon & bit != 0
    }

    #[inline(always)]
    fn slot(&self, h: u16) -> Slot {
        if h & 1 == 1 && h <= 7 {
            return Slot::Fixed;
        }
        if matches!(h, 0x09 | 0x0b | 0x0d)
            && self.dma_on(dma::DSKEN)
            && self.paula.disk.wants_slot()
        {
            return Slot::Disk;
        }
        if matches!(h, 0x0f | 0x11 | 0x13 | 0x15) {
            let n = usize::from((h - 0x0f) / 2);
            if self.dma_on(1 << n) && self.paula.audio[n].wants_slot() {
                return Slot::Audio(n);
            }
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

    /// Words per bitplane fetch (FMODE bits 1–0 on the AGA).
    fn fetch_words(&self) -> u32 {
        match (self.chipset.aga, self.fmode & 3) {
            (false, _) | (true, 0) => 1,
            (true, 3) => 4,
            _ => 2,
        }
    }

    /// Words per sprite fetch (FMODE bits 3–2 on the AGA).
    fn sprite_words(&self) -> u32 {
        match (self.chipset.aga, (self.fmode >> 2) & 3) {
            (false, _) | (true, 0) => 1,
            (true, 3) => 4,
            _ => 2,
        }
    }

    /// Colour clocks of one fetch unit (every plane fetched once): 8 in
    /// low resolution, 4 in high, 2 in super-hires, times the fetch width.
    #[inline(always)]
    fn fetch_unit(&self) -> u16 {
        let base = if self.denise.shres() {
            2
        } else if self.denise.hires() {
            4
        } else {
            8
        };
        base * self.fetch_words() as u16
    }

    /// (first slot, first slot after) of the bitplane fetch window.
    /// Groups start at DDFSTRT + 8n up to DDFSTOP; in high resolution the
    /// stop counts half a group later, so the usual hires DDFSTOP $D0 and
    /// $D4 both fetch 40 words (Workbench 1.3 uses $3C/$D0 for 640 pixels).
    /// With wider AGA fetches the window runs in whole fetch units: the
    /// unit in progress when DDFSTOP is reached completes and one more
    /// follows, so a DDFSTOP between units rounds up (BigTime's hires
    /// FMODE 3 picture: DDFSTRT $68, DDFSTOP $90 fetch four 16-clock
    /// units, 32 bytes a plane, as its modulo of 32 for interlace needs;
    /// the aligned values — lores FMODE 3 $38/$B8 for 320 pixels — are
    /// unchanged).
    fn fetch_window(&self) -> (u16, u16) {
        let start = (self.ddfstrt & 0xfc).max(0x18);
        let stop_raw = (self.ddfstop & 0xfc).min(0xd8);
        if self.fetch_words() > 1 {
            let unit = self.fetch_unit();
            if stop_raw < start {
                return (start, start);
            }
            let units = (stop_raw - start).div_ceil(unit) + 1;
            return (start, start + units * unit);
        }
        let stop = stop_raw
            + if self.denise.hires() || self.denise.shres() {
                4
            } else {
                0
            };
        if stop < start {
            return (start, start);
        }
        (start, start + ((stop - start) / 8 + 1) * 8)
    }

    #[inline(always)]
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
        // The OCS order (planes 7 and 8 take the slots the OCS leaves
        // empty); high resolution fetches every plane twice per 8 clocks,
        // super-hires (ECS Denise) four times — the two planes take the
        // bandwidth of four hires planes per the HRM; the order within the
        // unit is this project's assumption.
        const SLOTS8: [usize; 8] = [8, 4, 6, 2, 7, 3, 5, 1];
        const SLOTS4: [usize; 4] = [4, 2, 3, 1];
        const SLOTS2: [usize; 2] = [2, 1];
        let unit = self.fetch_unit();
        let off = usize::from((h - start) % unit);
        let p = match unit / self.fetch_words() as u16 {
            2 => SLOTS2.get(off).copied(),
            4 => SLOTS4.get(off).copied(),
            _ => SLOTS8.get(off).copied(),
        };
        p.filter(|&p| p <= planes).map(|p| p - 1)
    }

    /// Denise line colour clock of Agnus clock `h`.
    /// Denise's colour clock for Agnus' horizontal position `h`, in Denise
    /// line numbering (clocks before [`denise::LINE_START`] still belong to
    /// the previous line). Denise runs [`DENISE_LAG`] colour clocks behind
    /// Agnus: a register write in Agnus clock `h` shows from pixel 2h − 4
    /// (WinUAE screenshot of Interference's tunnel frame, `docs/sources.md`:
    /// the Copper's COLOR00 at $48 starts exactly at the window's pixel 148),
    /// while a word fetched in slot `h` still appears at pixel 2h + 3 (plus
    /// scroll and shifter), the DDFSTRT $38 / DIWSTRT $81 relation.
    fn denise_h(&self, h: u16) -> u32 {
        let hd = (h + self.line_len() - DENISE_LAG) % self.line_len();
        if hd >= denise::LINE_START {
            u32::from(hd)
        } else {
            u32::from(hd) + u32::from(PAL_LINE)
        }
    }

    /// Denise's clock of the fetch slot `h`, for the load time of its data.
    fn denise_slot(&self, h: u16) -> u32 {
        self.denise_h(h) + u32::from(DENISE_LAG)
    }

    /// Run one colour clock. `cpu_wants`: the CPU waits for a chip bus
    /// slot; [`TickOut::cpu_slot`] says whether it got this one.
    pub fn tick(&mut self, chip: &mut ChipRam, cpu_wants: bool) -> TickOut {
        // An overlay record or a light pen takes its own copy of the colour
        // clock, so the emulation without them runs the code it had before
        // they existed (their checks inline cost the A500 ~3 %, measured
        // 2026-10-25). The helpers called once per copy (`slot`,
        // `plane_at`, `fetch_unit`, `audio_tick`, `sprite_lines`,
        // `last_fetch_group`) are `inline(always)`: with two callers LLVM
        // kept them out of line, which cost more than the checks.
        if self.rec.is_some() || self.pen.is_some() {
            return self.tick_extras(chip, cpu_wants);
        }
        self.tick_with::<false>(chip, cpu_wants)
    }

    #[cold]
    #[inline(never)]
    fn tick_extras(&mut self, chip: &mut ChipRam, cpu_wants: bool) -> TickOut {
        self.tick_with::<true>(chip, cpu_wants)
    }

    /// [`Custom::tick`] with (`EXTRAS`) or without the overlay record and
    /// light pen.
    #[inline(always)]
    fn tick_with<const EXTRAS: bool>(&mut self, chip: &mut ChipRam, cpu_wants: bool) -> TickOut {
        let (v, h) = (self.v, self.h);
        let mut out = TickOut::default();
        if h == 0 {
            out.hsync = true;
            if v == 0 {
                out.vsync = true;
                self.copper.jump(false);
                self.bpl_vertical = false;
            }
            let (vstart, vstop) = self.vertical_window();
            if v == vstart {
                self.bpl_vertical = true;
            }
            if v == vstop {
                self.bpl_vertical = false;
            }
            self.fetched = false;
            self.sprite_lines(v);
        }
        if h == denise::LINE_START + DENISE_LAG {
            if EXTRAS && self.rec.is_some() {
                self.record_sprite_line(v);
            }
            self.denise.start_line();
        }
        if v == 0 && h == VERTB_DELAY {
            // VERTB appears in INTREQ two colour clocks after the vertical
            // strobe at 0/0 (logic-analyzer observation on an A500, see
            // `docs/sources.md`); code polling VHPOSR for line 0 sees it
            // before the interrupt is taken.
            self.paula.request(int::VERTB);
        }
        self.blitter.tick();
        self.copper.check_wait(v, h, self.blitter.busy);

        // The CPU takes any free slot (measured, see the module doc).
        let cpu_turn = cpu_wants;
        let cpu_unsatisfied = cpu_wants || std::mem::take(&mut self.cpu_pending);
        let mut blitter_took = false;
        let mut copper_took = false;
        let mut copper_held = false;
        let slot_kind = self.slot(h);
        match slot_kind {
            Slot::Fixed => {}
            Slot::Disk => {
                let ints = self.paula.disk.dma(chip, self.ptr_mask);
                self.paula.request(ints);
            }
            Slot::Audio(n) => {
                if self.paula.audio[n].fetch(chip, self.ptr_mask) {
                    self.paula.request(int::AUD0 << n);
                }
            }
            Slot::Sprite(n, first) => {
                let words = self.sprite_words();
                let w = self.fetch_wide(chip, self.sprpt[n], words);
                self.sprpt[n] = self.sprpt[n].wrapping_add(2 * words) & self.ptr_mask;
                let control = self.spr_dma[n] == SprDma::Control;
                let reg = usize::from(!first) + if control { 0 } else { 2 };
                self.denise.write_sprite_wide(n, reg, w);
                if control && !first {
                    self.spr_dma[n] = SprDma::Wait;
                }
            }
            Slot::Plane(p) => {
                let words = self.fetch_words();
                let w = self.fetch_wide(chip, self.bplpt[p], words);
                self.bplpt[p] = self.bplpt[p].wrapping_add(2 * words) & self.ptr_mask;
                if self.last_fetch_group(h) {
                    // The plane's last word of the line: its modulo is added
                    // with this fetch, so a Copper MOVE right after the fetch
                    // window (Desert Dream's cube writes BPL1PTL at $E0) is
                    // not shifted by it.
                    self.add_modulo(p);
                }
                let dh = self.denise_slot(h);
                self.denise.write_bpldat(p, w, dh);
                self.fetched = true;
            }
            Slot::Free => {
                if h & 1 == 0 && self.copper.busy_wake > 0 && self.dma_on(dma::COPEN) {
                    // The Copper's wake-up after a WAIT holds the bus.
                    self.copper.busy_wake -= 1;
                    copper_held = true;
                } else if h & 1 == 0 && self.copper.wants_slot() && self.dma_on(dma::COPEN) {
                    copper_took = true;
                    if let Some((r, val)) = self.copper.cycle(chip, v, h, self.blitter.busy) {
                        if EXTRAS && self.rec.is_some() {
                            self.record_copper_move(v, h, r, val);
                        }
                        self.write(r, val);
                    }
                } else if self.blitter.wants_slot() && self.dma_on(dma::BLTEN) {
                    if self.blitter.idle_now() {
                        // An idle blitter cycle passes; the bus stays free.
                        if self.blitter.cycle(chip) {
                            self.paula.request(int::BLIT);
                        }
                        out.cpu_slot = cpu_turn;
                    } else if cpu_turn && self.dmacon & dma::BLTPRI == 0 && self.cpu_starved >= 3 {
                        out.cpu_slot = true;
                    } else {
                        blitter_took = true;
                        if self.blitter.cycle(chip) {
                            self.paula.request(int::BLIT);
                        }
                    }
                } else if cpu_turn {
                    out.cpu_slot = true;
                }
            }
        }
        // BLITHOG off: colour clocks the Blitter, the bitplanes or a Copper
        // fetch take from a CPU waiting for its data slot count; the request
        // half-cycle and fixed slots neither count nor end the wait.
        self.cpu_starved = if out.cpu_slot || !cpu_unsatisfied {
            0
        } else if cpu_wants && (blitter_took || copper_took || matches!(slot_kind, Slot::Plane(_)))
        {
            self.cpu_starved.saturating_add(1)
        } else {
            self.cpu_starved
        };
        if EXTRAS && self.rec.is_some() {
            self.record_slot(
                v,
                h,
                slot_kind,
                copper_took || copper_held,
                blitter_took,
                out.cpu_slot,
            );
        }
        if self.fetched && h == self.fetch_window().1 {
            self.fetched = false;
        }
        self.render::<EXTRAS>(v, h);
        self.audio_tick();
        let a = &self.paula.audio;
        out.audio = [a[0].level() + a[3].level(), a[1].level() + a[2].level()];
        let ints = self.paula.uart.tick();
        self.paula.request(ints);

        self.h += 1;
        if self.h >= self.line_len() {
            let last_h = self.h - 1;
            self.h = 0;
            if self.standard == Standard::Ntsc
                && !(self.chipset.ecs_agnus && self.beamcon0() & beamcon0::LOLDIS != 0)
            {
                self.lol = !self.lol;
            } else if self.standard == Standard::Pal {
                self.lol = false;
            }
            self.v += 1;
            if self.v == LPEN_RELEASE_LINE {
                self.lpen_latch = None;
            }
            if self.v >= self.field_lines() {
                // No trigger in this field: the counters freeze here.
                if self.denise.bplcon0 & 0x0008 != 0 && self.lpen_latch.is_none() {
                    self.lpen_latch = Some((self.v - 1, last_h));
                }
                self.v = 0;
                self.fields += 1;
                out.field_done = true;
                if EXTRAS && self.rec.is_some() {
                    self.record_end_field();
                }
                if self.interlaced() {
                    self.lof = !self.lof;
                }
            }
        }
        out
    }

    /// The audio channels' colour clock: samples, block interrupts and the
    /// ADKCON attach modes (a channel's words set the next channel's
    /// volume or period; see `audio`).
    #[inline(always)]
    fn audio_tick(&mut self) {
        let adk = self.paula.adkcon;
        for n in 0..4 {
            let attach = u8::from(adk & 1 << n != 0) | u8::from(adk & 0x10 << n != 0) << 1;
            let t = self.paula.audio[n].tick(attach);
            if t.irq {
                self.paula.request(int::AUD0 << n);
            }
            if let (Some(m), Some(next)) = (t.modulate, (n < 3).then_some(n + 1)) {
                match m {
                    audio::Modulation::Volume(v) => self.paula.audio[next].vol = v,
                    audio::Modulation::Period(p) => self.paula.audio[next].per = p,
                }
            }
        }
    }

    /// Sprite DMA at the start of line `v`: control words on the first
    /// sprite line (end of vertical blank), data from VSTART, new control
    /// words at VSTOP. The VSTOP comparison wins over VSTART: a sprite
    /// whose VSTART equals its VSTOP (the usual way to park a sprite, e.g.
    /// $2900/$2900 in Sanity's "Interference") fetches new control words
    /// on that line and shows no data; before this rule it fetched data
    /// for the rest of the field, 16 slots a line the CPU never got.
    #[inline(always)]
    fn sprite_lines(&mut self, v: u16) {
        if v == 0 {
            self.spr_dma = [SprDma::Off; 8];
        }
        if v == self.standard.top() - 1 {
            self.spr_dma = [SprDma::Control; 8];
            return;
        }
        let aga = self.chipset.aga;
        for (n, st) in self.spr_dma.iter_mut().enumerate() {
            let s = &self.denise.sprites[n];
            let (vstart, vstop) = if aga {
                (s.vstart_aga(), s.vstop_aga())
            } else {
                (s.vstart(), s.vstop())
            };
            match *st {
                SprDma::Wait | SprDma::Data if v == vstop => *st = SprDma::Control,
                SprDma::Wait if v == vstart => *st = SprDma::Data,
                _ => {}
            }
        }
    }

    /// `words` words from `addr` on, left-aligned in 64 bits (an AGA fetch
    /// of 32 or 64 bits reads consecutive words).
    fn fetch_wide(&self, chip: &ChipRam, addr: u32, words: u32) -> u64 {
        let mut v = 0u64;
        for i in 0..words {
            v = v << 16 | u64::from(chip.word(addr.wrapping_add(2 * i) & self.ptr_mask));
        }
        v << (64 - 16 * words)
    }

    /// Is slot `h` in the last fetch unit of the line, where each plane
    /// fetches its last word (low resolution: the last 8 colour clocks of
    /// the window; high resolution fetches every plane twice per group, so
    /// the last 4; wider AGA fetches stretch the unit)?
    #[inline(always)]
    fn last_fetch_group(&self, h: u16) -> bool {
        let end = self.fetch_window().1;
        h + self.fetch_unit() >= end
    }

    /// Add the plane's modulo (BPL1MOD odd planes 1/3/5, BPL2MOD even
    /// planes 2/4/6) to its pointer, after its last fetch of the line.
    fn add_modulo(&mut self, p: usize) {
        let m = if p & 1 == 0 {
            self.bpl1mod
        } else {
            self.bpl2mod
        };
        self.bplpt[p] = self.bplpt[p].wrapping_add(m as i16 as i32 as u32) & self.ptr_mask;
    }

    fn render<const EXTRAS: bool>(&mut self, v: u16, h: u16) {
        let dh = self.denise_h(h);
        let mut px = [0u32; 4];
        self.denise.output(dh, &mut px);
        let row = if h >= denise::LINE_START + DENISE_LAG {
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
            self.frame[base + i] = denise::rgb24(*c);
        }
        if EXTRAS && self.pen.is_some() {
            self.pen_sees(base, x, y as u32);
        }
    }

    /// A light pen points into the picture: did the four pixels at frame
    /// index `base` (column `x`, row `y`) light it?
    #[cold]
    #[inline(never)]
    fn pen_sees(&mut self, base: usize, x: u32, y: u32) {
        if let Some((px, py)) = self.pen {
            if u32::from(py) == y && (x..x + 4).contains(&u32::from(px)) {
                let c = self.frame[base + (u32::from(px) - x) as usize];
                let luma =
                    (299 * u32::from(c.r) + 587 * u32::from(c.g) + 114 * u32::from(c.b)) / 1000;
                if luma >= self.pen_luma {
                    self.light_pen_strobe();
                }
            }
        }
    }

    // The overlay record's hooks: out of line, so the colour clock without
    // a record keeps its old code (one null test each).

    /// Denise finished raster line `v` - 1: the sprites it showed.
    #[cold]
    #[inline(never)]
    fn record_sprite_line(&mut self, v: u16) {
        if let Some(rec) = self.rec.as_deref_mut() {
            let seen = std::mem::replace(&mut self.denise.spr_seen, [overlay::UNSEEN; 8]);
            if rec.sprites && v > 0 {
                rec.sprite_line(v - 1, &seen);
            }
        }
    }

    #[cold]
    #[inline(never)]
    fn record_copper_move(&mut self, v: u16, h: u16, r: u16, val: u16) {
        if let Some(rec) = self.rec.as_deref_mut() {
            rec.copper_move(v, h, r, val);
        }
    }

    /// Who used colour clock (`v`, `h`).
    #[cold]
    #[inline(never)]
    fn record_slot(
        &mut self,
        v: u16,
        h: u16,
        slot_kind: Slot,
        copper: bool,
        blitter: bool,
        cpu: bool,
    ) {
        if let Some(rec) = self.rec.as_deref_mut() {
            if rec.dma {
                use overlay::slot;
                let code = match slot_kind {
                    Slot::Fixed => slot::REFRESH,
                    Slot::Disk => slot::DISK,
                    Slot::Audio(_) => slot::AUDIO,
                    Slot::Sprite(..) => slot::SPRITE,
                    Slot::Plane(_) => slot::BITPLANE,
                    Slot::Free if copper => slot::COPPER,
                    Slot::Free if blitter => slot::BLITTER,
                    Slot::Free if cpu => slot::CPU,
                    Slot::Free => slot::FREE,
                };
                rec.slot(v, h, code);
            }
        }
    }

    #[cold]
    #[inline(never)]
    fn record_end_field(&mut self) {
        if let Some(rec) = self.rec.as_deref_mut() {
            rec.end_field();
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
                Self::set_ptr(&mut self.paula.disk.pt, r == reg::DSKPTH, v, mask)
            }
            reg::DSKLEN => {
                self.paula.dsklen = v;
                let ints = self.paula.disk.write_dsklen(v);
                self.paula.request(ints);
            }
            reg::VPOSW => {
                self.lof = v & 0x8000 != 0;
                let high = if self.chipset.ecs_agnus { 7 } else { 1 };
                self.v = (self.v & 0xff) | (v & high) << 8;
                if self.chipset.ecs_agnus && self.standard == Standard::Ntsc {
                    self.lol = v & 0x80 != 0;
                }
            }
            reg::VHPOSW => {
                self.v = (self.v & 0x700) | v >> 8;
                self.h = (v & 0xff).min(self.line_len() - 1);
            }
            reg::COPCON => self.copper.danger = v & 2 != 0,
            reg::SERDAT => {
                let ints = self.paula.uart.write_serdat(v);
                self.paula.request(ints);
            }
            reg::SERPER => self.paula.uart.serper = v,
            reg::POTGO => self.paula.potgo = v,
            // The counters' upper six bits of both ports.
            reg::JOYTEST => {
                for j in &mut self.joydat {
                    *j = (*j & 0x0303) | (v & 0xfcfc);
                }
            }
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
            reg::BLTSIZV if self.chipset.ecs_agnus => self.blitter.sizv = v & 0x7fff,
            reg::BLTSIZH if self.chipset.ecs_agnus => self.blitter.start_ecs(v),
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
            reg::DIWSTRT | reg::DIWSTOP => {
                if r == reg::DIWSTRT {
                    self.diwstrt = v;
                    self.denise.diwstrt = v;
                } else {
                    self.diwstop = v;
                    self.denise.diwstop = v;
                }
                self.diwhigh = Self::diwhigh_default(self.diwstop);
                self.denise.diwhigh = self.diwhigh;
            }
            reg::DIWHIGH => {
                if self.chipset.ecs_agnus {
                    self.diwhigh = v;
                }
                if self.chipset.ecs_denise {
                    self.denise.diwhigh = v;
                }
            }
            reg::HTOTAL..=reg::HCENTER if self.chipset.ecs_agnus => {
                let i = usize::from((r - reg::HTOTAL) / 2);
                self.beam_regs[i] = v;
                if i == BEAMCON0 {
                    let standard = if v & beamcon0::PAL != 0 {
                        Standard::Pal
                    } else {
                        Standard::Ntsc
                    };
                    self.set_standard(standard);
                }
            }
            reg::DDFSTRT => self.ddfstrt = v & 0xfc,
            reg::DDFSTOP => self.ddfstop = v & 0xfc,
            reg::DMACON => {
                set_clr(&mut self.dmacon, v & 0x87ff);
                for n in 0..4 {
                    let on = self.dma_on(1 << n);
                    self.paula.audio[n].set_dma(on);
                }
            }
            reg::AUD0LCH..=reg::AUD3DAT => {
                let n = usize::from((r - reg::AUD0LCH) / 16);
                let c = &mut self.paula.audio[n];
                match (r - reg::AUD0LCH) % 16 {
                    0 => c.lc = (c.lc & 0xffff) | u32::from(v) << 16 & mask,
                    2 => c.lc = (c.lc & 0xffff_0000) | u32::from(v & 0xfffe),
                    4 => c.len = v,
                    6 => c.per = v,
                    8 => c.vol = (v & 0x7f).min(64) as u8,
                    10 => c.write_dat(v),
                    _ => {}
                }
            }
            reg::CLXCON => self.denise.clxcon = v,
            reg::INTENA => self.paula.write_intena(v),
            reg::INTREQ => self.paula.write_intreq(v),
            reg::ADKCON => self.paula.write_adkcon(v),
            reg::BPL1PTH..=reg::BPL6PTL => {
                let p = usize::from((r - reg::BPL1PTH) / 4);
                Self::set_ptr(&mut self.bplpt[p], r & 2 == 0, v, mask);
            }
            reg::BPL7PTH..=reg::BPL8PTL if self.chipset.aga => {
                let p = usize::from((r - reg::BPL1PTH) / 4);
                Self::set_ptr(&mut self.bplpt[p], r & 2 == 0, v, mask);
            }
            reg::BPLCON4 if self.chipset.aga => self.denise.bplcon4 = v,
            reg::CLXCON2 if self.chipset.aga => self.denise.clxcon2 = v,
            reg::FMODE if self.chipset.aga => {
                self.fmode = v;
                self.denise.fmode = v;
            }
            reg::BPL7DAT | reg::BPL8DAT if self.chipset.aga => {
                let dh = self.denise_slot(self.h);
                self.denise.write_bpldat(
                    usize::from((r - reg::BPL1DAT) / 2),
                    u64::from(v) << 48,
                    dh,
                );
            }
            reg::BPLCON0 => self.denise.bplcon0 = v,
            reg::BPLCON1 => self.denise.bplcon1 = v,
            reg::BPLCON2 => self.denise.bplcon2 = v,
            reg::BPLCON3 if self.chipset.ecs_denise => self.denise.bplcon3 = v,
            reg::BPL1MOD => self.bpl1mod = v & 0xfffe,
            reg::BPL2MOD => self.bpl2mod = v & 0xfffe,
            reg::BPL1DAT..=reg::BPL6DAT => {
                let dh = self.denise_slot(self.h);
                self.denise.write_bpldat(
                    usize::from((r - reg::BPL1DAT) / 2),
                    u64::from(v) << 48,
                    dh,
                );
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
                self.denise
                    .write_color(usize::from((r - reg::COLOR00) / 2), v);
            }
            _ => {}
        }
    }

    /// A register read by the CPU: reading CLXDAT clears it, reading
    /// DSKBYTR its DSKBYT flag.
    pub fn read(&mut self, r: u16) -> u16 {
        match r & 0x1fe {
            reg::CLXDAT => {
                let v = self.peek(r);
                self.denise.clxdat = 0;
                v
            }
            reg::DSKBYTR => self.paula.disk.read_dskbytr(),
            _ => self.peek(r),
        }
    }

    /// One disk bit cell, read side: the bit the selected drives deliver.
    pub fn disk_bit_in(&mut self, bit: bool) {
        let p = &mut self.paula;
        let ints = p.disk.bit_in(bit, p.dsksync, p.adkcon);
        p.request(ints);
    }

    /// One disk bit cell, write side: the bit to write, if writing.
    pub fn disk_bit_out(&mut self) -> Option<bool> {
        let (bit, ints) = self.paula.disk.bit_out();
        self.paula.request(ints);
        bit
    }

    /// Side-effect-free register read; write-only registers read $FFFF.
    pub fn peek(&self, r: u16) -> u16 {
        match r & 0x1fe {
            reg::DMACONR => {
                (self.dmacon & 0x07ff)
                    | if self.blitter.busy { dma::BBUSY } else { 0 }
                    | if self.blitter.zero { dma::BZERO } else { 0 }
            }
            reg::VPOSR => {
                let id = match (self.chipset.ecs_agnus, self.chipset.aga, self.native) {
                    (false, _, _) => 0,
                    (true, false, Standard::Pal) => 0x20,
                    (true, false, Standard::Ntsc) => 0x30,
                    (_, true, Standard::Pal) => 0x22,
                    (_, true, Standard::Ntsc) => 0x32,
                };
                let lol = u16::from(self.chipset.ecs_agnus && self.lol) << 7;
                let v = self.lpen_shown().map_or(self.v, |(v, _)| v);
                u16::from(self.lof) << 15 | id << 8 | lol | (v >> 8 & 7)
            }
            reg::VHPOSR => {
                let (v, h) = self.lpen_shown().unwrap_or((self.v, self.h));
                (v & 0xff) << 8 | (h & 0xff)
            }
            reg::DENISEID => {
                if self.chipset.aga {
                    0xfff8
                } else if self.chipset.ecs_denise {
                    0xfffc
                } else {
                    0xffff
                }
            }
            reg::DSKDATR => 0,
            reg::JOY0DAT => self.joydat[0],
            reg::JOY1DAT => self.joydat[1],
            reg::CLXDAT => 0x8000 | self.denise.clxdat,
            reg::ADKCONR => self.paula.adkcon,
            reg::POT0DAT | reg::POT1DAT => 0,
            reg::POTGOR => self.paula.potgor(self.pot_low),
            reg::SERDATR => self.paula.uart.serdatr(),
            reg::DSKBYTR => self.paula.disk.dskbytr(),
            reg::INTENAR => self.paula.intena,
            reg::INTREQR => self.paula.requests(),
            _ => 0xffff,
        }
    }

    /// Denise's game port counters (JOY0DAT/JOY1DAT): the two-bit phases
    /// `x`, `y` the port's lines show now. The low bits follow the lines;
    /// a step 3 → 0 counts the upper six bits up, 0 → 3 down; a jump by two
    /// (a missed phase) does not count (HRM chapter 8, "Reading the mouse").
    pub fn port_phases(&mut self, port: usize, x: u8, y: u8) {
        let mut v = self.joydat[port];
        for (shift, phase) in [(0, x), (8, y)] {
            let old = (v >> shift) as u8;
            let new_lo = phase & 3;
            let mut hi = old >> 2;
            match new_lo.wrapping_sub(old & 3) & 3 {
                1 if old & 3 == 3 => hi = hi.wrapping_add(1),
                3 if old & 3 == 0 => hi = hi.wrapping_sub(1),
                _ => {}
            }
            let byte = (hi & 0x3f) << 2 | new_lo;
            v = (v & !(0xff << shift)) | u16::from(byte) << shift;
        }
        self.joydat[port] = v;
    }

    /// The interrupt level Paula drives on IPL0–2.
    pub fn ipl(&self) -> u8 {
        self.paula.ipl()
    }
}

const TAG: [u8; 4] = *b"OCS ";

impl Savestate for Custom {
    fn save(&self, w: &mut StateWriter) {
        // Version 3: two pending shifter loads per plane group in Denise;
        // version 4: four; version 5: the Copper's wake-up after WAIT;
        // version 6: the serial port and the audio attach alternation;
        // version 7: the ECS registers; version 8: AGA (eight planes, FMODE,
        // Denise's 256 colours of 24 bits).
        w.chunk(TAG, 9, |w| {
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
            w.u8(self.cpu_starved);
            for s in self.spr_dma {
                w.u8(s as u8);
            }
            self.copper.save(w);
            self.blitter.save(w);
            self.denise.save(w);
            self.paula.save(w);
            for v in self.beam_regs {
                w.u16(v);
            }
            w.u16(self.diwhigh);
            w.u16(self.fmode);
            w.bool(self.lpen_latch.is_some());
            let (lv, lh) = self.lpen_latch.unwrap_or_default();
            w.u16(lv);
            w.u16(lh);
        });
    }

    /// Atomic; the standard and pointer width are configuration.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut c) = r.chunk(TAG, 9)?;
        let standard = match c.u8()? {
            0 => Standard::Pal,
            1 => Standard::Ntsc,
            _ => return Err(StateError::Invalid("video standard")),
        };
        let mut n = Custom::new(self.native, self.ptr_mask, self.chipset);
        let loopback = self.paula.uart.loopback;
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
            &mut n.pot_low,
        ] {
            *v = c.u16()?;
        }
        n.joydat = [c.u16()?, c.u16()?];
        n.lof = c.bool()?;
        n.lol = c.bool()?;
        n.fields = c.u64()?;
        let planes = if version >= 8 { 8 } else { 6 };
        for p in n.bplpt.iter_mut().take(planes).chain(n.sprpt.iter_mut()) {
            *p = c.u32()?;
        }
        n.bpl_vertical = c.bool()?;
        n.fetched = c.bool()?;
        n.cpu_starved = c.u8()?;
        for s in &mut n.spr_dma {
            *s = match c.u8()? {
                0 => SprDma::Off,
                1 => SprDma::Control,
                2 => SprDma::Wait,
                3 => SprDma::Data,
                _ => return Err(StateError::Invalid("sprite DMA state")),
            };
        }
        n.copper = Copper::load(&mut c, version)?;
        n.copper.ecs = self.chipset.ecs_agnus;
        n.blitter = crate::blitter::Blitter::load(&mut c, version)?;
        n.denise = Denise::load(&mut c, version)?;
        n.denise.ecs = self.chipset.ecs_denise;
        n.denise.aga = self.chipset.aga;
        n.paula = Paula::load(&mut c, version)?;
        n.paula.uart.loopback = loopback;
        if version >= 7 {
            for v in &mut n.beam_regs {
                *v = c.u16()?;
            }
            n.diwhigh = c.u16()?;
            if version >= 8 {
                n.fmode = c.u16()?;
                n.denise.fmode = n.fmode;
            }
            if version >= 9 {
                let latched = c.bool()?;
                let (lv, lh) = (c.u16()?, c.u16()?);
                n.lpen_latch = latched.then_some((lv, lh));
            }
        } else {
            n.diwhigh = Self::diwhigh_default(n.diwstop);
            n.denise.diwhigh = n.diwhigh;
        }
        if !self.chipset.ecs_agnus {
            n.beam_regs = [0; 18];
        }
        // The beam standard the state ran with: the strap, or BEAMCON0.
        if n.chipset.ecs_agnus {
            if n.beamcon0() & beamcon0::PAL != 0 {
                n.set_standard(Standard::Pal);
            } else {
                n.set_standard(Standard::Ntsc);
            }
        } else if standard != self.native {
            return Err(StateError::Invalid("video standard differs"));
        }
        if n.h >= n.line_len() || n.v >= n.field_lines() {
            return Err(StateError::Invalid("beam position"));
        }
        n.frame = std::mem::take(&mut self.frame);
        n.frame
            .resize((WIDTH * n.standard.height()) as usize, Rgb8::default());
        n.rec = self.rec.take();
        n.denise.rec_sprites = self.denise.rec_sprites;
        n.pen = self.pen;
        n.pen_luma = self.pen_luma;
        *self = n;
        Ok(())
    }
}

pub mod gfx;
pub mod overlay;

#[cfg(test)]
mod tests;
