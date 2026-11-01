//! The front panel as the machine's picture, laid out after a photograph
//! of a real IMSAI 8080 (see `docs/sources.md`): black plate with the
//! PROGRAMMED OUTPUT, STATUS BYTE, DATA BUS, ADDRESS BUS and RUN lamps, grey
//! sub-panel with the paddle switches: blue and red in groups of four, 16
//! address/data, then EXAMINE, DEPOSIT, RESET, RUN, SINGLE STEP and PWR.
//!
//! Lamps have 16 brightness levels, from the share of the frame's time the
//! line was active, so a program's bus pattern shows as on a real panel.

use emu_core::font;
use emu_core::frame::{FieldParity, OutputType, Rgb8, SignalInfo, VideoStandard};

pub const WIDTH: u32 = 1000;
pub const HEIGHT: u32 = 392;
const W: usize = WIDTH as usize;

const fn rgb(r: u8, g: u8, b: u8) -> Rgb8 {
    Rgb8 { r, g, b }
}

const CASE: u8 = 0;
const CASE_LIGHT: u8 = 1;
const PLATE: u8 = 2;
const GREY: u8 = 3;
const GREY_DARK: u8 = 4;
const WHITE: u8 = 5;
const SLOT: u8 = 6;
const BLUE: u8 = 7; // dark, face, lip
const RED: u8 = 10; // dark, face, lip
const LAMP: u8 = 13; // 16 levels

const fn lamp_colour(i: usize) -> Rgb8 {
    let n = i as u32;
    rgb(
        (0x26 + n * (0xff - 0x26) / 15) as u8,
        (0x06 + n * n * 0x60 / 225) as u8,
        (0x08 + n * n * 0x50 / 225) as u8,
    )
}

pub const PALETTE: [Rgb8; 29] = [
    rgb(0x3a, 0x8f, 0xc8),
    rgb(0x62, 0xb4, 0xe4),
    rgb(0x0c, 0x0b, 0x0d),
    rgb(0x6c, 0x6e, 0x70),
    rgb(0x4a, 0x4c, 0x4e),
    rgb(0xf2, 0xf2, 0xee),
    rgb(0x18, 0x18, 0x18),
    rgb(0x12, 0x58, 0xa8),
    rgb(0x1e, 0x78, 0xd0),
    rgb(0x3e, 0x9e, 0xee),
    rgb(0xa8, 0x1c, 0x24),
    rgb(0xd8, 0x2c, 0x34),
    rgb(0xf0, 0x50, 0x54),
    lamp_colour(0),
    lamp_colour(1),
    lamp_colour(2),
    lamp_colour(3),
    lamp_colour(4),
    lamp_colour(5),
    lamp_colour(6),
    lamp_colour(7),
    lamp_colour(8),
    lamp_colour(9),
    lamp_colour(10),
    lamp_colour(11),
    lamp_colour(12),
    lamp_colour(13),
    lamp_colour(14),
    lamp_colour(15),
];

/// Columns of bits 7…0 of a group: left and right block.
const LEFT_X: usize = 74;
const RIGHT_X: usize = 404;
const STEP: usize = 37;
const OUTPUT_Y: usize = 68;
const STATUS_Y: usize = 128;
const ADDRESS_Y: usize = 190;
const RUN_LAMP_X: usize = 806;
const RUN_LAMP_Y: usize = 182;
const PLATE_BOTTOM: usize = 250;
const PADDLE_TOP: usize = 266;
const PADDLE_H: usize = 66;
const PADDLE_W: usize = 34;
const CONTROL_X: usize = 722;
const CONTROL_STEP: usize = 44;

fn bit_x(left: usize, i: usize) -> usize {
    left + i * STEP
}

/// A paddle's position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paddle {
    Up,
    Middle,
    Down,
}

