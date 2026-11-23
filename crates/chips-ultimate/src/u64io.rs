//! The board-control page of the Ultimate 64 (`U64_IO_BASE`): sixteen
//! byte registers at I/O `$100400`–`$10040F` for the HDMI socket's
//! hot-plug line, the RESTORE key, the cartridge sense lines, a handful
//! of output latches and the board's own scan of the C64 keyboard and the
//! control ports.
//!
//! | address   | name            | read                                   | write                                  |
//! |-----------|-----------------|----------------------------------------|----------------------------------------|
//! | `$100400` | HDMI_REG        | bit 2 monitor present, bit 3 "was low" | `$20` DDC on, `$10` DDC off, `$08` acknowledge hot-plug |
//! | `$100401` | POWER_REG       | 0                                      | `$2B` then `$B2`: power off (first U64 only) |
//! | `$100402` | RESTORE_REG     | 1 while RESTORE is held                | –                                      |
//! | `$100403` | CART_DETECT     | bit 0 /GAME, bit 1 /EXROM of the port  | –                                      |
//! | `$100404` | HDMI_PLL_RESET  | latch                                  | 3, then 0: a reset pulse               |
//! | `$100405` | USERPORT_EN     | latch                                  | 3 user port powered, 0 off             |
//! | `$100406` | KEYB_JOY        | bits 0–4 of the chosen control port, low = closed; bits 5–7 high | bit 0 chooses the port: 0 port 2, 1 port 1 |
//! | `$100407` | BLACKBOARD      | bit 0 set                              | –                                      |
//! | `$100408` | HDMI_ENABLE     | latch                                  | 1 HDMI, 0 DVI                          |
//! | `$100409` | INT_CONNECTORS  | latch                                  | bit 0 parallel cable, bits 1–2 burst mode, bits 4–6 IEC wiring |
//! | `$10040A` | KEYB_COL        | latch                                  | keyboard select lines, low = selected  |
//! | `$10040B` | KEYB_ROW        | keyboard return lines, low = key down  | ignored (the firmware writes `$FF`)    |
//! | `$10040C` | LEDSTRIP_EN     | latch                                  | 1: the PWM pins drive the LED strip    |
//! | `$10040D` | PWM_DUTY        | latch                                  | duty cycle                             |
//! | `$10040E` | CASELED_SELECT  | latch                                  | two nibbles, a source per case LED     |
//! | `$10040F` | ETHSTREAM_ENA   | latch                                  | bit 0 video, 1 audio, 2 bus, 3 IEC stream |
//!
//! The rest of the page (`$100410`–`$1004FF`) reads 0 and ignores writes;
//! whether the hardware mirrors the sixteen registers there is not known.
//!
//! # The keyboard scan
//!
//! The firmware reads the C64's keyboard for its menu the way a C64
//! program does through CIA 1, only through this page: KEYB_COL takes the
//! place of port A (`$DC00`, the lines the program drives) and KEYB_ROW
//! the place of port B (`$DC01`, the lines it reads). Both are active
//! low. A key is named here by `(row, col)`: `col` is the number of the
//! port A line (PA0–PA7, bit of KEYB_COL) and `row` the number of the port
//! B line (PB0–PB7, bit of KEYB_ROW) that the key connects. The firmware's
//! key tables are indexed `col * 8 + row`. Some keys:
//!
//! | key         | col (PA) | row (PB) |
//! |-------------|----------|----------|
//! | INST/DEL    | 0        | 0        |
//! | RETURN      | 0        | 1        |
//! | F1          | 0        | 4        |
//! | cursor down | 0        | 7        |
//! | left SHIFT  | 1        | 7        |
//! | SPACE       | 7        | 4        |
//! | RUN/STOP    | 7        | 7        |
//!
//! KEYB_ROW is a pure function of the KEYB_COL latch and the keys held:
//! the firmware reads it in a loop until two reads agree and has no
//! timeout there. With nothing held it reads `$FF`, and KEYB_JOY reads
//! `$FF`: anything else is taken for a key or a joystick and steers the
//! menu.
//!
//! The model is the plain matrix (a held key shows on its return line
//! while its select line is low); the ghost keys three held keys make on
//! the real matrix are not produced, and the control ports do not show in
//! KEYB_ROW.
//!
//! # Hot plug
//!
//! Bit 2 of HDMI_REG follows the monitor. A change of the line raises
//! high interrupt source 5 of the ITU, which stays up until the firmware
//! writes `$08` to HDMI_REG. Bit 3 remembers that the line was low and is
//! cleared by the same write (it stays set while no monitor is there).
//! When the hardware raises the interrupt and what exactly bit 3 holds is
//! not published; the firmware only needs the acknowledge to drop the
//! line and never reads bit 3.
//!
//! Sources: UE2-C64U-Emulator `docs/hw/03-board-init.md` (register table
//! of the page, hot plug, keyboard registers), `05-ui-overlay-input.md`
//! (scan protocol, key table, hazards 7–10), `00-memory-map.md` (reset
//! values, C20, C36, M5, M6), `docs/specs/S36-joystick.md` (port select
//! of KEYB_JOY); 1541 Ultimate firmware `software/system/u64.h`
//! (addresses, bits of HDMI_REG and POWER_REG) and
//! `software/io/c64/keyboard_c64.cc` (the scan).

