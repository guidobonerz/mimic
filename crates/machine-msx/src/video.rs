//! The machine's video chip: the MSX1's TMS9918A/TMS9929A (`chips-ti`) or
//! the MSX2's V9938 (`chips-yamaha`), behind one set of calls.
//!
//! Both run 342 clocks per line, three for every two T-states of the Z80.
//! The TMS9918A's picture is 284 pixels wide as palette indices, the
//! V9938's 568 half-width pixels as RGB (its 512-pixel modes need them).

use chips_ti::{vdp, Tms9918, VdpModel};
use chips_yamaha::{v9938, V9938};
use emu_core::frame::{PixelData, VideoStandard};
use emu_core::gfx::{GfxOptions, GfxView, GfxViewInfo};
use emu_core::machine::ChipReg;
use emu_core::overlay::{Overlay, OverlayInfo};
use emu_core::{MemSpaceId, StateError, StateReader, StateWriter};

#[derive(Clone, Debug)]
pub enum Video {
    Tms(Tms9918),
    V9938(Box<V9938>),
}

impl Video {
    /// The chip at power-on; `v9938` picks the MSX2's.
    pub fn new(v9938: bool, standard: VideoStandard) -> Self {
        let pal = standard == VideoStandard::Pal;
        if v9938 {
            Video::V9938(Box::new(V9938::new(pal)))
        } else {
            Video::Tms(Tms9918::new(if pal {
                VdpModel::Pal
            } else {
                VdpModel::Ntsc
            }))
        }
    }

    /// The chip as after a power cycle.
    pub fn power_on(&self) -> Self {
        match self {
            Video::Tms(v) => Video::Tms(Tms9918::new(v.model)),
            Video::V9938(v) => Video::V9938(Box::new(V9938::new(v.pal_default()))),
        }
    }

    pub fn is_v9938(&self) -> bool {
        matches!(self, Video::V9938(_))
    }

