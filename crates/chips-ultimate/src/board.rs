//! The board's smaller blocks, gathered in one [`Board`]: everything on
//! the I/O bus that is neither the ITU, the SPI flash, the overlay, the
//! U64 I/O page, the C64 side nor the WiFi UART.
//!
//! | I/O address | Block | Depth here |
//! |---|---|---|
//! | `$060000`–`$0600FF` | SD card SPI ([`SdSpi`]) | registers; no card in the slot |
//! | `$060100`–`$0601FF` | I2C clock chip (not fitted, no user) | reads 0 |
//! | `$060300`–`$0603FF` | trace: profiler bytes at `$060304`/`$060305` | stored |
//! | `$060400`–`$0604FF` | seconds timer ([`RtcTimer`]) | complete |
//! | `$060500`–`$0605FF` | GCR codec ([`GcrCodec`]) | complete |
//! | `$060600`–`$0606FF` | ICAP, the FPGA's reconfiguration port ([`Icap`]) | records, sees the restart request |
//! | `$060700`–`$0607FF` | audio select | two stored nibbles |
//! | `$060800`–`$0608FF` | Ethernet MAC (RMII) | no link: stores writes, reads 0 |
//! | `$080000`–`$080FFF` | USB controller ("nano") | its 2 KB of memory and the run bit; the controller never runs |
//! | `$100000`–`$1000FF` | board pins ([`U2pIo`]): MDIO, board revision, speaker, hub and USB reset | latches; no Ethernet PHY answers |
//! | `$100100`–`$1002FF` | DDR2 PHY, clock measurement | read 0 |
//! | `$100500`–`$1005FF` | audio and speaker mixer, resampler ([`Mixer`]) | stored |
//! | `$100600`–`$1006FF` | LED strip ([`LedStrip`]) | registers, map, the LEDs' colours as of the last frame sent |
//! | `$100700`–`$1007FF` | I2C master with the chips behind it ([`I2c`]) | transactions with acknowledge; chips store what is written, the monitor's EDID reads |
//! | `$100800`–`$1008FF` | "Blingboard" keyboard receiver | flags stored; bit 2 of `$100802` reads set when fitted |
//! | `$100900`–`$1009FF` | the Blingboard's LED controller (a second [`LedStrip`], 71 LEDs) | as the LED strip when fitted, else nothing |
//! | `$200000`–`$2000FF` | the FPGA's clock synthesizer (MMCM) | stored |
//!
//! None of these blocks raises an interrupt here: the USB controller
//! never reports (low source 2), the Ethernet MAC never receives (low
//! source 5) and its "sent" line (low source 6) stays low.
//!
//! Sources (facts only): UE2-C64U-Emulator `docs/hw/00-memory-map.md`
//! (map, hazards), `03-board-init.md` (board pins, I2C master and chips,
//! mixers, LED strip, MMCM), `07-sd-card-filesystems.md` (SD registers,
//! seconds timer), `08-network-rmii.md`, `09-usb.md`,
//! `11-drives-iec-periph.md` (GCR codec), `06-spi-flash-config.md` (ICAP),
//! `12-gaps.md` (audio select); 1541 Ultimate
//! `fpga/1541/vhdl_source/gcr_codec.vhd`, `gcr2bin.vhd`,
//! `fpga/ip/clock/vhdl_source/real_time_clock.vhd`,
//! `software/io/i2c/i2c_drv.cc`, `hw_i2c_drv.cc`,
//! `software/u64/u64_config.cc` (what it reads from the EDID),
//! `software/u64/led_strip.cc`, `led_strip.h`, `software/system/u64.h`
//! (LED strip),
//! `software/io/flash/w25q_flash.cc` (the restart sequence),
//! `software/system/product.cc`, `u2p.h`; the EDID's layout from VESA
//! E-EDID 1.3 and CEA-861.

use crate::io::{IoCtx, IoDevice, CLOCK_HZ};
use mimic_core::{StateError, StateReader, StateWriter};

/// Board revision of the Ultimate 64 Elite II as produced in series
/// ("V2.2"). The firmware reads it from bits 7–3 of `$10000C`.
///
/// Others it knows: `$15` early prototype (the two clock generators trade
/// I2C channels), `$16` null series; `$10`–`$14` are boards of the first
/// Ultimate 64. What a Commodore 64 Ultimate reports is not known; the
/// firmware for this board family does not tell the two apart by it.
pub const BOARD_REV_ELITE2: u8 = 0x17;

/// Board revision on which the clock generators trade I2C channels.
const BOARD_REV_PROTO: u8 = 0x15;

/// The I2C master's channels.
pub const I2C_CH_HDMI: u8 = 0;
pub const I2C_CH_1V8: u8 = 1;
pub const I2C_CH_3V3: u8 = 2;

// ---------------------------------------------------------------------
// SD card SPI
// ---------------------------------------------------------------------

/// The SPI port of the SD card slot, `$060000`–`$0600FF`. Four registers
/// of four bytes each (every byte of a register is the register), the
/// four repeating through the page:
///
/// - `+0` data: a write shifts the byte out, a read shifts `$FF` out and
///   returns what came in. No card: the data line idles high, `$FF`.
/// - `+4` clock divider, bits 7–0 (reads `$F4` after reset).
/// - `+8` write: bit 0 take the select line by hand, bit 1 its level;
///   read: bit 0 and 1 as written, bit 2 card detect, bit 3 write protect.
///   Reads 2 after reset: no card. The firmware looks at this register
///   ten times a second and does nothing further without a card.
/// - `+C` read: the CRC-7 of all bytes shifted out since the last write
///   to it, in bits 7–1, bit 0 set; any write clears it.
///
/// Source: `07-sd-card-filesystems.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdSpi {
    rate: u8,
    force_ss: bool,
    level_ss: bool,
    crc7: u8,
}

impl SdSpi {
    fn new() -> Self {
        Self {
            rate: 0xF4,
            force_ss: false,
            level_ss: true,
            crc7: 0,
        }
    }

    /// One byte out; the byte in. Without a card nothing drives the line.
    fn transfer(&mut self, out: u8) -> u8 {
        for bit in (0..8).rev() {
            let fed = (out >> bit) & 1 ^ (self.crc7 >> 6) & 1;
            self.crc7 = (self.crc7 << 1) & 0x7F;
            if fed != 0 {
                self.crc7 ^= 0x09;
            }
        }
        0xFF
    }

    fn read(&mut self, addr: u32) -> u8 {
        if (addr >> 2) & 3 == 0 {
            self.transfer(0xFF)
        } else {
            self.peek(addr)
        }
    }

    fn peek(&self, addr: u32) -> u8 {
        match (addr >> 2) & 3 {
            0 => 0xFF,
            1 => self.rate,
            2 => u8::from(self.force_ss) | u8::from(self.level_ss) << 1,
            _ => self.crc7 << 1 | 1,
        }
    }

    fn write(&mut self, addr: u32, v: u8) {
        match (addr >> 2) & 3 {
            0 => {
                self.transfer(v);
            }
            1 => self.rate = v,
            2 => {
                self.force_ss = v & 1 != 0;
                self.level_ss = v & 2 != 0;
            }
            _ => self.crc7 = 0,
        }
    }
}

// ---------------------------------------------------------------------
// Seconds timer
// ---------------------------------------------------------------------

/// The seconds timer at `$060400`–`$060403`: a 32-bit count of seconds,
/// least significant byte first, which the firmware takes as Unix time
/// and sets the same way. Offsets `+4`–`+F` read 0.
///
/// A write to byte 0 stops the count ("lock") and a write to byte 3
/// starts it again, so a 32-bit store sets the time in one piece. The
/// fraction of the running second is kept through a lock. Without a
/// battery the count starts from a fixed date at power-on; the machine
/// gives the real one with [`Board::set_epoch`].
///
/// Source: `07-sd-card-filesystems.md`, `real_time_clock.vhd`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtcTimer {
    secs: u32,
    /// Clocks into the running second.
    frac: u64,
    /// The clock up to which `secs` and `frac` are counted.
    seen: u64,
    lock: bool,
}

/// The count after power-on: 2025-01-11 16:03:16 UTC.
const RTC_POWER_ON: u32 = 0x6782_9644;

impl RtcTimer {
    fn new() -> Self {
        Self {
            secs: RTC_POWER_ON,
            frac: 0,
            seen: 0,
            lock: false,
        }
    }

    fn advance(&mut self, now: u64) {
        let passed = now.saturating_sub(self.seen);
        self.seen = now;
        if !self.lock {
            let total = self.frac + passed;
            self.secs = self.secs.wrapping_add((total / CLOCK_HZ) as u32);
            self.frac = total % CLOCK_HZ;
        }
    }

    fn peek(&self, addr: u32) -> u8 {
        match addr & 0xF {
            n @ 0..=3 => (self.secs >> (8 * n)) as u8,
            _ => 0,
        }
    }

    fn write(&mut self, addr: u32, v: u8) {
        let n = addr & 0xF;
        if n > 3 {
            return;
        }
        self.secs = (self.secs & !(0xFF << (8 * n))) | u32::from(v) << (8 * n);
        if n == 0 {
            self.lock = true;
        }
        if n == 3 {
            self.lock = false;
        }
    }
}

// ---------------------------------------------------------------------
// GCR codec
// ---------------------------------------------------------------------

/// The 1541 group code: five bits on disk for four bits of data.
const GCR: [u8; 16] = [
    0x0A, 0x0B, 0x12, 0x13, 0x0E, 0x0F, 0x16, 0x17, 0x09, 0x19, 0x1A, 0x1B, 0x0D, 0x1D, 0x1E, 0x15,
];

/// The GCR codec at `$060500`–`$06050F`, with which the firmware turns
/// disk images into the tracks of the 1541 and back.
///
/// Every write, to any of the sixteen addresses, shifts its byte into a
/// 40-bit register from the right. Reads see that register two ways at
/// once:
///
/// - `+0`–`+3`: the 40 bits taken as eight GCR groups of five, decoded
///   to four bytes (first written = first read). A group that is no GCR
///   code decodes to 0.
/// - `+4`–`+7`: one bit for each of those eight groups, bit 7 for the
///   first, set where the group is no GCR code.
/// - `+8`–`+C`: the last four bytes written, encoded to five GCR bytes.
/// - `+D`–`+F`: 0.
///
/// After reset the register holds `$5555555555`.
///
/// Source: `11-drives-iec-periph.md`, `gcr_codec.vhd`, `gcr2bin.vhd`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GcrCodec {
    shift: u64,
}

impl GcrCodec {
    fn new() -> Self {
        Self {
            shift: 0x55_5555_5555,
        }
    }

    fn write(&mut self, v: u8) {
        self.shift = (self.shift << 8 | u64::from(v)) & 0xFF_FFFF_FFFF;
    }

    /// The four decoded bytes and the error bits.
    fn decoded(&self) -> (u32, u8) {
        let mut data = 0u32;
        let mut errors = 0u8;
        for i in 0..8 {
            let group = (self.shift >> (35 - 5 * i)) as u8 & 0x1F;
            let nibble = match GCR.iter().position(|&g| g == group) {
                Some(n) => n as u32,
                None => {
                    errors |= 0x80 >> i;
                    0
                }
            };
            data = data << 4 | nibble;
        }
        (data, errors)
    }

    /// The five encoded bytes, in the low 40 bits.
    fn encoded(&self) -> u64 {
        (0..8).fold(0u64, |acc, i| {
            let nibble = (self.shift >> (28 - 4 * i)) & 0xF;
            acc << 5 | u64::from(GCR[nibble as usize])
        })
    }

    fn peek(&self, addr: u32) -> u8 {
        match addr & 0xF {
            n @ 0..=3 => (self.decoded().0 >> (24 - 8 * n)) as u8,
            4..=7 => self.decoded().1,
            n @ 8..=0xC => (self.encoded() >> (32 - 8 * (n - 8))) as u8,
            _ => 0,
        }
    }
}

// ---------------------------------------------------------------------
// ICAP
// ---------------------------------------------------------------------

/// The FPGA's own configuration port at `$060600`: `+4` a strobe, `+8`
/// one byte of a configuration command stream. Reads give 0.
///
/// The firmware uses it for one thing: to make the FPGA load its
/// configuration again from an address in the flash, which restarts the
/// whole board. It writes two strobes, twenty bytes (the synchronisation
/// word `AA 99`, the flash address in two register writes, the command
/// that starts the reload), two strobes. The reload command is what
/// [`Board::take_reconfigure`] reports.
///
/// Source: `06-spi-flash-config.md`, `w25q_flash.cc`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Icap {
    /// The command bytes since the last synchronisation word, at most
    /// [`ICAP_KEEP`].
    stream: Vec<u8>,
    pulses: u32,
    reconfigure: bool,
    address: u32,
}