use crate::io::{IoCtx, IoDevice};
use mimic_core::{StateError, StateReader, StateWriter};

/// First address of the page.
pub const BASE: u32 = 0x10_0400;
/// Last address of the page.
pub const END: u32 = 0x10_04FF;

/// Register offsets in the page.
pub mod reg {
    pub const HDMI: u8 = 0x00;
    pub const POWER: u8 = 0x01;
    pub const RESTORE: u8 = 0x02;
    pub const CART_DETECT: u8 = 0x03;
    pub const HDMI_PLL_RESET: u8 = 0x04;
    pub const USERPORT_EN: u8 = 0x05;
    pub const KEYB_JOY: u8 = 0x06;
    pub const BLACKBOARD: u8 = 0x07;
    pub const HDMI_ENABLE: u8 = 0x08;
    pub const INT_CONNECTORS: u8 = 0x09;
    pub const KEYB_COL: u8 = 0x0A;
    pub const KEYB_ROW: u8 = 0x0B;
    pub const LEDSTRIP_EN: u8 = 0x0C;
    pub const PWM_DUTY: u8 = 0x0D;
    pub const CASELED_SELECT: u8 = 0x0E;
    pub const ETHSTREAM_ENA: u8 = 0x0F;
}

/// HDMI_REG, write: let the monitor's DDC lines through.
pub const HDMI_DDC_ENABLE: u8 = 0x20;
/// HDMI_REG, write: cut the DDC lines off.
pub const HDMI_DDC_DISABLE: u8 = 0x10;
/// HDMI_REG, write: acknowledge a hot-plug event.
pub const HDMI_HPD_RESET: u8 = 0x08;
/// HDMI_REG, read: the hot-plug line was low since the last acknowledge.
pub const HDMI_HPD_WASLOW: u8 = 0x08;
/// HDMI_REG, read: a monitor is connected.
pub const HDMI_HPD_CURRENT: u8 = 0x04;

/// The ITU's high interrupt source of the hot-plug line.
pub const IRQ_HIGH_HDMI: u8 = 5;

/// Joystick line bits of [`U64Io::set_joystick`].
pub const JOY_UP: u8 = 0x01;
pub const JOY_DOWN: u8 = 0x02;
pub const JOY_LEFT: u8 = 0x04;
pub const JOY_RIGHT: u8 = 0x08;
pub const JOY_FIRE: u8 = 0x10;

const POWER_OFF_1: u8 = 0x2B;
const POWER_OFF_2: u8 = 0xB2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct U64Io {
    /// What the firmware last wrote to each of the sixteen registers.
    latch: [u8; 16],
    /// Keys held: bit `row` of `keys[col]`.
    keys: [u8; 8],
    /// Closed switches of control port 1 and 2 (bits `JOY_*`).
    joy: [u8; 2],
    restore: bool,
    hpd: bool,
    hpd_was_low: bool,
    /// A hot-plug event waits for its acknowledge.
    hpd_event: bool,
    /// What the interrupt controller was last told.
    irq_shown: bool,
    ddc: bool,
    cart_detect: u8,
    blackboard: u8,
    /// The byte before the last one written to POWER_REG was `$2B`.
    power_armed: bool,
    power_off: bool,
}

impl Default for U64Io {
    fn default() -> Self {
        Self::new()
    }
}

