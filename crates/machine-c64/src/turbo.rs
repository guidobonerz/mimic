//! The Ultimate 64's turbo CPU: its registers and the speed they select.
//!
//! Source: 1541 Ultimate documentation, "Turbo Mode"
//! (<https://1541u-documentation.readthedocs.io/en/latest/config/turbo_mode.html>).
//!
//! The board's "Turbo Control" setting (`options["turbo"]`) decides who
//! sets the speed:
//!
//! | Setting     | Speed and bad lines from          | Registers          |
//! | ----------- | --------------------------------- | ------------------ |
//! | `off`       | 1 MHz, bad lines                  | none (read `$FF`)  |
//! | `manual`    | the menu (`cpu_speed`, `badlines`)| none (read `$FF`)  |
//! | `registers` | `$D031`                           | `$D031`            |
//! | `enablebit` | `$D031` while `$D030` bit 0 is set| `$D030`, `$D031`   |
//!
//! Registers:
//!
//! - `$D030` "Turbo Enable Bit": bit 0 written 0 = 1 MHz with bad lines,
//!   1 = the turbo settings; read back in bit 0.
//! - `$D031` "U64 Turbo Control": bits 0–3 the speed's index in the
//!   board's table ([`SPEEDS_U64`], [`SPEEDS_U64E2`]), bit 7 the bad line
//!   timing (0 = enabled, 1 = disabled).
//! - `$D07A` (write) normal speed, `$D07B` (write) turbo at 20 MHz.
//!
//! Unverified (the documentation does not say): the unused bits read 1 in
//! `$D030` and 0 in `$D031`; the registers answer at these four addresses
//! only, not at the VIC-II's mirrors; `$D031` starts with the menu's
//! settings and a reset puts them back; `$D07A`/`$D07B` set `$D031`'s
//! speed index and, with the enable bit, that bit. The SuperCPU detect
//! register `$D0BC` is not built.
//!
//! How the machine runs the extra CPU cycles is in `C64::turbo_cycles`.

use mimic_core::registry::ConfigError;
use mimic_core::{Savestate, StateError, StateReader, StateWriter};

/// CPU speeds in MHz by `$D031` index: Ultimate 64 and Ultimate 64 Elite.
pub const SPEEDS_U64: [u8; 16] = [1, 2, 3, 4, 5, 6, 8, 10, 12, 14, 16, 20, 24, 32, 40, 48];
/// CPU speeds in MHz by `$D031` index: Elite II board (Commodore 64 Ultimate).
pub const SPEEDS_U64E2: [u8; 16] = [1, 2, 3, 4, 6, 8, 10, 12, 14, 16, 20, 24, 32, 40, 48, 64];

/// The board's "Turbo Control" setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurboMode {
    Off,
    Manual,
    Registers,
    EnableBit,
}