const ICAP_KEEP: usize = 64;

impl Icap {
    fn new() -> Self {
        Self {
            stream: Vec::new(),
            pulses: 0,
            reconfigure: false,
            address: 0,
        }
    }

    fn write(&mut self, addr: u32, v: u8) {
        match addr & 0xF {
            4 => self.pulses = self.pulses.wrapping_add(1),
            8 => {
                if self.stream.len() >= ICAP_KEEP {
                    self.stream.remove(0);
                }
                self.stream.push(v);
                self.decode();
            }
            _ => {}
        }
    }

    /// Look for "write the command register: reload" at the end of the
    /// stream; the words are 16 bits, high byte first.
    fn decode(&mut self) {
        let s = &self.stream;
        let Some(sync) = s.windows(2).rposition(|w| w == [0xAA, 0x99]) else {
            return;
        };
        let t = &s[sync..];
        let words: Vec<u16> = (0..t.len() / 2)
            .map(|k| u16::from(t[2 * k]) << 8 | u16::from(t[2 * k + 1]))
            .collect();
        let n = words.len();
        if (s.len() - sync) & 1 != 0 || n < 3 {
            return;
        }
        // $30A1: one word to the command register; $000E (and $000F on
        // the newer FPGA family): reload.
        if words[n - 2] != 0x30A1 || !matches!(words[n - 1], 0x000E | 0x000F) {
            return;
        }
        // $3261: low 16 bits of the start address; $3281: the flash's
        // read command in the high byte, address bits 23–16 in the low.
        let mut address = 0u32;
        for pair in words.windows(2) {
            match pair[0] {
                0x3261 => address = (address & 0xFF_0000) | u32::from(pair[1]),
                0x3281 => address = (address & 0x00_FFFF) | u32::from(pair[1] & 0xFF) << 16,
                _ => {}
            }
        }
        self.address = address;
        self.reconfigure = true;
        self.stream.clear();
    }
}

// ---------------------------------------------------------------------
// Board pins
// ---------------------------------------------------------------------

/// Single pins of the board at `$100000`–`$10000F`:
///
/// - `$100006` read: the Ethernet PHY's MDIO line ([`crate::rmii::MdioPhy`]).
///   Idle it is high.
/// - `$100008`/`$100009`: I2C by hand, unused on this board; stored.
/// - `$10000A`/`$10000B` write: MDC and MDIO levels.
/// - `$10000C` read: board revision in bits 7–3, bits 2–0 zero. Write:
///   speaker enable. The firmware writes `$FF` here, so what is read must
///   not follow what was written.
/// - `$10000D`: bit 0 holds the USB hub in reset; read back (the
///   firmware reads it only to pass time).
/// - `$10000F`: bit 0 holds the USB transceiver in reset, bit 7 enables
///   a buffer; stored, reads back.
///
/// Source: `03-board-init.md`, `00-memory-map.md` (M1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct U2pIo {
    revision: u8,
    /// What was written to offsets 0–15.
    written: [u8; 16],
    /// The Ethernet PHY behind the MDC and MDIO pins.
    pub phy: crate::rmii::MdioPhy,
}

impl U2pIo {
    fn new(revision: u8) -> Self {
        Self {
            revision: revision & 0x1F,
            written: [0; 16],
            phy: crate::rmii::MdioPhy::default(),
        }
    }

    fn peek(&self, addr: u32) -> u8 {
        match addr & 0xFF {
            0x06 | 0x07 | 0x0B => self.phy.get_mdio(),
            0x0A => u8::from(self.phy.mdc()),
            0x08 | 0x09 | 0x0F => self.written[(addr & 0xF) as usize],
            0x0C => self.revision << 3,
            0x0D => self.written[0x0D] & 1,
            _ => 0,
        }
    }

    fn write(&mut self, addr: u32, v: u8, link_up: bool) {
        match addr & 0xFF {
            0x0A => self.phy.set_mdc(v & 1 != 0, link_up),
            0x0B => self.phy.set_mdio(v & 1 != 0),
            _ => {}
        }
        if addr & 0xFF < 0x10 {
            self.written[(addr & 0xF) as usize] = v;
        }
    }
}

// ---------------------------------------------------------------------
// Mixers and resampler
// ---------------------------------------------------------------------

/// Audio mixer `$100500`–`$100513`, speaker mixer `$100540`–`$100553`
/// and the resampler's filter loader `$100580`–`$100589`. Write only:
/// everything reads 0.
///
/// Each mixer has ten inputs of two bytes, a gain for the right and one
/// for the left channel: UltiSID 1 and 2, SID socket 1 and 2, sampler
/// left and right, drive 1 and 2, tape read and write.
///
/// Resampler: `$100580` a 32-bit coefficient per store (675 for PAL, 512
/// for NTSC), `$100584` reset, `$100588` a filter length ("labor"),
/// `$100589` 1 while loading. Only the count of coefficients is kept.
///
/// Source: `03-board-init.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mixer {
    pub audio: [u8; 20],
    pub speaker: [u8; 20],
    /// Coefficients stored since the last reset of the resampler.
    pub resample_words: u32,
    pub resample_labor: u8,
    pub resample_flush: u8,
}

impl Mixer {
    fn new() -> Self {
        Self {
            audio: [0; 20],
            speaker: [0; 20],
            resample_words: 0,
            resample_labor: 0,
            resample_flush: 0,
        }
    }

    fn write(&mut self, addr: u32, v: u8) {
        match addr & 0xFF {
            n @ 0x00..=0x13 => self.audio[n as usize] = v,
            n @ 0x40..=0x53 => self.speaker[(n - 0x40) as usize] = v,
            // The last byte of a 32-bit store completes a coefficient.
            0x83 => self.resample_words = self.resample_words.wrapping_add(1),
            0x84 => self.resample_words = 0,
            0x88 => self.resample_labor = v,
            0x89 => self.resample_flush = v,
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------
// LED strip
// ---------------------------------------------------------------------

/// LEDs the board's LED strip controller drives.
pub const LEDS: usize = 84;
/// LEDs the Blingboard's controller drives (under the keyboard of a
/// Commodore 64 Ultimate).
pub const BLING_LEDS: usize = 71;
/// Colour bytes a controller's registers hold.
const LED_BYTES: usize = 252;

/// The chip type the firmware set the controller to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedProtocol {
    /// Not set yet (the map register never held `$40` or `$80`).
    Unset,
    /// WS2812: one wire, no brightness field (map register `$40`).
    Ws2812,
    /// APA102: clock and data, 5 bits of brightness per LED (`$80`).
    Apa102,
}

/// An LED strip controller and the strip behind it. The board has one
/// at `$100600`–`$1006FF` for 84 LEDs of three colours each; the
/// Blingboard of a Commodore 64 Ultimate adds a second of the same kind
/// at `$100900`–`$1009FF` for the 71 LEDs under its keyboard. Write only:
/// everything reads 0.
///
/// - `$xx00`–`$xxFB`: colour bytes, red, green, blue per LED (the
///   controller puts them into the order the LED chip wants); while bit 0
///   of the map register is set, the writes go to the address map instead.
/// - `$xxFC`: map register: 1 map writing on, 0 off; afterwards Gideon's
///   firmware stores the strip's chip type here: `$40` WS2812, `$80`
///   APA102. Commodore's leaves 0 in the board's and 2 in the
///   Blingboard's (meaning not known).
/// - `$xxFD`: start offset, added to every map entry.
/// - `$xxFE`: brightness, 0–31 in bits 6–2.
/// - `$xxFF`: any write sends the strip a frame.
///
/// The map has one entry per colour byte sent: entry `3k+c` says which
/// colour byte LED `k` gets for its channel `c` (0 red, 1 green, 2 blue).
/// The start offset is added, wrapping after the last LED's bytes (252
/// for 84 LEDs, 213 for 71): so the firmware scrolls a pattern by writing
/// one new colour and moving the offset, and lights all LEDs alike with a
/// map of 0, 1, 2 throughout.
///
/// On an Ultimate 64 Elite II a frame of the board's controller reaches
/// the strip only while the board's pins are switched to it (LEDSTRIP_EN
/// of the U64 I/O page, [`Board::set_led_enable`]: the strip shares the
/// user port's PWM pins); the LEDs keep their last colours otherwise, as
/// the LED chips do. On a Commodore 64 Ultimate every frame counts:
/// Commodore's firmware plays its start animation on the case lights with
/// that register still 0, so the lights there are taken to have pins of
/// their own (an assumption from that observation), as the Blingboard's
/// LEDs have.
///
/// The board's LEDs: on a Commodore 64 Ultimate 30 along the case's
/// bottom, 30 along its top and 24 at the keyboard, each from left to
/// right, in this order; on an Ultimate 64 Elite II whatever strip is
/// plugged in (the firmware's setting allows 1 to 40 LEDs, 24 by default).
///
/// Sources: `03-board-init.md`; 1541 Ultimate `software/u64/led_strip.cc`
/// (order of the bytes, chip types, what the firmware expects of map and
/// offset, the LEDs' places). The second controller, its 71 LEDs and
/// where its offset wraps: seen in what Commodore's firmware 1.2.1 writes
/// (2026-11-23). The controllers' VHDL is not published: that the sum
/// wraps with the LED count, that only bit 0 of the map register gates
/// map writing and how brightness scales the colours of a chip without a
/// brightness field (here linearly, n/31) are assumptions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedStrip {
    pub data: [u8; LED_BYTES],
    pub map: [u8; LED_BYTES],
    pub map_reg: u8,
    pub start: u8,
    pub intensity: u8,
    /// Frames sent since reset.
    pub frames: u32,
    /// The pins lead to the strip.
    output: bool,
    /// What the strip shows: the colour bytes of the last frame that
    /// reached it, brightness applied.
    shown: [u8; LED_BYTES],
    /// LEDs on the strip (configuration).
    count: usize,
}

impl LedStrip {
    fn new(count: usize) -> Self {
        Self {
            count,
            data: [0; LED_BYTES],
            map: [0; LED_BYTES],
            map_reg: 0,
            start: 0,
            intensity: 0,
            frames: 0,
            output: false,
            shown: [0; LED_BYTES],
        }
    }

    /// The Blingboard's: its pins always lead to its LEDs.
    fn bling() -> Self {
        Self {
            output: true,
            ..Self::new(BLING_LEDS)
        }
    }

    /// The three colour bytes (red, green, blue) the controller would
    /// send LED `k` now, before brightness.
    pub fn color(&self, k: usize) -> [u8; 3] {
        let ring = 3 * self.count;
        let mut c = [0; 3];
        for (i, out) in c.iter_mut().enumerate() {
            let m = usize::from(self.map[(3 * k + i) % ring]);
            *out = self.data[(m + usize::from(self.start)) % ring];
        }
        c
    }

    /// LEDs on the strip.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Brightness as set, 0–31.
    pub fn brightness(&self) -> u8 {
        (self.intensity >> 2) & 0x1F
    }

    /// The chip type the firmware chose.
    pub fn protocol(&self) -> LedProtocol {
        match self.map_reg & 0xC0 {
            0x40 => LedProtocol::Ws2812,
            0x80 => LedProtocol::Apa102,
            _ => LedProtocol::Unset,
        }
    }

    /// Lead the controller's pins to the strip or away from it. The strip
    /// keeps what it shows.
    pub fn set_output(&mut self, on: bool) {
        self.output = on;
    }

    /// The pins lead to the strip: frames sent reach it.
    pub fn enabled(&self) -> bool {
        self.output
    }

    /// What the strip's LEDs show: red, green, blue as the light they
    /// give (0–255, linear), as of the last frame that reached the strip.
    /// All dark after power-on.
    pub fn leds(&self) -> Vec<[u8; 3]> {
        (0..self.count)
            .map(|k| {
                [
                    self.shown[3 * k],
                    self.shown[3 * k + 1],
                    self.shown[3 * k + 2],
                ]
            })
            .collect()
    }

    /// Some LED is lit.
    pub fn lit(&self) -> bool {
        self.shown.iter().any(|&b| b != 0)
    }

    /// Power returned: the LED chips start dark. (The FPGA's reset alone
    /// does not reach them.)
    pub fn power_on(&mut self) {
        self.shown = [0; LED_BYTES];
    }

