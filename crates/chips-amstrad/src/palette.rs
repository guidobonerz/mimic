//! The Gate Array's 32 hardware colours (27 distinct): each RGB component
//! is off, half or full (cpcwiki, "Gate Array", colour table). Half
//! intensity measures nearer 40 % than 50 % on real machines; 50 % is used
//! here, the monitor profile adjusts the rest.

use emu_core::frame::Rgb8;

/// Levels 0, ½, 1 of each hardware colour as (R, G, B).
const LEVELS: [(u8, u8, u8); 32] = [
    (1, 1, 1), // 0 white
    (1, 1, 1), // 1 white
    (0, 2, 1), // 2 sea green
    (2, 2, 1), // 3 pastel yellow
    (0, 0, 1), // 4 blue
    (2, 0, 1), // 5 purple
    (0, 1, 1), // 6 cyan
    (2, 1, 1), // 7 pink
    (2, 0, 1), // 8 purple
    (2, 2, 1), // 9 pastel yellow
    (2, 2, 0), // 10 bright yellow
    (2, 2, 2), // 11 bright white
    (2, 0, 0), // 12 bright red
    (2, 0, 2), // 13 bright magenta
    (2, 1, 0), // 14 orange
    (2, 1, 2), // 15 pastel magenta
    (0, 0, 1), // 16 blue
    (0, 2, 1), // 17 sea green
    (0, 2, 0), // 18 bright green
    (0, 2, 2), // 19 bright cyan
    (0, 0, 0), // 20 black
    (0, 0, 2), // 21 bright blue
    (0, 1, 0), // 22 green
    (0, 1, 2), // 23 sky blue
    (1, 0, 1), // 24 magenta
    (1, 2, 1), // 25 pastel green
    (1, 2, 0), // 26 lime
    (1, 2, 2), // 27 pastel cyan
    (1, 0, 0), // 28 red
    (1, 0, 2), // 29 mauve
    (1, 1, 0), // 30 yellow
    (1, 1, 2), // 31 pastel blue
];

/// RGB of the 32 hardware colours.
pub fn palette() -> [Rgb8; 32] {
    let level = |l: u8| [0x00, 0x80, 0xff][usize::from(l)];
    LEVELS.map(|(r, g, b)| Rgb8 {
        r: level(r),
        g: level(g),
        b: level(b),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_seven_distinct_colours() {
        let mut p = palette().to_vec();
        p.sort_by_key(|c| (c.r, c.g, c.b));
        p.dedup();
        assert_eq!(p.len(), 27);
        let black = palette()[20];
        assert_eq!((black.r, black.g, black.b), (0, 0, 0));
    }
}