impl U64Io {
    /// The page with a monitor connected, no key held, no cartridge.
    pub fn new() -> Self {
        Self {
            latch: [0; 16],
            keys: [0; 8],
            joy: [0; 2],
            restore: false,
            hpd: true,
            hpd_was_low: false,
            hpd_event: false,
            irq_shown: false,
            ddc: false,
            cart_detect: 3,
            blackboard: 1,
            power_armed: false,
            power_off: false,
        }
    }

    // ---- host side ------------------------------------------------------

    /// A key of the C64's keyboard goes down or up. `col` is the CIA 1
    /// port A line (bit of KEYB_COL), `row` the port B line (bit of
    /// KEYB_ROW) the key connects; see the module's table. Numbers above
    /// 7 are ignored.
    pub fn set_key(&mut self, row: u8, col: u8, down: bool) {
        if row > 7 || col > 7 {
            return;
        }
        let m = 1 << row;
        if down {
            self.keys[usize::from(col)] |= m;
        } else {
            self.keys[usize::from(col)] &= !m;
        }
    }

    /// Let go of every key.
    pub fn release_keys(&mut self) {
        self.keys = [0; 8];
    }

    /// The keys held: bit `row` of element `col`.
    pub fn keys(&self) -> [u8; 8] {
        self.keys
    }

    /// The switches of control port `port` (1 or 2) as the board sees
    /// them: a set bit of `lines` is a closed switch (`JOY_UP` …
    /// `JOY_FIRE`), 0 is the stick at rest. The register shows them
    /// inverted. Where the firmware pulls port lines itself (its software
    /// joystick in the C64 port block), the machine passes the wired sum
    /// of stick and firmware, because the menu compares this register with
    /// what it injected. Other port numbers are ignored.
    pub fn set_joystick(&mut self, port: u8, lines: u8) {
        if let 1 | 2 = port {
            self.joy[usize::from(port - 1)] = lines & 0x1F;
        }
    }

    /// RESTORE is held (the firmware looks at power-on: safe mode).
    pub fn set_restore(&mut self, held: bool) {
        self.restore = held;
    }

    /// A monitor is plugged into the HDMI socket or not. A change is a
    /// hot-plug event: interrupt until the firmware acknowledges.
    pub fn set_hpd(&mut self, connected: bool) {
        if self.hpd != connected {
            self.hpd = connected;
            self.hpd_event = true;
            if !connected {
                self.hpd_was_low = true;
            }
        }
    }

    /// The cartridge port's sense lines: bit 0 /GAME, bit 1 /EXROM, a
    /// clear bit is a line a cartridge pulls. 3: no cartridge.
    pub fn set_cart_detect(&mut self, v: u8) {
        self.cart_detect = v & 3;
    }

    /// BLACKBOARD (bit 0 must be set for the firmware's Assembly64
    /// client; that is the default).
    pub fn set_blackboard(&mut self, v: u8) {
        self.blackboard = v;
    }

    /// A monitor is connected.
    pub fn hpd(&self) -> bool {
        self.hpd
    }

    /// The firmware has the monitor's DDC lines switched through (it
    /// does that around reading the EDID).
    pub fn ddc_enabled(&self) -> bool {
        self.ddc
    }

    /// What the firmware last wrote to register `offset` ([`reg`]).
    pub fn latch(&self, offset: u8) -> u8 {
        self.latch[usize::from(offset & 15)]
    }

    /// The control port KEYB_JOY shows: 1 or 2.
    pub fn joy_select(&self) -> u8 {
        if self.latch[usize::from(reg::KEYB_JOY)] & 1 != 0 {
            1
        } else {
            2
        }
    }

    /// The firmware asked for HDMI (with sound and info frames) instead
    /// of DVI.
    pub fn hdmi_enabled(&self) -> bool {
        self.latch[usize::from(reg::HDMI_ENABLE)] & 1 != 0
    }

    /// The firmware wrote the power-off sequence (first Ultimate 64 only;
    /// this board's firmware switches off another way).
    pub fn power_off_requested(&self) -> bool {
        self.power_off
    }

    // ---- registers ------------------------------------------------------

    fn keyb_row(&self) -> u8 {
        let col = self.latch[usize::from(reg::KEYB_COL)];
        let mut down = 0;
        for (c, keys) in self.keys.iter().enumerate() {
            if col & (1 << c) == 0 {
                down |= keys;
            }
        }
        !down
    }