    /// Bytes of the map as LED numbers, for looking at a pattern: which
    /// LED's colour (counted from the start offset) each LED shows.
    pub fn pattern(&self) -> Vec<u8> {
        (0..self.count).map(|k| self.map[3 * k] / 3).collect()
    }

    fn send(&mut self) {
        self.frames = self.frames.wrapping_add(1);
        if !self.output {
            return;
        }
        let level = u16::from(self.brightness());
        for k in 0..self.count {
            let c = self.color(k);
            for (i, &v) in c.iter().enumerate() {
                self.shown[3 * k + i] = ((u16::from(v) * level + 15) / 31) as u8;
            }
        }
    }

    fn write(&mut self, addr: u32, v: u8) {
        match addr & 0xFF {
            n @ 0..=0xFB => {
                if self.map_reg & 1 != 0 {
                    self.map[n as usize] = v;
                } else {
                    self.data[n as usize] = v;
                }
            }
            0xFC => self.map_reg = v,
            0xFD => self.start = v,
            0xFE => self.intensity = v,
            _ => self.send(),
        }
    }
}

// ---------------------------------------------------------------------
// I2C master and the chips on its buses
// ---------------------------------------------------------------------

/// Whom the running I2C transfer addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    None,
    Codec,
    Expander8,
    Expander16,
    /// The clock generator on the HDMI channel.
    PllCh0,
    /// The clock generator on the 1.8 V channel.
    PllCh1,
    Hub,
    Edid,
    Segment,
}

impl Target {
    const ALL: [Target; 9] = [
        Target::None,
        Target::Codec,
        Target::Expander8,
        Target::Expander16,
        Target::PllCh0,
        Target::PllCh1,
        Target::Hub,
        Target::Edid,
        Target::Segment,
    ];
}

/// The I2C master at `$100700`–`$100708` and the chips it reaches.
///
/// | Address | Write | Read |
/// |---|---|---|
/// | `$100700` | send a byte, after a start condition if none is out | the shift register: the byte last received, `$FF` after sending |
/// | `$100701` | – | status: bit 0 a start is out, bit 2 the last byte was not acknowledged, bit 3 a command came while busy, bit 7 busy. Reading clears bits 2 and 3 |
/// | `$100702` | repeated start | 0 |
/// | `$100703` | stop | 0 |
/// | `$100704` | receive a byte, do not acknowledge it (the last one) | the shift register |
/// | `$100705` | receive a byte and acknowledge it | the shift register |
/// | `$100706` | bits 1–0: the bus | the same |
/// | `$100707` | bit 0: reset the master | 0 |
/// | `$100708` | bit 0: lend the 1.8 V bus to the FPGA's keyboard scanner | 0 |
///
/// Every command completes within its own bus access, so the busy bit is
/// never seen set; the firmware waits for it without a time limit.
///
/// The buses (the address is the 7-bit one; the firmware writes it
/// doubled, with bit 0 for reading):
///
/// - 0, HDMI: the monitor's EDID memory at `$50` and its segment
///   register at `$30`, both only while a monitor is connected
///   ([`Board::set_monitor`]); a clock generator at `$64`.
/// - 1, 1.8 V: audio codec at `$1A` (two bytes per write: a 7-bit
///   register number and nine bits of data), an 8-bit port expander at
///   `$20` (SID socket supplies), a 16-bit one at `$21` (keyboard
///   matrix), a clock generator at `$64`.
/// - 2, 3.3 V: the USB hub at `$2C`.
/// - 3: nothing.
///
/// Codec, expanders, clock generators and hub acknowledge everything and
/// keep what is written to them; none of them acts on it. An address no
/// chip has is not acknowledged.
///
/// Source: `03-board-init.md`; transfers as `i2c_drv.cc` makes them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct I2c {
    started: bool,
    error: bool,
    channel: u8,
    scan_enable: bool,
    data_out: u8,
    /// The next byte sent is an address.
    address_next: bool,
    target: Target,
    reading: bool,
    /// Bytes written to the target in this transfer, up to 2.
    count: u8,
    /// Codec: the first byte of a write. Clock generator: bit 7 of its
    /// command byte (a single register and not a block).
    first: u8,
    codec: [u16; 128],
    expander8: [u8; 4],
    expander8_ptr: u8,
    expander16: [u8; 8],
    expander16_ptr: u8,
    pll: [[u8; 128]; 2],
    pll_ptr: [u8; 2],
    hub: [u8; 256],
    hub_ptr: u8,
    /// The monitor's EDID, a multiple of 128 bytes; empty: no monitor.
    edid: Vec<u8>,
    edid_ptr: u8,
    edid_segment: u8,
}

/// The longest EDID kept: 256 blocks.
const EDID_MAX: usize = 128 * 256;

impl I2c {
    fn new() -> Self {
        Self {
            started: false,
            error: false,
            channel: 0,
            scan_enable: false,
            data_out: 0,
            address_next: false,
            target: Target::None,
            reading: false,
            count: 0,
            first: 0,
            codec: [0; 128],
            expander8: [0; 4],
            expander8_ptr: 0,
            expander16: [0; 8],
            expander16_ptr: 0,
            pll: [[0; 128]; 2],
            pll_ptr: [0; 2],
            hub: [0; 256],
            hub_ptr: 0,
            edid: default_edid(),
            edid_ptr: 0,
            edid_segment: 0,
        }
    }

    /// The master's reset: the buses' chips keep what they hold.
    fn reset_master(&mut self) {
        self.started = false;
        self.error = false;
        self.channel = 0;
        self.scan_enable = false;
        self.data_out = 0;
        self.end_transfer();
    }

    fn end_transfer(&mut self) {
        self.address_next = false;
        self.target = Target::None;
        self.reading = false;
        self.count = 0;
        // The EDID's segment register falls back to 0 with every stop.
        self.edid_segment = 0;
    }

    fn peek(&self, addr: u32) -> u8 {
        match addr & 0xFF {
            0 | 4 | 5 => self.data_out,
            1 => u8::from(self.started) | u8::from(self.error) << 2,
            6 => self.channel,
            _ => 0,
        }
    }

    fn read(&mut self, addr: u32) -> u8 {
        let v = self.peek(addr);
        if addr & 0xFF == 1 {
            self.error = false;
        }
        v
    }

    fn write(&mut self, addr: u32, v: u8) {
        match addr & 0xFF {
            0 => {
                if !self.started {
                    self.started = true;
                    self.address_next = true;
                }
                let ack = self.send(v);
                self.error = !ack;
                // Sending shifts ones into the register.
                self.data_out = 0xFF;
            }
            2 => {
                self.started = true;
                self.address_next = true;
                self.reading = false;
                self.count = 0;
            }
            3 => {
                self.started = false;
                self.end_transfer();
            }
            4 | 5 => self.data_out = self.receive(),
            6 => self.channel = v & 3,
            7 => {
                if v & 1 != 0 {
                    self.reset_master();
                }
            }
            8 => self.scan_enable = v & 1 != 0,
            _ => {}
        }
    }

    /// Who answers to a 7-bit address on the selected bus.
    fn resolve(&self, addr7: u8) -> Target {
        let monitor = !self.edid.is_empty();
        match (self.channel, addr7) {
            (I2C_CH_HDMI, 0x50) if monitor => Target::Edid,
            (I2C_CH_HDMI, 0x30) if monitor => Target::Segment,
            (I2C_CH_HDMI, 0x64) => Target::PllCh0,
            (I2C_CH_1V8, 0x1A) => Target::Codec,
            (I2C_CH_1V8, 0x20) => Target::Expander8,
            (I2C_CH_1V8, 0x21) => Target::Expander16,
            (I2C_CH_1V8, 0x64) => Target::PllCh1,
            (I2C_CH_3V3, 0x2C) => Target::Hub,
            _ => Target::None,
        }
    }

    /// One byte from the master; `true`: acknowledged.
    fn send(&mut self, v: u8) -> bool {
        if self.address_next {
            self.address_next = false;
            self.target = self.resolve(v >> 1);
            self.reading = v & 1 != 0;
            self.count = 0;
            return self.target != Target::None;
        }
        if self.reading {
            return false;
        }
        let n = self.count;
        self.count = (n + 1).min(2);
        match self.target {
            Target::None => return false,
            Target::Codec => match n {
                0 => self.first = v,
                1 => {
                    let reg = usize::from(self.first >> 1);
                    if reg == 0 {
                        // Register 0 is the codec's reset.
                        self.codec = [0; 128];
                    } else {
                        self.codec[reg] = u16::from(self.first & 1) << 8 | u16::from(v);
                    }
                }
                _ => {}
            },
            Target::Expander8 => {
                if n == 0 {
                    self.expander8_ptr = v & 3;
                } else {
                    self.expander8[usize::from(self.expander8_ptr)] = v;
                }
            }
            Target::Expander16 => {
                if n == 0 {
                    self.expander16_ptr = v & 7;
                } else {
                    self.expander16[usize::from(self.expander16_ptr)] = v;
                }
            }
            Target::PllCh0 | Target::PllCh1 => {
                let i = usize::from(self.target == Target::PllCh1);
                match n {
                    // The command byte: a register number; with bit 7 a
                    // single register follows, without it a block.
                    0 => {
                        self.first = v & 0x80;
                        self.pll_ptr[i] = v & 0x7F;
                    }
                    // A block starts with its length.
                    1 if self.first == 0 => {}
                    _ => {
                        let p = self.pll_ptr[i];
                        self.pll[i][usize::from(p)] = v;
                        self.pll_ptr[i] = (p + 1) & 0x7F;
                    }
                }
            }
            Target::Hub => match n {
                0 => self.hub_ptr = v,
                // Every write to the hub is a block: its length.
                1 => {}
                _ => {
                    self.hub[usize::from(self.hub_ptr)] = v;
                    self.hub_ptr = self.hub_ptr.wrapping_add(1);
                }
            },
            Target::Edid => {
                // The memory takes its read address and nothing else.
                if n == 0 {
                    self.edid_ptr = v;
                }
            }
            Target::Segment => self.edid_segment = v,
        }
        true
    }

    /// One byte from the addressed chip; nobody there: the idle bus.
    fn receive(&mut self) -> u8 {
        if !self.reading {
            return 0xFF;
        }
        match self.target {
            Target::Edid => {
                let at = usize::from(self.edid_segment) << 8 | usize::from(self.edid_ptr);
                self.edid_ptr = self.edid_ptr.wrapping_add(1);
                self.edid.get(at).copied().unwrap_or(0xFF)
            }
            // The expanders' input registers: nothing pulls a line low.
            Target::Expander8 => match self.expander8_ptr {
                0 => 0xFF,
                p => self.expander8[usize::from(p)],
            },
            Target::Expander16 => match self.expander16_ptr {
                0 | 1 => 0xFF,
                p => self.expander16[usize::from(p)],
            },
            Target::PllCh0 | Target::PllCh1 => {
                let i = usize::from(self.target == Target::PllCh1);
                let p = self.pll_ptr[i];
                self.pll_ptr[i] = (p + 1) & 0x7F;
                self.pll[i][usize::from(p)]
            }
            Target::Hub => {
                let p = self.hub_ptr;
                self.hub_ptr = p.wrapping_add(1);
                self.hub[usize::from(p)]
            }
            Target::None | Target::Codec | Target::Segment => 0xFF,
        }
    }
}

/// An 18-byte detailed timing of an EDID. Lengths in pixels and lines,
/// the clock in units of 10 kHz.
struct Timing {
    clock: u16,
    h_active: u16,
    h_blank: u16,
    v_active: u16,
    v_blank: u16,
    h_sync_offset: u16,
    h_sync_width: u16,
    v_sync_offset: u16,
    v_sync_width: u16,
    /// Bits 2 and 1: positive vertical and horizontal sync.
    flags: u8,
}

impl Timing {
    fn bytes(&self, width_mm: u16, height_mm: u16) -> [u8; 18] {
        let lo = |v: u16| v as u8;
        let hi = |v: u16| (v >> 8) as u8 & 0x0F;
        [
            lo(self.clock),
            (self.clock >> 8) as u8,
            lo(self.h_active),
            lo(self.h_blank),
            hi(self.h_active) << 4 | hi(self.h_blank),
            lo(self.v_active),
            lo(self.v_blank),
            hi(self.v_active) << 4 | hi(self.v_blank),
            lo(self.h_sync_offset),
            lo(self.h_sync_width),
            (lo(self.v_sync_offset) & 0x0F) << 4 | (lo(self.v_sync_width) & 0x0F),
            (hi(self.h_sync_offset) & 3) << 6
                | (hi(self.h_sync_width) & 3) << 4
                | ((self.v_sync_offset >> 4) as u8 & 3) << 2
                | ((self.v_sync_width >> 4) as u8 & 3),
            lo(width_mm),
            lo(height_mm),
            hi(width_mm) << 4 | hi(height_mm),
            0,
            0,
            // Digital separate sync.
            0x18 | self.flags,
        ]
    }
}