/// The lamps, 0–15 each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lamps {
    /// PROGRAMMED OUTPUT, bit 7 first.
    pub output: [u8; 8],
    /// MEMR INP M1 OUT HLTA STACK /WO INTA.
    pub status: [u8; 8],
    /// D7 first.
    pub data: [u8; 8],
    /// A15 first.
    pub address: [u8; 16],
    /// INTERRUPTS ENABLED, RUN, WAIT, HOLD.
    pub run: [u8; 4],
}

/// What a click on the picture hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// Address/data switch (bit).
    Sense(u8),
    /// Control paddle 0–5 (EXAMINE … PWR), its upper half or not.
    Control(u8, bool),
}

/// The paddle at frame position `(x, y)`.
pub fn hit(x: u16, y: u16) -> Option<Hit> {
    let (x, y) = (usize::from(x), usize::from(y));
    if !(PADDLE_TOP - 6..PADDLE_TOP + PADDLE_H + 6).contains(&y) {
        return None;
    }
    let upper = y < PADDLE_TOP + PADDLE_H / 2;
    let over = |left: usize, n: usize, step: usize| {
        (0..n).find(|&i| {
            let c = left + i * step;
            x + PADDLE_W / 2 + 1 >= c && x < c + PADDLE_W / 2 + 1
        })
    };
    if let Some(i) = over(LEFT_X, 8, STEP) {
        return Some(Hit::Sense(15 - i as u8));
    }
    if let Some(i) = over(RIGHT_X, 8, STEP) {
        return Some(Hit::Sense(7 - i as u8));
    }
    over(CONTROL_X, 6, CONTROL_STEP).map(|i| Hit::Control(i as u8, upper))
}

pub fn signal(fps: u32) -> SignalInfo {
    SignalInfo {
        pixel_clock_hz: WIDTH * HEIGHT * fps,
        active_pixels: WIDTH,
        total_pixels: WIDTH,
        lines_per_field: HEIGHT,
        standard: VideoStandard::Ntsc,
        output: OutputType::Rgb,
        field: FieldParity::Progressive,
    }
}

#[derive(Clone, Debug)]
pub struct Panel {
    pixels: Vec<u8>,
}

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

