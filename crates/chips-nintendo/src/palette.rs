//! The NES colours, computed from the PPU's composite signal.
//!
//! The 2C02 has no RGB palette: each colour is a square wave between two
//! of eight voltage levels, in one of twelve phases of the colour
//! subcarrier. This module decodes those waves the way a television does
//! (YIQ by averaging over one subcarrier period) for all 64 colours and
//! the 8 combinations of the colour emphasis bits, which attenuate the
//! signal to 74.6 % during the half-periods of three phases.
//!
//! Levels (terminated, relative): low 0.228 0.312 0.552 0.880 and high
//! 0.616 0.840 1.100 1.100 for luma rows 0–3; black 0.312, white 1.100.
//! Colour 0 of a row is the high level only, colours $D–$F the low level
//! ($E/$F of row 1, black). Source: NESdev wiki, "NTSC video" (levels,
//! phases, emphasis). The decoding (hue offset 3.75 phases, saturation
//! 1.3) is this project's choice of a television's tint and colour
//! controls.

use std::sync::OnceLock;

use emu_core::frame::Rgb8;

/// Entries: `emphasis << 6 | colour`, emphasis bit 0 red, 1 green, 2 blue
/// (the 2C02's $2001 bits 5–7).
pub const ENTRIES: usize = 512;

const LOW: [f64; 4] = [0.228, 0.312, 0.552, 0.880];
const HIGH: [f64; 4] = [0.616, 0.840, 1.100, 1.100];
const BLACK: f64 = 0.312;
const WHITE: f64 = 1.100;
const EMPHASIS: f64 = 0.746;
const HUE: f64 = 3.75;
const SATURATION: f64 = 1.3;

fn in_phase(colour: u32, phase: u32) -> bool {
    (colour + phase) % 12 < 6
}

fn rgb(entry: usize) -> Rgb8 {
    let colour = (entry & 0x0f) as u32;
    let mut row = (entry >> 4) & 3;
    if colour > 13 {
        row = 1;
    }
    let emphasis = entry >> 6;
    let (mut low, mut high) = (LOW[row], HIGH[row]);
    if colour == 0 {
        low = high;
    }
    if colour > 12 {
        high = low;
    }
    let (mut y, mut i, mut q) = (0.0, 0.0, 0.0);
    for p in 0..12 {
        let mut s = if in_phase(colour, p) { high } else { low };
        let attenuated = (emphasis & 1 != 0 && in_phase(0, p))
            || (emphasis & 2 != 0 && in_phase(4, p))
            || (emphasis & 4 != 0 && in_phase(8, p));
        if attenuated && colour < 14 {
            s *= EMPHASIS;
        }
        let s = (s - BLACK) / (WHITE - BLACK);
        let a = std::f64::consts::PI / 6.0 * (f64::from(p) + HUE);
        y += s;
        i += s * a.cos();
        q += s * a.sin();
    }
    y /= 12.0;
    i *= SATURATION / 12.0;
    q *= SATURATION / 12.0;
    let to8 = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    Rgb8 {
        r: to8(y + 0.946_882 * i + 0.623_557 * q),
        g: to8(y - 0.274_788 * i - 0.635_691 * q),
        b: to8(y - 1.108_545 * i + 1.709_007 * q),
    }
}

/// The 512 colours.
pub fn table() -> &'static [Rgb8; ENTRIES] {
    static TABLE: OnceLock<[Rgb8; ENTRIES]> = OnceLock::new();
    TABLE.get_or_init(|| std::array::from_fn(rgb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greys_and_hues() {
        let t = table();
        let c = |n: usize| t[n];
        assert_eq!(c(0x0f), Rgb8 { r: 0, g: 0, b: 0 }, "$0F black");
        assert_eq!(
            c(0x20),
            Rgb8 {
                r: 255,
                g: 255,
                b: 255
            },
            "$20 white"
        );
        assert!(c(0x00).r == c(0x00).g && c(0x00).g == c(0x00).b, "$00 grey");
        let red = c(0x16);
        assert!(red.r > red.g * 2 && red.r > red.b * 2, "$16 red: {red:?}");
        let green = c(0x1a);
        assert!(
            green.g > green.r * 2 && green.g > green.b * 2,
            "$1A green: {green:?}"
        );
        let blue = c(0x12);
        assert!(
            blue.b > blue.r * 2 && blue.b > blue.g * 2,
            "$12 blue: {blue:?}"
        );
        // All three emphasis bits darken white to a grey.
        let dark = c(7 << 6 | 0x30);
        assert!(
            dark.r < 200 && dark.r == dark.g && dark.g == dark.b,
            "{dark:?}"
        );
        // Red emphasis tints white red.
        let pink = c(1 << 6 | 0x30);
        assert!(pink.r > pink.b + 40, "{pink:?}");
    }
}