const T_1080P60: Timing = Timing {
    clock: 14850,
    h_active: 1920,
    h_blank: 280,
    v_active: 1080,
    v_blank: 45,
    h_sync_offset: 88,
    h_sync_width: 44,
    v_sync_offset: 4,
    v_sync_width: 5,
    flags: 0x06,
};

const T_1080P50: Timing = Timing {
    clock: 14850,
    h_active: 1920,
    h_blank: 720,
    v_active: 1080,
    v_blank: 45,
    h_sync_offset: 528,
    h_sync_width: 44,
    v_sync_offset: 4,
    v_sync_width: 5,
    flags: 0x06,
};

const T_576P50: Timing = Timing {
    clock: 2700,
    h_active: 720,
    h_blank: 144,
    v_active: 576,
    v_blank: 49,
    h_sync_offset: 12,
    h_sync_width: 64,
    v_sync_offset: 5,
    v_sync_width: 5,
    flags: 0x00,
};

/// Make the last byte of a 128-byte block bring its sum to 0.
fn seal(block: &mut [u8]) {
    let sum = block[..127].iter().fold(0u8, |a, &b| a.wrapping_add(b));
    block[127] = sum.wrapping_neg();
}

/// The EDID of the monitor that is connected unless the machine says
/// otherwise: an HDMI monitor with stereo sound, 1920×1080 at 60 Hz
/// preferred, also 1920×1080 at 50 Hz, 1280×720 at 50 and 60 Hz, 720×576
/// at 50 Hz, 720×480 and 640×480 at 60 Hz. Two blocks of 128 bytes: the
/// base block (EDID 1.3) and a CEA-861 extension, in which the firmware
/// looks for the HDMI vendor block to choose HDMI over DVI.
pub fn default_edid() -> Vec<u8> {
    const WIDTH_MM: u16 = 531;
    const HEIGHT_MM: u16 = 299;
    let mut e = vec![0u8; 256];
    e[..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
    // Manufacturer "MIM" (three letters of five bits, A = 1), product 1,
    // serial number 1, made in week 1 of 2026 (years count from 1990).
    let letters = [13u16, 9, 13];
    let id = letters[0] << 10 | letters[1] << 5 | letters[2];
    e[8] = (id >> 8) as u8;
    e[9] = id as u8;
    e[10] = 1;
    e[12] = 1;
    e[16] = 1;
    e[17] = 36;
    // EDID 1.3; digital input; 53 × 30 cm; gamma 2.2; RGB colour, the
    // first detailed timing is the preferred one.
    e[18] = 1;
    e[19] = 3;
    e[20] = 0x80;
    e[21] = 53;
    e[22] = 30;
    e[23] = 120;
    e[24] = 0x0A;
    // Colour points of sRGB, ten bits each (x and y of red, green, blue,
    // white): the two low bits packed in bytes 25 and 26, the high eight
    // in 27 to 34.
    let points = [655u16, 338, 307, 614, 154, 61, 320, 337];
    for (i, p) in points.iter().enumerate() {
        e[25 + i / 4] |= (*p as u8 & 3) << (6 - 2 * (i % 4));
        e[27 + i] = (p >> 2) as u8;
    }
    // Established timings: 640×480 and 800×600 at 60 Hz, 1024×768 at 60.
    e[35] = 0x21;
    e[36] = 0x08;
    // Standard timings (width / 8 − 31, aspect and rate − 60):
    // 1280×720, 1920×1080 (16:9), 1280×1024 (5:4), all at 60 Hz; the
    // rest unused.
    let standard = [[0x81, 0xC0], [0xD1, 0xC0], [0x81, 0x80]];
    for i in 0..8 {
        let pair = standard.get(i).copied().unwrap_or([1, 1]);
        e[38 + 2 * i..40 + 2 * i].copy_from_slice(&pair);
    }
    e[54..72].copy_from_slice(&T_1080P60.bytes(WIDTH_MM, HEIGHT_MM));
    e[72..90].copy_from_slice(&T_1080P50.bytes(WIDTH_MM, HEIGHT_MM));
    // Range limits: 49–61 Hz vertical, 15–68 kHz horizontal, pixel clock
    // up to 150 MHz, no secondary timing formula.
    e[90..108].copy_from_slice(&[
        0, 0, 0, 0xFD, 0, 49, 61, 15, 68, 15, 0, 0x0A, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20,
    ]);
    // The monitor's name, ended by a line feed and padded with blanks.
    e[108..113].copy_from_slice(&[0, 0, 0, 0xFC, 0]);
    let mut name = [0x20u8; 13];
    name[..11].copy_from_slice(b"mimic HDMI\n");
    e[113..126].copy_from_slice(&name);
    // One extension block follows.
    e[126] = 1;
    seal(&mut e[..128]);

    // CEA-861 extension, revision 3; basic audio.
    let x = &mut e[128..];
    x[0] = 0x02;
    x[1] = 0x03;
    x[3] = 0x40;
    // Data blocks, each headed by type (bits 7–5) and length (bits 4–0).
    let blocks: [&[u8]; 4] = [
        // Video: formats 16 (1080p60, native), 31 (1080p50), 4 (720p60),
        // 19 (720p50), 17 (576p50), 2 (480p60), 1 (640×480).
        &[0x47, 0x90, 31, 4, 19, 17, 2, 1],
        // Audio: PCM, two channels; 32, 44.1 and 48 kHz; 16, 20, 24 bits.
        &[0x23, 0x09, 0x07, 0x07],
        // Speakers: front left and right.
        &[0x83, 0x01, 0x00, 0x00],
        // Vendor block of HDMI (identifier 00-0C-03, low byte first),
        // source address 1.0.0.0.
        &[0x65, 0x03, 0x0C, 0x00, 0x10, 0x00],
    ];
    let mut at = 4;
    for b in blocks {
        x[at..at + b.len()].copy_from_slice(b);
        at += b.len();
    }
    // Where the detailed timings begin.
    x[2] = at as u8;
    x[at..at + 18].copy_from_slice(&T_576P50.bytes(WIDTH_MM, HEIGHT_MM));
    seal(x);
    e
}

// ---------------------------------------------------------------------
// The board
// ---------------------------------------------------------------------

/// All the blocks of this module behind one [`IoDevice`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Board {
    pub sd: SdSpi,
    pub rtc: RtcTimer,
    pub gcr: GcrCodec,
    pub icap: Icap,
    pub u2p: U2pIo,
    pub mixer: Mixer,
    pub led: LedStrip,
    /// The Blingboard's LED controller; nothing reaches it without the
    /// Blingboard.
    pub bling_led: LedStrip,
    pub i2c: I2c,
    /// Bytes last written to `$060304` (profiler: subsystem) and
    /// `$060305` (profiler: task), which the firmware stores at every
    /// task switch. Read 0.
    profiler: [u8; 2],
    /// Audio select `$060700`/`$060701` (repeating every 16 bytes): the
    /// source of the left and of the right analog output, four bits each,
    /// which read back.
    audio_select: [u8; 2],
    /// Ethernet MAC `$060800`–`$06083F`: what was written. All reads are
    /// 0, which says: transmitter free (`$06081A`), no packet received
    /// (`$06082F`).
    rmii: [u8; 0x40],
    /// The USB controller's memory, `$080000`–`$0807FF`.
    usb_ram: Vec<u8>,
    /// Bit 0 of the last write to `$080800`–`$080FFF`: the controller's
    /// processor is let run. Reads 0.
    usb_start: bool,
    /// Blingboard receiver flags as written (`$100802`). Reads 0: no
    /// Blingboard (bit 2).
    bling_flags: u8,
    /// The Blingboard (LED and keyboard controller) is fitted: the
    /// Commodore 64 Ultimate has it, and Commodore's firmware takes its
    /// presence as the sign of a Commodore board. Configuration.
    blingboard: bool,
    /// The clock synthesizer's registers, `$200000`–`$2000FF`: 16 bits
    /// each at twice their number, and the reset register at `$2000FF`.
    /// Read 0.
    mmcm: [u8; 256],
}

const USB_RAM: usize = 0x800;

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

impl Board {
    /// An Ultimate 64 Elite II of the series ([`BOARD_REV_ELITE2`]) with
    /// the default monitor connected.
    pub fn new() -> Self {
        Self::with_revision(BOARD_REV_ELITE2)
    }

    /// A board of another revision (five bits).
    pub fn with_revision(revision: u8) -> Self {
        Self {
            sd: SdSpi::new(),
            rtc: RtcTimer::new(),
            gcr: GcrCodec::new(),
            icap: Icap::new(),
            u2p: U2pIo::new(revision),
            mixer: Mixer::new(),
            led: LedStrip::new(LEDS),
            bling_led: LedStrip::bling(),
            i2c: I2c::new(),
            profiler: [0; 2],
            audio_select: [0; 2],
            rmii: [0; 0x40],
            usb_ram: vec![0; USB_RAM],
            usb_start: false,
            bling_flags: 0,
            blingboard: false,
            mmcm: [0; 256],
        }
    }

    /// The I/O addresses of this block.
    pub fn claims(addr: u32) -> bool {
        matches!(
            addr,
            0x06_0000..=0x06_01FF
                | 0x06_0300..=0x06_08FF
                | 0x08_0000..=0x08_0FFF
                | 0x10_0000..=0x10_02FF
                | 0x10_0500..=0x10_09FF
                | 0x20_0000..=0x20_00FF
        )
    }

    pub fn revision(&self) -> u8 {
        self.u2p.revision
    }

    /// Fit or remove the Blingboard (`$100802` bit 2 reads set with it).
    /// UE2-C64U-Emulator `docs/status/install.md` (option `--board`):
    /// Commodore's firmware reads it to tell its own board.
    pub fn set_blingboard(&mut self, fitted: bool) {
        self.blingboard = fitted;
        self.led.output |= fitted;
    }

    /// LEDSTRIP_EN of the U64 I/O page, bit 0: the user port's PWM pins
    /// go to the LED strip. A board with the Blingboard (a Commodore 64
    /// Ultimate) reaches its case lights regardless, see [`LedStrip`].
    pub fn set_led_enable(&mut self, on: bool) {
        self.led.set_output(on || self.blingboard);
    }

    pub fn blingboard(&self) -> bool {
        self.blingboard
    }

    /// Set the seconds timer to a time of day (Unix seconds); it runs on
    /// from there with the board's clock. The fraction of the second
    /// starts anew.
    pub fn set_epoch(&mut self, unix_secs: u32) {
        self.rtc.secs = unix_secs;
        self.rtc.frac = 0;
    }

    /// The seconds timer's count as of the last access to it.
    pub fn epoch(&self) -> u32 {
        self.rtc.secs
    }

    /// Connect a monitor with this EDID (blocks of 128 bytes; a ragged
    /// end is cut off), or remove the monitor with `None`. The hot-plug
    /// line itself is in the U64 I/O page; the machine keeps the two in
    /// step.
    pub fn set_monitor(&mut self, edid: Option<&[u8]>) {
        let e = edid.unwrap_or(&[]);
        let n = (e.len() / 128 * 128).min(EDID_MAX);
        self.i2c.edid = e[..n].to_vec();
    }

    pub fn monitor(&self) -> bool {
        !self.i2c.edid.is_empty()
    }

    /// The firmware asked the FPGA to load its configuration anew, which
    /// restarts the board. Reported once.
    pub fn take_reconfigure(&mut self) -> bool {
        std::mem::take(&mut self.icap.reconfigure)
    }

    /// The flash address the last reconfiguration request named.
    pub fn reconfigure_address(&self) -> u32 {
        self.icap.address
    }

    /// The firmware has let the USB controller's processor run.
    pub fn usb_started(&self) -> bool {
        self.usb_start
    }

    /// The USB controller's 2 KB of memory.
    pub fn usb_ram(&self) -> &[u8] {
        &self.usb_ram
    }

    /// The USB hub has been told to attach (bit 0 of its register `$FF`).
    pub fn usb_hub_attached(&self) -> bool {
        self.i2c.hub[0xFF] & 1 != 0
    }

    /// The hub is held in reset, the USB transceiver is held in reset.
    pub fn usb_resets(&self) -> (bool, bool) {
        (
            self.u2p.written[0x0D] & 1 != 0,
            self.u2p.written[0x0F] & 1 != 0,
        )
    }