const MODES: [TurboMode; 4] = [
    TurboMode::Off,
    TurboMode::Manual,
    TurboMode::Registers,
    TurboMode::EnableBit,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turbo {
    mode: TurboMode,
    speeds: &'static [u8; 16],
    /// The menu's settings (configuration).
    menu_index: u8,
    menu_badlines: bool,
    /// `$D031` bits 0–3.
    index: u8,
    /// `$D031` bit 7 clear.
    badlines: bool,
    /// `$D030` bit 0.
    enable: bool,
}

impl Turbo {
    /// From `options["turbo"]`, `["cpu_speed"]` (MHz, a value of the
    /// board's table) and `["badlines"]` ("on"/"off").
    pub fn from_options<'a>(
        speeds: &'static [u8; 16],
        option: impl Fn(&str) -> Option<&'a str>,
    ) -> Result<Self, ConfigError> {
        let mode = match option("turbo") {
            None | Some("off") => TurboMode::Off,
            Some("manual") => TurboMode::Manual,
            Some("registers") => TurboMode::Registers,
            Some("enablebit") => TurboMode::EnableBit,
            Some(other) => {
                return Err(ConfigError::Invalid(format!(
                    "turbo option '{other}' (off, manual, registers or enablebit)"
                )))
            }
        };
        let menu_index = match option("cpu_speed") {
            None => 0,
            Some(v) => v
                .parse::<u8>()
                .ok()
                .and_then(|mhz| speeds.iter().position(|&s| s == mhz))
                .ok_or_else(|| {
                    let list: Vec<String> = speeds.iter().map(u8::to_string).collect();
                    ConfigError::Invalid(format!("cpu_speed '{v}' (MHz: {})", list.join(", ")))
                })? as u8,
        };
        let menu_badlines = match option("badlines") {
            None | Some("on") => true,
            Some("off") => false,
            Some(other) => return Err(ConfigError::Invalid(format!("badlines option '{other}'"))),
        };
        let mut t = Self {
            mode,
            speeds,
            menu_index,
            menu_badlines,
            index: 0,
            badlines: true,
            enable: false,
        };
        t.reset();
        Ok(t)
    }

    /// The registers take the menu's settings again.
    pub fn reset(&mut self) {
        self.index = self.menu_index;
        self.badlines = self.menu_badlines;
        self.enable = false;
    }

    pub fn mode(&self) -> TurboMode {
        self.mode
    }

    /// The menu's settings: Turbo Control, the speed's index, bad lines.
    pub fn menu(&self) -> (TurboMode, u8, bool) {
        (self.mode, self.menu_index, self.menu_badlines)
    }

    /// The board's speed table (MHz by index).
    pub fn speeds(&self) -> &'static [u8; 16] {
        self.speeds
    }

    /// Settings changed in the menu; the registers take them over.
    pub fn set_menu(&mut self, mode: TurboMode, index: u8, badlines: bool) {
        self.mode = mode;
        self.menu_index = index & 0x0f;
        self.menu_badlines = badlines;
        self.reset();
    }

    fn settings_apply(&self) -> bool {
        match self.mode {
            TurboMode::Off => false,
            TurboMode::Manual | TurboMode::Registers => true,
            TurboMode::EnableBit => self.enable,
        }
    }

    /// CPU cycles per VIC-II cycle (the speed in MHz; the 2 % by which a
    /// PAL C64's cycle is longer than a microsecond are not modelled).
    pub fn factor(&self) -> u8 {
        if self.settings_apply() {
            self.speeds[usize::from(self.index)]
        } else {
            1
        }
    }

    /// Bad line timing: the CPU stops while the VIC-II holds BA low.
    pub fn badlines(&self) -> bool {
        !self.settings_apply() || self.badlines
    }

    /// The machine runs as a stock C64 (nothing to do per cycle).
    pub fn idle(&self) -> bool {
        self.factor() == 1 && self.badlines()
    }

    fn has_d031(&self) -> bool {
        matches!(self.mode, TurboMode::Registers | TurboMode::EnableBit)
    }

    /// A read of `addr`, if a turbo register answers there (side-effect
    /// free: also the peek).
    pub fn peek(&self, addr: u16) -> Option<u8> {
        match addr {
            0xd030 if self.mode == TurboMode::EnableBit => Some(0xfe | u8::from(self.enable)),
            0xd031 if self.has_d031() => Some(self.d031()),
            _ => None,
        }
    }

    /// `$D031` as it reads.
    pub fn d031(&self) -> u8 {
        self.index | if self.badlines { 0 } else { 0x80 }
    }

    /// A write to `addr`; true if a turbo register took it.
    pub fn write(&mut self, addr: u16, v: u8) -> bool {
        match addr {
            0xd030 if self.mode == TurboMode::EnableBit => self.enable = v & 1 != 0,
            0xd031 if self.has_d031() => {
                self.index = v & 0x0f;
                self.badlines = v & 0x80 == 0;
            }
            0xd07a | 0xd07b if self.has_d031() => {
                let fast = addr == 0xd07b;
                self.index = if fast {
                    self.speeds.iter().position(|&s| s == 20).unwrap_or(0) as u8
                } else {
                    0
                };
                if self.mode == TurboMode::EnableBit {
                    self.enable = fast;
                }
            }
            _ => return false,
        }
        true
    }
}

impl Savestate for Turbo {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"U64T", 2, |w| {
            w.u8(self.d031());
            w.bool(self.enable);
            // Version 2: the settings, which the menu can change.
            w.u8(MODES.iter().position(|&m| m == self.mode).unwrap_or(0) as u8);
            w.u8(self.menu_index);
            w.bool(self.menu_badlines);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut c) = r.chunk(*b"U64T", 2)?;
        let d031 = c.u8()?;
        let enable = c.bool()?;
        if version >= 2 {
            let mode = *MODES
                .get(usize::from(c.u8()?))
                .ok_or(StateError::Invalid("turbo mode"))?;
            let index = c.u8()? & 0x0f;
            self.menu_badlines = c.bool()?;
            self.mode = mode;
            self.menu_index = index;
        }
        self.enable = enable;
        self.index = d031 & 0x0f;
        self.badlines = d031 & 0x80 == 0;
        Ok(())
    }
}
