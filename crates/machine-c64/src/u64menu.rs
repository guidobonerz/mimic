//! The Ultimate 64's menu and its firmware updater.
//!
//! Sources: 1541 Ultimate documentation, "User Interface"
//! (<https://1541u-documentation.readthedocs.io/en/latest/u64/user_interface.html>),
//! "Multi Button" (`u64/multi_button.html`), the Ultimate 64 manual
//! (`ultimate64-manual.html`) and "Installing 3.10i"
//! (`howto/installing_310i.html`). The firmware itself (GPL) was not
//! read; it cannot run here either (it is a program for a processor
//! inside the FPGA), so this is a rebuilt menu.
//!
//! From the documentation: a brief press of the multi button opens the
//! menu and freezes the C64, leaving it resumes the C64; cursor up/down
//! move, RETURN or cursor right select, DEL or cursor left go up a level,
//! RUN/STOP leaves, F2 is the configuration, F5 the tools, F3 shows the
//! keys, `+`/`-` and cursor left/right change a setting; an update file
//! is started with RETURN and "Run Update"; the file is `update.u64` for
//! the Ultimate 64.
//!
//! From the user's 3.15a update files (`update_v3.15a.u64`, `.ue2`,
//! `.cfw`, `.u2p`, `.u2l`, `.u2r`), looked at as data (their first bytes
//! and the texts in them; no code was disassembled): the files' layout
//! (see `inspect_image`), where the version text stands (`find_version`),
//! the messages "Invalid Update Image" and "Update for Wrong Product",
//! the updater's title lines, "About to update. Continue?", "Not
//! flashing.", "Done!" and "Turning OFF machine in 5 seconds....", the
//! help page's lines, "< No Items >", and the entries "Mount Disk", "Run
//! Disk", "Load", "Run Cart", "Run Update", "Reset C64", "Reboot C64".
//!
//! From the firmware's sources (GPL; read for facts only since the
//! user's decision of 2026-11-23, golden rule 7; commit 40188c7 of
//! github.com/GideonZ/1541ultimate): `software/filetypes/filetype_u2p.cc`
//! (which files are update files, the block layout, the `.cfw` header
//! and its three messages), `software/system/product.cc` (product names,
//! families "U64MK1"/"U64MK2", the update extensions, the title's
//! format), `software/application/update_u2p/update_u64.cc`,
//! `update_u64ii.cc` and `update_common.h` (the updater's course: it
//! checks the checksums of the FPGA image and the application it
//! carries, asks, writes ROM and HTML files to the flash disk, flashes
//! "Runtime FPGA" and "Ultimate Application", write-protects, asks
//! whether to reset the configuration and switches the machine off after
//! five seconds).
//!
//! Own: the layout (40 × 25 characters over the C64's screen area),
//! colours, the remaining texts, which entries are offered and their
//! order, F2's settings (only those this machine has), the updater's
//! steps and its progress bar. Not rebuilt of the real updater: the
//! checksums of the images inside the file (where they lie and what they
//! must be is fixed when the firmware is built, not by the file's
//! layout), the separate parts and files it writes, the WiFi module's
//! update and the question about the configuration. Instead the whole
//! file goes into one flash image here, and the loader's walk through
//! the file is checked more strictly than the firmware does it.
//!
//! The browser shows the frontend's file area (the command interface's
//! folder with its mounted folders). The flash is the file
//! `/Flash/.firmware` there, in a format of its own: the update file's
//! name, version text, length and CRC-32, then its bytes. The updater
//! checks the file, asks, erases, programs and verifies over many
//! frames; a power cut while it writes leaves a damaged flash, and the
//! board then shows a notice instead of starting (a real one would need
//! its recovery procedure). Deleting the file brings the factory
//! firmware back.

use mimic_core::host::{lock, FileHost, FsError, HostServices, OpenMode};
use mimic_core::input::MachineKey;
use mimic_core::{StateError, StateReader, StateWriter};

use crate::keyboard::keys;
use crate::turbo::{Turbo, TurboMode};

pub const COLS: usize = 40;
pub const ROWS: usize = 25;
/// File list: rows 2 to 21.
const LIST_ROWS: usize = 20;
const FLASH_DIR: &str = "/Flash";
const FLASH_FILE: &str = "/Flash/.firmware";
const MAGIC: &[u8; 8] = b"MIMICFW2";
/// Bytes the updater moves in one frame.
const CHUNK: usize = 32 * 1024;
/// Frames the erase step takes.
const ERASE_FRAMES: u8 = 50;
const PLUS: MachineKey = MachineKey(5 * 8);
const MINUS: MachineKey = MachineKey(5 * 8 + 3);
const KEY_Y: MachineKey = MachineKey(3 * 8 + 1);
const KEY_N: MachineKey = MachineKey(4 * 8 + 7);
/// Update files of the Ultimate family (the 3.10i package's names).
/// A `.cfw` file's first bytes, before the product name.
const CFW_MAGIC: [u8; 4] = [0xde, 0xad, 0x15, 0x41];
/// The length of a `.cfw` file's header.
const CFW_HEADER: usize = 36;
/// The text an image's version stands in front of (the title's format).
const TITLE_FORMAT: &[u8] = b"%s (V1.%b) %s";
/// Frames from "Done!" to the machine switching off (5 seconds).
const OFF_FRAMES: u32 = 250;

/// What the menu and the updater know of the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Board {
    /// The name in the menu's title.
    pub title: &'static str,
    /// The firmware's product name (a `.cfw` file may ask for it).
    pub product: &'static str,
    /// The firmware's product family (a `.cfw` file may ask for it).
    pub family: &'static str,
    /// The updater's title line.
    pub updater: &'static str,
    /// The extension of this product's update files.
    pub ext: &'static str,
}

/// Ultimate 64 (the Elite calls itself "Ultimate 64 Elite", same family
/// and extension).
pub const U64: Board = Board {
    title: "Ultimate 64",
    product: "Ultimate 64",
    family: "U64MK1",
    updater: "** Ultimate 64 Updater **",
    ext: "u64",
};

/// The Elite II board: Ultimate 64 Elite-II and the Commodore 64
/// Ultimate. Gideon's firmware calls the product "Ultimate 64-II"; what
/// Commodore's own firmware calls it is not known.
pub const C64U: Board = Board {
    title: "Commodore 64 Ultimate",
    product: "Ultimate 64-II",
    family: "U64MK2",
    updater: "** Ultimate 64 Elite-II Updater **",
    ext: "ue2",
};