    /// The speaker enable as written to `$10000C`.
    pub fn speaker_enable(&self) -> u8 {
        self.u2p.written[0x0C]
    }

    /// Audio codec register `reg` (7 bits): nine bits of data.
    pub fn codec_reg(&self, reg: u8) -> u16 {
        self.i2c.codec[usize::from(reg & 0x7F)]
    }

    /// The registers of the clock generator that makes the C64's clock.
    /// Registers `$14`–`$1B` hold the divider setting.
    pub fn video_pll(&self) -> &[u8; 128] {
        &self.i2c.pll[usize::from(self.u2p.revision != BOARD_REV_PROTO)]
    }

    /// The registers of the clock generator for the HDMI pixel clock.
    pub fn hdmi_pll(&self) -> &[u8; 128] {
        &self.i2c.pll[usize::from(self.u2p.revision == BOARD_REV_PROTO)]
    }

    /// The 8-bit port expander's output register: the SID sockets'
    /// supply, shunt and capacitor switches, a nibble per socket.
    pub fn sid_socket_control(&self) -> u8 {
        self.i2c.expander8[1]
    }

    /// The I2C bus of the keyboard matrix is lent to the FPGA's scanner.
    pub fn keyboard_scan_enabled(&self) -> bool {
        self.i2c.scan_enable
    }

    /// Register `index` of the FPGA's clock synthesizer.
    pub fn mmcm_reg(&self, index: u8) -> u16 {
        let i = usize::from(index & 0x7F) * 2;
        u16::from(self.mmcm[i]) | u16::from(self.mmcm[i + 1]) << 8
    }

    /// The analog outputs' sources (left, right).
    pub fn audio_select(&self) -> (u8, u8) {
        (self.audio_select[0], self.audio_select[1])
    }

    /// The Ethernet MAC's station address as the firmware set it.
    pub fn rmii_mac(&self) -> [u8; 6] {
        let mut m = [0; 6];
        m.copy_from_slice(&self.rmii[..6]);
        m
    }

    /// The bytes the firmware last gave the profiler: (subsystem, task).
    pub fn profiler(&self) -> (u8, u8) {
        (self.profiler[0], self.profiler[1])
    }
}

impl IoDevice for Board {
    fn read(&mut self, addr: u32, ctx: &mut IoCtx<'_>) -> u8 {
        match addr >> 8 {
            0x0600 => self.sd.read(addr),
            0x0604 => {
                self.rtc.advance(ctx.now);
                self.rtc.peek(addr)
            }
            0x1007 => self.i2c.read(addr),
            _ => self.peek(addr),
        }
    }

    fn peek(&self, addr: u32) -> u8 {
        match addr >> 8 {
            0x0600 => self.sd.peek(addr),
            0x0604 => self.rtc.peek(addr),
            0x0605 => self.gcr.peek(addr),
            0x0607 => match addr & 0xF {
                n @ 0..=1 => self.audio_select[n as usize],
                _ => 0,
            },
            0x0800..=0x0807 => self.usb_ram[(addr & 0x7FF) as usize],
            0x1000 => self.u2p.peek(addr),
            0x1007 => self.i2c.peek(addr),
            // Blingboard receiver flags: bit 2 = the board is there.
            0x1008 if addr & 0xFF == 2 && self.blingboard => 0x04,
            _ => 0,
        }
    }

    fn write(&mut self, addr: u32, v: u8, ctx: &mut IoCtx<'_>) {
        match addr >> 8 {
            0x0600 => self.sd.write(addr, v),
            0x0603 => {
                if let n @ 4..=5 = addr & 0xFF {
                    self.profiler[(n - 4) as usize] = v;
                }
            }
            0x0604 => {
                self.rtc.advance(ctx.now);
                self.rtc.write(addr, v);
            }
            0x0605 => self.gcr.write(v),
            0x0606 => self.icap.write(addr, v),
            0x0607 => {
                if let n @ 0..=1 = addr & 0xF {
                    self.audio_select[n as usize] = v & 0x0F;
                }
            }
            0x0608 => self.rmii[(addr & 0x3F) as usize] = v,
            0x0800..=0x0807 => self.usb_ram[(addr & 0x7FF) as usize] = v,
            0x0808..=0x080F => self.usb_start = v & 1 != 0,
            0x1000 => {
                let link = ctx.net.as_ref().is_some_and(|n| n.link_up());
                self.u2p.write(addr, v, link);
            }
            0x1005 => self.mixer.write(addr, v),
            0x1006 => self.led.write(addr, v),
            0x1009 if self.blingboard => self.bling_led.write(addr, v),
            0x1007 => self.i2c.write(addr, v),
            0x1008 => {
                if addr & 0xFF == 2 {
                    self.bling_flags = v;
                }
            }
            0x2000 => self.mmcm[(addr & 0xFF) as usize] = v,
            _ => {}
        }
    }

    /// The FPGA's reset. What the host gave stays: the board revision,
    /// the monitor, the time of day. The chips on the I2C buses are not
    /// part of the FPGA and keep what was written to them.
    fn reset(&mut self) {
        let mut fresh = Self::with_revision(self.u2p.revision);
        fresh.blingboard = self.blingboard;
        fresh.led.output = self.blingboard;
        fresh.rtc = self.rtc.clone();
        fresh.rtc.lock = false;
        fresh.i2c = self.i2c.clone();
        fresh.i2c.reset_master();
        // The strip is not part of the FPGA: its LEDs keep their colours.
        fresh.led.shown = self.led.shown;
        fresh.bling_led.shown = self.bling_led.shown;
        *self = fresh;
    }

    fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"UBRD", 2, |w| {
            w.u8(self.sd.rate);
            w.bool(self.sd.force_ss);
            w.bool(self.sd.level_ss);
            w.u8(self.sd.crc7);

            w.u32(self.rtc.secs);
            w.u64(self.rtc.frac);
            w.u64(self.rtc.seen);
            w.bool(self.rtc.lock);

            w.u64(self.gcr.shift);

            w.bytes(&self.icap.stream);
            w.u32(self.icap.pulses);
            w.bool(self.icap.reconfigure);
            w.u32(self.icap.address);

            w.u8(self.u2p.revision);
            w.raw(&self.u2p.written);
            self.u2p.phy.save(w);

            w.raw(&self.mixer.audio);
            w.raw(&self.mixer.speaker);
            w.u32(self.mixer.resample_words);
            w.u8(self.mixer.resample_labor);
            w.u8(self.mixer.resample_flush);

            w.raw(&self.led.data);
            w.raw(&self.led.map);
            w.u8(self.led.map_reg);
            w.u8(self.led.start);
            w.u8(self.led.intensity);
            w.u32(self.led.frames);
            w.bool(self.led.output);
            w.raw(&self.led.shown);
            w.raw(&self.bling_led.data);
            w.raw(&self.bling_led.map);
            w.u8(self.bling_led.map_reg);
            w.u8(self.bling_led.start);
            w.u8(self.bling_led.intensity);
            w.u32(self.bling_led.frames);
            w.raw(&self.bling_led.shown);

            let i = &self.i2c;
            w.bool(i.started);
            w.bool(i.error);
            w.u8(i.channel);
            w.bool(i.scan_enable);
            w.u8(i.data_out);
            w.bool(i.address_next);
            w.u8(i.target as u8);
            w.bool(i.reading);
            w.u8(i.count);
            w.u8(i.first);
            for v in i.codec {
                w.u16(v);
            }
            w.raw(&i.expander8);
            w.u8(i.expander8_ptr);
            w.raw(&i.expander16);
            w.u8(i.expander16_ptr);
            w.raw(&i.pll[0]);
            w.raw(&i.pll[1]);
            w.raw(&i.pll_ptr);
            w.raw(&i.hub);
            w.u8(i.hub_ptr);
            w.bytes(&i.edid);
            w.u8(i.edid_ptr);
            w.u8(i.edid_segment);

            w.raw(&self.profiler);
            w.raw(&self.audio_select);
            w.raw(&self.rmii);
            w.raw(&self.usb_ram);
            w.bool(self.usb_start);
            w.u8(self.bling_flags);
            w.raw(&self.mmcm);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut r) = r.chunk(*b"UBRD", 2)?;
        let r = &mut r;
        let mut n = Self::new();

        n.sd.rate = r.u8()?;
        n.sd.force_ss = r.bool()?;
        n.sd.level_ss = r.bool()?;
        n.sd.crc7 = r.u8()? & 0x7F;

        n.rtc.secs = r.u32()?;
        n.rtc.frac = r.u64()?;
        if n.rtc.frac >= CLOCK_HZ {
            return Err(StateError::Invalid("seconds timer fraction"));
        }
        n.rtc.seen = r.u64()?;
        n.rtc.lock = r.bool()?;

        n.gcr.shift = r.u64()? & 0xFF_FFFF_FFFF;

        let stream = r.bytes()?;
        if stream.len() > ICAP_KEEP {
            return Err(StateError::Invalid("ICAP stream length"));
        }
        n.icap.stream = stream.to_vec();
        n.icap.pulses = r.u32()?;
        n.icap.reconfigure = r.bool()?;
        n.icap.address = r.u32()?;

        n.u2p.revision = r.u8()?;
        if n.u2p.revision > 0x1F {
            return Err(StateError::Invalid("board revision"));
        }
        r.raw_into(&mut n.u2p.written)?;
        n.u2p.phy.load(r)?;

        r.raw_into(&mut n.mixer.audio)?;
        r.raw_into(&mut n.mixer.speaker)?;
        n.mixer.resample_words = r.u32()?;
        n.mixer.resample_labor = r.u8()?;
        n.mixer.resample_flush = r.u8()?;

        r.raw_into(&mut n.led.data)?;
        r.raw_into(&mut n.led.map)?;
        n.led.map_reg = r.u8()?;
        n.led.start = r.u8()?;
        n.led.intensity = r.u8()?;
        n.led.frames = r.u32()?;
        n.led.output = r.bool()?;
        r.raw_into(&mut n.led.shown)?;
        r.raw_into(&mut n.bling_led.data)?;
        r.raw_into(&mut n.bling_led.map)?;
        n.bling_led.map_reg = r.u8()?;
        n.bling_led.start = r.u8()?;
        n.bling_led.intensity = r.u8()?;
        n.bling_led.frames = r.u32()?;
        r.raw_into(&mut n.bling_led.shown)?;

        let i = &mut n.i2c;
        i.started = r.bool()?;
        i.error = r.bool()?;
        i.channel = r.u8()?;
        if i.channel > 3 {
            return Err(StateError::Invalid("I2C channel"));
        }
        i.scan_enable = r.bool()?;
        i.data_out = r.u8()?;
        i.address_next = r.bool()?;
        i.target = *Target::ALL
            .get(usize::from(r.u8()?))
            .ok_or(StateError::Invalid("I2C target"))?;
        i.reading = r.bool()?;
        i.count = r.u8()?;
        if i.count > 2 {
            return Err(StateError::Invalid("I2C byte count"));
        }
        i.first = r.u8()?;
        for v in &mut i.codec {
            *v = r.u16()? & 0x1FF;
        }
        r.raw_into(&mut i.expander8)?;
        i.expander8_ptr = r.u8()? & 3;
        r.raw_into(&mut i.expander16)?;
        i.expander16_ptr = r.u8()? & 7;
        r.raw_into(&mut i.pll[0])?;
        r.raw_into(&mut i.pll[1])?;
        r.raw_into(&mut i.pll_ptr)?;
        i.pll_ptr[0] &= 0x7F;
        i.pll_ptr[1] &= 0x7F;
        r.raw_into(&mut i.hub)?;
        i.hub_ptr = r.u8()?;
        let edid = r.bytes()?;
        if edid.len() % 128 != 0 || edid.len() > EDID_MAX {
            return Err(StateError::Invalid("EDID length"));
        }
        i.edid = edid.to_vec();
        i.edid_ptr = r.u8()?;
        i.edid_segment = r.u8()?;

        r.raw_into(&mut n.profiler)?;
        r.raw_into(&mut n.audio_select)?;
        n.audio_select[0] &= 0x0F;
        n.audio_select[1] &= 0x0F;
        r.raw_into(&mut n.rmii)?;
        r.raw_into(&mut n.usb_ram)?;
        n.usb_start = r.bool()?;
        n.bling_flags = r.u8()?;
        r.raw_into(&mut n.mmcm)?;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::NullC64;
    use crate::irq::Irq;

