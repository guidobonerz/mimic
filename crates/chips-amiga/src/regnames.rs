//! Names of the custom chip registers ($DFF000 + offset) as the Amiga
//! Hardware Reference Manual's register summary (appendix A, with the ECS
//! additions of appendix C) lists them, for the Copper disassembler and
//! debugger views.

#[rustfmt::skip]
const NAMES: [&str; 256] = [
    // $000
    "BLTDDAT", "DMACONR", "VPOSR", "VHPOSR", "DSKDATR", "JOY0DAT", "JOY1DAT", "CLXDAT",
    "ADKCONR", "POT0DAT", "POT1DAT", "POTGOR", "SERDATR", "DSKBYTR", "INTENAR", "INTREQR",
    // $020
    "DSKPTH", "DSKPTL", "DSKLEN", "DSKDAT", "REFPTR", "VPOSW", "VHPOSW", "COPCON",
    "SERDAT", "SERPER", "POTGO", "JOYTEST", "STREQU", "STRVBL", "STRHOR", "STRLONG",
    // $040
    "BLTCON0", "BLTCON1", "BLTAFWM", "BLTALWM", "BLTCPTH", "BLTCPTL", "BLTBPTH", "BLTBPTL",
    "BLTAPTH", "BLTAPTL", "BLTDPTH", "BLTDPTL", "BLTSIZE", "BLTCON0L", "BLTSIZV", "BLTSIZH",
    // $060
    "BLTCMOD", "BLTBMOD", "BLTAMOD", "BLTDMOD", "$068", "$06A", "$06C", "$06E",
    "BLTCDAT", "BLTBDAT", "BLTADAT", "$076", "$078", "$07A", "DENISEID", "DSKSYNC",
    // $080
    "COP1LCH", "COP1LCL", "COP2LCH", "COP2LCL", "COPJMP1", "COPJMP2", "COPINS", "DIWSTRT",
    "DIWSTOP", "DDFSTRT", "DDFSTOP", "DMACON", "CLXCON", "INTENA", "INTREQ", "ADKCON",
    // $0A0
    "AUD0LCH", "AUD0LCL", "AUD0LEN", "AUD0PER", "AUD0VOL", "AUD0DAT", "$0AC", "$0AE",
    "AUD1LCH", "AUD1LCL", "AUD1LEN", "AUD1PER", "AUD1VOL", "AUD1DAT", "$0BC", "$0BE",
    // $0C0
    "AUD2LCH", "AUD2LCL", "AUD2LEN", "AUD2PER", "AUD2VOL", "AUD2DAT", "$0CC", "$0CE",
    "AUD3LCH", "AUD3LCL", "AUD3LEN", "AUD3PER", "AUD3VOL", "AUD3DAT", "$0DC", "$0DE",
    // $0E0
    "BPL1PTH", "BPL1PTL", "BPL2PTH", "BPL2PTL", "BPL3PTH", "BPL3PTL", "BPL4PTH", "BPL4PTL",
    "BPL5PTH", "BPL5PTL", "BPL6PTH", "BPL6PTL", "BPL7PTH", "BPL7PTL", "BPL8PTH", "BPL8PTL",
    // $100
    "BPLCON0", "BPLCON1", "BPLCON2", "BPLCON3", "BPL1MOD", "BPL2MOD", "BPLCON4", "CLXCON2",
    "BPL1DAT", "BPL2DAT", "BPL3DAT", "BPL4DAT", "BPL5DAT", "BPL6DAT", "BPL7DAT", "BPL8DAT",
    // $120
    "SPR0PTH", "SPR0PTL", "SPR1PTH", "SPR1PTL", "SPR2PTH", "SPR2PTL", "SPR3PTH", "SPR3PTL",
    "SPR4PTH", "SPR4PTL", "SPR5PTH", "SPR5PTL", "SPR6PTH", "SPR6PTL", "SPR7PTH", "SPR7PTL",
    // $140
    "SPR0POS", "SPR0CTL", "SPR0DATA", "SPR0DATB", "SPR1POS", "SPR1CTL", "SPR1DATA", "SPR1DATB",
    "SPR2POS", "SPR2CTL", "SPR2DATA", "SPR2DATB", "SPR3POS", "SPR3CTL", "SPR3DATA", "SPR3DATB",
    // $160
    "SPR4POS", "SPR4CTL", "SPR4DATA", "SPR4DATB", "SPR5POS", "SPR5CTL", "SPR5DATA", "SPR5DATB",
    "SPR6POS", "SPR6CTL", "SPR6DATA", "SPR6DATB", "SPR7POS", "SPR7CTL", "SPR7DATA", "SPR7DATB",
    // $180
    "COLOR00", "COLOR01", "COLOR02", "COLOR03", "COLOR04", "COLOR05", "COLOR06", "COLOR07",
    "COLOR08", "COLOR09", "COLOR10", "COLOR11", "COLOR12", "COLOR13", "COLOR14", "COLOR15",
    // $1A0
    "COLOR16", "COLOR17", "COLOR18", "COLOR19", "COLOR20", "COLOR21", "COLOR22", "COLOR23",
    "COLOR24", "COLOR25", "COLOR26", "COLOR27", "COLOR28", "COLOR29", "COLOR30", "COLOR31",
    // $1C0 (ECS)
    "HTOTAL", "HSSTOP", "HBSTRT", "HBSTOP", "VTOTAL", "VSSTOP", "VBSTRT", "VBSTOP",
    "SPRHSTRT", "SPRHSTOP", "BPLHSTRT", "BPLHSTOP", "HHPOSW", "HHPOSR", "BEAMCON0", "HSSTRT",
    // $1E0
    "VSSTRT", "HCENTER", "DIWHIGH", "BPLHMOD", "SPRHPTH", "SPRHPTL", "BPLHPTH", "BPLHPTL",
    "$1F0", "$1F2", "$1F4", "$1F6", "$1F8", "$1FA", "FMODE", "NO-OP",
];

/// The name of the register at offset `reg` ($000–$1FE).
pub fn name(reg: u16) -> &'static str {
    NAMES[usize::from((reg & 0x1fe) >> 1)]
}

/// The offset of a register by name (case-insensitive).
pub fn offset(name: &str) -> Option<u16> {
    NAMES
        .iter()
        .position(|n| n.eq_ignore_ascii_case(name))
        .map(|i| (i as u16) << 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_offsets() {
        assert_eq!(name(0x180), "COLOR00");
        assert_eq!(name(0x096), "DMACON");
        assert_eq!(name(0x058), "BLTSIZE");
        assert_eq!(name(0x0e0), "BPL1PTH");
        assert_eq!(name(0x140), "SPR0POS");
        assert_eq!(offset("intena"), Some(0x09a));
    }
}
