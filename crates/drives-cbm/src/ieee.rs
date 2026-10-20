//! IEEE-488 bus (PET, CBM 2031/4040/8050): ATN, EOI, the handshake lines
//! DAV, NRFD and NDAC, and eight data lines, all open-collector with
//! negative logic: a line is asserted (logic true, a data bit 1) as soon as
//! any device pulls it low.

/// Lines pulled low; `data` holds a bit per pulled data line (DIO1 = bit 0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IeeeLines {
    pub atn: bool,
    pub eoi: bool,
    /// Data valid (from the talker).
    pub dav: bool,
    /// Not ready for data (from the listeners).
    pub nrfd: bool,
    /// Not data accepted (from the listeners).
    pub ndac: bool,
    pub data: u8,
}

impl IeeeLines {
    /// Wired-OR of what each participant pulls.
    pub fn combine(parts: &[IeeeLines]) -> Self {
        parts.iter().fold(Self::default(), |a, p| Self {
            atn: a.atn || p.atn,
            eoi: a.eoi || p.eoi,
            dav: a.dav || p.dav,
            nrfd: a.nrfd || p.nrfd,
            ndac: a.ndac || p.ndac,
            data: a.data | p.data,
        })
    }
}