    fn keyb_joy(&self) -> u8 {
        !self.joy[usize::from(self.joy_select() - 1)]
    }

    fn sync_irq(&mut self, ctx: &mut IoCtx<'_>) {
        ctx.irq.set_high(IRQ_HIGH_HDMI, self.hpd_event);
        self.irq_shown = self.hpd_event;
    }
}

impl IoDevice for U64Io {
    fn read(&mut self, addr: u32, ctx: &mut IoCtx<'_>) -> u8 {
        self.sync_irq(ctx);
        self.peek(addr)
    }

    fn peek(&self, addr: u32) -> u8 {
        if !(BASE..=BASE + 15).contains(&addr) {
            return 0;
        }
        let offset = (addr - BASE) as u8;
        match offset {
            reg::HDMI => {
                (if self.hpd { HDMI_HPD_CURRENT } else { 0 })
                    | (if self.hpd_was_low { HDMI_HPD_WASLOW } else { 0 })
            }
            reg::POWER => 0,
            reg::RESTORE => u8::from(self.restore),
            reg::CART_DETECT => self.cart_detect,
            reg::KEYB_JOY => self.keyb_joy(),
            reg::BLACKBOARD => self.blackboard,
            reg::KEYB_ROW => self.keyb_row(),
            _ => self.latch[usize::from(offset)],
        }
    }

    fn write(&mut self, addr: u32, v: u8, ctx: &mut IoCtx<'_>) {
        if (BASE..=BASE + 15).contains(&addr) {
            let offset = (addr - BASE) as u8;
            self.latch[usize::from(offset)] = v;
            match offset {
                reg::HDMI => {
                    if v & HDMI_DDC_ENABLE != 0 {
                        self.ddc = true;
                    }
                    if v & HDMI_DDC_DISABLE != 0 {
                        self.ddc = false;
                    }
                    if v & HDMI_HPD_RESET != 0 {
                        self.hpd_event = false;
                        self.hpd_was_low = !self.hpd;
                    }
                }
                reg::POWER => {
                    if self.power_armed && v == POWER_OFF_2 {
                        self.power_off = true;
                    }
                    self.power_armed = v == POWER_OFF_1;
                }
                _ => {}
            }
        }
        self.sync_irq(ctx);
    }

    /// At once while the interrupt controller does not show the state of
    /// the hot-plug event (after [`U64Io::set_hpd`]), else never.
    fn next_event(&self) -> u64 {
        if self.hpd_event == self.irq_shown {
            u64::MAX
        } else {
            0
        }
    }

    fn tick(&mut self, ctx: &mut IoCtx<'_>) {
        self.sync_irq(ctx);
    }

    /// The firmware's latches and the hot-plug event go; what the host
    /// holds (keys, sticks, RESTORE, monitor, cartridge lines) stays.
    fn reset(&mut self) {
        self.latch = [0; 16];
        self.hpd_was_low = !self.hpd;
        self.hpd_event = false;
        self.ddc = false;
        self.power_armed = false;
        self.power_off = false;
    }

    fn save(&self, w: &mut StateWriter) {
        w.raw(&self.latch);
        w.raw(&self.keys);
        w.raw(&self.joy);
        for b in [
            self.restore,
            self.hpd,
            self.hpd_was_low,
            self.hpd_event,
            self.irq_shown,
            self.ddc,
            self.power_armed,
            self.power_off,
        ] {
            w.bool(b);
        }
        w.u8(self.cart_detect);
        w.u8(self.blackboard);
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut s = Self::new();
        r.raw_into(&mut s.latch)?;
        r.raw_into(&mut s.keys)?;
        r.raw_into(&mut s.joy)?;
        if s.joy.iter().any(|j| j & !0x1F != 0) {
            return Err(StateError::Invalid("u64io joystick lines"));
        }
        s.restore = r.bool()?;
        s.hpd = r.bool()?;
        s.hpd_was_low = r.bool()?;
        s.hpd_event = r.bool()?;
        s.irq_shown = r.bool()?;
        s.ddc = r.bool()?;
        s.power_armed = r.bool()?;
        s.power_off = r.bool()?;
        s.cart_detect = r.u8()?;
        if s.cart_detect > 3 {
            return Err(StateError::Invalid("u64io cartridge lines"));
        }
        s.blackboard = r.u8()?;
        *self = s;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Irq, NullC64};

