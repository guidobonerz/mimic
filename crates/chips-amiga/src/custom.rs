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
            joydat: [0; 2],
            pot_low: 0,
            copper: Copper::new(),
            blitter: Blitter::new(),
            denise: Denise::new(),
            paula: Paula::new(),
            bpl_vertical: false,
            fetched: false,
            cpu_starved: 0,
            cpu_pending: false,
            spr_dma: [SprDma::Off; 8],
            frame: vec![Rgb8::default(); (WIDTH * standard.height()) as usize],
            lut: (0..4096u16).map(denise::rgb12).collect(),
        }
    }

    /// Power-on / RESET: registers to their reset values; the beam keeps
    /// running.
    pub fn reset(&mut self) {
        let mut n = Self::new(self.standard, self.ptr_mask);
        n.paula.uart.loopback = self.paula.uart.loopback;
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

    /// The CPU starts a chip bus cycle: the next colour clock is the first
    /// half of it, in which the CPU already waits for the bus but cannot
    /// use it yet (its data comes in the second half at the earliest).
    pub fn cpu_request(&mut self) {
        self.cpu_pending = true;
    }

    fn dma_on(&self, bit: u16) -> bool {
        self.dmacon & dma::DMAEN != 0 && self.dmacon & bit != 0
    }

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

    /// (first slot, first slot after) of the bitplane fetch window.
    /// Groups start at DDFSTRT + 8n up to DDFSTOP; in high resolution the
    /// stop counts half a group later, so the usual hires DDFSTOP $D0 and
    /// $D4 both fetch 40 words (Workbench 1.3 uses $3C/$D0 for 640 pixels).
    fn fetch_window(&self) -> (u16, u16) {
        let start = (self.ddfstrt & 0xfc).max(0x18);
        let stop = (self.ddfstop & 0xfc).min(0xd8) + if self.denise.hires() { 4 } else { 0 };
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
        let (v, h) = (self.v, self.h);
        let mut out = TickOut::default();
        if h == 0 {
            out.hsync = true;
            if v == 0 {
                out.vsync = true;
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
        if h == denise::LINE_START + DENISE_LAG {
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
                } else if h & 1 == 0 && self.copper.wants_slot() && self.dma_on(dma::COPEN) {
                    copper_took = true;
                    if let Some((r, val)) = self.copper.cycle(chip, v, h, self.blitter.busy) {
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
        if self.fetched && h == self.fetch_window().1 {
            self.fetched = false;
        }
        self.render(v, h);
        self.audio_tick();
        let a = &self.paula.audio;
        out.audio = [a[0].level() + a[3].level(), a[1].level() + a[2].level()];
        let ints = self.paula.uart.tick();
        self.paula.request(ints);

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

    /// The audio channels' colour clock: samples, block interrupts and the
    /// ADKCON attach modes (a channel's words set the next channel's
    /// volume or period; see `audio`).
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
                SprDma::Wait | SprDma::Data if v == s.vstop() => *st = SprDma::Control,
                SprDma::Wait if v == s.vstart() => *st = SprDma::Data,
                _ => {}
            }
        }
    }

    /// Is slot `h` in the last fetch group of the line, where each plane
    /// fetches its last word (low resolution: the last 8 colour clocks of
    /// the window; high resolution fetches every plane twice per group, so
    /// the last 4)?
    fn last_fetch_group(&self, h: u16) -> bool {
        let end = self.fetch_window().1;
        let group = if self.denise.hires() { 4 } else { 8 };
        h + group >= end
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

    fn render(&mut self, v: u16, h: u16) {
        let dh = self.denise_h(h);
        let mut px = [0u16; 4];
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
                Self::set_ptr(&mut self.paula.disk.pt, r == reg::DSKPTH, v, mask)
            }
            reg::DSKLEN => {
                self.paula.dsklen = v;
                let ints = self.paula.disk.write_dsklen(v);
                self.paula.request(ints);
            }
            reg::VPOSW => {
                self.lof = v & 0x8000 != 0;
                self.v = (self.v & 0xff) | (v & 1) << 8;
            }
            reg::VHPOSW => {
                self.v = (self.v & 0x100) | v >> 8;
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
            reg::BPLCON0 => self.denise.bplcon0 = v,
            reg::BPLCON1 => self.denise.bplcon1 = v,
            reg::BPLCON2 => self.denise.bplcon2 = v,
            reg::BPL1MOD => self.bpl1mod = v & 0xfffe,
            reg::BPL2MOD => self.bpl2mod = v & 0xfffe,
            reg::BPL1DAT..=reg::BPL6DAT => {
                let dh = self.denise_slot(self.h);
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
            reg::VPOSR => u16::from(self.lof) << 15 | self.v >> 8,
            reg::VHPOSR => (self.v & 0xff) << 8 | self.h,
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
        // version 6: the serial port and the audio attach alternation.
        w.chunk(TAG, 6, |w| {
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
        });
    }

    /// Atomic; the standard and pointer width are configuration.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut c) = r.chunk(TAG, 6)?;
        let standard = match c.u8()? {
            0 => Standard::Pal,
            1 => Standard::Ntsc,
            _ => return Err(StateError::Invalid("video standard")),
        };
        if standard != self.standard {
            return Err(StateError::Invalid("video standard differs"));
        }
        let mut n = Custom::new(self.standard, self.ptr_mask);
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
        for p in n.bplpt.iter_mut().chain(n.sprpt.iter_mut()) {
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
        n.blitter = crate::blitter::Blitter::load(&mut c)?;
        n.denise = Denise::load(&mut c, version)?;
        n.paula = Paula::load(&mut c, version)?;
        n.paula.uart.loopback = loopback;
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