impl Panel {
    pub fn new() -> Self {
        let mut p = Self {
            pixels: vec![CASE; W * HEIGHT as usize],
        };
        p.draw_plate();
        p
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    fn rect(&mut self, x: usize, y: usize, w: usize, h: usize, c: u8) {
        for row in y..y + h {
            self.pixels[row * W + x..row * W + x + w].fill(c);
        }
    }

    fn text(&mut self, x: usize, y: usize, scale: usize, s: &str) {
        for (n, ch) in s.bytes().enumerate() {
            for row in 0..7 {
                // Column 1 of the font's cell is its leftmost dot.
                let dots = font::row(ch, row as u8);
                for col in 0..5 {
                    if dots & (1 << (5 - col)) != 0 {
                        let px = x + (n * 6 + col) * scale;
                        self.rect(px, y + row * scale, scale, scale, WHITE);
                    }
                }
            }
        }
    }

    /// Text centred on column `cx`.
    fn label(&mut self, cx: usize, y: usize, s: &str) {
        self.text(cx - (s.len() * 6 - 1) / 2, y, 1, s);
    }

    fn draw_plate(&mut self) {
        self.rect(0, 0, W, 6, CASE_LIGHT);
        self.rect(0, HEIGHT as usize - 6, W, 6, CASE);
        self.rect(14, 16, W - 28, PLATE_BOTTOM - 16, PLATE);
        self.rect(
            14,
            PLATE_BOTTOM,
            W - 28,
            HEIGHT as usize - PLATE_BOTTOM - 14,
            GREY,
        );
        self.rect(14, PLATE_BOTTOM, W - 28, 3, GREY_DARK);

        // The logo's heavy letters: the font's dots a little wider.
        for dx in [0, 2] {
            self.text(632 + dx, 30, 5, "IMSAI 8080");
        }
        self.rect(632, 72, 208, 2, WHITE);
        self.text(844, 70, 1, "MICROCOMPUTER SYSTEM");

        const STATUS: [&str; 8] = ["MEMR", "INP", "M1", "OUT", "HLTA", "STACK", "/WO", "INTA"];
        for (i, name) in STATUS.iter().enumerate() {
            let digit = (b'7' - i as u8) as char;
            let digit = digit.to_string();
            let hi = (15 - i).to_string();
            self.label(bit_x(LEFT_X, i), OUTPUT_Y + 16, &digit);
            self.label(bit_x(LEFT_X, i), STATUS_Y - 22, name);
            self.label(bit_x(RIGHT_X, i), STATUS_Y - 22, &digit);
            self.label(bit_x(LEFT_X, i), ADDRESS_Y + 16, &hi);
            self.label(bit_x(RIGHT_X, i), ADDRESS_Y + 16, &digit);
        }
        let side = bit_x(LEFT_X, 7) + 24;
        self.text(side, OUTPUT_Y - 8, 1, "PROGRAMMED");
        self.text(side, OUTPUT_Y + 3, 1, "OUTPUT");
        self.text(side, STATUS_Y - 8, 1, "STATUS");
        self.text(side, STATUS_Y + 3, 1, "BYTE");
        self.text(side, ADDRESS_Y - 8, 1, "ADDRESS");
        self.text(side, ADDRESS_Y + 3, 1, "BUS");
        let side = bit_x(RIGHT_X, 7) + 24;
        self.text(side, STATUS_Y - 4, 1, "DATA BUS");
        self.text(side, ADDRESS_Y - 8, 1, "ADDRESS");
        self.text(side, ADDRESS_Y + 3, 1, "BUS");

        // The 8 4 2 1 brackets above the address lamps.
        for left in [LEFT_X, RIGHT_X] {
            self.rect(left - 14, ADDRESS_Y - 26, 7 * STEP + 28, 1, WHITE);
            for i in 0..8 {
                let v = [8, 4, 2, 1][i % 4].to_string();
                self.label(bit_x(left, i), ADDRESS_Y - 36, &v);
            }
            for x in [left - 14, left + 4 * STEP - 18, left + 7 * STEP + 14] {
                self.rect(x, ADDRESS_Y - 36, 1, 11, WHITE);
            }
        }
        self.label(
            bit_x(LEFT_X, 0) + STEP * 7 / 2,
            PLATE_BOTTOM - 9,
            "ADDRESS-PROGRAMMED INPUT",
        );
        self.label(
            bit_x(RIGHT_X, 0) + STEP * 7 / 2,
            PLATE_BOTTOM - 9,
            "ADDRESS-DATA",
        );
        for (i, s) in ["INT ENAB", "RUN", "WAIT", "HOLD"].iter().enumerate() {
            self.label(RUN_LAMP_X + i * 42, RUN_LAMP_Y - 16, s);
        }
        // The control paddles are named on the plate above them.
        const TOP: [&str; 6] = ["EXAMINE", "DEPOSIT", "RESET", "RUN", "STEP", "PWR ON"];
        const BOTTOM: [&str; 6] = ["EXAMINE", "DEPOSIT", "EXT.CLR", "STOP", "STEP", "PWR OFF"];
        for i in 0..6 {
            let cx = CONTROL_X + i * CONTROL_STEP;
            self.label(cx, 208, TOP[i]);
            self.rect(cx - 19, 217, 38, 1, WHITE);
            self.label(cx, 221, BOTTOM[i]);
            if i < 2 {
                self.label(cx, 231, "NEXT");
            }
        }
        // Under each address/data paddle its bit.
        let (y1, y2) = (PADDLE_TOP + PADDLE_H + 9, PADDLE_TOP + PADDLE_H + 20);
        for i in 0..8 {
            self.label(bit_x(LEFT_X, i), y1, &format!("A{}", 15 - i));
            self.label(bit_x(LEFT_X, i), y2, &format!("S{}", 7 - i));
            self.label(bit_x(RIGHT_X, i), y1, &format!("A{}", 7 - i));
            self.label(bit_x(RIGHT_X, i), y2, &format!("D{}", 7 - i));
        }
    }

    /// A filled disc lamp, with a brighter core when lit.
    fn lamp(&mut self, cx: usize, cy: usize, level: u8) {
        let colour = LAMP + level.min(15);
        for dy in -6i32..=6 {
            for dx in -6i32..=6 {
                let d = dx * dx + dy * dy;
                if d <= 36 {
                    let at = (cy as i32 + dy) as usize * W + (cx as i32 + dx) as usize;
                    // The rim stays dimmer, as a LED under a red lens.
                    let c = if d > 16 {
                        LAMP + level.min(15) / 2
                    } else {
                        colour
                    };
                    self.pixels[at] = c;
                }
            }
        }
    }

    pub fn draw_lamps(&mut self, l: &Lamps) {
        for i in 0..8 {
            self.lamp(bit_x(LEFT_X, i), OUTPUT_Y, l.output[i]);
            self.lamp(bit_x(LEFT_X, i), STATUS_Y, l.status[i]);
            self.lamp(bit_x(RIGHT_X, i), STATUS_Y, l.data[i]);
            self.lamp(bit_x(LEFT_X, i), ADDRESS_Y, l.address[i]);
            self.lamp(bit_x(RIGHT_X, i), ADDRESS_Y, l.address[i + 8]);
        }
        for i in 0..4 {
            self.lamp(RUN_LAMP_X + i * 42, RUN_LAMP_Y, l.run[i]);
        }
    }

    /// One paddle centred on column `cx`: `colour` is [`BLUE`] or [`RED`].
    fn paddle(&mut self, cx: usize, colour: u8, pos: Paddle) {
        let x = cx - PADDLE_W / 2;
        self.rect(x - 1, PADDLE_TOP - 5, PADDLE_W + 2, PADDLE_H + 10, GREY);
        self.rect(x, PADDLE_TOP, PADDLE_W, PADDLE_H, SLOT);
        let (dark, face, lip) = (colour, colour + 1, colour + 2);
        match pos {
            Paddle::Up => {
                self.rect(x + 1, PADDLE_TOP - 4, PADDLE_W - 2, 16, lip);
                self.rect(x + 1, PADDLE_TOP + 12, PADDLE_W - 2, PADDLE_H - 22, face);
                self.rect(x + 1, PADDLE_TOP + PADDLE_H - 10, PADDLE_W - 2, 6, dark);
            }
            Paddle::Middle => {
                self.rect(x + 1, PADDLE_TOP + 2, PADDLE_W - 2, PADDLE_H - 4, face);
                self.rect(x + 1, PADDLE_TOP + PADDLE_H / 2 - 1, PADDLE_W - 2, 2, dark);
            }
            Paddle::Down => {
                self.rect(x + 1, PADDLE_TOP + 4, PADDLE_W - 2, 6, dark);
                self.rect(x + 1, PADDLE_TOP + 10, PADDLE_W - 2, PADDLE_H - 22, face);
                self.rect(x + 1, PADDLE_TOP + PADDLE_H - 12, PADDLE_W - 2, 16, lip);
            }
        }
    }

    /// `switches`: bit n up. `controls`: the six control paddles.
    pub fn draw_switches(&mut self, switches: u16, controls: &[Paddle; 6]) {
        for i in 0..8 {
            let colour = if i < 4 { BLUE } else { RED };
            let at = |b: usize| {
                if switches & (1 << b) != 0 {
                    Paddle::Up
                } else {
                    Paddle::Down
                }
            };
            self.paddle(bit_x(LEFT_X, i), colour, at(15 - i));
            self.paddle(bit_x(RIGHT_X, i), colour, at(7 - i));
        }
        for (i, &pos) in controls.iter().enumerate() {
            let colour = if i % 2 == 0 { BLUE } else { RED };
            self.paddle(CONTROL_X + i * CONTROL_STEP, colour, pos);
        }
    }
}