// VIC-II colours.
const BLACK: u8 = 0;
const WHITE: u8 = 1;
const RED: u8 = 2;
const GREEN: u8 = 5;
const YELLOW: u8 = 7;
const GREY: u8 = 15;
const DARK: u8 = 11;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// What the board's flash holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Flash {
    /// Not looked at yet (after power-on).
    #[default]
    Unchecked,
    /// No flash file: the firmware the board came with.
    Factory,
    /// A complete update.
    Installed {
        name: String,
        /// The version text found in the image ("3.15a"; empty: none).
        version: String,
        size: u32,
        crc: u32,
    },
    /// An update that was cut short or does not verify.
    Damaged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Browser,
    Context,
    Tools,
    Config,
    ConfigItems,
    Info,
    Help,
    Update,
    Damaged,
}

const VIEWS: [View; 9] = [
    View::Browser,
    View::Context,
    View::Tools,
    View::Config,
    View::ConfigItems,
    View::Info,
    View::Help,
    View::Update,
    View::Damaged,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Check,
    /// "About to update. Continue?"
    Confirm,
    Erase(u8),
    Program,
    Verify,
    Done,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Update {
    src: String,
    name: String,
    size: u32,
    pos: u32,
    /// Running CRC of the current step.
    crc: u32,
    /// The file's CRC from the check.
    want: u32,
    phase: Phase,
    error: String,
    /// The version text found in the image.
    version: String,
    /// The end of the data checked so far (the version search crosses
    /// the steps' borders).
    carry: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cmd {
    RunDisk,
    MountDisk,
    RunPrg,
    LoadPrg,
    RunCart,
    RunUpdate,
    Reset,
    Reboot,
    Eject,
    Info,
}

/// What the machine does for the menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Mount {
        path: String,
        run: bool,
    },
    Prg {
        path: String,
        run: bool,
    },
    Cart {
        path: String,
    },
    Reset,
    /// Power-on reset (also after an update).
    Reboot,
    Eject,
}

/// The machine as the menu shows it.
pub struct Status {
    pub ntsc: bool,
    pub uci: bool,
    pub reu: Option<usize>,
    pub disk: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    /// The board (configuration).
    board: Board,
    open: bool,
    view: View,
    path: String,
    entries: Vec<Entry>,
    cursor: usize,
    top: usize,
    /// Cursor of a popup or of the settings list.
    sub: usize,
    /// Settings category.
    cat: usize,
    /// Shift keys held (bit 0 left, bit 1 right).
    shift: u8,
    message: String,
    flash: Flash,
    update: Option<Update>,
}

fn crc_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    crc
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map_or(String::new(), |(_, e)| e.to_ascii_lowercase())
}

fn read_at(f: &mut dyn FileHost, path: &str, pos: u64, len: usize) -> Result<Vec<u8>, FsError> {
    let h = f.open(
        path,
        OpenMode {
            read: true,
            ..OpenMode::default()
        },
    )?;
    let mut buf = vec![0; len];
    let mut got = 0;
    let r = f.seek(h, pos).and_then(|()| {
        while got < len {
            match f.read(h, &mut buf[got..])? {
                0 => break,
                n => got += n,
            }
        }
        Ok(())
    });
    f.close(h);
    buf.truncate(got);
    r.map(|()| buf)
}

fn write_at(
    f: &mut dyn FileHost,
    path: &str,
    pos: u64,
    data: &[u8],
    truncate: bool,
) -> Result<(), FsError> {
    let h = f.open(
        path,
        OpenMode {
            write: true,
            create: true,
            truncate,
            ..OpenMode::default()
        },
    )?;
    let mut done = 0;
    let r = f.seek(h, pos).and_then(|()| {
        while done < data.len() {
            match f.write(h, &data[done..])? {
                0 => return Err(FsError::Io("flash write failed".into())),
                n => done += n,
            }
        }
        Ok(())
    });
    f.close(h);
    r
}

/// The flash file's header: magic, length, CRC, the update's name and
/// its version text.
fn flash_header(name: &str, version: &str, size: u32, crc: u32) -> Vec<u8> {
    let name = &name.as_bytes()[..name.len().min(255)];
    let version = &version.as_bytes()[..version.len().min(255)];
    let mut h = MAGIC.to_vec();
    h.extend_from_slice(&size.to_le_bytes());
    h.extend_from_slice(&crc.to_le_bytes());
    h.push(name.len() as u8);
    h.extend_from_slice(name);
    h.push(version.len() as u8);
    h.extend_from_slice(version);
    h
}

/// Read the flash file: what it holds.
fn read_flash(f: &mut dyn FileHost) -> Flash {
    let size = match f.stat(FLASH_FILE) {
        Ok(s) if !s.is_dir => s.size,
        Ok(_) => return Flash::Damaged,
        Err(_) => return Flash::Factory,
    };
    let Ok(head) = read_at(f, FLASH_FILE, 0, 18 + 2 * 255) else {
        return Flash::Damaged;
    };
    if head.len() < 18 || &head[..8] != MAGIC {
        return Flash::Damaged;
    }
    let len = u32::from_le_bytes([head[8], head[9], head[10], head[11]]);
    let crc = u32::from_le_bytes([head[12], head[13], head[14], head[15]]);
    let name_end = 17 + usize::from(head[16]);
    let (Some(name), Some(&version_len)) = (head.get(17..name_end), head.get(name_end)) else {
        return Flash::Damaged;
    };
    let start = name_end + 1 + usize::from(version_len);
    let Some(version) = head.get(name_end + 1..start) else {
        return Flash::Damaged;
    };
    let start = start as u64;
    if size != start + u64::from(len) {
        return Flash::Damaged;
    }
    let mut sum = u32::MAX;
    let mut pos = 0u64;
    while pos < u64::from(len) {
        match read_at(f, FLASH_FILE, start + pos, CHUNK) {
            Ok(d) if !d.is_empty() => {
                sum = crc_update(sum, &d);
                pos += d.len() as u64;
            }
            _ => return Flash::Damaged,
        }
    }
    if !sum != crc {
        return Flash::Damaged;
    }
    Flash::Installed {
        name: String::from_utf8_lossy(name).into_owned(),
        version: String::from_utf8_lossy(version).into_owned(),
        size: len,
        crc,
    }
}