    /// The I/O base of the blocks under test, by page.
    const SD: u32 = 0x06_0000;
    const RTC: u32 = 0x06_0400;
    const GCR_BASE: u32 = 0x06_0500;
    const ICAP: u32 = 0x06_0600;
    const RMII: u32 = 0x06_0800;
    const USB: u32 = 0x08_0000;
    const U2P: u32 = 0x10_0000;
    const I2C: u32 = 0x10_0700;
    const MMCM: u32 = 0x20_0000;

    struct Rig {
        board: Board,
        ram: Vec<u8>,
        irq: Irq,
        console: Vec<u8>,
        c64: NullC64,
        now: u64,
    }

    impl Rig {
        fn new() -> Self {
            Self::with(Board::new())
        }

        fn with(board: Board) -> Self {
            Self {
                board,
                ram: vec![0; 0x1000],
                irq: Irq::default(),
                console: Vec::new(),
                c64: NullC64::default(),
                now: 0,
            }
        }

        fn rd(&mut self, addr: u32) -> u8 {
            let mut ctx = IoCtx {
                now: self.now,
                ram: &mut self.ram,
                irq: &mut self.irq,
                console: &mut self.console,
                c64: &mut self.c64,
                storage: None,
                net: None,
            };
            self.board.read(addr, &mut ctx)
        }

        fn wr(&mut self, addr: u32, v: u8) {
            let mut ctx = IoCtx {
                now: self.now,
                ram: &mut self.ram,
                irq: &mut self.irq,
                console: &mut self.console,
                c64: &mut self.c64,
                storage: None,
                net: None,
            };
            self.board.write(addr, v, &mut ctx);
        }

        fn rd32(&mut self, addr: u32) -> u32 {
            (0..4).fold(0, |acc, i| acc | u32::from(self.rd(addr + i)) << (8 * i))
        }

        fn wr16(&mut self, addr: u32, v: u16) {
            self.wr(addr, v as u8);
            self.wr(addr + 1, (v >> 8) as u8);
        }

        fn wr32(&mut self, addr: u32, v: u32) {
            for (i, b) in v.to_le_bytes().into_iter().enumerate() {
                self.wr(addr + i as u32, b);
            }
        }

        // The I2C transfers as the firmware's driver makes them.

        /// Wait while busy; `true`: the last byte was acknowledged.
        fn i2c_wait(&mut self) -> bool {
            let st = self.rd(I2C + 1);
            assert_eq!(st & 0x80, 0, "the firmware waits on this bit forever");
            st & 0x04 == 0
        }

        fn i2c_start(&mut self, channel: u8) {
            self.wr(I2C + 6, channel);
            self.i2c_wait();
        }

        fn i2c_tx(&mut self, v: u8) -> bool {
            self.wr(I2C, v);
            self.i2c_wait()
        }

        fn i2c_stop(&mut self) {
            self.wr(I2C + 3, 1);
            self.i2c_wait();
        }

        fn i2c_rx(&mut self, ack: bool) -> u8 {
            self.wr(I2C + if ack { 5 } else { 4 }, 1);
            self.i2c_wait();
            self.rd(I2C)
        }

        fn i2c_probe(&mut self, channel: u8, dev: u8) -> bool {
            self.i2c_start(channel);
            let ack = self.i2c_tx(dev);
            self.i2c_stop();
            ack
        }

        fn i2c_write(&mut self, channel: u8, bytes: &[u8]) -> bool {
            self.i2c_start(channel);
            let ok = bytes.iter().all(|&b| self.i2c_tx(b));
            self.i2c_stop();
            ok
        }

        fn i2c_read_block(&mut self, dev: u8, reg: u8, n: usize) -> Option<Vec<u8>> {
            self.i2c_start(I2C_CH_HDMI);
            if !self.i2c_tx(dev) || !self.i2c_tx(reg) {
                self.i2c_stop();
                return None;
            }
            self.wr(I2C + 2, 1);
            self.i2c_wait();
            if !self.i2c_tx(dev | 1) {
                self.i2c_stop();
                return None;
            }
            let v = (0..n).map(|i| self.i2c_rx(i != n - 1)).collect();
            self.i2c_stop();
            Some(v)
        }
    }

    #[test]
    fn claims_its_pages_and_nobody_elses() {
        for a in [
            0x06_0000, 0x06_0100, 0x06_0304, 0x06_0400, 0x06_0500, 0x06_0604, 0x06_0700, 0x06_082F,
            0x08_0000, 0x08_0FFF, 0x10_000C, 0x10_0100, 0x10_0200, 0x10_0500, 0x10_0600, 0x10_0708,
            0x10_0802, 0x10_0900, 0x20_00FF,
        ] {
            assert!(Board::claims(a), "{a:#x}");
        }
        for a in [
            0x00_0000, 0x06_0200, 0x06_020F, 0x06_0900, 0x06_090F, 0x02_0000, 0x04_0000, 0x05_D012,
            0x10_0300, 0x10_030B, 0x10_0400, 0x10_04FF, 0x14_0000, 0x18_0000, 0x08_1000, 0x20_0100,
        ] {
            assert!(!Board::claims(a), "{a:#x}");
        }
    }

    #[test]
    fn c11_c19_board_revision_does_not_follow_writes() {
        let mut r = Rig::new();
        assert_eq!(r.rd(U2P + 0x0C), 0xB8);
        // The firmware's speaker write to the same address.
        r.wr(U2P + 0x0C, 0xFF);
        assert_eq!(r.rd(U2P + 0x0C), 0xB8);
        assert_eq!(r.rd(U2P + 0x0C) >> 3, BOARD_REV_ELITE2);
        assert_eq!(r.board.speaker_enable(), 0xFF);
        let mut r = Rig::with(Board::with_revision(0x15));
        assert_eq!(r.rd(U2P + 0x0C) >> 3, 0x15);
    }

    #[test]
    fn board_pins_idle() {
        let mut r = Rig::new();
        // The MDIO line idles high, whatever is clocked without a frame.
        r.wr(U2P + 0x0B, 1);
        r.wr(U2P + 0x0A, 1);
        r.wr(U2P + 0x0A, 0);
        assert_eq!(r.rd(U2P + 0x06), 1);
        // Hub and transceiver reset latch.
        r.wr(U2P + 0x0D, 1);
        r.wr(U2P + 0x0F, 1);
        assert_eq!(r.rd(U2P + 0x0D), 1);
        assert_eq!(r.board.usb_resets(), (true, true));
        r.wr(U2P + 0x0D, 0);
        r.wr(U2P + 0x0F, 0x80);
        assert_eq!(r.board.usb_resets(), (false, false));
        // DDR2 PHY and clock measurement read 0.
        r.wr(0x10_0100, 0xFF);
        assert_eq!(r.rd(0x10_0100), 0);
        assert_eq!(r.rd32(0x10_0200), 0);
    }

    #[test]
    fn b2_b3_i2c_is_never_busy_and_the_codec_acknowledges() {
        let mut r = Rig::new();
        // The driver's first act: scanner off.
        r.wr(I2C + 8, 0);
        assert_eq!(r.rd(I2C + 1) & 0x80, 0);
        // The codec's reset and two of its registers, as at boot.
        assert!(r.i2c_write(I2C_CH_1V8, &[0x34, 0x00, 0x00]));
        assert!(r.i2c_write(I2C_CH_1V8, &[0x34, 0x02, 0xCF]));
        assert!(r.i2c_write(I2C_CH_1V8, &[0x34, 0x1D, 0x08]));
        assert_eq!(r.board.codec_reg(0x01), 0x0CF);
        assert_eq!(r.board.codec_reg(0x0E), 0x108);
        // Sending leaves ones in the shift register.
        assert_eq!(r.rd(I2C), 0xFF);
        // The codec is on the 1.8 V bus only.
        assert!(!r.i2c_probe(I2C_CH_HDMI, 0x34));
        assert!(!r.i2c_probe(I2C_CH_3V3, 0x34));
        r.wr(I2C + 8, 1);
        assert!(r.board.keyboard_scan_enabled());
    }

    #[test]
    fn i2c_status_bits() {
        let mut r = Rig::new();
        assert_eq!(r.rd(I2C + 1), 0);
        r.wr(I2C + 6, 2);
        assert_eq!(r.rd(I2C + 6), 2);
        // A start goes out with the first byte; nobody at $5A.
        r.wr(I2C, 0x5A);
        assert_eq!(r.board.peek(I2C + 1), 0x05);
        assert_eq!(r.rd(I2C + 1), 0x05);
        // Read once, the "not acknowledged" bit is gone.
        assert_eq!(r.rd(I2C + 1), 0x01);
        r.wr(I2C + 3, 1);
        assert_eq!(r.rd(I2C + 1), 0x00);
        // The master's reset.
        r.wr(I2C, 0x58);
        r.wr(I2C + 7, 1);
        assert_eq!(r.rd(I2C + 1), 0x00);
        assert_eq!(r.rd(I2C + 6), 0);
    }

    #[test]
    fn b5_usb_hub_is_found_and_configured() {
        let mut r = Rig::new();
        assert!(r.i2c_probe(I2C_CH_3V3, 0x58));
        assert!(!r.i2c_probe(I2C_CH_3V3, 0x5A));
        assert!(!r.board.usb_hub_attached());
        // Reset, a configuration block at register 0, attach.
        assert!(r.i2c_write(I2C_CH_3V3, &[0x58, 0xFF, 1, 0x02]));
        assert!(r.i2c_write(I2C_CH_3V3, &[0x58, 0x00, 4, 0x24, 0x04, 0x13, 0x25]));
        assert!(r.i2c_write(I2C_CH_3V3, &[0x58, 0xFF, 1, 0x01]));
        assert!(r.board.usb_hub_attached());
        assert_eq!(&r.board.i2c.hub[..4], &[0x24, 0x04, 0x13, 0x25]);
    }

    #[test]
    fn b6_expanders_and_clock_generators_store() {
        let mut r = Rig::new();
        assert!(r.i2c_write(I2C_CH_1V8, &[0x40, 0x01, 0x88]));
        assert!(r.i2c_write(I2C_CH_1V8, &[0x40, 0x03, 0x00]));
        assert_eq!(r.board.sid_socket_control(), 0x88);
        assert!(r.i2c_write(I2C_CH_1V8, &[0x42, 0x07, 0xFF]));
        assert_eq!(r.board.i2c.expander16[7], 0xFF);
        // Video clock: a block of eight at register $14 on the 1.8 V bus.
        let block = [0x57, 0x00, 0x01, 0x01, 0x20, 0xD7, 0xBB, 0xF1];
        let mut msg = vec![0xC8, 0x14, 8];
        msg.extend_from_slice(&block);
        assert!(r.i2c_write(I2C_CH_1V8, &msg));
        assert_eq!(&r.board.video_pll()[0x14..0x1C], &block);
        assert_eq!(r.board.hdmi_pll()[0x14], 0);
        // HDMI clock: single register 1 through command $81.
        assert!(r.i2c_write(I2C_CH_HDMI, &[0xC8, 0x81, 0x18]));
        assert_eq!(r.board.hdmi_pll()[1], 0x18);
        // The early prototype has the two the other way round.
        let mut r = Rig::with(Board::with_revision(BOARD_REV_PROTO));
        assert!(r.i2c_write(I2C_CH_HDMI, &msg));
        assert_eq!(&r.board.video_pll()[0x14..0x1C], &block);
    }

    #[test]
    fn c20_edid_reads_as_a_valid_hdmi_monitor() {
        let mut r = Rig::new();
        let base = r.i2c_read_block(0xA0, 0x00, 128).unwrap();
        assert_eq!(
            &base[..8],
            &[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]
        );
        assert_eq!(base.iter().fold(0u8, |a, &b| a.wrapping_add(b)), 0);
        assert_eq!(base[126], 1);
        let ext = r.i2c_read_block(0xA0, 0x80, 128).unwrap();
        assert_eq!(ext.iter().fold(0u8, |a, &b| a.wrapping_add(b)), 0);
        assert_eq!(ext[0], 0x02);
        // Walk the data blocks as the firmware does, to the HDMI one.
        let end = usize::from(ext[2]);
        assert!((4..=127).contains(&end));
        let mut at = 4;
        let mut hdmi = false;
        while at < end {
            let (tag, len) = (ext[at] >> 5, usize::from(ext[at] & 0x1F));
            assert!(at + 1 + len <= end);
            hdmi |= tag == 3 && ext[at + 1..at + 4] == [0x03, 0x0C, 0x00];
            at += 1 + len;
        }
        assert_eq!(at, end);
        assert!(hdmi);
        // The preferred timing: 148.5 MHz, 1920 × 1080.
        assert_eq!(u16::from(base[54]) | u16::from(base[55]) << 8, 14850);
        assert_eq!(u16::from(base[56]) | u16::from(base[58] >> 4) << 8, 1920);
        assert_eq!(u16::from(base[59]) | u16::from(base[61] >> 4) << 8, 1080);
        // 720 × 576 at 50 Hz in the extension: 27 MHz, 864 × 625 total.
        let t = &ext[end..end + 18];
        assert_eq!(u16::from(t[0]) | u16::from(t[1]) << 8, 2700);
        assert_eq!(u16::from(t[2]) | u16::from(t[4] >> 4) << 8, 720);
        assert_eq!(u16::from(t[3]) | u16::from(t[4] & 15) << 8, 144);
        assert_eq!(u16::from(t[5]) | u16::from(t[7] >> 4) << 8, 576);
        assert_eq!(t[6], 49);
        // 1080p50: 2640 pixels a line.
        assert_eq!(u16::from(base[75]) | u16::from(base[76] & 15) << 8, 720);
    }