    struct Rig {
        ram: Vec<u8>,
        irq: Irq,
        console: Vec<u8>,
        c64: NullC64,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                ram: vec![0; 16],
                irq: Irq {
                    global: true,
                    ..Irq::default()
                },
                console: Vec::new(),
                c64: NullC64::default(),
            }
        }

        fn ctx(&mut self) -> IoCtx<'_> {
            IoCtx {
                now: 0,
                ram: &mut self.ram,
                irq: &mut self.irq,
                console: &mut self.console,
                c64: &mut self.c64,
                storage: None,
                net: None,
            }
        }
    }

    const COL: u32 = BASE + 0x0A;
    const ROW: u32 = BASE + 0x0B;
    const JOY: u32 = BASE + 0x06;

    /// The firmware's matrix scan, reduced to what touches the bus:
    /// the keys found as `col * 8 + row`.
    fn scan(io: &mut U64Io, rig: &mut Rig) -> Vec<u8> {
        let mut found = Vec::new();
        io.write(ROW, 0xFF, &mut rig.ctx());
        io.write(COL, 0xFF, &mut rig.ctx());
        if io.read(JOY, &mut rig.ctx()) & 0x1F != 0x1F {
            return found;
        }
        io.write(COL, 0, &mut rig.ctx());
        if io.read(ROW, &mut rig.ctx()) == 0xFF {
            return found;
        }
        for c in 0..8u8 {
            let sel = !(1u8 << c);
            io.write(COL, 0xFF, &mut rig.ctx());
            io.write(COL, sel, &mut rig.ctx());
            // The stable-read loop: must end, and at once.
            let mut row;
            let mut turns = 0;
            loop {
                row = io.read(ROW, &mut rig.ctx());
                io.write(COL, sel, &mut rig.ctx());
                turns += 1;
                if row == io.read(ROW, &mut rig.ctx()) {
                    break;
                }
                assert!(turns < 4, "KEYB_ROW does not settle");
            }
            assert_eq!(turns, 1);
            for r in 0..8 {
                if row & (1 << r) == 0 {
                    found.push(c * 8 + r);
                }
            }
        }
        found
    }

    #[test]
    fn idle_values_keep_the_firmware_on_its_way() {
        let mut rig = Rig::new();
        let mut io = U64Io::new();
        let mut rd = |o: u32| io.read(BASE + o, &mut rig.ctx());
        assert_eq!(rd(0x00), 0x04, "monitor present");
        assert_eq!(rd(0x02), 0x00, "no safe mode");
        assert_eq!(rd(0x03), 0x03, "no cartridge");
        assert_eq!(rd(0x06), 0xFF, "stick at rest");
        assert_eq!(rd(0x07), 0x01);
        assert_eq!(rd(0x0B), 0xFF, "no key");
        // Beyond the sixteen registers: nothing.
        assert_eq!(rd(0x10), 0);
        assert_eq!(rd(0xFF), 0);
        assert!(!rig.irq.line());
        assert_eq!(io.next_event(), u64::MAX);
    }

    #[test]
    fn row_is_idle_for_every_column_latch_without_keys() {
        let mut rig = Rig::new();
        let mut io = U64Io::new();
        for col in [0x00, 0xFF, 0xFE, 0x7F, 0x55] {
            io.write(COL, col, &mut rig.ctx());
            assert_eq!(io.read(ROW, &mut rig.ctx()), 0xFF);
            assert_eq!(io.peek(COL), col);
        }
        assert!(scan(&mut io, &mut rig).is_empty());
    }

    #[test]
    fn single_keys_at_their_c64_matrix_places() {
        // (row = PB line, col = PA line), index col*8+row in the
        // firmware's key table.
        let keys = [
            ("RETURN", 1, 0, 0x01),
            ("SPACE", 4, 7, 0x3C),
            ("cursor down", 7, 0, 0x07),
            ("F1", 4, 0, 0x04),
            ("RUN/STOP", 7, 7, 0x3F),
        ];
        for (name, row, col, index) in keys {
            let mut rig = Rig::new();
            let mut io = U64Io::new();
            io.set_key(row, col, true);
            // All select lines low: the key's return line shows.
            io.write(COL, 0, &mut rig.ctx());
            assert_eq!(io.read(ROW, &mut rig.ctx()), !(1 << row), "{name}");
            // Only its own select line shows it.
            for c in 0..8u8 {
                io.write(COL, !(1 << c), &mut rig.ctx());
                let want = if c == col { !(1u8 << row) } else { 0xFF };
                assert_eq!(io.read(ROW, &mut rig.ctx()), want, "{name} col {c}");
            }
            // No line selected: nothing.
            io.write(COL, 0xFF, &mut rig.ctx());
            assert_eq!(io.read(ROW, &mut rig.ctx()), 0xFF, "{name}");
            assert_eq!(scan(&mut io, &mut rig), [index], "{name}");
            io.set_key(row, col, false);
            assert!(scan(&mut io, &mut rig).is_empty(), "{name}");
        }
    }

    #[test]
    fn several_keys() {
        let mut rig = Rig::new();
        let mut io = U64Io::new();
        io.set_key(7, 1, true); // left SHIFT
        io.set_key(7, 0, true); // cursor down
        io.set_key(1, 0, true); // RETURN
        io.write(COL, 0, &mut rig.ctx());
        assert_eq!(io.read(ROW, &mut rig.ctx()), !0x82);
        io.write(COL, 0xFE, &mut rig.ctx());
        assert_eq!(io.read(ROW, &mut rig.ctx()), !0x82);
        io.write(COL, 0xFD, &mut rig.ctx());
        assert_eq!(io.read(ROW, &mut rig.ctx()), !0x80);
        io.write(COL, 0xFC, &mut rig.ctx());
        assert_eq!(io.read(ROW, &mut rig.ctx()), !0x82);
        assert_eq!(scan(&mut io, &mut rig), [0x01, 0x07, 0x0F]);
        // Out of range: ignored, no panic.
        io.set_key(8, 0, true);
        io.set_key(0, 200, true);
        assert_eq!(scan(&mut io, &mut rig), [0x01, 0x07, 0x0F]);
        io.release_keys();
        assert!(scan(&mut io, &mut rig).is_empty());
    }

    #[test]
    fn writing_row_changes_nothing() {
        let mut rig = Rig::new();
        let mut io = U64Io::new();
        io.set_key(4, 7, true);
        io.write(COL, 0, &mut rig.ctx());
        io.write(ROW, 0xFF, &mut rig.ctx());
        io.write(ROW, 0x00, &mut rig.ctx());
        assert_eq!(io.read(ROW, &mut rig.ctx()), 0xEF);
    }

    #[test]
    fn joystick_of_the_selected_port() {
        let mut rig = Rig::new();
        let mut io = U64Io::new();
        io.set_joystick(2, JOY_DOWN | JOY_FIRE);
        io.set_joystick(1, JOY_UP);
        io.set_joystick(3, 0x1F);
        io.set_joystick(0, 0x1F);
        assert_eq!(io.joy_select(), 2);
        assert_eq!(io.read(JOY, &mut rig.ctx()), 0xED);
        io.write(JOY, 1, &mut rig.ctx());
        assert_eq!(io.joy_select(), 1);
        assert_eq!(io.read(JOY, &mut rig.ctx()), 0xFE);
        io.write(JOY, 0, &mut rig.ctx());
        io.set_joystick(2, 0xFF);
        assert_eq!(io.read(JOY, &mut rig.ctx()), 0xE0);
        // A stick in use keeps the scan from the matrix.
        io.set_key(1, 0, true);
        assert!(scan(&mut io, &mut rig).is_empty());
        io.set_joystick(2, 0);
        assert_eq!(scan(&mut io, &mut rig), [1]);
    }

    #[test]
    fn restore_and_cartridge_lines() {
        let mut rig = Rig::new();
        let mut io = U64Io::new();
        io.set_restore(true);
        assert_eq!(io.read(BASE + 2, &mut rig.ctx()), 1);
        io.set_restore(false);
        assert_eq!(io.read(BASE + 2, &mut rig.ctx()), 0);
        io.set_cart_detect(0xFE);
        assert_eq!(io.read(BASE + 3, &mut rig.ctx()), 2);
        io.set_blackboard(0);
        assert_eq!(io.read(BASE + 7, &mut rig.ctx()), 0);
    }

    #[test]
    fn latches_read_back() {
        let mut rig = Rig::new();
        let mut io = U64Io::new();
        for (o, v) in [
            (0x04, 3),
            (0x05, 3),
            (0x08, 1),
            (0x09, 0x70),
            (0x0C, 1),
            (0x0D, 0xD8),
            (0x0E, 0x40),
            (0x0F, 0x05),
        ] {
            io.write(BASE + o, v, &mut rig.ctx());
            assert_eq!(io.read(BASE + o, &mut rig.ctx()), v, "offset {o:#x}");
            assert_eq!(io.latch(o as u8), v);
        }
        assert!(io.hdmi_enabled());
        // The rest of the page takes writes and reads 0.
        io.write(BASE + 0x40, 0xAA, &mut rig.ctx());
        assert_eq!(io.read(BASE + 0x40, &mut rig.ctx()), 0);
        assert_eq!(io.latch(0), 0);
        // The fixed inputs are not changed by writes.
        io.write(BASE + 2, 1, &mut rig.ctx());
        io.write(BASE + 3, 0, &mut rig.ctx());
        io.write(BASE + 7, 0, &mut rig.ctx());
        assert_eq!(io.read(BASE + 2, &mut rig.ctx()), 0);
        assert_eq!(io.read(BASE + 3, &mut rig.ctx()), 3);
        assert_eq!(io.read(BASE + 7, &mut rig.ctx()), 1);
    }

    #[test]
    fn hdmi_register_writes_do_not_show_in_reads() {
        let mut rig = Rig::new();
        let mut io = U64Io::new();
        io.write(BASE, HDMI_DDC_ENABLE, &mut rig.ctx());
        assert!(io.ddc_enabled());
        assert_eq!(io.read(BASE, &mut rig.ctx()), 0x04);
        io.write(BASE, HDMI_DDC_DISABLE, &mut rig.ctx());
        assert!(!io.ddc_enabled());
        io.write(BASE, HDMI_HPD_RESET, &mut rig.ctx());
        assert_eq!(io.read(BASE, &mut rig.ctx()), 0x04);
        assert!(!rig.irq.line());
    }

    #[test]
    fn hot_plug_interrupt_until_acknowledged() {
        let mut rig = Rig::new();
        rig.irq.mask_high = 1 << IRQ_HIGH_HDMI;
        let mut io = U64Io::new();
        // The same state again is no event.
        io.set_hpd(true);
        assert_eq!(io.next_event(), u64::MAX);

        io.set_hpd(false);
        assert_eq!(io.next_event(), 0, "the machine must tick at once");
        assert!(!rig.irq.line());
        io.tick(&mut rig.ctx());
        assert_eq!(io.next_event(), u64::MAX);
        assert_eq!(rig.irq.high_active(), 0x20);
        assert!(rig.irq.line());
        assert_eq!(io.peek(BASE), HDMI_HPD_WASLOW);
        // Reading acknowledges nothing.
        assert_eq!(io.read(BASE, &mut rig.ctx()), HDMI_HPD_WASLOW);
        assert!(rig.irq.line());
        // The handler's write does; "was low" stays while unplugged.
        io.write(BASE, HDMI_HPD_RESET, &mut rig.ctx());
        assert!(!rig.irq.line());
        assert_eq!(io.read(BASE, &mut rig.ctx()), HDMI_HPD_WASLOW);
        assert_eq!(io.next_event(), u64::MAX);

        // Plugging in again: an event, the line is high, the latch
        // remembers the low until the acknowledge.
        io.set_hpd(true);
        assert_eq!(io.next_event(), 0);
        // An access to the page carries the interrupt as well as a tick.
        assert_eq!(io.read(BASE, &mut rig.ctx()), 0x0C);
        assert!(rig.irq.line());
        io.write(BASE, HDMI_HPD_RESET, &mut rig.ctx());
        assert!(!rig.irq.line());
        assert_eq!(io.read(BASE, &mut rig.ctx()), 0x04);

        // Other high sources are left alone.
        rig.irq.set_high(3, true);
        io.set_hpd(false);
        io.tick(&mut rig.ctx());
        io.write(BASE, HDMI_HPD_RESET, &mut rig.ctx());
        assert_eq!(rig.irq.high, 0x08);
    }

    #[test]
    fn power_off_sequence() {
        let mut rig = Rig::new();
        let mut io = U64Io::new();
        io.write(BASE + 1, 0xB2, &mut rig.ctx());
        assert!(!io.power_off_requested());
        io.write(BASE + 1, 0x2B, &mut rig.ctx());
        io.write(BASE + 1, 0x00, &mut rig.ctx());
        io.write(BASE + 1, 0xB2, &mut rig.ctx());
        assert!(!io.power_off_requested());
        io.write(BASE + 1, 0x2B, &mut rig.ctx());
        io.write(BASE + 1, 0xB2, &mut rig.ctx());
        assert!(io.power_off_requested());
        assert_eq!(io.read(BASE + 1, &mut rig.ctx()), 0);
    }

    #[test]
    fn reset_keeps_what_the_host_holds() {
        let mut rig = Rig::new();
        let mut io = U64Io::new();
        io.set_key(1, 0, true);
        io.set_joystick(2, JOY_FIRE);
        io.set_restore(true);
        io.set_hpd(false);
        io.tick(&mut rig.ctx());
        io.write(COL, 0xFE, &mut rig.ctx());
        io.write(BASE + 0x0D, 0xD8, &mut rig.ctx());
        io.reset();
        assert_eq!(io.latch(reg::PWM_DUTY), 0);
        assert_eq!(io.peek(COL), 0);
        assert_eq!(io.peek(ROW), 0xFD);
        assert_eq!(io.peek(JOY), 0xEF);
        assert_eq!(io.peek(BASE + 2), 1);
        assert_eq!(io.peek(BASE), HDMI_HPD_WASLOW);
        // The event is gone; the controller hears of it.
        assert_eq!(io.next_event(), 0);
        io.tick(&mut rig.ctx());
        assert_eq!(rig.irq.high, 0);
    }

    fn busy() -> (U64Io, Rig) {
        let mut rig = Rig::new();
        let mut io = U64Io::new();
        io.set_key(4, 7, true);
        io.set_key(1, 0, true);
        io.set_joystick(1, JOY_LEFT);
        io.set_restore(true);
        io.set_cart_detect(1);
        io.set_blackboard(0x81);
        io.write(COL, 0x7F, &mut rig.ctx());
        io.write(JOY, 1, &mut rig.ctx());
        io.write(BASE, HDMI_DDC_ENABLE, &mut rig.ctx());
        io.write(BASE + 1, 0x2B, &mut rig.ctx());
        io.write(BASE + 0x0E, 0x40, &mut rig.ctx());
        io.set_hpd(false);
        (io, rig)
    }

    #[test]
    fn savestate_round_trip() {
        let (io, mut rig) = busy();
        let mut w = StateWriter::new();
        io.save(&mut w);
        let bytes = w.into_bytes();
        let mut back = U64Io::new();
        let mut r = StateReader::new(&bytes);
        back.load(&mut r).unwrap();
        assert!(r.is_at_end());
        assert_eq!(back, io);
        assert_eq!(back.peek(ROW), 0xEF);
        assert_eq!(back.peek(JOY), 0xFB);
        assert_eq!(back.next_event(), 0);
        // The armed power-off sequence is part of it.
        back.write(BASE + 1, 0xB2, &mut rig.ctx());
        assert!(back.power_off_requested());
    }

    #[test]
    fn savestate_truncated_or_corrupt() {
        let (io, _) = busy();
        let mut w = StateWriter::new();
        io.save(&mut w);
        let bytes = w.into_bytes();
        for n in 0..bytes.len() {
            let mut back = U64Io::new();
            let before = back.clone();
            assert!(back.load(&mut StateReader::new(&bytes[..n])).is_err());
            assert_eq!(back, before, "a failed load leaves the block alone");
        }
        // A bool that is neither 0 nor 1.
        let mut bad = bytes.clone();
        bad[16 + 8 + 2] = 7;
        assert!(U64Io::new().load(&mut StateReader::new(&bad)).is_err());
        // Joystick lines beyond the five.
        let mut bad = bytes.clone();
        bad[16 + 8] = 0x20;
        assert!(U64Io::new().load(&mut StateReader::new(&bad)).is_err());
        // Cartridge lines beyond the two.
        let mut bad = bytes;
        let n = bad.len();
        bad[n - 2] = 4;
        assert!(U64Io::new().load(&mut StateReader::new(&bad)).is_err());
    }
}
