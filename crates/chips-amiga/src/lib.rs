//! Amiga OCS custom chips: Agnus (beam counters, DMA slot allocation,
//! bitplane fetch, [`copper`], [`blitter`]), [`denise`] (bitplane display,
//! colours) and [`paula`] (interrupts; audio and disk later), joined in
//! [`custom::Custom`], which runs them one colour clock (CCK, one DMA slot)
//! at a time against [`chipram::ChipRam`].
//!
//! The 8520 CIAs are `chips_cbm::cia` with `CiaModel::Mos8520`.
//!
//! Sources: Commodore-Amiga, *Amiga Hardware Reference Manual*, 3rd edition
//! (register summary, appendix A–C; chapters 2 "Coprocessor hardware",
//! 3 "Playfield hardware", 6 "Blitter hardware", 7 "System control
//! hardware" with the DMA time-slot allocation figure). The implementation
//! is this project's own.

#![forbid(unsafe_code)]

pub mod blitter;
pub mod chipram;
pub mod copper;
pub mod custom;
pub mod denise;
pub mod paula;

pub use chipram::ChipRam;
pub use custom::{Custom, Standard, TickOut};

/// Custom chip register offsets from $DFF000 (HRM appendix A).
pub mod reg {
    pub const DMACONR: u16 = 0x002;
    pub const VPOSR: u16 = 0x004;
    pub const VHPOSR: u16 = 0x006;
    pub const DSKDATR: u16 = 0x008;
    pub const JOY0DAT: u16 = 0x00a;
    pub const JOY1DAT: u16 = 0x00c;
    pub const CLXDAT: u16 = 0x00e;
    pub const ADKCONR: u16 = 0x010;
    pub const POT0DAT: u16 = 0x012;
    pub const POT1DAT: u16 = 0x014;
    pub const POTGOR: u16 = 0x016;
    pub const SERDATR: u16 = 0x018;
    pub const DSKBYTR: u16 = 0x01a;
    pub const INTENAR: u16 = 0x01c;
    pub const INTREQR: u16 = 0x01e;
    pub const DSKPTH: u16 = 0x020;
    pub const DSKPTL: u16 = 0x022;
    pub const DSKLEN: u16 = 0x024;
    pub const VPOSW: u16 = 0x02a;
    pub const VHPOSW: u16 = 0x02c;
    pub const COPCON: u16 = 0x02e;
    pub const SERDAT: u16 = 0x030;
    pub const SERPER: u16 = 0x032;
    pub const POTGO: u16 = 0x034;
    pub const JOYTEST: u16 = 0x036;
    pub const BLTCON0: u16 = 0x040;
    pub const BLTCON1: u16 = 0x042;
    pub const BLTAFWM: u16 = 0x044;
    pub const BLTALWM: u16 = 0x046;
    pub const BLTCPTH: u16 = 0x048;
    pub const BLTDPTL: u16 = 0x056;
    pub const BLTSIZE: u16 = 0x058;
    pub const BLTCMOD: u16 = 0x060;
    pub const BLTDMOD: u16 = 0x066;
    pub const BLTCDAT: u16 = 0x070;
    pub const BLTBDAT: u16 = 0x072;
    pub const BLTADAT: u16 = 0x074;
    pub const DSKSYNC: u16 = 0x07e;
    pub const COP1LCH: u16 = 0x080;
    pub const COP1LCL: u16 = 0x082;
    pub const COP2LCH: u16 = 0x084;
    pub const COP2LCL: u16 = 0x086;
    pub const COPJMP1: u16 = 0x088;
    pub const COPJMP2: u16 = 0x08a;
    pub const DIWSTRT: u16 = 0x08e;
    pub const DIWSTOP: u16 = 0x090;
    pub const DDFSTRT: u16 = 0x092;
    pub const DDFSTOP: u16 = 0x094;
    pub const DMACON: u16 = 0x096;
    pub const CLXCON: u16 = 0x098;
    pub const INTENA: u16 = 0x09a;
    pub const INTREQ: u16 = 0x09c;
    pub const ADKCON: u16 = 0x09e;
    pub const BPL1PTH: u16 = 0x0e0;
    pub const BPL6PTL: u16 = 0x0f6;
    pub const BPLCON0: u16 = 0x100;
    pub const BPLCON1: u16 = 0x102;
    pub const BPLCON2: u16 = 0x104;
    pub const BPL1MOD: u16 = 0x108;
    pub const BPL2MOD: u16 = 0x10a;
    pub const BPL1DAT: u16 = 0x110;
    pub const BPL6DAT: u16 = 0x11a;
    pub const SPR0PTH: u16 = 0x120;
    pub const COLOR00: u16 = 0x180;
    pub const COLOR31: u16 = 0x1be;
    pub const DENISEID: u16 = 0x07c;
}

/// DMACON bits (HRM appendix A, DMACON).
pub mod dma {
    pub const SET: u16 = 0x8000;
    pub const BBUSY: u16 = 0x4000;
    pub const BZERO: u16 = 0x2000;
    pub const BLTPRI: u16 = 0x0400;
    pub const DMAEN: u16 = 0x0200;
    pub const BPLEN: u16 = 0x0100;
    pub const COPEN: u16 = 0x0080;
    pub const BLTEN: u16 = 0x0040;
    pub const SPREN: u16 = 0x0020;
    pub const DSKEN: u16 = 0x0010;
}

/// INTENA/INTREQ bits and their 68000 levels (HRM chapter 7, table 7-1).
pub mod int {
    pub const SET: u16 = 0x8000;
    pub const INTEN: u16 = 0x4000;
    pub const EXTER: u16 = 0x2000;
    pub const DSKSYN: u16 = 0x1000;
    pub const RBF: u16 = 0x0800;
    pub const AUD0: u16 = 0x0080;
    pub const BLIT: u16 = 0x0040;
    pub const VERTB: u16 = 0x0020;
    pub const COPER: u16 = 0x0010;
    pub const PORTS: u16 = 0x0008;
    pub const SOFT: u16 = 0x0004;
    pub const DSKBLK: u16 = 0x0002;
    pub const TBE: u16 = 0x0001;
}