    #[test]
    fn edid_segment_pointer_and_monitor_removal() {
        let mut r = Rig::new();
        let mut edid = default_edid();
        edid.resize(128 * 4, 0x5A);
        r.board.set_monitor(Some(&edid));
        // Extension 2 lies in segment 1: segment write, repeated start,
        // then the usual block read.
        r.i2c_start(I2C_CH_HDMI);
        assert!(r.i2c_tx(0x60));
        assert!(r.i2c_tx(1));
        r.wr(I2C + 2, 1);
        assert!(r.i2c_tx(0xA0));
        assert!(r.i2c_tx(0x00));
        r.wr(I2C + 2, 1);
        assert!(r.i2c_tx(0xA1));
        assert_eq!(r.i2c_rx(false), 0x5A);
        r.i2c_stop();
        // The stop put the segment back to 0.
        assert_eq!(r.i2c_read_block(0xA0, 0, 2).unwrap(), [0x00, 0xFF]);
        // Past the end of the memory the bus idles.
        r.board.set_monitor(Some(&edid[..128]));
        assert_eq!(r.i2c_read_block(0xA0, 0x80, 1).unwrap(), [0xFF]);
        // No monitor: no acknowledge, the read fails.
        r.board.set_monitor(None);
        assert!(!r.board.monitor());
        assert!(r.i2c_read_block(0xA0, 0, 128).is_none());
        assert!(!r.i2c_probe(I2C_CH_HDMI, 0x60));
        // The clock generator on that bus is still there.
        assert!(r.i2c_probe(I2C_CH_HDMI, 0xC8));
    }

    #[test]
    fn b5_c31_usb_memory_takes_every_width_and_never_interrupts() {
        let mut r = Rig::new();
        r.wr(USB + 0x800, 0);
        for a in (0..0x800).step_by(2) {
            r.wr16(USB + a, 0);
        }
        r.wr16(USB + 0x10, 0xBEEF);
        r.wr32(USB + 0x20, 0x1234_5678);
        r.wr(USB + 0x7FF, 0xA5);
        assert_eq!(r.rd32(USB + 0x10), 0xBEEF);
        assert_eq!(r.rd32(USB + 0x20), 0x1234_5678);
        assert_eq!(r.rd(USB + 0x7FF), 0xA5);
        // The run register: stored, reads 0, through its whole half.
        r.wr(USB + 0x800, 1);
        assert!(r.board.usb_started());
        assert_eq!(r.rd(USB + 0x800), 0);
        r.wr(USB + 0xFFF, 0);
        assert!(!r.board.usb_started());
        // Report queue pointers: stable, equal, 0.
        for _ in 0..4 {
            assert_eq!(r.rd(USB + 0x7F0), 0);
            assert_eq!(r.rd(USB + 0x7F2), 0);
        }
        assert_eq!(r.irq, Irq::default());
        assert_eq!(r.board.next_event(), u64::MAX);
    }

    #[test]
    fn c3_sd_slot_is_empty() {
        let mut r = Rig::new();
        // Card detect is bit 2, write protect bit 3.
        assert_eq!(r.rd(SD + 8) & 0x0C, 0);
        assert_eq!(r.rd(SD + 8), 0x02);
        r.wr(SD + 8, 0x01);
        assert_eq!(r.rd(SD + 8), 0x01);
        assert_eq!(r.rd(SD + 4), 0xF4);
        r.wr(SD + 4, 254);
        assert_eq!(r.rd(SD + 4), 254);
        // No card drives the data line.
        assert_eq!(r.rd32(SD), 0xFFFF_FFFF);
        // CRC-7 of the reset command 40 00 00 00 00: $4A, sent as $95.
        r.wr(SD + 0xC, 0);
        for b in [0x40, 0, 0, 0, 0] {
            r.wr(SD, b);
        }
        assert_eq!(r.rd(SD + 0xC), 0x95);
        // The registers repeat through the page.
        assert_eq!(r.rd(SD + 0x1C), 0x95);
        // The clock chip's page next door reads 0.
        assert_eq!(r.rd(0x06_0100), 0);
    }

    #[test]
    fn c21_seconds_timer_runs_from_the_given_time() {
        let mut r = Rig::new();
        assert_eq!(r.rd32(RTC), RTC_POWER_ON);
        r.board.set_epoch(1_800_000_000);
        assert_eq!(r.rd32(RTC), 1_800_000_000);
        r.now += CLOCK_HZ - 1;
        assert_eq!(r.rd32(RTC), 1_800_000_000);
        r.now += 1;
        assert_eq!(r.rd32(RTC), 1_800_000_001);
        r.now += 3600 * CLOCK_HZ;
        assert_eq!(r.rd32(RTC), 1_800_003_601);
        assert_eq!(r.board.epoch(), 1_800_003_601);
        // The other offsets read 0.
        assert_eq!(r.rd32(RTC + 4), 0);
    }

    #[test]
    fn seconds_timer_locks_while_it_is_set() {
        let mut r = Rig::new();
        r.now = CLOCK_HZ / 2;
        // A 32-bit store sets it in one piece.
        r.wr32(RTC, 1000);
        assert_eq!(r.rd32(RTC), 1000);
        // The half second from before the store still counts.
        r.now += CLOCK_HZ / 2;
        assert_eq!(r.rd32(RTC), 1001);
        // Byte 0 alone stops the count until byte 3 is written.
        r.wr(RTC, 0x10);
        r.now += 10 * CLOCK_HZ;
        assert_eq!(r.rd32(RTC), 0x0000_0310);
        r.wr(RTC + 1, 0);
        r.wr(RTC + 2, 0);
        r.wr(RTC + 3, 0);
        r.now += 2 * CLOCK_HZ;
        assert_eq!(r.rd32(RTC), 0x12);
        // A clock that starts over (the machine restarted) is no step
        // back in time.
        r.now = 0;
        assert_eq!(r.rd32(RTC), 0x12);
        r.now = CLOCK_HZ;
        assert_eq!(r.rd32(RTC), 0x13);
    }

    #[test]
    fn gcr_encodes_four_bytes_to_five() {
        let mut r = Rig::new();
        // The start of a 1541 sector header.
        r.wr32(GCR_BASE + 8, u32::from_le_bytes([0x08, 0x01, 0x00, 0x01]));
        let out: Vec<u8> = (8..=0xC).map(|i| r.rd(GCR_BASE + i)).collect();
        assert_eq!(out, [0x52, 0x54, 0xB5, 0x29, 0x4B]);
        assert_eq!(r.rd(GCR_BASE + 0xD), 0);
        // Every nibble once.
        r.wr32(GCR_BASE, u32::from_le_bytes([0x01, 0x23, 0x45, 0x67]));
        let a: Vec<u8> = (8..=0xC).map(|i| r.rd(GCR_BASE + i)).collect();
        r.wr32(GCR_BASE, u32::from_le_bytes([0x89, 0xAB, 0xCD, 0xEF]));
        let b: Vec<u8> = (8..=0xC).map(|i| r.rd(GCR_BASE + i)).collect();
        // And back: five bytes in, four out, no errors.
        for (gcr, bin) in [(a, [0x01, 0x23, 0x45, 0x67]), (b, [0x89, 0xAB, 0xCD, 0xEF])] {
            for g in gcr {
                r.wr(GCR_BASE, g);
            }
            let dec: Vec<u8> = (0..4).map(|i| r.rd(GCR_BASE + i)).collect();
            assert_eq!(dec, bin);
            assert_eq!(r.rd(GCR_BASE + 4), 0);
        }
    }

    #[test]
    fn gcr_decode_flags_bad_groups() {
        let mut r = Rig::new();
        // After reset: 01010 10101 … alternating, all valid codes (0, F).
        assert_eq!(r.rd32(GCR_BASE), 0x0F0F_0F0F);
        assert_eq!(r.rd(GCR_BASE + 4), 0);
        // All zero: no group is a code.
        for _ in 0..5 {
            r.wr(GCR_BASE + 0xF, 0);
        }
        assert_eq!(r.rd32(GCR_BASE), 0);
        assert_eq!(r.rd(GCR_BASE + 4), 0xFF);
        assert_eq!(r.rd(GCR_BASE + 7), 0xFF);
        // The header again, its second group spoiled (01001 → 00000):
        // only bit 6 reports, that nibble reads 0.
        for g in [0x50, 0x14, 0xB5, 0x29, 0x4B] {
            r.wr(GCR_BASE, g);
        }
        assert_eq!(r.rd(GCR_BASE + 4), 0x40);
        let dec: Vec<u8> = (0..4).map(|i| r.rd(GCR_BASE + i)).collect();
        assert_eq!(dec, [0x00, 0x01, 0x00, 0x01]);
    }

    #[test]
    fn icap_reports_the_reload_request_once() {
        let mut r = Rig::new();
        // Strobes and unrelated bytes alone do nothing.
        r.wr(ICAP + 4, 0);
        r.wr(ICAP + 4, 0);
        r.wr(ICAP + 8, 0x30);
        assert!(!r.board.take_reconfigure());
        // The firmware's sequence for flash address $3C0000.
        let seq = [
            0xAA, 0x99, 0x32, 0x61, 0x00, 0x00, 0x32, 0x81, 0x03, 0x3C, 0x32, 0xA1, 0x00, 0x4F,
            0x30, 0xA1, 0x00, 0x0E, 0x20, 0x00,
        ];
        for b in seq {
            r.wr(ICAP + 8, b);
        }
        r.wr(ICAP + 4, 0);
        r.wr(ICAP + 4, 0);
        assert!(r.board.take_reconfigure());
        assert!(!r.board.take_reconfigure());
        assert_eq!(r.board.reconfigure_address(), 0x3C_0000);
        assert_eq!(r.rd(ICAP), 0);
        // Without the synchronisation word the same bytes are nothing.
        for b in &seq[2..] {
            r.wr(ICAP + 8, *b);
        }
        assert!(!r.board.take_reconfigure());
    }

    #[test]
    fn c24_ethernet_is_down() {
        let mut r = Rig::new();
        for (i, b) in [2, 0x15, 0x41, 1, 2, 3].into_iter().enumerate() {
            r.wr(RMII + i as u32, b);
        }
        assert_eq!(r.board.rmii_mac(), [2, 0x15, 0x41, 1, 2, 3]);
        // A packet handed over: the transmitter is free at once.
        r.wr32(RMII + 0x10, 0x0010_0000);
        r.wr16(RMII + 0x14, 60);
        r.wr(RMII + 0x18, 1);
        assert_eq!(r.rd(RMII + 0x1A), 0);
        // Buffers offered, receiver on: nothing ever arrives.
        for id in 0..32 {
            r.wr(RMII + 0x24, id);
        }
        r.wr(RMII + 0x08, 1);
        assert_eq!(r.rd(RMII + 0x2F), 0);
        assert_eq!(r.irq, Irq::default());
    }