/// Go through an update file as the firmware's loader does, and say
/// what is wrong with it.
///
/// The files (1541 Ultimate firmware, `software/filetypes/filetype_u2p.cc`;
/// seen in the 3.15a files): blocks of three little-endian words (load
/// address, length, start address) and `length` bytes, which the loader
/// copies to the load address, until the file ends; it then jumps to the
/// last start address that was not zero. A `.cfw` file has 36 bytes in
/// front: the fingerprint `DE AD 15 41`, a product family and a product
/// name of 16 bytes each; a family or name that is given must be the
/// board's.
///
/// The loader refuses only a bad `.cfw` header (the three messages in
/// capitals below are its own). It does not check the blocks: a damaged
/// file is loaded and started all the same. What would go wrong then is
/// reported here in this project's words, as the reason not to flash.
fn inspect_image(
    board: &Board,
    cfw: bool,
    f: &mut dyn FileHost,
    path: &str,
    size: u32,
) -> Result<(), String> {
    let io = |e: FsError| e.to_string();
    let mut at = 0u64;
    if cfw {
        let head = read_at(f, path, 0, CFW_HEADER).map_err(io)?;
        if head.len() < CFW_HEADER || head[..4] != CFW_MAGIC {
            return Err("Invalid Update Image".into());
        }
        let text = |field: &[u8]| -> Vec<u8> {
            field[..field.iter().position(|&b| b == 0).unwrap_or(field.len())].to_vec()
        };
        let (family, name) = (text(&head[4..20]), text(&head[20..36]));
        if !family.is_empty() && family != board.family.as_bytes() {
            return Err("Wrong Product Family".into());
        }
        if !name.is_empty() && name != board.product.as_bytes() {
            return Err("Update for Wrong Product".into());
        }
        at = CFW_HEADER as u64;
    }
    let size = u64::from(size);
    let mut start = 0u32;
    let mut blocks: Vec<(u32, u32)> = Vec::new();
    while at < size {
        let head = read_at(f, path, at, 12).map_err(io)?;
        if head.len() < 12 {
            // The loader stops at a rest shorter than a block's header.
            break;
        }
        let word = |i: usize| u32::from_le_bytes([head[i], head[i + 1], head[i + 2], head[i + 3]]);
        let (load, length) = (word(0), word(4));
        if word(8) != 0 {
            start = word(8);
        }
        at += 12 + u64::from(length);
        if at > size {
            return Err("Image cut short: it would crash".into());
        }
        blocks.push((load, length));
        if blocks.len() > 64 {
            return Err("Image has no sense: it would crash".into());
        }
    }
    if start == 0 {
        return Err("No start address: nothing would run".into());
    }
    if !blocks
        .iter()
        .any(|&(load, length)| start >= load && start - load < length)
    {
        return Err("Start outside image: it would crash".into());
    }
    Ok(())
}

/// The version text of an image, if `data` holds the place: the last
/// text in front of the title's format that looks like a version.
fn find_version(data: &[u8]) -> Option<String> {
    let at = data
        .windows(TITLE_FORMAT.len())
        .position(|w| w == TITLE_FORMAT)?;
    data[at.saturating_sub(48)..at]
        .split(|&b| b == 0)
        .rev()
        .find(|t| {
            (3..=10).contains(&t.len())
                && t[0].is_ascii_digit()
                && t.contains(&b'.')
                && t.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'.')
        })
        .map(|t| String::from_utf8_lossy(t).into_owned())
}

impl Menu {
    pub fn new(board: Board) -> Self {
        Self {
            board,
            open: false,
            view: View::Browser,
            path: "/".into(),
            entries: Vec::new(),
            cursor: 0,
            top: 0,
            sub: 0,
            cat: 0,
            shift: 0,
            message: String::new(),
            flash: Flash::Unchecked,
            update: None,
        }
    }

