//! Commodore serial (IEC) bus: open-collector lines ATN, CLK and DATA, and
//! SRQ, which the C128 and 1571 use as the clock of fast serial transfers.
//! A line is low ("asserted", logic true) as soon as any device pulls it.

/// Levels of the bus lines; `true` = pulled low.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IecLines {
    pub atn: bool,
    pub clk: bool,
    pub data: bool,
    pub srq: bool,
}

impl IecLines {
    /// Wired-OR of what each participant pulls.
    pub fn combine(parts: &[IecLines]) -> Self {
        parts.iter().fold(Self::default(), |a, p| Self {
            atn: a.atn || p.atn,
            clk: a.clk || p.clk,
            data: a.data || p.data,
            srq: a.srq || p.srq,
        })
    }
}