    #[test]
    fn write_only_blocks_store_and_read_zero() {
        let mut r = Rig::new();
        // Mixers.
        r.wr(0x10_0500, 0x80);
        r.wr(0x10_0513, 0x40);
        r.wr(0x10_0540, 0x11);
        assert_eq!(r.board.mixer.audio[0], 0x80);
        assert_eq!(r.board.mixer.audio[19], 0x40);
        assert_eq!(r.board.mixer.speaker[0], 0x11);
        assert_eq!(r.rd(0x10_0500), 0);
        // Resampler: reset, 512 coefficients, the filter length.
        r.wr(0x10_0584, 1);
        r.wr(0x10_0589, 1);
        for i in 0..512 {
            r.wr32(0x10_0580, i);
        }
        r.wr(0x10_0588, 169);
        r.wr(0x10_0589, 0);
        assert_eq!(r.board.mixer.resample_words, 512);
        assert_eq!(r.board.mixer.resample_labor, 169);
        // LED strip: map, then colours, then a frame.
        r.wr(0x10_06FC, 1);
        for i in 0..252 {
            r.wr(0x10_0600 + i, i as u8);
        }
        r.wr(0x10_06FC, 0);
        r.wr(0x10_06FC, 0x80);
        r.wr(0x10_06FE, 0x20);
        for (i, c) in [10, 20, 30].into_iter().enumerate() {
            r.wr(0x10_0600 + i as u32, c);
        }
        r.wr(0x10_06FD, 0);
        r.wr(0x10_06FF, 0);
        assert_eq!(r.board.led.frames, 1);
        assert_eq!(r.board.led.color(0), [10, 20, 30]);
        assert_eq!(r.rd(0x10_0600), 0);
        // Clock synthesizer: register $14 at byte $28, and the reset.
        r.wr16(MMCM + 0x28, 0x130D);
        r.wr(MMCM + 0xFF, 0xB3);
        assert_eq!(r.board.mmcm_reg(0x14), 0x130D);
        assert_eq!(r.rd(MMCM + 0x28), 0);
        // Blingboard: none.
        r.wr(0x10_0802, 1);
        assert_eq!(r.rd(0x10_0802), 0);
        // Profiler bytes and audio select.
        r.wr(0x06_0304, 3);
        r.wr(0x06_0305, 9);
        assert_eq!(r.board.profiler(), (3, 9));
        assert_eq!(r.rd(0x06_0305), 0);
        r.wr(0x06_0700, 0xF6);
        r.wr(0x06_0711, 7);
        assert_eq!(r.board.audio_select(), (6, 7));
        assert_eq!(r.rd(0x06_0700), 6);
    }

    #[test]
    fn reset_keeps_what_the_host_gave() {
        let mut r = Rig::with(Board::with_revision(0x16));
        r.board.set_epoch(5000);
        r.board.set_monitor(None);
        r.wr(USB + 4, 0x77);
        r.wr(U2P + 0x0D, 1);
        r.wr(RTC, 1);
        assert!(r.i2c_write(I2C_CH_1V8, &[0x34, 0x02, 0xCF]));
        r.wr(I2C, 0x34);
        r.board.reset();
        assert_eq!(r.board.revision(), 0x16);
        assert!(!r.board.monitor());
        assert_eq!(r.rd(USB + 4), 0);
        assert_eq!(r.rd(U2P + 0x0D), 0);
        assert_eq!(r.rd(I2C + 1), 0);
        // The codec is a chip of its own, the timer runs again.
        assert_eq!(r.board.codec_reg(1), 0x0CF);
        r.now += CLOCK_HZ;
        assert_eq!(r.rd32(RTC), (5000 & !0xFF | 1) + 1);
    }

    /// A board with something in every block.
    fn busy_board() -> Board {
        let mut r = Rig::with(Board::with_revision(0x15));
        r.now = 12_345_678;
        r.board.set_epoch(77);
        r.wr(RTC, 5);
        r.wr(SD, 0x40);
        r.wr(SD + 8, 1);
        r.wr(GCR_BASE, 0x12);
        r.wr(ICAP + 8, 0xAA);
        r.wr(ICAP + 4, 0);
        r.wr(U2P + 0x0F, 0x80);
        r.wr(0x10_0501, 9);
        r.wr(0x10_0583, 9);
        r.board.set_blingboard(true);
        r.wr(0x10_0601, 9);
        r.wr(0x10_06FE, 0x7C);
        r.wr(0x10_06FF, 9);
        r.board.set_led_enable(false);
        r.wr(0x10_0902, 7);
        r.wr(0x10_09FE, 0x7C);
        r.wr(0x10_09FF, 0);
        r.board.set_blingboard(false);
        r.wr(USB + 0x123, 9);
        r.wr(USB + 0x800, 1);
        r.wr(RMII + 3, 9);
        r.wr(MMCM + 3, 9);
        r.wr(0x06_0304, 9);
        r.wr(0x06_0701, 9);
        r.wr(0x10_0802, 1);
        r.i2c_write(I2C_CH_1V8, &[0x34, 0x02, 0xCF]);
        r.i2c_write(I2C_CH_3V3, &[0x58, 0xFF, 1, 1]);
        // In the middle of an EDID read.
        r.i2c_start(I2C_CH_HDMI);
        r.i2c_tx(0xA0);
        r.i2c_tx(0x08);
        r.wr(I2C + 2, 1);
        r.i2c_tx(0xA1);
        r.i2c_rx(true);
        r.board
    }

    /// Gideon's firmware in its mode "Fixed Color": the map 0, 1, 2 for
    /// every LED, the chip type, then brightness, colour, offset, send.
    fn led_fixed(r: &mut Rig, base: u32, rgb: [u8; 3], brightness: u8) {
        r.wr(base + 0xFC, 1);
        for i in 0..252 {
            r.wr(base + i, (i % 3) as u8);
        }
        r.wr(base + 0xFC, 0);
        r.wr(base + 0xFC, 0x80);
        r.wr(base + 0xFE, brightness << 2);
        for (i, c) in rgb.into_iter().enumerate() {
            r.wr(base + i as u32, c);
        }
        r.wr(base + 0xFD, 0);
        r.wr(base + 0xFF, 0);
    }

    #[test]
    fn led_strip_shows_the_last_frame_sent() {
        let mut r = Rig::new();
        assert_eq!(r.board.led.count(), LEDS);
        assert_eq!(r.board.led.leds(), vec![[0; 3]; 84], "dark after power-on");
        assert_eq!(r.board.led.protocol(), LedProtocol::Unset);
        // The pins are the user port's: the frame is lost.
        led_fixed(&mut r, 0x10_0600, [255, 128, 0], 31);
        assert_eq!(r.board.led.frames, 1);
        assert!(!r.board.led.enabled());
        assert!(!r.board.led.lit());
        // Switched to the strip: every LED the one colour.
        r.board.set_led_enable(true);
        led_fixed(&mut r, 0x10_0600, [255, 128, 0], 31);
        assert_eq!(r.board.led.protocol(), LedProtocol::Apa102);
        assert_eq!(r.board.led.brightness(), 31);
        assert_eq!(r.board.led.leds(), vec![[255, 128, 0]; 84]);
        // A new colour and brightness show with the next frame only.
        r.wr(0x10_06FE, 8 << 2);
        r.wr(0x10_0600, 0);
        r.wr(0x10_0601, 63);
        assert_eq!(r.board.led.leds()[83], [255, 128, 0]);
        r.wr(0x10_0602, 255);
        r.wr(0x10_06FF, 0);
        assert_eq!(r.board.led.leds(), vec![[0, 16, 66]; 84]);
        // Brightness 0 is dark; the chip type WS2812.
        r.wr(0x10_06FC, 0x40);
        r.wr(0x10_06FE, 0);
        r.wr(0x10_06FF, 0);
        assert_eq!(r.board.led.protocol(), LedProtocol::Ws2812);
        assert!(!r.board.led.lit());
        // Pins away: the strip keeps its colours whatever is sent.
        r.wr(0x10_06FE, 31 << 2);
        r.wr(0x10_06FF, 0);
        r.board.set_led_enable(false);
        r.wr(0x10_0602, 9);
        r.wr(0x10_06FF, 0);
        assert_eq!(r.board.led.leds()[0], [0, 63, 255]);
        // The FPGA's reset does not reach the LEDs, power does.
        r.board.reset();
        assert_eq!(r.board.led.leds()[0], [0, 63, 255]);
        assert_eq!(r.board.led.frames, 0);
        r.board.led.power_on();
        assert!(!r.board.led.lit());
    }

    /// A pattern scrolls: the map names each LED's own bytes, every new
    /// colour goes one LED down the ring and the offset follows it.
    #[test]
    fn led_strip_scrolls_by_its_start_offset() {
        let mut r = Rig::new();
        r.board.set_led_enable(true);
        r.wr(0x10_06FC, 1);
        for i in 0..252 {
            r.wr(0x10_0600 + i, i as u8);
        }
        r.wr(0x10_06FC, 0);
        r.wr(0x10_06FE, 31 << 2);
        let mut offset = 0u32;
        for n in 1..=100u8 {
            r.wr(0x10_0600 + offset, n);
            r.wr(0x10_06FD, offset as u8);
            offset = if offset == 0 { 3 * 83 } else { offset - 3 };
            r.wr(0x10_06FF, 0);
        }
        // The newest colour on LED 0, older ones behind it, 84 in all.
        let leds = r.board.led.leds();
        assert_eq!(leds[0], [100, 0, 0]);
        assert_eq!(leds[1], [99, 0, 0]);
        assert_eq!(leds[83], [17, 0, 0]);
        assert_eq!(r.board.led.pattern()[..3], [0, 1, 2]);
    }

    #[test]
    fn the_blingboard_has_leds_of_its_own() {
        let mut r = Rig::new();
        // Not fitted: nobody there.
        led_fixed(&mut r, 0x10_0900, [1, 2, 3], 31);
        assert_eq!(r.board.bling_led.frames, 0);
        r.board.set_blingboard(true);
        assert_eq!(r.board.bling_led.count(), BLING_LEDS);
        led_fixed(&mut r, 0x10_0900, [10, 20, 30], 31);
        assert_eq!(r.board.bling_led.leds(), vec![[10, 20, 30]; 71]);
        assert_eq!(r.rd(0x10_0900), 0);
        assert!(!r.board.led.lit(), "the board's strip is another");
        // Its ring is 71 LEDs long: the offset wraps after byte 212.
        r.wr(0x10_09FC, 1);
        for i in 0..213 {
            r.wr(0x10_0900 + i, i as u8);
        }
        r.wr(0x10_09FC, 2);
        for i in 0..213 {
            r.wr(0x10_0900 + i, (i / 3) as u8);
        }
        r.wr(0x10_09FD, 3 * 70);
        r.wr(0x10_09FF, 0);
        let leds = r.board.bling_led.leds();
        assert_eq!(leds[0], [70; 3]);
        assert_eq!(leds[1], [0; 3]);
        assert_eq!(leds[70], [69; 3]);
        // With the Blingboard the case lights need no LEDSTRIP_EN.
        led_fixed(&mut r, 0x10_0600, [9, 9, 9], 31);
        assert_eq!(r.board.led.leds()[0], [9, 9, 9]);
        r.board.set_led_enable(false);
        assert!(r.board.led.enabled());
        r.board.reset();
        assert!(r.board.led.enabled());
        assert_eq!(r.board.bling_led.leds()[0], [70; 3]);
    }

    #[test]
    fn savestate_round_trip() {
        let board = busy_board();
        let mut w = StateWriter::new();
        board.save(&mut w);
        let bytes = w.into_bytes();
        let mut other = Board::new();
        other.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(other, board);
        // The copy goes on where the original was: the next EDID byte.
        let mut r = Rig::with(other);
        assert_eq!(r.i2c_rx(false), default_edid()[9]);
        // No monitor survives, too.
        let mut none = Board::new();
        none.set_monitor(None);
        let mut w = StateWriter::new();
        none.save(&mut w);
        let mut other = Board::new();
        other.load(&mut StateReader::new(&w.into_bytes())).unwrap();
        assert!(!other.monitor());
    }

    #[test]
    fn truncated_or_corrupt_savestates_are_errors() {
        let board = busy_board();
        let mut w = StateWriter::new();
        board.save(&mut w);
        let bytes = w.into_bytes();
        for n in (0..bytes.len()).step_by(7) {
            let mut other = Board::new();
            assert!(other.load(&mut StateReader::new(&bytes[..n])).is_err());
            assert_eq!(other, Board::new(), "a failed load changes nothing");
        }
        // Any damaged byte: an error or a state that works, never a panic.
        for i in (10..bytes.len()).step_by(3) {
            let mut bad = bytes.clone();
            bad[i] ^= 0xFF;
            let mut other = Board::new();
            if other.load(&mut StateReader::new(&bad)).is_ok() {
                let mut r = Rig::with(other);
                r.now = 1 << 40;
                r.rd32(RTC);
                r.i2c_rx(true);
                r.i2c_tx(0x12);
                r.wr(ICAP + 8, 0x0E);
                r.board.led.color(83);
                r.rd(GCR_BASE + 4);
            }
        }
    }
}