    /// The menu is on the screen and the C64 stands still.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The browser's list.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn flash(&self) -> &Flash {
        &self.flash
    }

    /// An update is being written (a power cut now damages the flash).
    pub fn flashing(&self) -> bool {
        self.update
            .as_ref()
            .is_some_and(|u| matches!(u.phase, Phase::Erase(_) | Phase::Program | Phase::Verify))
    }

    /// Power-on: the menu is gone, the flash is looked at again.
    pub fn power_on(&mut self) {
        *self = Self::new(self.board);
    }

    /// A reset of the C64 closes the menu (not the damaged board's notice).
    pub fn reset(&mut self) {
        if self.view != View::Damaged {
            self.open = false;
            self.update = None;
            self.view = View::Browser;
        }
    }

    /// The multi button, pressed briefly: opens or leaves the menu.
    pub fn button(&mut self, host: &HostServices) {
        if self.view == View::Damaged || self.update.is_some() {
            return;
        }
        if self.open {
            self.open = false;
        } else {
            self.open = true;
            self.view = View::Browser;
            self.message.clear();
            self.list(host);
        }
    }

    fn list(&mut self, host: &HostServices) {
        self.entries.clear();
        let Some(files) = &host.files else {
            self.message = "No storage attached".into();
            return;
        };
        match lock(files).list_dir(&self.path) {
            Ok(list) => {
                self.entries = list
                    .into_iter()
                    .filter(|e| !e.name.starts_with('.'))
                    .map(|e| Entry {
                        name: e.name,
                        is_dir: e.stat.is_dir,
                        size: e.stat.size,
                    })
                    .collect();
                // Directories first; the host sorted by name.
                self.entries.sort_by_key(|e| !e.is_dir);
            }
            Err(e) => self.message = format!("{}: {e}", self.path),
        }
        self.cursor = self.cursor.min(self.entries.len().saturating_sub(1));
        self.scroll();
    }

    fn scroll(&mut self) {
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + LIST_ROWS {
            self.top = self.cursor + 1 - LIST_ROWS;
        }
    }

    fn step(&mut self, by: isize) {
        let last = self.entries.len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(by).min(last);
        self.scroll();
    }

    fn selected_path(&self) -> Option<String> {
        let e = self.entries.get(self.cursor)?;
        Some(if self.path == "/" {
            format!("/{}", e.name)
        } else {
            format!("{}/{}", self.path, e.name)
        })
    }

    fn context_items(&self) -> Vec<(&'static str, Cmd)> {
        let Some(e) = self.entries.get(self.cursor) else {
            return Vec::new();
        };
        let ext = extension(&e.name);
        match ext.as_str() {
            "d64" | "g64" | "d71" | "d81" => {
                vec![("Run Disk", Cmd::RunDisk), ("Mount Disk", Cmd::MountDisk)]
            }
            "prg" => vec![("Run", Cmd::RunPrg), ("Load", Cmd::LoadPrg)],
            "crt" => vec![("Run Cart", Cmd::RunCart)],
            // The product's own files, and `.cfw` files on every board.
            x if x == self.board.ext || x == "cfw" => vec![("Run Update", Cmd::RunUpdate)],
            _ => Vec::new(),
        }
    }

    fn tools_items() -> Vec<(&'static str, Cmd)> {
        vec![
            ("Reset C64", Cmd::Reset),
            ("Reboot C64", Cmd::Reboot),
            ("Remove Disk from Drive A", Cmd::Eject),
            ("System Information", Cmd::Info),
        ]
    }

    const CATEGORIES: [&'static str; 2] = ["U64 Specific Settings", "C64 and Cartridge Settings"];

    /// The settings of a category: name, value, changeable.
    fn config_items(&self, turbo: &Turbo, st: &Status) -> Vec<(&'static str, String, bool)> {
        let (mode, index, badlines) = turbo.menu();
        if self.cat == 0 {
            vec![
                (
                    "Turbo Control",
                    match mode {
                        TurboMode::Off => "Off",
                        TurboMode::Manual => "Manual",
                        TurboMode::Registers => "U64 Turbo Registers",
                        TurboMode::EnableBit => "TurboEnable Bit",
                    }
                    .into(),
                    true,
                ),
                (
                    "CPU Speed",
                    format!("{} MHz", turbo.speeds()[usize::from(index)]),
                    true,
                ),
                (
                    "Badline Timing",
                    if badlines { "Enabled" } else { "Disabled" }.into(),
                    true,
                ),
                (
                    "System Mode",
                    if st.ntsc { "NTSC" } else { "PAL" }.into(),
                    false,
                ),
            ]
        } else {
            let onoff = |on| if on { "Enabled" } else { "Disabled" }.to_string();
            vec![
                ("Command Interface", onoff(st.uci), false),
                ("RAM Expansion Unit", onoff(st.reu.is_some()), false),
                (
                    "REU Size",
                    st.reu.map_or("-".into(), |s| {
                        if s >= 1 << 20 {
                            format!("{} MB", s >> 20)
                        } else {
                            format!("{} KB", s >> 10)
                        }
                    }),
                    false,
                ),
            ]
        }
    }

    fn change_setting(&mut self, turbo: &mut Turbo, up: bool) {
        if self.cat != 0 {
            return;
        }
        let (mut mode, mut index, mut badlines) = turbo.menu();
        match self.sub {
            0 => {
                const MODES: [TurboMode; 4] = [
                    TurboMode::Off,
                    TurboMode::Manual,
                    TurboMode::Registers,
                    TurboMode::EnableBit,
                ];
                let i = MODES.iter().position(|&m| m == mode).unwrap_or(0);
                mode = MODES[(i + if up { 1 } else { 3 }) % 4];
            }
            1 => index = (index + if up { 1 } else { 15 }) % 16,
            2 => badlines = !badlines,
            _ => return,
        }
        turbo.set_menu(mode, index, badlines);
    }

    /// A key of the C64's keyboard while the menu is open.
    pub fn key(
        &mut self,
        key: MachineKey,
        down: bool,
        host: &HostServices,
        turbo: &mut Turbo,
    ) -> Action {
        let bit = match key {
            keys::LSHIFT => 1,
            keys::RSHIFT => 2,
            _ => 0,
        };
        if bit != 0 {
            self.shift = if down {
                self.shift | bit
            } else {
                self.shift & !bit
            };
            return Action::None;
        }
        if !down || !self.open {
            return Action::None;
        }
        let shift = self.shift != 0;
        let up = key == keys::CRSR_DOWN && shift;
        let down_key = key == keys::CRSR_DOWN && !shift;
        let left = key == keys::CRSR_RIGHT && shift;
        let right = key == keys::CRSR_RIGHT && !shift;
        let back = left || key == keys::DEL || key == keys::RUN_STOP;
        match self.view {
            View::Damaged => {}
            View::Update => match self.update.as_ref().map(|u| u.phase) {
                // "About to update. Continue?": RETURN or Y goes on.
                Some(Phase::Confirm) => {
                    if key == keys::RETURN || key == KEY_Y {
                        if let Some(u) = &mut self.update {
                            u.phase = Phase::Erase(ERASE_FRAMES);
                        }
                    } else if key == keys::RUN_STOP || key == KEY_N {
                        self.update = None;
                        self.view = View::Browser;
                    }
                }
                // The updater switches the machine off by itself.
                Some(Phase::Done) => {}
                Some(Phase::Failed) => {
                    self.update = None;
                    self.view = View::Browser;
                }
                _ => {}
            },
            View::Info | View::Help => self.view = View::Browser,
            View::Browser => {
                self.message.clear();
                if key == keys::RUN_STOP {
                    self.open = false;
                } else if up {
                    self.step(-1);
                } else if down_key {
                    self.step(1);
                } else if key == keys::F1 && shift {
                    self.view = View::Config;
                    self.sub = 0;
                } else if key == keys::F1 {
                    self.step(-(LIST_ROWS as isize));
                } else if key == keys::F7 {
                    self.step(LIST_ROWS as isize);
                } else if key == keys::F3 {
                    self.view = View::Help;
                } else if key == keys::F5 {
                    self.view = View::Tools;
                    self.sub = 0;
                } else if right || key == keys::RETURN {
                    match self.entries.get(self.cursor) {
                        Some(e) if e.is_dir => {
                            self.path = self.selected_path().unwrap_or_else(|| "/".into());
                            self.cursor = 0;
                            self.top = 0;
                            self.list(host);
                        }
                        Some(_) if self.context_items().is_empty() => {
                            self.message = "Nothing to do with this file".into();
                        }
                        Some(_) => {
                            self.view = View::Context;
                            self.sub = 0;
                        }
                        None => {}
                    }
                } else if (left || key == keys::DEL) && self.path != "/" {
                    let (parent, name) = self.path.rsplit_once('/').unwrap_or(("", ""));
                    let name = name.to_string();
                    self.path = if parent.is_empty() {
                        "/".into()
                    } else {
                        parent.into()
                    };
                    self.cursor = 0;
                    self.top = 0;
                    self.list(host);
                    if let Some(i) = self.entries.iter().position(|e| e.name == name) {
                        self.cursor = i;
                        self.scroll();
                    }
                }
            }
            View::Context | View::Tools => {
                let items = if self.view == View::Context {
                    self.context_items()
                } else {
                    Self::tools_items()
                };
                if back {
                    self.view = View::Browser;
                } else if up {
                    self.sub = self.sub.saturating_sub(1);
                } else if down_key {
                    self.sub = (self.sub + 1).min(items.len().saturating_sub(1));
                } else if key == keys::RETURN || right {
                    self.view = View::Browser;
                    if let Some(&(_, cmd)) = items.get(self.sub) {
                        return self.run(cmd, host);
                    }
                }
            }
            View::Config => {
                if back {
                    self.view = View::Browser;
                } else if up {
                    self.sub = self.sub.saturating_sub(1);
                } else if down_key {
                    self.sub = (self.sub + 1).min(Self::CATEGORIES.len() - 1);
                } else if key == keys::RETURN || right {
                    self.cat = self.sub;
                    self.sub = 0;
                    self.view = View::ConfigItems;
                }
            }
            View::ConfigItems => {
                if key == keys::RUN_STOP || key == keys::DEL {
                    self.sub = self.cat;
                    self.view = View::Config;
                } else if up {
                    self.sub = self.sub.saturating_sub(1);
                } else if down_key {
                    self.sub = (self.sub + 1).min(if self.cat == 0 { 3 } else { 2 });
                } else if right || key == PLUS || key == keys::RETURN || key == keys::SPACE {
                    self.change_setting(turbo, true);
                } else if left || key == MINUS {
                    self.change_setting(turbo, false);
                }
            }
        }
        Action::None
    }

    fn run(&mut self, cmd: Cmd, host: &HostServices) -> Action {
        let path = self.selected_path().unwrap_or_default();
        match cmd {
            Cmd::RunDisk | Cmd::MountDisk => {
                self.open = false;
                Action::Mount {
                    path,
                    run: cmd == Cmd::RunDisk,
                }
            }
            Cmd::RunPrg | Cmd::LoadPrg => {
                self.open = false;
                Action::Prg {
                    path,
                    run: cmd == Cmd::RunPrg,
                }
            }
            Cmd::RunCart => {
                self.open = false;
                Action::Cart { path }
            }
            Cmd::Reset => {
                self.open = false;
                Action::Reset
            }
            Cmd::Reboot => {
                self.open = false;
                Action::Reboot
            }
            Cmd::Eject => {
                self.message = "Disk removed".into();
                Action::Eject
            }
            Cmd::Info => {
                self.view = View::Info;
                Action::None
            }
            Cmd::RunUpdate => {
                self.start_update(path, host);
                Action::None
            }
        }
    }

    /// The machine could not do what the menu asked for: back to the
    /// browser with the reason.
    pub fn failed(&mut self, why: &str) {
        self.open = true;
        self.view = View::Browser;
        self.message = why.into();
    }

    fn start_update(&mut self, src: String, host: &HostServices) {
        let entry = self.entries.get(self.cursor).cloned();
        let name = entry.as_ref().map_or(String::new(), |e| e.name.clone());
        let size = entry.map_or(0, |e| e.size);
        let mut u = Update {
            src,
            name,
            size: size.min(u64::from(u32::MAX)) as u32,
            pos: 0,
            crc: u32::MAX,
            want: 0,
            phase: Phase::Check,
            error: String::new(),
            version: String::new(),
            carry: Vec::new(),
        };
        if size == 0 || size > 64 << 20 {
            u.phase = Phase::Failed;
            u.error = "Invalid Update Image".into();
        } else if host.files.is_none() {
            u.phase = Phase::Failed;
            u.error = "No storage attached".into();
        }
        self.update = Some(u);
        self.view = View::Update;
    }

    /// One frame of the updater.
    fn update_step(
        board: &Board,
        u: &mut Update,
        f: &mut dyn FileHost,
    ) -> Result<Option<Flash>, FsError> {
        let start = flash_header(&u.name, &u.version, u.size, u.want).len() as u64;
        match u.phase {
            Phase::Check => {
                let d = read_at(f, &u.src, u64::from(u.pos), CHUNK)?;
                if u.pos == 0 {
                    let cfw = extension(&u.name) == "cfw";
                    if let Err(why) = inspect_image(board, cfw, f, &u.src, u.size) {
                        return Err(FsError::Io(why));
                    }
                }
                if u.version.is_empty() {
                    u.carry.extend_from_slice(&d);
                    if let Some(v) = find_version(&u.carry) {
                        u.version = v;
                        u.carry.clear();
                    } else {
                        let keep = u.carry.len().min(128);
                        u.carry.drain(..u.carry.len() - keep);
                    }
                }
                u.crc = crc_update(u.crc, &d);
                u.pos += d.len() as u32;
                if u.pos >= u.size || d.is_empty() {
                    if u.pos != u.size {
                        return Err(FsError::Io("Invalid Update Image".into()));
                    }
                    u.want = !u.crc;
                    u.carry.clear();
                    u.phase = Phase::Confirm;
                }
            }
            Phase::Erase(n) => {
                if n == ERASE_FRAMES {
                    match f.mkdir(FLASH_DIR) {
                        Ok(()) | Err(FsError::Exists) => {}
                        Err(e) => return Err(e),
                    }
                    let head = flash_header(&u.name, &u.version, u.size, u.want);
                    write_at(f, FLASH_FILE, 0, &head, true)?;
                }
                u.phase = if n > 1 {
                    Phase::Erase(n - 1)
                } else {
                    u.pos = 0;
                    Phase::Program
                };
            }
            Phase::Program => {
                let d = read_at(f, &u.src, u64::from(u.pos), CHUNK)?;
                if d.is_empty() {
                    return Err(FsError::Io("The update file changed".into()));
                }
                write_at(f, FLASH_FILE, start + u64::from(u.pos), &d, false)?;
                u.pos += d.len() as u32;
                if u.pos >= u.size {
                    u.pos = 0;
                    u.crc = u32::MAX;
                    u.phase = Phase::Verify;
                }
            }
            Phase::Verify => {
                let d = read_at(f, FLASH_FILE, start + u64::from(u.pos), CHUNK)?;
                u.crc = crc_update(u.crc, &d);
                u.pos += d.len() as u32;
                if u.pos >= u.size || d.is_empty() {
                    if u.pos != u.size || !u.crc != u.want {
                        return Err(FsError::Io("Verify error".into()));
                    }
                    // From here `pos` counts the frames to switching off.
                    u.pos = 0;
                    u.phase = Phase::Done;
                    return Ok(Some(Flash::Installed {
                        name: u.name.clone(),
                        version: u.version.clone(),
                        size: u.size,
                        crc: u.want,
                    }));
                }
            }
            Phase::Confirm | Phase::Done | Phase::Failed => {}
        }
        Ok(None)
    }

    /// Once a frame: the flash is looked at after power-on, a running
    /// update goes on.
    pub fn frame(&mut self, host: &HostServices) -> Action {
        if self.flash == Flash::Unchecked {
            self.flash = match &host.files {
                Some(files) => read_flash(&mut *lock(files)),
                None => Flash::Factory,
            };
            if self.flash == Flash::Damaged {
                self.open = true;
                self.view = View::Damaged;
            }
        }
        let board = self.board;
        let Some(u) = &mut self.update else {
            return Action::None;
        };
        match u.phase {
            Phase::Confirm | Phase::Failed => return Action::None,
            // The updater switches the machine off five seconds later.
            Phase::Done => {
                u.pos += 1;
                if u.pos < OFF_FRAMES {
                    return Action::None;
                }
                self.update = None;
                self.open = false;
                self.view = View::Browser;
                return Action::Reboot;
            }
            _ => {}
        }
        let Some(files) = &host.files else {
            return Action::None;
        };
        let writing = !matches!(u.phase, Phase::Check);
        match Self::update_step(&board, u, &mut *lock(files)) {
            Ok(Some(flash)) => self.flash = flash,
            Ok(None) => {}
            Err(e) => {
                u.phase = Phase::Failed;
                u.error = e.to_string();
                if writing {
                    self.flash = Flash::Damaged;
                }
            }
        }
        Action::None
    }

    // ---- picture -------------------------------------------------------

    /// Draw the menu over the middle of a frame of `width` × `height`
    /// indexed pixels.
    pub fn render(
        &self,
        turbo: &Turbo,
        st: &Status,
        pixels: &mut [u8],
        width: usize,
        height: usize,
    ) {
        if width < COLS * 8 || height < ROWS * 8 || pixels.len() < width * height {
            return;
        }
        let mut s = Screen::new();
        let title = match &self.flash {
            Flash::Installed { version, .. } if !version.is_empty() => {
                format!("*** {} {version} ***", self.board.title)
            }
            _ => format!("*** {} ***", self.board.title),
        };
        s.fill(0, GREY, DARK);
        s.center(0, &title, WHITE, DARK);
        match self.view {
            View::Damaged => {
                s.center(4, "NO VALID FIRMWARE", RED, BLACK);
                s.text(7, 2, "The flash holds an update that was", GREY, BLACK);
                s.text(8, 2, "cut short or does not verify. A real", GREY, BLACK);
                s.text(9, 2, "board does not start now and needs", GREY, BLACK);
                s.text(10, 2, "its recovery procedure.", GREY, BLACK);
                s.text(13, 2, "Here: delete Flash/.firmware in the", GREY, BLACK);
                s.text(14, 2, "Ultimate folder and switch the", GREY, BLACK);
                s.text(15, 2, "machine on again.", GREY, BLACK);
            }
            View::Update => self.render_update(&mut s),
            View::Info => self.render_info(&mut s, turbo, st),
            View::Help => {
                const HELP: [&str; 12] = [
                    "Cursor Keys:Up/Left/Down/Right",
                    "  (Up/Down) Selection up/down",
                    "  (Left)    Go one level up",
                    "            leave directory or disk",
                    "  (Right)   Enter selected item",
                    "SPACE /     Select / change item",
                    "  RETURN",
                    "+ / -       Increase/Decrease value",
                    "F1          Page Up",
                    "F7          Page Down",
                    "F3          Show this help text",
                    "RUN/STOP:   Exit",
                ];
                s.center(2, "Setup Menu Help", YELLOW, BLACK);
                for (i, l) in HELP.iter().enumerate() {
                    s.text(4 + i, 2, l, GREY, BLACK);
                }
            }
            View::Config | View::ConfigItems => {
                s.text(1, 0, "Configuration", YELLOW, BLACK);
                if self.view == View::Config {
                    for (i, c) in Self::CATEGORIES.iter().enumerate() {
                        s.line(3 + i, 1, c, i == self.sub);
                    }
                } else {
                    s.text(2, 1, Self::CATEGORIES[self.cat], WHITE, BLACK);
                    for (i, (name, value, can)) in
                        self.config_items(turbo, st).into_iter().enumerate()
                    {
                        let l = format!("{name:<19}{value:>19}");
                        if i == self.sub {
                            s.line(4 + i, 1, &l, true);
                        } else {
                            s.text(4 + i, 1, &l, if can { GREY } else { DARK }, BLACK);
                        }
                    }
                }
                s.text(24, 0, "CRSR/+/- change  RUN/STOP back", GREY, DARK);
            }
            View::Browser | View::Context | View::Tools => {
                s.text(1, 0, &self.path, YELLOW, BLACK);
                for (row, (i, e)) in self
                    .entries
                    .iter()
                    .enumerate()
                    .skip(self.top)
                    .take(LIST_ROWS)
                    .enumerate()
                {
                    let size = if e.is_dir {
                        "DIR".to_string()
                    } else if e.size >= 10 << 20 {
                        format!("{}M", e.size >> 20)
                    } else if e.size >= 10 << 10 {
                        format!("{}K", e.size >> 10)
                    } else {
                        e.size.to_string()
                    };
                    let mut name: String = e.name.chars().take(30).collect();
                    if e.name.chars().count() > 30 {
                        name.pop();
                        name.push('~');
                    }
                    let l = format!("{name:<31}{size:>7}");
                    if i == self.cursor {
                        s.line(2 + row, 1, &l, true);
                    } else {
                        s.text(2 + row, 1, &l, if e.is_dir { WHITE } else { GREY }, BLACK);
                    }
                }
                if self.entries.is_empty() && self.message.is_empty() {
                    s.text(3, 1, "< No Items >", DARK, BLACK);
                }
                s.text(23, 0, &self.message, YELLOW, BLACK);
                s.fill(24, GREY, DARK);
                s.text(
                    24,
                    0,
                    "F2 Config F5 Tools F3 Help RUN/STOP Exit",
                    GREY,
                    DARK,
                );
                if self.view != View::Browser {
                    let (title, items) = if self.view == View::Context {
                        ("File", self.context_items())
                    } else {
                        ("Tools", Self::tools_items())
                    };
                    let w = items
                        .iter()
                        .map(|i| i.0.len())
                        .max()
                        .unwrap_or(0)
                        .max(title.len())
                        + 4;
                    let (r0, c0) = (8, (COLS - w) / 2);
                    for r in 0..items.len() + 2 {
                        s.text(r0 + r, c0, &" ".repeat(w), WHITE, DARK);
                    }
                    s.text(r0, c0 + 1, title, YELLOW, DARK);
                    for (i, (label, _)) in items.iter().enumerate() {
                        let l = format!(" {label:<width$}", width = w - 3);
                        if i == self.sub {
                            s.text(r0 + 1 + i, c0 + 1, &l, DARK, WHITE);
                        } else {
                            s.text(r0 + 1 + i, c0 + 1, &l, WHITE, DARK);
                        }
                    }
                }
            }
        }
        s.blit(pixels, width, height);
    }

    fn flash_lines(&self) -> [String; 2] {
        match &self.flash {
            Flash::Unchecked | Flash::Factory => ["Factory firmware".into(), String::new()],
            Flash::Installed {
                name,
                version,
                size,
                crc,
            } => [
                if version.is_empty() {
                    name.clone()
                } else {
                    format!("{version} ({name})")
                },
                format!("{size} bytes, CRC {crc:08X}"),
            ],
            Flash::Damaged => ["DAMAGED".into(), String::new()],
        }
    }

    fn render_info(&self, s: &mut Screen, turbo: &Turbo, st: &Status) {
        s.center(2, "System Information", YELLOW, BLACK);
        let [fw, detail] = self.flash_lines();
        let reu = st
            .reu
            .map_or("off".to_string(), |r| format!("{} KB", r >> 10));
        let lines = [
            format!("Product   {}", self.board.title),
            format!("Firmware  {fw}"),
            format!("          {detail}"),
            format!("Video     {}", if st.ntsc { "NTSC" } else { "PAL" }),
            format!("CPU       {} MHz", turbo.factor()),
            format!("Badlines  {}", if turbo.badlines() { "on" } else { "off" }),
            format!("UCI       {}", if st.uci { "on" } else { "off" }),
            format!("REU       {reu}"),
            format!("Drive A   {}", if st.disk { "disk" } else { "no disk" }),
        ];
        for (i, l) in lines.iter().enumerate() {
            s.text(4 + i, 2, l, GREY, BLACK);
        }
        s.text(24, 0, "Press a key", GREY, DARK);
    }

    fn render_update(&self, s: &mut Screen) {
        let Some(u) = &self.update else {
            return;
        };
        s.center(2, self.board.updater, YELLOW, BLACK);
        s.text(4, 2, &format!("File     {}", u.name), GREY, BLACK);
        s.text(5, 2, &format!("Size     {} bytes", u.size), GREY, BLACK);
        if !u.version.is_empty() {
            s.text(6, 2, &format!("Version  {}", u.version), GREY, BLACK);
        }
        let rank = match u.phase {
            Phase::Check => 0,
            Phase::Confirm => 1,
            Phase::Erase(_) => 1,
            Phase::Program => 2,
            Phase::Verify => 3,
            Phase::Done => 4,
            Phase::Failed => 9,
        };
        let part = |pos: u32| (u64::from(pos) * 100 / u64::from(u.size.max(1))) as usize;
        let percent = match u.phase {
            Phase::Erase(n) => usize::from(ERASE_FRAMES - n) * 100 / usize::from(ERASE_FRAMES),
            Phase::Check | Phase::Program | Phase::Verify => part(u.pos),
            Phase::Confirm => 0,
            Phase::Done | Phase::Failed => 100,
        };
        const STEPS: [&str; 4] = [
            "Checking the update image..",
            "Erasing flash..",
            "Programming flash..",
            "Verifying..",
        ];
        for (i, step) in STEPS.iter().enumerate() {
            let (mark, colour) = match rank {
                9 => ("  ", DARK),
                r if r > i => ("OK", GREEN),
                r if r == i && u.phase != Phase::Confirm => ("..", WHITE),
                _ => ("  ", DARK),
            };
            s.text(8 + i, 2, &format!("{step:<28}{mark}"), colour, BLACK);
        }
        if rank < 4 && u.phase != Phase::Confirm {
            let filled = percent * 30 / 100;
            let bar = format!("[{}{}]", "#".repeat(filled), "-".repeat(30 - filled));
            s.text(14, 2, &bar, WHITE, BLACK);
            s.text(14, 35, &format!("{percent:>3}%"), WHITE, BLACK);
        }
        match u.phase {
            Phase::Confirm => {
                s.center(15, "About to update. Continue?", WHITE, BLACK);
                s.center(17, "RETURN = Yes     RUN/STOP = No", GREY, BLACK);
            }
            Phase::Erase(_) | Phase::Program | Phase::Verify => {
                s.center(17, "DO NOT SWITCH OFF", RED, BLACK);
            }
            Phase::Done => {
                s.center(16, "Done!", GREEN, BLACK);
                s.center(18, "Turning OFF machine in 5 seconds....", WHITE, BLACK);
            }
            Phase::Failed => {
                let msg: String = u.error.chars().take(38).collect();
                s.center(15, &msg, RED, BLACK);
                if self.flash == Flash::Damaged {
                    s.center(18, "The flash is damaged now", RED, BLACK);
                } else {
                    s.center(17, "Not flashing.", WHITE, BLACK);
                }
                s.center(22, "Press a key", GREY, BLACK);
            }
            Phase::Check => {}
        }
    }

    // ---- savestate -----------------------------------------------------

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"U64M", 1, |w| {
            w.bool(self.open);
            w.u8(VIEWS.iter().position(|&v| v == self.view).unwrap_or(0) as u8);
            w.str(&self.path);
            w.u32(self.entries.len() as u32);
            for e in &self.entries {
                w.str(&e.name);
                w.bool(e.is_dir);
                w.u64(e.size);
            }
            w.u32(self.cursor as u32);
            w.u32(self.top as u32);
            w.u32(self.sub as u32);
            w.u8(self.cat as u8);
            w.u8(self.shift);
            w.str(&self.message);
            match &self.flash {
                Flash::Unchecked => w.u8(0),
                Flash::Factory => w.u8(1),
                Flash::Damaged => w.u8(2),
                Flash::Installed {
                    name,
                    version,
                    size,
                    crc,
                } => {
                    w.u8(3);
                    w.str(name);
                    w.str(version);
                    w.u32(*size);
                    w.u32(*crc);
                }
            }
            w.bool(self.update.is_some());
            if let Some(u) = &self.update {
                w.str(&u.src);
                w.str(&u.name);
                w.u32(u.size);
                w.u32(u.pos);
                w.u32(u.crc);
                w.u32(u.want);
                let (phase, n) = match u.phase {
                    Phase::Check => (0, 0),
                    Phase::Erase(n) => (1, n),
                    Phase::Program => (2, 0),
                    Phase::Verify => (3, 0),
                    Phase::Done => (4, 0),
                    Phase::Failed => (5, 0),
                    Phase::Confirm => (6, 0),
                };
                w.u8(phase);
                w.u8(n);
                w.str(&u.error);
                w.str(&u.version);
                w.bytes(&u.carry);
            }
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"U64M", 1)?;
        let open = c.bool()?;
        let view = *VIEWS
            .get(usize::from(c.u8()?))
            .ok_or(StateError::Invalid("menu view"))?;
        let path = c.string()?;
        let n = c.u32()?;
        let mut entries = Vec::new();
        for _ in 0..n {
            entries.push(Entry {
                name: c.string()?,
                is_dir: c.bool()?,
                size: c.u64()?,
            });
        }
        let cursor = c.u32()? as usize;
        let top = c.u32()? as usize;
        let sub = c.u32()? as usize;
        let cat = usize::from(c.u8()?);
        let shift = c.u8()?;
        let message = c.string()?;
        let flash = match c.u8()? {
            0 => Flash::Unchecked,
            1 => Flash::Factory,
            2 => Flash::Damaged,
            3 => Flash::Installed {
                name: c.string()?,
                version: c.string()?,
                size: c.u32()?,
                crc: c.u32()?,
            },
            _ => return Err(StateError::Invalid("menu flash")),
        };
        let update = if c.bool()? {
            let (src, name) = (c.string()?, c.string()?);
            let (size, pos, crc, want) = (c.u32()?, c.u32()?, c.u32()?, c.u32()?);
            let phase = match (c.u8()?, c.u8()?) {
                (0, _) => Phase::Check,
                (1, n) if (1..=ERASE_FRAMES).contains(&n) => Phase::Erase(n),
                (2, _) => Phase::Program,
                (3, _) => Phase::Verify,
                (4, _) => Phase::Done,
                (5, _) => Phase::Failed,
                (6, _) => Phase::Confirm,
                _ => return Err(StateError::Invalid("menu update phase")),
            };
            Some(Update {
                src,
                name,
                size,
                pos,
                crc,
                want,
                phase,
                error: c.string()?,
                version: c.string()?,
                carry: c.bytes()?.to_vec(),
            })
        } else {
            None
        };
        if cat >= Self::CATEGORIES.len() || (view == View::Update && update.is_none()) {
            return Err(StateError::Invalid("menu state"));
        }
        *self = Self {
            board: self.board,
            open,
            view,
            path,
            entries,
            cursor,
            top,
            sub,
            cat,
            shift,
            message,
            flash,
            update,
        };
        Ok(())
    }
}