    pub fn chip_name(&self) -> &'static str {
        match self {
            Video::Tms(v) => v.model.chip_name(),
            Video::V9938(_) => "V9938",
        }
    }

    /// One clock; true at the end of a field.
    #[inline]
    pub fn tick(&mut self) -> bool {
        match self {
            Video::Tms(v) => v.tick(),
            Video::V9938(v) => v.tick(),
        }
    }

    pub fn reset(&mut self) {
        match self {
            Video::Tms(v) => v.reset(),
            Video::V9938(v) => v.reset(),
        }
    }

    pub fn int(&self) -> bool {
        match self {
            Video::Tms(v) => v.int(),
            Video::V9938(v) => v.int(),
        }
    }

    /// A read of port `n` (0–3): data, status; the V9938's ports 2 and 3
    /// are write-only.
    pub fn read(&mut self, n: u8) -> u8 {
        match (self, n) {
            (Video::Tms(v), 0) => v.read_data(),
            (Video::Tms(v), 1) => v.read_status(),
            (Video::V9938(v), 0) => v.read_data(),
            (Video::V9938(v), 1) => v.read_status(),
            _ => 0xff,
        }
    }

    /// A write to port `n` (0–3): data, control, and on the V9938 the
    /// palette and the indirect register port.
    pub fn write(&mut self, n: u8, value: u8) {
        match (self, n) {
            (Video::Tms(v), 0) => v.write_data(value),
            (Video::Tms(v), 1) => v.write_control(value),
            (Video::V9938(v), 0) => v.write_data(value),
            (Video::V9938(v), 1) => v.write_control(value),
            (Video::V9938(v), 2) => v.write_palette(value),
            (Video::V9938(v), 3) => v.write_indirect(value),
            _ => {}
        }
    }

    /// Port `n` without side effects.
    pub fn peek(&self, n: u8) -> u8 {
        let (data, status) = match self {
            Video::Tms(v) => v.peek(),
            Video::V9938(v) => v.peek(),
        };
        match n {
            0 => data,
            1 => status,
            _ => 0xff,
        }
    }

    /// The beam: line, clock in the line, fields since power-on.
    pub fn beam(&self) -> (u16, u16, u64) {
        match self {
            Video::Tms(v) => (v.line, v.dot, v.fields),
            Video::V9938(v) => (v.line, v.dot, v.fields),
        }
    }

    /// The picture column of clock `dot`.
    pub fn column(&self, dot: u16) -> u16 {
        match self {
            Video::Tms(_) => dot.min(vdp::WIDTH as u16 - 1),
            Video::V9938(_) => (2 * dot).min(v9938::WIDTH as u16 - 1),
        }
    }

    pub fn standard(&self) -> VideoStandard {
        let pal = match self {
            Video::Tms(v) => v.model == VdpModel::Pal,
            Video::V9938(v) => v.is_pal(),
        };
        if pal {
            VideoStandard::Pal
        } else {
            VideoStandard::Ntsc
        }
    }

    /// Lines per field.
    pub fn lines(&self) -> u16 {
        match self {
            Video::Tms(v) => v.model.lines(),
            Video::V9938(v) => v.lines(),
        }
    }

    pub fn width(&self) -> u32 {
        match self {
            Video::Tms(_) => vdp::WIDTH,
            Video::V9938(_) => v9938::WIDTH,
        }
    }

    pub fn height(&self) -> u32 {
        match self {
            Video::Tms(v) => v.model.height(),
            Video::V9938(v) => v.height(),
        }
    }

    pub fn pixels(&self) -> PixelData<'_> {
        match self {
            Video::Tms(v) => PixelData::Indexed {
                pixels: v.frame(),
                palette: &vdp::PALETTE,
            },
            Video::V9938(v) => PixelData::Rgb(v.frame()),
        }
    }

    pub fn line_row(&self, line: u16) -> Option<u32> {
        match self {
            Video::Tms(v) => v.line_row(line),
            Video::V9938(v) => v.line_row(line),
        }
    }

    pub fn vram(&self) -> &[u8] {
        match self {
            Video::Tms(v) => &v.vram,
            Video::V9938(v) => &v.vram,
        }
    }

    pub fn vram_mut(&mut self) -> &mut [u8] {
        match self {
            Video::Tms(v) => &mut v.vram,
            Video::V9938(v) => &mut v.vram,
        }
    }

    /// Address bits of the VRAM space.
    pub fn vram_bits(&self) -> u8 {
        match self {
            Video::Tms(_) => 14,
            Video::V9938(_) => 17,
        }
    }

    /// The registers for the debugger's chip view.
    pub fn chip_regs(&self) -> Vec<ChipReg> {
        const R: [&str; 47] = [
            "R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7", "R8", "R9", "R10", "R11", "R12", "R13",
            "R14", "R15", "R16", "R17", "R18", "R19", "R20", "R21", "R22", "R23", "", "", "", "",
            "", "", "", "", "R32", "R33", "R34", "R35", "R36", "R37", "R38", "R39", "R40", "R41",
            "R42", "R43", "R44", "R45", "R46",
        ];
        const S: [&str; 10] = ["S0", "S1", "S2", "S3", "S4", "S5", "S6", "S7", "S8", "S9"];
        const P: [&str; 16] = [
            "P0", "P1", "P2", "P3", "P4", "P5", "P6", "P7", "P8", "P9", "P10", "P11", "P12", "P13",
            "P14", "P15",
        ];
        let reg = |name, value: u32, bits| ChipReg {
            name,
            addr: None,
            value,
            bits,
        };
        let (line, dot, _) = self.beam();
        let mut regs = Vec::new();
        match self {
            Video::Tms(v) => {
                regs.extend(R.iter().zip(v.regs).map(|(n, r)| reg(*n, u32::from(r), 8)));
                regs.push(reg("STATUS", u32::from(v.peek().1), 8));
                regs.push(reg("ADDR", u32::from(v.addr), 14));
            }
            Video::V9938(v) => {
                regs.extend(
                    R.iter()
                        .zip(v.regs)
                        .filter(|(n, _)| !n.is_empty())
                        .map(|(n, r)| reg(*n, u32::from(r), 8)),
                );
                regs.extend(
                    S.iter()
                        .zip(0u8..)
                        .map(|(n, i)| reg(*n, u32::from(v.status_peek(i)), 8)),
                );
                regs.extend(
                    P.iter()
                        .zip(v.palette)
                        .map(|(n, p)| reg(*n, u32::from(p), 11)),
                );
                regs.push(reg("ADDR", v.vram_addr() as u32, 17));
            }
        }
        regs.push(reg("LINE", u32::from(line), 9));
        regs.push(reg("DOT", u32::from(dot), 9));
        regs
    }

    pub fn gfx_views(&self) -> Vec<GfxViewInfo> {
        match self {
            Video::Tms(v) => vdp::gfx::views(v.model.chip_name()),
            Video::V9938(_) => v9938::gfx::views(),
        }
    }

    pub fn gfx_view(&self, id: &str, space: MemSpaceId, opts: &GfxOptions) -> Option<GfxView> {
        match self {
            Video::Tms(v) => v.gfx_view(id, space, opts),
            Video::V9938(v) => v.gfx_view(id, space, opts),
        }
    }

    pub fn overlays(&self) -> Vec<OverlayInfo> {
        match self {
            Video::Tms(v) => vdp::overlay::infos(v.model.chip_name()),
            Video::V9938(_) => v9938::overlay::infos(),
        }
    }

    pub fn set_overlays(&mut self, ids: &[&str]) {
        match self {
            Video::Tms(v) => v.set_overlays(ids),
            Video::V9938(v) => v.set_overlays(ids),
        }
    }

    pub fn overlays_on(&self) -> bool {
        match self {
            Video::Tms(v) => v.overlays_on(),
            Video::V9938(v) => v.overlays_on(),
        }
    }

    pub fn overlay_end_frame(&mut self) {
        match self {
            Video::Tms(v) => v.overlay_end_frame(),
            Video::V9938(v) => v.overlay_end_frame(),
        }
    }

    pub fn overlay(&self, id: &str, last: bool) -> Option<Overlay> {
        match self {
            Video::Tms(v) => v.overlay(id, last),
            Video::V9938(v) => v.overlay(id, last),
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        match self {
            Video::Tms(v) => v.save(w),
            Video::V9938(v) => v.save(w),
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        match self {
            Video::Tms(v) => v.load(r),
            Video::V9938(v) => v.load(r),
        }
    }
}