/// The menu's 40 × 25 characters with their colours.
struct Screen {
    cells: [[(u8, u8, u8); COLS]; ROWS],
}

impl Screen {
    fn new() -> Self {
        Self {
            cells: [[(b' ', GREY, BLACK); COLS]; ROWS],
        }
    }

    fn fill(&mut self, row: usize, fg: u8, bg: u8) {
        self.cells[row] = [(b' ', fg, bg); COLS];
    }

    fn text(&mut self, row: usize, col: usize, text: &str, fg: u8, bg: u8) {
        if row >= ROWS {
            return;
        }
        for (i, ch) in text.chars().enumerate() {
            let Some(cell) = self.cells[row].get_mut(col + i) else {
                break;
            };
            let ch = if ch.is_ascii() && !ch.is_ascii_control() {
                ch as u8
            } else {
                b'?'
            };
            *cell = (ch, fg, bg);
        }
    }

    fn center(&mut self, row: usize, text: &str, fg: u8, bg: u8) {
        let n = text.chars().count().min(COLS);
        self.text(row, (COLS - n) / 2, text, fg, bg);
    }

    /// A list line, reversed under the cursor.
    fn line(&mut self, row: usize, col: usize, text: &str, selected: bool) {
        if selected {
            self.text(row, col, text, BLACK, WHITE);
        } else {
            self.text(row, col, text, GREY, BLACK);
        }
    }

    fn blit(&self, pixels: &mut [u8], width: usize, height: usize) {
        let x0 = (width - COLS * 8) / 2;
        let y0 = (height - ROWS * 8) / 2;
        for (r, row) in self.cells.iter().enumerate() {
            for (c, &(ch, fg, bg)) in row.iter().enumerate() {
                for y in 0..8 {
                    let bits = mimic_core::font::row(ch, y as u8);
                    let at = (y0 + r * 8 + y) * width + x0 + c * 8;
                    for (x, p) in pixels[at..at + 8].iter_mut().enumerate() {
                        *p = if bits & (0x80 >> x) != 0 { fg } else { bg };
                    }
                }
            }
        }
    }
}
