//! The WiFi DMA UART (`$060900`–`$06090F`) and, behind it, a stand-in for
//! the board's ESP32 control module ("u64ctrl").
//!
//! The UART moves whole packets between the board's memory and the serial
//! line by itself: the firmware hands it the address and length of a
//! packet to send, and addresses of buffers to receive into. The processor
//! never sees single bytes, so this model works on packets too; baud rate
//! and flow control have no effect the firmware could observe beyond
//! reading the bits back, and the SLIP bit only decides how received
//! bytes are cut into packets (see "Without SLIP").
//!
//! Registers (offset from `$060900`; a 16- or 32-bit access arrives as
//! bytes, lowest address first):
//!
//! | Offset | Read | Write |
//! |---|---|---|
//! | 0 | baud divisor bits 7–0 | the same |
//! | 1 | divisor bits 9–8 | divisor bits 10–8 |
//! | 2 | status, see below | interrupt enables: bit 7 = 1 sets, = 0 clears the enables named by bit 0 (packet received), bit 1 (transmitter free), bit 2 (receive address wanted); bit 3 clears the overflow flag |
//! | 3 | bits 0–2 and 4–6 as written, bits 3 and 7 always 0 | bit 0 hardware flow control, bit 1 loopback, bit 2 SLIP, bits 6–4 module mode, bit 7 soft reset (strobe) |
//! | 4–7 | 0 | address of the packet to send (28 bits kept) |
//! | 8–9 | length of the packet received | length of the packet to send |
//! | A | 0 | any value: send |
//! | B | 0 | any value: the received packet has been taken |
//! | C–F | 0 | address of a receive buffer; writing byte F hands it over |
//!
//! Status: bit 0 = receive enable and a packet is reported, bit 1 =
//! transmit enable and the transmitter is free, bit 2 = address enable and
//! no receive address is waiting, bit 3 overflow (never here), bit 4 the
//! module is ready to take data (always here), bit 5 a packet is reported,
//! bit 6 the transmitter is free, bit 7 no receive address is waiting.
//! Bits 0–2 together are the interrupt line, high interrupt source 3 of
//! the ITU. There is no acknowledge register: the line falls when its
//! cause is removed. All of it is computed from the present state at every
//! read, which is what keeps the firmware's interrupt routine from looping
//! (it repeats while bits 0–2 are not all 0).
//!
//! The receiver holds one address in its register and one it has already
//! taken, writes a packet to the taken one as soon as the packet arrives,
//! and reports its length once the previous report has been taken. Packets
//! from the module are never dropped for want of a buffer; they wait.
//!
//! The module answers every request except "send network packet" with the
//! request's own four header bytes (command, thread, sequence) in front,
//! about a millisecond later. It reports its WiFi as switched on and not
//! connected, there is no network behind it. Requests to switch the
//! machine off or to restart it are answered and, a second later, handed
//! to the machine through [`Wifi::take_power_event`].
//!
//! Where the FPGA is built to decode the module's button events (the
//! Commodore 64 Ultimate's rocker switch: [`Wifi::set_events`]), a packet
//! from the module whose first byte has bit 7 set never reaches memory;
//! it comes out of [`Wifi::take_button_event`] and the machine applies it
//! (menu and freeze buttons of the ITU, C64 reset, each for 200 ms).
//! Whether the Ultimate 64 Elite II's FPGA does so is not known; its menu
//! button is the ITU's button register, so the decode is off by default.
//!
//! # Without SLIP
//!
//! With the SLIP bit clear the receiver passes the line's bytes as they
//! are and ends a packet only after 1536 of them. Setting the bit ends
//! the piece collected so far with a line feed (`$0A`) added. That is how
//! the firmware reads the text the module's ROM prints. Text that arrives
//! while the bit is set is lost (the receiver waits for a frame's start),
//! and a frame that arrives while it is clear is seen with its framing
//! (`$C0`, escapes). The transmitter does the framing itself, so the
//! module here gets and gives whole frames' contents.
//!
//! # The module's ROM loader
//!
//! The module is an ESP32-S3 with 8 MiB of flash. Module mode 1 (and 5)
//! holds its boot pin while it is released from reset: the ROM prints its
//! greeting (it contains `DOWNLOAD`, which the firmware looks for) and
//! then speaks Espressif's serial loader protocol, with which the
//! firmware's updater writes new software into the module. A request
//! frame is: 0 `$00`, 1 command, 2–3 length of the data, 4–7 checksum
//! (only "flash data" uses it: all data bytes after its 16 header bytes
//! and `$EF` exclusive-ored), 8… data, all numbers least significant byte
//! first. The answer: 0 `$01`, 1 the command, 2–3 `$0004`, 4–7 a value,
//! then four status bytes: failed (0 or 1), reason, 0, 0. Reasons: 5 the
//! request is malformed or unknown, 6 it could not be done, 7 wrong
//! checksum.
//!
//! | Command | Data (32-bit words) | Here |
//! |---|---|---|
//! | `$08` sync | `07 07 12 20` and 32 times `$55` | answered eight times, value `$20120707` |
//! | `$0D` attach flash | pins (0), a reserved word | accepted |
//! | `$0F` change baud rate | new rate, 0 | accepted (rates have no effect here) |
//! | `$0B` flash parameters | id, size, block, sector, page size, status mask | accepted |
//! | `$02` flash begin | size, number of blocks, block size, offset, and on this chip a fifth word (0: not encrypted) | erases the sectors the size covers, opens the transfer |
//! | `$03` flash data | length, block number, 0, 0, then the bytes | written at offset + block number × block size |
//! | `$04` flash end | 0 restart, 1 stay | closes the transfer |
//! | `$0A` read register | address | value 9 at `$40001000` (the chip's kind), else 0 |
//!
//! Everything else (memory download with which a tool would start a
//! "stub" loader, MD5, compressed data, security information) answers
//! reason 5; the firmware's updater uses none of it and never sends
//! "flash end" either.
//!
//! When the module is next started in a run mode, the version text in
//! what was written (`ESP32 WiFi Bridge V1.14`, [`identity_in_image`])
//! becomes the version the stand-in reports, and
//! [`Wifi::take_identity_changed`] tells the machine once, which keeps it
//! for the next run ([`Wifi::set_identity`]). The written bytes are
//! dropped at that start: nothing can read them back, they are part of a
//! savestate only during an update and are not meant to be kept by the
//! machine. An updater does not flash when the module's version is the
//! one it carries.
//!
//! Sources (facts only): UE2-C64U-Emulator `docs/hw/04-esp32-wifi.md`
//! (registers, hazards H1–H11, request and reply layouts),
//! `docs/specs/S05-wifi-u64ctrl.md`, `S42-esp32-rocker-events.md`,
//! `docs/status/install.md` §5; 1541 Ultimate
//! `fpga/io/uart_lite/vhdl_source/uart_dma.vhd`, `rx_dma.vhd`,
//! `tx_dma.vhd`, `software/io/uart/dma_uart.h`/`.cc`,
//! `software/u64ctrl/main/rpc_calls.h`, `rpc_dispatch.h`,
//! `software/io/wifi/esp32.h`, `esp32.cc` (the updater's loader
//! sequence), `wifi_cmd.h`,
//! `software/application/update_u2p/update_u64ii.cc` (what is flashed
//! where), `fpga/io/uart_lite/vhdl_source/slip_decoder.vhd` (reception
//! without SLIP), `software/u64ctrl/sdkconfig` (chip, flash size),
//! `software/u64ctrl/main/rpc_dispatch.h` (version text); Espressif,
//! esptool documentation, "Serial Protocol" (commands, layout, checksum,
//! status bytes).

use crate::io::{IoCtx, IoDevice, CLOCK_HZ};
use mimic_core::{StateError, StateReader, StateWriter};
use std::collections::{BTreeMap, VecDeque};

/// First address of the block.
pub const BASE: u32 = 0x06_0900;
/// The ITU's high interrupt source of this block.
pub const IRQ_HIGH: u8 = 3;

/// Clocks from "send" until the transmitter is free again (the packet has
/// been copied out of memory).
const TX_CLOCKS: u64 = 2_000;
/// Clocks from a request to its answer: 1 ms.
const REPLY_CLOCKS: u64 = CLOCK_HZ / 1_000;
/// Clocks from a power request to the module acting on it: 1 s.
const POWER_CLOCKS: u64 = CLOCK_HZ;
/// Clocks from the module's reset into its ROM loader to the ROM's
/// greeting: 10 ms.
const BOOT_CLOCKS: u64 = CLOCK_HZ / 100;
/// The longest packet the receiver writes; a longer one is cut. Without
/// SLIP: the number of bytes that make a packet.
const MAX_PACKET: usize = 1536;
/// The module's flash: 8 MiB in sectors of 4 KiB.
const FLASH_SIZE: u32 = 0x80_0000;
const FLASH_SECTOR: u32 = 0x1000;
/// What the ESP32-S3's ROM prints when it starts with the boot pin held.
const ROM_GREETING: &[u8] = b"ESP-ROM:esp32s3-20210327\r\nBuild:Mar 27 2021\r\n\
rst:0x1 (POWERON),boot:0x0 (DOWNLOAD(USB/UART0))\r\nwaiting for download\r\n";
/// The DMA's address width: 26 bits.
const DMA_MASK: u32 = 0x03FF_FFFF;
/// More waiting packets than this are not kept (corrupt savestates).
const MAX_QUEUE: usize = 4096;

/// ESP-IDF's error numbers in the replies.
const ESP_OK: u32 = 0;
const ESP_ERR_INVALID_ARG: u32 = 0x102;
const ESP_ERR_NOT_SUPPORTED: u32 = 0x106;

/// Module modes in bits 6–4 of the flow control register (Elite II
/// encoding) in which the module's own program does not run.
const MODE_BOOT: u8 = 1;
const MODE_OFF: u8 = 3;
const MODE_BOOT_UART: u8 = 5;

/// What the module does in a module mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Module {
    /// Its own program runs.
    Run,
    /// Its ROM's serial loader runs.
    Loader,
    /// Held in reset.
    Off,
}

fn module_in(flow: u8) -> Module {
    match (flow >> 4) & 7 {
        MODE_BOOT | MODE_BOOT_UART => Module::Loader,
        MODE_OFF => Module::Off,
        _ => Module::Run,
    }
}

/// The ROM loader's commands and the reasons in its status bytes.
mod rom {
    pub const FLASH_BEGIN: u8 = 0x02;
    pub const FLASH_DATA: u8 = 0x03;
    pub const FLASH_END: u8 = 0x04;
    pub const SYNC: u8 = 0x08;
    pub const READ_REG: u8 = 0x0A;
    pub const SPI_SET_PARAMS: u8 = 0x0B;
    pub const SPI_ATTACH: u8 = 0x0D;
    pub const CHANGE_BAUDRATE: u8 = 0x0F;
    /// The value in the answers to a sync.
    pub const SYNC_VALUE: u32 = 0x2012_0707;
    /// The register that tells the chip's kind, and the ESP32-S3's value.
    pub const CHIP_MAGIC_REG: u32 = 0x4000_1000;
    pub const CHIP_MAGIC_S3: u32 = 9;
    pub const ERR_INVALID: u8 = 5;
    pub const ERR_FAILED: u8 = 6;
    pub const ERR_CHECKSUM: u8 = 7;
}

/// Requests (first byte of a packet to the module).
mod cmd {
    pub const ECHO: u8 = 0x01;
    pub const IDENTIFY: u8 = 0x02;
    pub const SET_BAUD: u8 = 0x03;
    pub const WIFI_SCAN: u8 = 0x04;
    pub const WIFI_DISCONNECT: u8 = 0x06;
    pub const WIFI_GETMAC: u8 = 0x07;
    pub const SEND_PACKET: u8 = 0x08;
    pub const MODEM_ON: u8 = 0x09;
    pub const MODEM_OFF: u8 = 0x0A;
    pub const WIFI_IS_CONNECTED: u8 = 0x0B;
    pub const GET_VOLTAGES: u8 = 0x0C;
    pub const WIFI_ENABLE: u8 = 0x0D;
    pub const WIFI_DISABLE: u8 = 0x0E;
    pub const MACHINE_OFF: u8 = 0x0F;
    pub const CLEAR_APS: u8 = 0x11;
    pub const WIFI_AUTOCONNECT: u8 = 0x12;
    pub const MACHINE_REBOOT: u8 = 0x13;
    pub const SET_SERIAL: u8 = 0x14;
    pub const GET_SERIAL: u8 = 0x15;
    pub const SET_POWER_MODE: u8 = 0x16;
    pub const GET_POWER_MODE: u8 = 0x17;
    pub const SET_WAKE_ON_WIFI: u8 = 0x18;
    pub const GET_WAKE_ON_WIFI: u8 = 0x19;
    pub const SET_RECOVERY_URL: u8 = 0x1A;
    pub const GET_RECOVERY_URL: u8 = 0x1B;
    pub const EVENT_DISCONNECTED: u8 = 0x42;
    pub const EVENT_DISABLED: u8 = 0x46;
}

/// What the firmware asked the module to do with the machine's power.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerEvent {
    /// Switch the machine off.
    Off,
    /// Switch it off and on again: a cold start of FPGA and firmware.
    Reboot,
}

/// A gesture on the Commodore 64 Ultimate's rocker switch, which the
/// module owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rocker {
    UpShort,
    UpLong,
    DownShort,
    DownLong,
}

/// What a decoded button event does on the board, each for 200 ms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonEvent {
    /// ITU button 1 (button register bit 6): the menu.
    Menu,
    /// ITU button 2 (button register bit 7): freeze.
    Freeze,
    /// The C64's reset line.
    Reset,
}

/// What the module keeps in its own non-volatile memory. The machine may
/// read it out and put it back to make it outlast a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleSettings {
    /// What happens when power returns: 0 stay off, 1 switch on, 2 as it
    /// was.
    pub power_mode: u8,
    /// 1: the machine was on at the last power change.
    pub last_state: u8,
    /// Wake on WiFi: 0 or 1.
    pub wake_on_wifi: u8,
}

impl Default for ModuleSettings {
    fn default() -> Self {
        Self {
            power_mode: 0,
            last_state: 1,
            wake_on_wifi: 0,
        }
    }
}

/// The version an image of firmware or of an updater carries for the
/// control module: the numbers after the text `ESP32 WiFi Bridge V`.
/// Given to [`Wifi::set_identity`], the updater in that image finds the
/// module up to date.
pub fn identity_in_image(image: &[u8]) -> Option<(u16, u16)> {
    const KEY: &[u8] = b"ESP32 WiFi Bridge V";
    let at = image.windows(KEY.len()).position(|w| w == KEY)?;
    let rest = &image[at + KEY.len()..];
    let number = |s: &[u8]| -> Option<(u16, usize)> {
        let n = s.iter().take_while(|c| c.is_ascii_digit()).count();
        if n == 0 || n > 4 {
            return None;
        }
        let v = s[..n]
            .iter()
            .fold(0u16, |a, c| a * 10 + u16::from(c - b'0'));
        Some((v, n))
    };
    let (major, n) = number(rest)?;
    if rest.get(n) != Some(&b'.') {
        return None;
    }
    let (minor, _) = number(&rest[n + 1..])?;
    Some((major, minor))
}

/// A packet on its way from the module.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Packet {
    /// The clock from which it is on the line.
    due: u64,
    /// Bytes without framing (the ROM's text) instead of a frame's
    /// contents.
    raw: bool,
    data: Vec<u8>,
}

/// A frame's contents as they are on the line.
fn slip_encode(data: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(data.len() + 2);
    v.push(0xC0);
    for &b in data {
        match b {
            0xC0 => v.extend_from_slice(&[0xDB, 0xDC]),
            0xDB => v.extend_from_slice(&[0xDB, 0xDD]),
            b => v.push(b),
        }
    }
    v.push(0xC0);
    v
}

/// A transfer "flash begin" has opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Transfer {
    offset: u32,
    blocks: u32,
    block_size: u32,
}

/// The serial loader in the module's ROM.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct RomLoader {
    transfer: Option<Transfer>,
    /// The sectors written since the module's program last started:
    /// sector number → its 4096 bytes. A sector not in here has not been
    /// written (erased, or as it was).
    flash: BTreeMap<u32, Vec<u8>>,
}

impl RomLoader {
    fn answer(op: u8, value: u32, error: Option<u8>) -> Vec<u8> {
        let mut v = vec![0x01, op, 4, 0];
        v.extend_from_slice(&value.to_le_bytes());
        v.extend_from_slice(&[u8::from(error.is_some()), error.unwrap_or(0), 0, 0]);
        v
    }

    /// One request frame; the answers in order.
    fn command(&mut self, req: &[u8]) -> Vec<Vec<u8>> {
        // Not a request: the ROM waits for the next frame.
        if req.len() < 8 || req[0] != 0 {
            return Vec::new();
        }
        let op = req[1];
        let body = &req[8..];
        let checksum = u32::from_le_bytes([req[4], req[5], req[6], req[7]]);
        let word = |i: usize| {
            body.get(4 * i..4 * i + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let ok = |value: u32| vec![Self::answer(op, value, None)];
        let fail = |reason: u8| vec![Self::answer(op, 0, Some(reason))];
        if usize::from(u16::from_le_bytes([req[2], req[3]])) != body.len() {
            return fail(rom::ERR_INVALID);
        }
        match (op, body.len()) {
            (rom::SYNC, 36) => (0..8)
                .map(|_| Self::answer(op, rom::SYNC_VALUE, None))
                .collect(),
            // Nothing to do for these: the flash is there, the rate has
            // no effect, the parameters describe what is modelled anyway.
            (rom::SPI_ATTACH, 4 | 8) | (rom::CHANGE_BAUDRATE, 8) | (rom::SPI_SET_PARAMS, 24) => {
                ok(0)
            }
            (rom::FLASH_BEGIN, 16 | 20) => {
                let (Some(size), Some(blocks), Some(block_size), Some(offset)) =
                    (word(0), word(1), word(2), word(3))
                else {
                    return fail(rom::ERR_INVALID);
                };
                let room = u64::from(FLASH_SIZE.saturating_sub(offset));
                if block_size == 0
                    || offset >= FLASH_SIZE
                    || u64::from(size) > room
                    || u64::from(blocks) * u64::from(block_size) > room
                {
                    return fail(rom::ERR_FAILED);
                }
                if size > 0 {
                    // Erased: whole sectors, as if never written.
                    let first = offset / FLASH_SECTOR;
                    let last = (offset + size - 1) / FLASH_SECTOR;
                    self.flash.retain(|&s, _| s < first || s > last);
                }
                self.transfer = Some(Transfer {
                    offset,
                    blocks,
                    block_size,
                });
                ok(0)
            }
            (rom::FLASH_DATA, 16..) => {
                let (Some(len), Some(seq)) = (word(0), word(1)) else {
                    return fail(rom::ERR_INVALID);
                };
                let data = &body[16..];
                if len as usize != data.len() {
                    return fail(rom::ERR_INVALID);
                }
                if u32::from(data.iter().fold(0xEF, |a, b| a ^ b)) != checksum {
                    return fail(rom::ERR_CHECKSUM);
                }
                let Some(t) = self.transfer else {
                    return fail(rom::ERR_FAILED);
                };
                if seq >= t.blocks || len > t.block_size {
                    return fail(rom::ERR_FAILED);
                }
                // In range: "flash begin" checked the whole transfer.
                self.program(t.offset + seq * t.block_size, data);
                ok(0)
            }
            (rom::FLASH_END, 4) => {
                self.transfer = None;
                ok(0)
            }
            (rom::READ_REG, 4) => ok(if word(0) == Some(rom::CHIP_MAGIC_REG) {
                rom::CHIP_MAGIC_S3
            } else {
                0
            }),
            _ => fail(rom::ERR_INVALID),
        }
    }

    /// Programming can only clear bits.
    fn program(&mut self, at: u32, data: &[u8]) {
        for (i, &b) in data.iter().enumerate() {
            let a = at.saturating_add(i as u32);
            if a >= FLASH_SIZE {
                break;
            }
            let sector = self
                .flash
                .entry(a / FLASH_SECTOR)
                .or_insert_with(|| vec![0xFF; FLASH_SECTOR as usize]);
            sector[(a % FLASH_SECTOR) as usize] &= b;
        }
    }

    /// The version text in what has been written, lowest address first.
    fn flashed_identity(&self) -> Option<(u16, u16)> {
        let mut run: Vec<u8> = Vec::new();
        let mut next = None;
        for (&s, data) in &self.flash {
            if next != Some(s) {
                if let Some(id) = identity_in_image(&run) {
                    return Some(id);
                }
                run.clear();
            }
            run.extend_from_slice(data);
            next = Some(s + 1);
        }
        identity_in_image(&run)
    }
}

/// The stand-in for the control module's program.
#[derive(Clone, Debug, PartialEq, Eq)]
struct U64Ctrl {
    major: u16,
    minor: u16,
    settings: ModuleSettings,
    /// The WiFi radio is switched on (it never connects).
    radio: bool,
    /// The firmware asked for received network packets.
    modem: bool,
    mac: [u8; 6],
    serial: [u8; 16],
    recovery_url: Vec<u8>,
    /// A power request and the clock at which the module acts on it.
    power_due: Option<(u64, PowerEvent)>,
    /// A power request the module has acted on.
    power: Option<PowerEvent>,
}

impl U64Ctrl {
    fn new() -> Self {
        Self {
            major: 1,
            minor: 30,
            settings: ModuleSettings::default(),
            radio: true,
            modem: false,
            // Locally administered, not a manufacturer's.
            mac: [0x02, 0x15, 0x41, 0x00, 0x00, 0x01],
            serial: [0; 16],
            recovery_url: Vec::new(),
            power_due: None,
            power: None,
        }
    }

    /// What survives the module's restart with the board: settings, the
    /// identity; pending requests do not.
    fn restart(&mut self) {
        self.radio = true;
        self.modem = false;
        self.power_due = None;
        self.power = None;
    }

    fn ident_string(&self) -> String {
        format!("ESP32 WiFi Bridge V{}.{:02}", self.major, self.minor)
    }

    /// One request; the answers (reply first, then events) in order.
    fn request(&mut self, req: &[u8], now: u64) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        if req.len() < 4 {
            return out;
        }
        let hdr = &req[..4];
        // The usual reply: header and an error number.
        let plain = |err: u32| {
            let mut v = hdr.to_vec();
            v.extend_from_slice(&err.to_le_bytes());
            v
        };
        let event = |code: u8| vec![code, 0xFF, 0, 0];
        match req[0] {
            cmd::ECHO => out.push(req.to_vec()),
            cmd::IDENTIFY => {
                // major u16 @4, minor u16 @6, text with its NUL @8.
                let mut v = hdr.to_vec();
                v.extend_from_slice(&self.major.to_le_bytes());
                v.extend_from_slice(&self.minor.to_le_bytes());
                v.extend_from_slice(self.ident_string().as_bytes());
                v.push(0);
                // The module sends the structure's size plus the text's
                // length: one byte beyond the NUL.
                v.push(0);
                out.push(v);
            }
            cmd::WIFI_SCAN => {
                // Error number, number of records (none), a reserved byte.
                let mut v = plain(ESP_OK);
                v.extend_from_slice(&[0, 0]);
                out.push(v);
                self.radio_on(&mut out);
            }
            cmd::WIFI_DISCONNECT => {
                out.push(plain(ESP_OK));
                if self.radio {
                    out.push(event(cmd::EVENT_DISCONNECTED));
                }
            }
            cmd::WIFI_GETMAC => {
                let mut v = plain(ESP_OK);
                v.extend_from_slice(&self.mac);
                v.extend_from_slice(&[0, 0]);
                out.push(v);
            }
            // A network packet: never answered, and it goes nowhere.
            cmd::SEND_PACKET => {}
            cmd::MODEM_ON | cmd::MODEM_OFF => {
                self.modem = req[0] == cmd::MODEM_ON;
                out.push(plain(ESP_OK));
            }
            cmd::WIFI_IS_CONNECTED => {
                // Error number, status 0 = not connected, padding.
                let mut v = plain(ESP_OK);
                v.extend_from_slice(&[0, 0, 0, 0]);
                out.push(v);
                if !self.radio {
                    out.push(event(cmd::EVENT_DISABLED));
                }
            }
            cmd::GET_VOLTAGES => {
                // Seven rails in millivolts: input, auxiliary, 5 V, 3.3 V,
                // 1.8 V, 1.0 V, USB. An input below 8.5 V makes the
                // firmware warn.
                let mut v = plain(ESP_OK);
                for mv in [12_000u16, 12_000, 5_000, 3_300, 1_800, 1_000, 5_000] {
                    v.extend_from_slice(&mv.to_le_bytes());
                }
                v.extend_from_slice(&[0, 0]);
                out.push(v);
            }
            cmd::WIFI_ENABLE | cmd::WIFI_AUTOCONNECT => {
                out.push(plain(ESP_OK));
                self.radio_on(&mut out);
            }
            cmd::WIFI_DISABLE => {
                out.push(plain(ESP_OK));
                self.radio = false;
                out.push(event(cmd::EVENT_DISABLED));
            }
            cmd::MACHINE_OFF | cmd::MACHINE_REBOOT => {
                out.push(plain(ESP_OK));
                let what = if req[0] == cmd::MACHINE_OFF {
                    PowerEvent::Off
                } else {
                    PowerEvent::Reboot
                };
                self.power_due = Some((now.saturating_add(POWER_CLOCKS), what));
            }
            cmd::CLEAR_APS | cmd::SET_BAUD => out.push(plain(ESP_OK)),
            cmd::SET_SERIAL => {
                if let Some(s) = req.get(4..20) {
                    self.serial.copy_from_slice(s);
                    out.push(plain(ESP_OK));
                } else {
                    out.push(plain(ESP_ERR_INVALID_ARG));
                }
            }
            cmd::GET_SERIAL => {
                let mut v = plain(ESP_OK);
                v.extend_from_slice(&self.serial);
                out.push(v);
            }
            cmd::SET_POWER_MODE => match req.get(4) {
                Some(&m) if req.len() >= 6 && m <= 2 => {
                    self.settings.power_mode = m;
                    out.push(plain(ESP_OK));
                }
                _ => out.push(plain(ESP_ERR_INVALID_ARG)),
            },
            cmd::GET_POWER_MODE => {
                let mut v = plain(ESP_OK);
                v.extend_from_slice(&[self.settings.power_mode, self.settings.last_state, 0, 0]);
                out.push(v);
            }
            cmd::SET_WAKE_ON_WIFI => match req.get(4) {
                Some(&e) if req.len() >= 6 => {
                    self.settings.wake_on_wifi = u8::from(e != 0);
                    out.push(plain(ESP_OK));
                }
                _ => out.push(plain(ESP_ERR_INVALID_ARG)),
            },
            cmd::GET_WAKE_ON_WIFI => {
                let mut v = plain(ESP_OK);
                v.extend_from_slice(&[self.settings.wake_on_wifi, 0, 0, 0]);
                out.push(v);
            }
            cmd::SET_RECOVERY_URL => {
                let text = &req[4..];
                let n = text.iter().position(|&c| c == 0).unwrap_or(text.len());
                if n < 128 {
                    self.recovery_url = text[..n].to_vec();
                    out.push(plain(ESP_OK));
                } else {
                    out.push(plain(ESP_ERR_INVALID_ARG));
                }
            }
            cmd::GET_RECOVERY_URL => {
                let mut v = plain(ESP_OK);
                let mut url = [0u8; 128];
                url[..self.recovery_url.len()].copy_from_slice(&self.recovery_url);
                v.extend_from_slice(&url);
                out.push(v);
            }
            // Connecting (there is no access point), the time of day and
            // whatever else: not supported.
            _ => out.push(plain(ESP_ERR_NOT_SUPPORTED)),
        }
        out
    }

    /// A request that needs the radio starts a stopped one; the module
    /// then reports that it is not connected.
    fn radio_on(&mut self, out: &mut Vec<Vec<u8>>) {
        if !self.radio {
            self.radio = true;
            out.push(vec![cmd::EVENT_DISCONNECTED, 0xFF, 0, 0]);
        }
    }
}

/// The WiFi DMA UART with the control module behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wifi {
    /// Baud divisor, 11 bits.
    divisor: u16,
    rx_enable: bool,
    tx_enable: bool,
    buf_enable: bool,
    /// Flow control register: bits 0–2 and 4–6.
    flow: u8,
    tx_addr: u32,
    tx_len: u16,
    /// The clock at which the transmitter is free again; `None`: it is.
    tx_done: Option<u64>,
    /// The receive address register and whether it holds an address the
    /// receiver has not taken yet.
    rx_addr: u32,
    rx_addr_valid: bool,
    /// The address the receiver has taken for the next packet.
    rx_active: Option<u32>,
    /// A packet has been written; its length waits to be reported.
    rx_held: Option<u16>,
    /// The reported length and whether a report is standing.
    rx_len: u16,
    rx_len_valid: bool,
    /// The FPGA decodes button events (first byte with bit 7).
    events: bool,
    /// Packets from the module, in the order sent.
    queue: VecDeque<Packet>,
    /// Decoded button event codes the machine has not taken.
    buttons: VecDeque<u8>,
    /// Without SLIP: the bytes the receiver has collected for a packet
    /// that is not complete. It holds a receive address while there are
    /// any.
    ascii: Vec<u8>,
    ctrl: U64Ctrl,
    loader: RomLoader,
    /// A flash gave the module another version and the machine has not
    /// asked yet.
    identity_changed: bool,
    /// The clock at the last access or tick.
    now: u64,
    /// The clock at which something happens by itself.
    wake: u64,
}

impl Default for Wifi {
    fn default() -> Self {
        Self::new()
    }
}

impl Wifi {
    pub fn new() -> Self {
        Self {
            // The FPGA block's default divisor less one; the firmware sets
            // its own before use.
            divisor: 34,
            rx_enable: false,
            tx_enable: false,
            buf_enable: false,
            // Module mode bit 4 is set after reset.
            flow: 0x10,
            tx_addr: 0,
            tx_len: 0,
            tx_done: None,
            rx_addr: 0,
            rx_addr_valid: false,
            rx_active: None,
            rx_held: None,
            rx_len: 0,
            rx_len_valid: false,
            events: false,
            queue: VecDeque::new(),
            buttons: VecDeque::new(),
            ascii: Vec::new(),
            ctrl: U64Ctrl::new(),
            loader: RomLoader::default(),
            identity_changed: false,
            now: 0,
            wake: u64::MAX,
        }
    }

    /// The I/O addresses of this block.
    pub fn claims(addr: u32) -> bool {
        (BASE..=BASE + 0x0F).contains(&addr)
    }

    /// The version the module reports (default 1.30, text
    /// `ESP32 WiFi Bridge V1.30`). An updater leaves the module alone when
    /// this is the version it carries; see [`identity_in_image`]. The
    /// machine gives the version it kept from an earlier run.
    pub fn set_identity(&mut self, major: u16, minor: u16) {
        self.ctrl.major = major;
        self.ctrl.minor = minor;
    }

    pub fn identity(&self) -> (u16, u16) {
        (self.ctrl.major, self.ctrl.minor)
    }

    /// The firmware has written other software into the module and
    /// started it: the version it reports from now on. Reported once; the
    /// machine keeps it and gives it to [`Wifi::set_identity`] at its
    /// next start. (The written bytes themselves are not kept.)
    pub fn take_identity_changed(&mut self) -> Option<(u16, u16)> {
        std::mem::take(&mut self.identity_changed).then(|| self.identity())
    }

    /// The MAC address of the module's WiFi station.
    pub fn set_mac(&mut self, mac: [u8; 6]) {
        self.ctrl.mac = mac;
    }

    pub fn settings(&self) -> ModuleSettings {
        self.ctrl.settings
    }

    /// Out-of-range values are brought into range.
    pub fn set_settings(&mut self, s: ModuleSettings) {
        self.ctrl.settings = ModuleSettings {
            power_mode: s.power_mode.min(2),
            last_state: s.last_state.min(1),
            wake_on_wifi: s.wake_on_wifi.min(1),
        };
    }

    /// The firmware asked the module to switch the machine off or to
    /// restart it, and the module's second of delay is over. Reported
    /// once; the machine then ends or restarts the board.
    pub fn take_power_event(&mut self) -> Option<PowerEvent> {
        self.ctrl.power.take()
    }

    /// Whether the FPGA decodes the module's button events. On: the
    /// Commodore 64 Ultimate's wiring as its open top level has it. Off
    /// (default): every packet from the module is data.
    pub fn set_events(&mut self, on: bool) {
        self.events = on;
    }

    pub fn events(&self) -> bool {
        self.events
    }

    /// The module sends a rocker gesture. `false` without the event
    /// decode: there the packet would arrive as data the firmware has no
    /// use for, so it is not sent.
    pub fn rocker(&mut self, gesture: Rocker) -> bool {
        if !self.events {
            return false;
        }
        let code = match gesture {
            Rocker::DownLong => 0x80,
            Rocker::DownShort => 0x81,
            Rocker::UpShort => 0x82,
            Rocker::UpLong => 0x83,
        };
        // Behind whatever is on the line already.
        let due = self.queue.back().map_or(self.now, |p| p.due.max(self.now));
        self.queue.push_back(Packet {
            due,
            raw: false,
            data: vec![code, 0xFF, 0, 0],
        });
        self.wake = self.wake.min(due);
        true
    }

    /// A button event the FPGA decoded. Codes without an effect on the
    /// board (the long press down, a joystick report) are dropped here.
    pub fn take_button_event(&mut self) -> Option<ButtonEvent> {
        while let Some(code) = self.buttons.pop_front() {
            match code {
                0x81 => return Some(ButtonEvent::Freeze),
                0x82 => return Some(ButtonEvent::Menu),
                0x83 => return Some(ButtonEvent::Reset),
                _ => {}
            }
        }
        None
    }

    fn tx_ready(&self) -> bool {
        self.tx_done.is_none()
    }

    fn status(&self) -> u8 {
        let rx = self.rx_len_valid;
        let tx = self.tx_ready();
        let need = !self.rx_addr_valid;
        u8::from(self.rx_enable && rx)
            | u8::from(self.tx_enable && tx) << 1
            | u8::from(self.buf_enable && need) << 2
            | 0x10
            | u8::from(rx) << 5
            | u8::from(tx) << 6
            | u8::from(need) << 7
    }

    fn slip(&self) -> bool {
        self.flow & 0x04 != 0
    }

    /// The flow control register was written; `old` is what it held.
    /// A change of what the module does resets the module.
    fn module_mode_written(&mut self, old: u8, now: u64) {
        let is = module_in(self.flow);
        if module_in(old) == is {
            return;
        }
        // What it had not sent yet is never sent.
        self.queue.retain(|p| p.due <= now);
        self.loader.transfer = None;
        match is {
            Module::Loader => {
                let due = now.saturating_add(BOOT_CLOCKS);
                self.enqueue(due, true, ROM_GREETING.to_vec());
            }
            Module::Run => {
                // The program that starts is the one in the flash.
                if let Some(id) = self.loader.flashed_identity() {
                    if id != self.identity() {
                        self.set_identity(id.0, id.1);
                        self.identity_changed = true;
                    }
                }
                self.loader.flash.clear();
            }
            Module::Off => {}
        }
    }

    /// The SLIP bit was set: the bytes collected without it become a
    /// packet, a line feed at its end.
    fn close_ascii(&mut self, ctx: &mut IoCtx<'_>) {
        if self.ascii.is_empty() {
            return;
        }
        let mut data = std::mem::take(&mut self.ascii);
        data.push(0x0A);
        if let (Some(addr), None) = (self.rx_active, self.rx_held) {
            self.deliver(addr, &data, ctx);
        }
    }

    /// The receiver writes a packet to the address it holds.
    fn deliver(&mut self, addr: u32, data: &[u8], ctx: &mut IoCtx<'_>) {
        let n = data.len().min(MAX_PACKET);
        if !ctx.ram.is_empty() {
            let mask = ctx.ram.len() - 1;
            let base = (addr & DMA_MASK) as usize;
            for (i, &b) in data[..n].iter().enumerate() {
                ctx.ram[(base + i) & mask] = b;
            }
        }
        self.rx_active = None;
        self.rx_held = Some(n as u16);
    }

    fn soft_reset(&mut self, now: u64) {
        self.rx_enable = false;
        self.tx_enable = false;
        self.buf_enable = false;
        self.tx_done = None;
        self.rx_addr_valid = false;
        self.rx_active = None;
        self.rx_held = None;
        self.rx_len_valid = false;
        self.ascii.clear();
        // What had arrived in the receive FIFO is flushed; what the module
        // has not sent yet still comes.
        while self.queue.front().is_some_and(|p| p.due <= now) {
            self.queue.pop_front();
        }
    }

    /// "Send": the packet leaves memory and reaches the module.
    fn transmit(&mut self, ctx: &mut IoCtx<'_>) {
        let len = usize::from(self.tx_len);
        let mut data = vec![0u8; len];
        if !ctx.ram.is_empty() {
            let mask = ctx.ram.len() - 1;
            let base = (self.tx_addr & DMA_MASK) as usize;
            for (i, b) in data.iter_mut().enumerate() {
                *b = ctx.ram[(base + i) & mask];
            }
        }
        self.tx_done = Some(ctx.now.saturating_add(TX_CLOCKS));
        let due = ctx.now.saturating_add(REPLY_CLOCKS);
        if self.flow & 0x02 != 0 {
            // Loopback: what is sent comes back, framed or not as sent.
            self.enqueue(due, !self.slip(), data);
            return;
        }
        match module_in(self.flow) {
            Module::Run => {
                for answer in self.ctrl.request(&data, ctx.now) {
                    self.enqueue(due, false, answer);
                }
            }
            // The ROM takes frames only.
            Module::Loader if self.slip() => {
                for answer in self.loader.command(&data) {
                    self.enqueue(due, false, answer);
                }
            }
            _ => {}
        }
    }

    fn enqueue(&mut self, due: u64, raw: bool, data: Vec<u8>) {
        if self.queue.len() < MAX_QUEUE && !data.is_empty() {
            // The line keeps the order.
            let due = self.queue.back().map_or(due, |p| p.due.max(due));
            self.queue.push_back(Packet { due, raw, data });
        }
    }

    /// Everything that happens by itself up to `ctx.now`, then the
    /// interrupt line.
    fn service(&mut self, ctx: &mut IoCtx<'_>) {
        let now = ctx.now;
        self.now = now;
        if self.tx_done.is_some_and(|t| t <= now) {
            self.tx_done = None;
        }
        if let Some((t, what)) = self.ctrl.power_due {
            if t <= now {
                self.ctrl.power_due = None;
                self.ctrl.power = Some(what);
            }
        }
        loop {
            // A written packet is reported once the previous report has
            // been taken; until then the receiver does nothing else.
            if let Some(len) = self.rx_held {
                if self.rx_len_valid {
                    break;
                }
                self.rx_len = len;
                self.rx_len_valid = true;
                self.rx_held = None;
            }
            // Without the event decode the receiver takes a waiting
            // address at once, which frees the register for the next.
            if !self.events && self.rx_active.is_none() && self.rx_addr_valid {
                self.rx_active = Some(self.rx_addr);
                self.rx_addr_valid = false;
            }
            if !self.queue.front().is_some_and(|p| p.due <= now) {
                break;
            }
            let slip = self.slip();
            if slip && self.queue.front().is_some_and(|p| p.raw) {
                // Text while the receiver waits for a frame's start: lost.
                self.queue.pop_front();
                continue;
            }
            if self.events {
                // (Without SLIP nothing is decoded here: the module
                // sends its events as frames.)
                if slip && self.queue.front().is_some_and(|p| p.data[0] & 0x80 != 0) {
                    // An event: decoded, never written, needs no buffer.
                    if let Some(p) = self.queue.pop_front() {
                        if self.buttons.len() < 16 {
                            self.buttons.push_back(p.data[0]);
                        }
                    }
                    continue;
                }
                // With the decode the address stays in the register until
                // a data packet takes it.
                if self.rx_active.is_none() {
                    if !self.rx_addr_valid {
                        break;
                    }
                    self.rx_active = Some(self.rx_addr);
                    self.rx_addr_valid = false;
                }
            }
            let Some(addr) = self.rx_active else {
                break;
            };
            let Some(p) = self.queue.pop_front() else {
                break;
            };
            if slip {
                self.deliver(addr, &p.data, ctx);
                continue;
            }
            // Without SLIP: bytes as on the line, a packet every 1536.
            let mut bytes = if p.raw { p.data } else { slip_encode(&p.data) };
            let room = MAX_PACKET.saturating_sub(self.ascii.len());
            if bytes.len() > room {
                let rest = bytes.split_off(room);
                self.queue.push_front(Packet {
                    due: p.due,
                    raw: true,
                    data: rest,
                });
            }
            self.ascii.extend_from_slice(&bytes);
            if self.ascii.len() >= MAX_PACKET {
                let data = std::mem::take(&mut self.ascii);
                self.deliver(addr, &data, ctx);
            }
        }
        // What comes next by itself: only what lies ahead; a packet that
        // waits for a buffer is moved by the access that arms one.
        let mut wake = u64::MAX;
        if let Some(t) = self.tx_done {
            wake = wake.min(t);
        }
        if let Some((t, _)) = self.ctrl.power_due {
            wake = wake.min(t);
        }
        if let Some(p) = self.queue.front() {
            if p.due > now {
                wake = wake.min(p.due);
            }
        }
        self.wake = wake;
        ctx.irq.set_high(IRQ_HIGH, self.status() & 7 != 0);
    }
}

impl IoDevice for Wifi {
    fn read(&mut self, addr: u32, ctx: &mut IoCtx<'_>) -> u8 {
        self.service(ctx);
        self.peek(addr)
    }

    fn peek(&self, addr: u32) -> u8 {
        match addr & 0x0F {
            0x0 => self.divisor as u8,
            0x1 => (self.divisor >> 8) as u8 & 0x03,
            0x2 => self.status(),
            // Bit 7 (the reset strobe) and bit 3 never read back: the
            // firmware changes single bits by read, modify, write.
            0x3 => self.flow & 0x77,
            0x8 => self.rx_len as u8,
            0x9 => (self.rx_len >> 8) as u8,
            _ => 0,
        }
    }

    fn write(&mut self, addr: u32, v: u8, ctx: &mut IoCtx<'_>) {
        self.service(ctx);
        let byte = |old: u32, n: u32| (old & !(0xFF << (8 * n))) | u32::from(v) << (8 * n);
        match addr & 0x0F {
            0x0 => self.divisor = (self.divisor & 0x700) | u16::from(v),
            0x1 => self.divisor = (self.divisor & 0x0FF) | u16::from(v & 7) << 8,
            0x2 => {
                let on = v & 0x80 != 0;
                if v & 1 != 0 {
                    self.rx_enable = on;
                }
                if v & 2 != 0 {
                    self.tx_enable = on;
                }
                if v & 4 != 0 {
                    self.buf_enable = on;
                }
            }
            0x3 => {
                let old = self.flow;
                self.flow = v & 0x77;
                if v & 0x80 != 0 {
                    self.soft_reset(ctx.now);
                }
                self.module_mode_written(old, ctx.now);
                if old & 0x04 == 0 && self.slip() {
                    self.close_ascii(ctx);
                }
            }
            a @ 0x4..=0x7 => self.tx_addr = byte(self.tx_addr, a - 4) & 0x0FFF_FFFF,
            0x8 => self.tx_len = (self.tx_len & 0xFF00) | u16::from(v),
            0x9 => self.tx_len = (self.tx_len & 0x00FF) | u16::from(v) << 8,
            0xA => self.transmit(ctx),
            0xB => self.rx_len_valid = false,
            a @ 0xC..=0xF => {
                self.rx_addr = byte(self.rx_addr, a - 0xC) & 0x0FFF_FFFF;
                if a == 0xF {
                    self.rx_addr_valid = true;
                }
            }
            _ => {}
        }
        self.service(ctx);
    }

    fn next_event(&self) -> u64 {
        self.wake
    }

    fn tick(&mut self, ctx: &mut IoCtx<'_>) {
        self.service(ctx);
    }

    /// The FPGA's reset. The module is a processor of its own and keeps
    /// its settings, identity and flash; the host's choices (event
    /// decode, MAC) stay as well.
    fn reset(&mut self) {
        let mut fresh = Self::new();
        fresh.events = self.events;
        fresh.ctrl = self.ctrl.clone();
        fresh.ctrl.restart();
        fresh.loader.flash = std::mem::take(&mut self.loader.flash);
        fresh.identity_changed = self.identity_changed;
        *self = fresh;
    }

    fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"UWIF", 1, |w| {
            w.u16(self.divisor);
            w.bool(self.rx_enable);
            w.bool(self.tx_enable);
            w.bool(self.buf_enable);
            w.u8(self.flow);
            w.u32(self.tx_addr);
            w.u16(self.tx_len);
            opt_u64(w, self.tx_done);
            w.u32(self.rx_addr);
            w.bool(self.rx_addr_valid);
            opt_u64(w, self.rx_active.map(u64::from));
            opt_u64(w, self.rx_held.map(u64::from));
            w.u16(self.rx_len);
            w.bool(self.rx_len_valid);
            w.bool(self.events);
            w.u32(self.queue.len() as u32);
            for p in &self.queue {
                w.u64(p.due);
                w.bool(p.raw);
                w.bytes(&p.data);
            }
            w.bytes(&Vec::from(self.buttons.clone()));
            w.u64(self.now);
            w.u64(self.wake);
            let c = &self.ctrl;
            w.u16(c.major);
            w.u16(c.minor);
            w.u8(c.settings.power_mode);
            w.u8(c.settings.last_state);
            w.u8(c.settings.wake_on_wifi);
            w.bool(c.radio);
            w.bool(c.modem);
            w.raw(&c.mac);
            w.raw(&c.serial);
            w.bytes(&c.recovery_url);
            w.u8(power_code(c.power));
            w.u8(power_code(c.power_due.map(|p| p.1)));
            w.u64(c.power_due.map_or(0, |p| p.0));
            w.bytes(&self.ascii);
            w.bool(self.identity_changed);
            let t = self.loader.transfer;
            w.bool(t.is_some());
            let t = t.unwrap_or(Transfer {
                offset: 0,
                blocks: 0,
                block_size: 0,
            });
            w.u32(t.offset);
            w.u32(t.blocks);
            w.u32(t.block_size);
            w.u32(self.loader.flash.len() as u32);
            for (&s, data) in &self.loader.flash {
                w.u32(s);
                w.raw(data);
            }
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut r) = r.chunk(*b"UWIF", 1)?;
        let r = &mut r;
        let mut n = Self::new();
        n.divisor = r.u16()? & 0x7FF;
        n.rx_enable = r.bool()?;
        n.tx_enable = r.bool()?;
        n.buf_enable = r.bool()?;
        n.flow = r.u8()? & 0x77;
        n.tx_addr = r.u32()? & 0x0FFF_FFFF;
        n.tx_len = r.u16()?;
        n.tx_done = read_opt_u64(r)?;
        n.rx_addr = r.u32()? & 0x0FFF_FFFF;
        n.rx_addr_valid = r.bool()?;
        n.rx_active = read_opt_u64(r)?.map(|v| v as u32 & 0x0FFF_FFFF);
        n.rx_held = match read_opt_u64(r)? {
            None => None,
            Some(v) if v <= MAX_PACKET as u64 => Some(v as u16),
            Some(_) => return Err(StateError::Invalid("WiFi UART packet length")),
        };
        n.rx_len = r.u16()?;
        n.rx_len_valid = r.bool()?;
        n.events = r.bool()?;
        let count = r.u32()? as usize;
        if count > MAX_QUEUE {
            return Err(StateError::Invalid("WiFi UART queue length"));
        }
        for _ in 0..count {
            let due = r.u64()?;
            let raw = r.bool()?;
            let data = r.bytes()?;
            if data.is_empty() || data.len() > 0x1_0000 {
                return Err(StateError::Invalid("WiFi UART packet"));
            }
            n.queue.push_back(Packet {
                due,
                raw,
                data: data.to_vec(),
            });
        }
        let buttons = r.bytes()?;
        if buttons.len() > 16 {
            return Err(StateError::Invalid("WiFi UART button events"));
        }
        n.buttons = buttons.iter().copied().collect();
        n.now = r.u64()?;
        n.wake = r.u64()?;
        let c = &mut n.ctrl;
        c.major = r.u16()?;
        c.minor = r.u16()?;
        c.settings.power_mode = r.u8()?;
        c.settings.last_state = r.u8()?;
        c.settings.wake_on_wifi = r.u8()?;
        if c.settings.power_mode > 2 || c.settings.last_state > 1 || c.settings.wake_on_wifi > 1 {
            return Err(StateError::Invalid("control module settings"));
        }
        c.radio = r.bool()?;
        c.modem = r.bool()?;
        r.raw_into(&mut c.mac)?;
        r.raw_into(&mut c.serial)?;
        let url = r.bytes()?;
        if url.len() >= 128 {
            return Err(StateError::Invalid("control module recovery address"));
        }
        c.recovery_url = url.to_vec();
        c.power = power_from(r.u8()?)?;
        let due = power_from(r.u8()?)?;
        let at = r.u64()?;
        c.power_due = due.map(|p| (at, p));
        n.ascii = r.bytes()?.to_vec();
        // Collected bytes need an address, no SLIP and a free receiver.
        if n.ascii.len() >= MAX_PACKET
            || (!n.ascii.is_empty() && (n.slip() || n.rx_active.is_none() || n.rx_held.is_some()))
        {
            return Err(StateError::Invalid("WiFi UART bytes without SLIP"));
        }
        n.identity_changed = r.bool()?;
        let open = r.bool()?;
        let t = Transfer {
            offset: r.u32()?,
            blocks: r.u32()?,
            block_size: r.u32()?,
        };
        if open {
            if t.block_size == 0
                || t.offset >= FLASH_SIZE
                || u64::from(t.blocks) * u64::from(t.block_size) > u64::from(FLASH_SIZE - t.offset)
            {
                return Err(StateError::Invalid("module flash transfer"));
            }
            n.loader.transfer = Some(t);
        }
        let sectors = r.u32()?;
        if sectors > FLASH_SIZE / FLASH_SECTOR {
            return Err(StateError::Invalid("module flash sectors"));
        }
        let mut next = 0;
        for _ in 0..sectors {
            let s = r.u32()?;
            if s < next || s >= FLASH_SIZE / FLASH_SECTOR {
                return Err(StateError::Invalid("module flash sector"));
            }
            next = s + 1;
            n.loader
                .flash
                .insert(s, r.raw(FLASH_SECTOR as usize)?.to_vec());
        }
        *self = n;
        Ok(())
    }
}

fn opt_u64(w: &mut StateWriter, v: Option<u64>) {
    w.bool(v.is_some());
    w.u64(v.unwrap_or(0));
}

fn read_opt_u64(r: &mut StateReader<'_>) -> Result<Option<u64>, StateError> {
    let some = r.bool()?;
    let v = r.u64()?;
    Ok(some.then_some(v))
}

fn power_code(p: Option<PowerEvent>) -> u8 {
    match p {
        None => 0,
        Some(PowerEvent::Off) => 1,
        Some(PowerEvent::Reboot) => 2,
    }
}

fn power_from(v: u8) -> Result<Option<PowerEvent>, StateError> {
    match v {
        0 => Ok(None),
        1 => Ok(Some(PowerEvent::Off)),
        2 => Ok(Some(PowerEvent::Reboot)),
        _ => Err(StateError::Invalid("power request")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::NullC64;
    use crate::irq::Irq;

    /// A board around the block: memory, interrupt controller, clock.
    struct Rig {
        wifi: Wifi,
        ram: Vec<u8>,
        irq: Irq,
        console: Vec<u8>,
        c64: NullC64,
        now: u64,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                wifi: Wifi::new(),
                ram: vec![0; 0x10_0000],
                irq: Irq::default(),
                console: Vec::new(),
                c64: NullC64::default(),
                now: 0,
            }
        }

        fn with<T>(&mut self, f: impl FnOnce(&mut Wifi, &mut IoCtx<'_>) -> T) -> T {
            let mut ctx = IoCtx {
                now: self.now,
                ram: &mut self.ram,
                irq: &mut self.irq,
                console: &mut self.console,
                c64: &mut self.c64,
                storage: None,
                net: None,
            };
            f(&mut self.wifi, &mut ctx)
        }

        fn rd(&mut self, off: u32) -> u8 {
            self.with(|w, c| w.read(BASE + off, c))
        }

        fn wr(&mut self, off: u32, v: u8) {
            self.with(|w, c| w.write(BASE + off, v, c));
        }

        fn wr32(&mut self, off: u32, v: u32) {
            for (i, b) in v.to_le_bytes().into_iter().enumerate() {
                self.wr(off + i as u32, b);
            }
        }

        /// Run the clock on, ticking the block when it asks.
        fn run(&mut self, clocks: u64) {
            let end = self.now + clocks;
            loop {
                let next = self.wifi.next_event();
                if next > end {
                    break;
                }
                self.now = self.now.max(next);
                self.with(|w, c| w.tick(c));
                assert!(
                    self.wifi.next_event() > self.now,
                    "an event in the past would spin the run loop"
                );
            }
            self.now = end;
        }

        fn line(&self) -> bool {
            self.irq.high & 1 << IRQ_HIGH != 0
        }

        /// What the firmware's set-up does: reset, flow control, SLIP,
        /// interrupts for "packet received" and "address wanted".
        fn init(&mut self) {
            self.wr(3, 0x80);
            self.wr(3, 0x80);
            let f = self.rd(3);
            self.wr(3, f | 0x01);
            let f = self.rd(3);
            self.wr(3, f | 0x80);
            let f = self.rd(3);
            self.wr(3, f | 0x04);
            self.wr(2, 0x85);
        }

        fn arm(&mut self, addr: u32) {
            self.wr32(0xC, addr);
        }

        fn send(&mut self, addr: u32, packet: &[u8]) {
            self.ram[addr as usize..addr as usize + packet.len()].copy_from_slice(packet);
            self.wr32(4, addr);
            self.wr(8, packet.len() as u8);
            self.wr(9, (packet.len() >> 8) as u8);
            self.wr(0xA, 1);
        }

        /// Take the reported packet out of the buffer at `addr`.
        fn receive(&mut self, addr: u32) -> Vec<u8> {
            assert_ne!(self.rd(2) & 0x20, 0, "no packet reported");
            let n = usize::from(self.rd(8)) | usize::from(self.rd(9)) << 8;
            let v = self.ram[addr as usize..addr as usize + n].to_vec();
            self.wr(0xB, 1);
            v
        }

        /// One request and its reply through buffers at fixed addresses.
        fn rpc(&mut self, request: &[u8]) -> Vec<u8> {
            self.arm(0x2000);
            self.send(0x1000, request);
            self.run(2 * REPLY_CLOCKS);
            self.receive(0x2000)
        }
    }

    fn err(reply: &[u8]) -> u32 {
        u32::from_le_bytes([reply[4], reply[5], reply[6], reply[7]])
    }

    #[test]
    fn a7_constructor_accesses_leave_the_line_low() {
        let mut r = Rig::new();
        // Reset state: nothing enabled, transmitter free, no address.
        assert_eq!(r.rd(2), 0xD0);
        r.rd(0);
        r.rd(1);
        r.wr(3, 0x80);
        r.wr(3, 0x80);
        assert!(!r.line());
        assert_eq!(r.rd(2) & 7, 0);
    }

    #[test]
    fn h3_flow_control_never_reads_the_reset_bit() {
        let mut r = Rig::new();
        r.wr(3, 0x80);
        assert_eq!(r.rd(3), 0x00);
        r.wr(3, 0xFF);
        assert_eq!(r.rd(3), 0x77);
        // Read, modify, write must not reset again: the enables stay.
        r.wr(2, 0x85);
        let f = r.rd(3);
        r.wr(3, f | 0x04);
        assert_eq!(r.rd(2) & 4, 4);
        // The reset strobe drops them.
        r.wr(3, f | 0x80);
        assert_eq!(r.rd(2) & 7, 0);
    }

    #[test]
    fn h1_h2_status_is_live_and_the_line_follows_it() {
        let mut r = Rig::new();
        r.init();
        // Address wanted: pending until one is written.
        assert_eq!(r.rd(2) & 7, 4);
        assert!(r.line());
        r.arm(0x2000);
        // The receiver took it at once, so the register is free again:
        // a second address is wanted (two slots, H11).
        assert_eq!(r.rd(2) & 7, 4);
        r.arm(0x3000);
        assert_eq!(r.rd(2) & 7, 0);
        assert!(!r.line());
        // Dropping an enable removes its cause as well.
        r.wr(3, 0x80);
        r.wr(2, 0x84);
        assert!(r.line());
        r.wr(2, 0x04);
        assert!(!r.line());
        assert_eq!(r.rd(2) & 7, 0);
    }

    #[test]
    fn h6_transmit_completes_and_raises_the_transmit_interrupt() {
        let mut r = Rig::new();
        r.init();
        r.arm(0x2000);
        r.arm(0x3000);
        r.wr(2, 0x82);
        assert_eq!(r.rd(2) & 2, 2, "the idle transmitter asks for a packet");
        r.send(0x1000, &[cmd::SEND_PACKET, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(r.rd(2) & 0x42, 0, "busy right after the push");
        assert!(!r.line());
        r.run(TX_CLOCKS);
        assert_eq!(r.rd(2) & 0x42, 0x42);
        assert!(r.line());
        // The firmware frees the buffer and, with nothing more to send,
        // drops the enable.
        r.wr(2, 0x02);
        assert!(!r.line());
        // A network packet is never answered (H10).
        r.run(CLOCK_HZ);
        assert_eq!(r.rd(2) & 0x20, 0);
    }

    #[test]
    fn h4_identify_is_answered_within_the_timeout() {
        let mut r = Rig::new();
        r.init();
        r.arm(0x2000);
        r.send(0x1000, &[cmd::IDENTIFY, 5, 0x34, 0x12]);
        assert_eq!(r.rd(2) & 1, 0);
        // Well inside the firmware's half second.
        r.run(REPLY_CLOCKS);
        assert_eq!(r.rd(2) & 1, 1);
        assert!(r.line());
        let reply = r.receive(0x2000);
        assert_eq!(r.rd(2) & 1, 0, "taking the packet removes its cause");
        // What keeps the line up now is the wish for more addresses:
        // one for the receiver, one for the register.
        assert!(r.line());
        r.arm(0x3000);
        r.arm(0x4000);
        assert!(!r.line());
        assert_eq!(&reply[..4], &[cmd::IDENTIFY, 5, 0x34, 0x12]);
        assert_eq!(&reply[4..8], &[1, 0, 30, 0]);
        let text = b"ESP32 WiFi Bridge V1.30\0";
        assert_eq!(&reply[8..8 + text.len()], text);
        assert_eq!(reply.len(), 10 + text.len() - 1);
    }

    #[test]
    fn identity_follows_the_image() {
        let image = b"....ESP32 WiFi Bridge V1.14\0....";
        assert_eq!(identity_in_image(image), Some((1, 14)));
        assert_eq!(identity_in_image(b"ESP32 WiFi Bridge Vx"), None);
        assert_eq!(identity_in_image(b"nothing here"), None);
        let mut r = Rig::new();
        r.wifi.set_identity(1, 14);
        r.init();
        let reply = r.rpc(&[cmd::IDENTIFY, 0xFF, 0, 0]);
        assert_eq!(&reply[4..8], &[1, 0, 14, 0]);
        assert_eq!(&reply[8..31], b"ESP32 WiFi Bridge V1.14");
    }

    #[test]
    fn h5_every_request_but_the_network_packet_is_answered() {
        let mut r = Rig::new();
        r.init();
        for c in (1..=0x30u8).filter(|&c| c != cmd::SEND_PACKET) {
            let reply = r.rpc(&[c, 3, c, 0, 0, 0, 0, 0]);
            assert_eq!(&reply[..4], &[c, 3, c, 0], "header of request {c:#x}");
            assert!(reply.len() >= 8);
            // Events some requests bring are taken out of the way.
            while r.wifi.queue.front().is_some() || r.rd(2) & 0x20 != 0 {
                r.arm(0x2000);
                r.run(2 * REPLY_CLOCKS);
                r.receive(0x2000);
            }
        }
        // Unknown: not supported.
        let reply = r.rpc(&[0x30, 0, 0, 0]);
        assert_eq!(err(&reply), ESP_ERR_NOT_SUPPORTED);
        assert_eq!(reply.len(), 8);
    }

    #[test]
    fn h7_voltages_are_above_the_warning_level() {
        let mut r = Rig::new();
        r.init();
        let reply = r.rpc(&[cmd::GET_VOLTAGES, 0, 0, 0]);
        assert_eq!(reply.len(), 24);
        assert_eq!(err(&reply), 0);
        let vbus = u16::from_le_bytes([reply[8], reply[9]]);
        assert!(vbus >= 8500);
    }

    #[test]
    fn h8_h9_power_mode_and_wake_on_wifi_read_back() {
        let mut r = Rig::new();
        r.init();
        let reply = r.rpc(&[cmd::GET_POWER_MODE, 0, 0, 0]);
        assert_eq!(reply.len(), 12);
        assert_eq!((err(&reply), reply[8], reply[9]), (0, 0, 1));
        assert_eq!(err(&r.rpc(&[cmd::SET_POWER_MODE, 0, 0, 0, 2, 0])), 0);
        assert_eq!(r.rpc(&[cmd::GET_POWER_MODE, 0, 0, 0])[8], 2);
        // Out of range and too short: refused, nothing changes.
        assert_eq!(
            err(&r.rpc(&[cmd::SET_POWER_MODE, 0, 0, 0, 3, 0])),
            ESP_ERR_INVALID_ARG
        );
        assert_eq!(
            err(&r.rpc(&[cmd::SET_POWER_MODE, 0, 0, 0])),
            ESP_ERR_INVALID_ARG
        );
        assert_eq!(r.rpc(&[cmd::GET_POWER_MODE, 0, 0, 0])[8], 2);
        assert_eq!(r.rpc(&[cmd::GET_WAKE_ON_WIFI, 0, 0, 0])[8], 0);
        assert_eq!(err(&r.rpc(&[cmd::SET_WAKE_ON_WIFI, 0, 0, 0, 1, 0])), 0);
        let reply = r.rpc(&[cmd::GET_WAKE_ON_WIFI, 0, 0, 0]);
        assert_eq!((reply.len(), reply[8]), (12, 1));
        assert_eq!(
            r.wifi.settings(),
            ModuleSettings {
                power_mode: 2,
                last_state: 1,
                wake_on_wifi: 1
            }
        );
    }

    #[test]
    fn mac_scan_and_connection_replies() {
        let mut r = Rig::new();
        r.init();
        let reply = r.rpc(&[cmd::WIFI_GETMAC, 1, 0, 0]);
        assert_eq!(reply.len(), 16);
        assert_eq!(&reply[8..14], &[0x02, 0x15, 0x41, 0x00, 0x00, 0x01]);
        // A scan always carries a record count, here none.
        let reply = r.rpc(&[cmd::WIFI_SCAN, 1, 0, 0]);
        assert_eq!((reply.len(), err(&reply), reply[8]), (10, 0, 0));
        let reply = r.rpc(&[cmd::WIFI_IS_CONNECTED, 1, 0, 0]);
        assert_eq!((reply.len(), err(&reply), reply[8]), (12, 0, 0));
        // The radio runs: no event follows.
        r.arm(0x2000);
        r.run(CLOCK_HZ);
        assert_eq!(r.rd(2) & 0x20, 0);
    }

    #[test]
    fn radio_off_and_on_bring_their_events() {
        let mut r = Rig::new();
        r.init();
        assert_eq!(err(&r.rpc(&[cmd::WIFI_DISABLE, 2, 0, 0])), 0);
        r.arm(0x2000);
        r.run(2 * REPLY_CLOCKS);
        assert_eq!(r.receive(0x2000), [cmd::EVENT_DISABLED, 0xFF, 0, 0]);
        r.rpc(&[cmd::WIFI_IS_CONNECTED, 2, 0, 0]);
        r.arm(0x2000);
        r.run(2 * REPLY_CLOCKS);
        assert_eq!(r.receive(0x2000), [cmd::EVENT_DISABLED, 0xFF, 0, 0]);
        assert_eq!(err(&r.rpc(&[cmd::WIFI_ENABLE, 2, 0, 0])), 0);
        r.arm(0x2000);
        r.run(2 * REPLY_CLOCKS);
        assert_eq!(r.receive(0x2000), [cmd::EVENT_DISCONNECTED, 0xFF, 0, 0]);
        // Running again: enabling once more says nothing further.
        r.rpc(&[cmd::WIFI_ENABLE, 2, 0, 0]);
        r.arm(0x2000);
        r.run(CLOCK_HZ);
        assert_eq!(r.rd(2) & 0x20, 0);
    }

    #[test]
    fn h11_packets_wait_for_buffers_and_fill_them_in_order() {
        let mut r = Rig::new();
        r.init();
        // Three requests, no receive buffer armed.
        for seq in 0..3u8 {
            r.send(0x1000, &[cmd::CLEAR_APS, 0, seq, 0]);
        }
        r.run(CLOCK_HZ);
        assert_eq!(r.rd(2) & 0x20, 0, "nothing reported without a buffer");
        assert_eq!(r.wifi.next_event(), u64::MAX, "and nothing to wait for");
        // Two buffers armed: the oldest gets the first reply, the second
        // is written too, but reported only after the first is taken.
        r.arm(0x2000);
        r.arm(0x3000);
        assert_eq!(r.rd(2) & 0xA0, 0xA0, "reported, and an address is wanted");
        assert_eq!(r.ram[0x2002], 0);
        assert_eq!(r.ram[0x3002], 1);
        assert_eq!(r.receive(0x2000)[2], 0);
        assert_eq!(r.receive(0x3000)[2], 1);
        assert_eq!(r.rd(2) & 0x20, 0);
        r.arm(0x4000);
        assert_eq!(r.receive(0x4000)[2], 2);
    }

    #[test]
    fn power_requests_reach_the_machine_after_a_second() {
        let mut r = Rig::new();
        r.init();
        assert_eq!(err(&r.rpc(&[cmd::MACHINE_OFF, 0, 0, 0])), 0);
        assert_eq!(r.wifi.take_power_event(), None);
        r.run(POWER_CLOCKS);
        assert_eq!(r.wifi.take_power_event(), Some(PowerEvent::Off));
        assert_eq!(r.wifi.take_power_event(), None);
        r.rpc(&[cmd::MACHINE_REBOOT, 0, 0, 0]);
        r.run(POWER_CLOCKS);
        assert_eq!(r.wifi.take_power_event(), Some(PowerEvent::Reboot));
    }

    #[test]
    fn the_rom_loader_does_not_answer_the_programs_requests() {
        let mut r = Rig::new();
        r.init();
        let f = r.rd(3);
        r.wr(3, (f & 0x8F) | MODE_BOOT << 4);
        r.arm(0x2000);
        r.send(0x1000, &[cmd::IDENTIFY, 0, 0, 0]);
        r.run(CLOCK_HZ);
        assert_eq!(r.rd(2) & 0x60, 0x40, "sent, nothing came back");
        // Back in run mode it answers.
        let f = r.rd(3);
        r.wr(3, f & 0x8F);
        r.send(0x1000, &[cmd::IDENTIFY, 0, 0, 0]);
        r.run(2 * REPLY_CLOCKS);
        assert_eq!(r.receive(0x2000)[0], cmd::IDENTIFY);
    }

    #[test]
    fn loopback_returns_the_packet() {
        let mut r = Rig::new();
        r.init();
        let f = r.rd(3);
        r.wr(3, f | 0x02);
        assert_eq!(r.rpc(&[9, 8, 7, 6, 5]), [9, 8, 7, 6, 5]);
    }

    #[test]
    fn soft_reset_flushes_the_receiver() {
        let mut r = Rig::new();
        r.init();
        r.arm(0x2000);
        r.send(0x1000, &[cmd::CLEAR_APS, 0, 0, 0]);
        r.run(2 * REPLY_CLOCKS);
        assert_eq!(r.rd(2) & 0x20, 0x20);
        r.wr(3, 0x80);
        assert_eq!(r.rd(2), 0xD0);
        assert!(!r.line());
    }

    #[test]
    fn without_the_decode_a_bit_7_packet_is_data_and_the_rocker_is_refused() {
        let mut r = Rig::new();
        r.init();
        assert!(!r.wifi.rocker(Rocker::UpShort));
        let f = r.rd(3);
        r.wr(3, f | 0x02);
        assert_eq!(r.rpc(&[0x82, 0xFF, 0, 0]), [0x82, 0xFF, 0, 0]);
        assert_eq!(r.wifi.take_button_event(), None);
    }

    #[test]
    fn with_the_decode_an_event_needs_no_buffer_and_never_reaches_memory() {
        let mut r = Rig::new();
        r.wifi.set_events(true);
        r.init();
        assert!(r.wifi.rocker(Rocker::UpShort));
        r.run(1);
        assert_eq!(r.wifi.take_button_event(), Some(ButtonEvent::Menu));
        assert_eq!(r.rd(2) & 0xA0, 0x80, "no report, still no address");
        // The long press down is decoded and has no effect.
        assert!(r.wifi.rocker(Rocker::DownLong));
        assert!(r.wifi.rocker(Rocker::DownShort));
        assert!(r.wifi.rocker(Rocker::UpLong));
        r.run(1);
        assert_eq!(r.wifi.take_button_event(), Some(ButtonEvent::Freeze));
        assert_eq!(r.wifi.take_button_event(), Some(ButtonEvent::Reset));
        assert_eq!(r.wifi.take_button_event(), None);
    }

    #[test]
    fn with_the_decode_an_event_waits_behind_data_and_the_address_for_data() {
        let mut r = Rig::new();
        r.wifi.set_events(true);
        r.init();
        // An armed address stays in the register through an event.
        r.arm(0x2000);
        assert_eq!(r.rd(2) & 0x80, 0);
        assert!(r.wifi.rocker(Rocker::UpShort));
        r.run(1);
        assert_eq!(r.wifi.take_button_event(), Some(ButtonEvent::Menu));
        assert_eq!(r.rd(2) & 0x80, 0);
        // A reply takes it.
        r.send(0x1000, &[cmd::IDENTIFY, 0, 0, 0]);
        r.run(2 * REPLY_CLOCKS);
        assert_eq!(r.rd(2) & 0xA0, 0xA0);
        assert_eq!(r.receive(0x2000)[0], cmd::IDENTIFY);
        // A reply without a buffer blocks the event behind it.
        r.send(0x1000, &[cmd::IDENTIFY, 0, 0, 0]);
        r.run(2 * REPLY_CLOCKS);
        assert!(r.wifi.rocker(Rocker::DownShort));
        r.run(CLOCK_HZ);
        assert_eq!(r.wifi.take_button_event(), None);
        r.arm(0x2000);
        assert_eq!(r.wifi.take_button_event(), Some(ButtonEvent::Freeze));
        assert_eq!(r.receive(0x2000)[0], cmd::IDENTIFY);
    }

    #[test]
    fn dma_addresses_wrap_inside_memory() {
        let mut r = Rig::new();
        r.ram = vec![0; 0x1000];
        r.init();
        // Far outside the small test memory: masked, never a panic.
        r.arm(0x0FFF_FFFE);
        r.ram[0xFFE] = cmd::IDENTIFY;
        r.wr32(4, 0x0FFF_FFFE);
        r.wr(8, 0xFF);
        r.wr(9, 0xFF);
        r.wr(0xA, 1);
        r.run(2 * REPLY_CLOCKS);
        assert_eq!(r.rd(2) & 0x20, 0x20);
        // No memory at all.
        r.ram = Vec::new();
        r.wr(0xB, 1);
        r.arm(0);
        r.send_empty();
    }

    impl Rig {
        fn send_empty(&mut self) {
            self.wr(8, 4);
            self.wr(9, 0);
            self.wr(0xA, 1);
            self.run(2 * REPLY_CLOCKS);
        }
    }

    #[test]
    fn reset_keeps_the_module_and_the_hosts_choices() {
        let mut r = Rig::new();
        r.wifi.set_events(true);
        r.wifi.set_identity(1, 14);
        r.init();
        r.rpc(&[cmd::SET_POWER_MODE, 0, 0, 0, 1, 0]);
        r.wifi.reset();
        assert_eq!(r.wifi.peek(BASE + 2), 0xD0);
        assert_eq!(r.wifi.peek(BASE + 3), 0x10);
        assert!(r.wifi.events());
        assert_eq!(r.wifi.identity(), (1, 14));
        assert_eq!(r.wifi.settings().power_mode, 1);
        assert_eq!(r.wifi.next_event(), u64::MAX);
    }

    #[test]
    fn savestate_round_trip() {
        let mut r = Rig::new();
        r.wifi.set_events(true);
        r.init();
        r.arm(0x2000);
        r.send(0x1000, &[cmd::WIFI_DISABLE, 1, 2, 3]);
        r.send(0x1000, &[cmd::MACHINE_OFF, 1, 2, 3]);
        r.wifi.rocker(Rocker::UpShort);
        r.rpc(&[cmd::SET_RECOVERY_URL, 0, 0, 0, b'h', b'i', 0]);
        let mut w = StateWriter::new();
        r.wifi.save(&mut w);
        let bytes = w.into_bytes();
        let mut other = Wifi::new();
        other.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(other, r.wifi);
        // The copy goes on exactly as the original.
        let mut w2 = StateWriter::new();
        other.save(&mut w2);
        assert_eq!(w2.into_bytes(), bytes);
    }

    #[test]
    fn truncated_or_corrupt_savestates_are_errors() {
        let mut r = Rig::new();
        r.init();
        r.send(0x1000, &[cmd::WIFI_DISABLE, 1, 2, 3]);
        let mut w = StateWriter::new();
        r.wifi.save(&mut w);
        let bytes = w.into_bytes();
        for n in 0..bytes.len() {
            let mut other = Wifi::new();
            assert!(other.load(&mut StateReader::new(&bytes[..n])).is_err());
        }
        // Every single byte damaged: an error or a state, never a panic.
        for i in 10..bytes.len() {
            let mut bad = bytes.clone();
            bad[i] = 0xFF;
            let mut other = Wifi::new();
            let _ = other.load(&mut StateReader::new(&bad));
            let mut rig = Rig::new();
            rig.wifi = other;
            rig.now = 5 * CLOCK_HZ;
            rig.with(|w, c| w.tick(c));
            rig.rd(2);
        }
    }

    // The ROM loader, driven the way the firmware's `Esp32::Download`,
    // `Flash` and `EnableRunMode` drive it. The frames are what the
    // firmware hands to the UART: the FPGA adds and removes the SLIP
    // framing, so these are the frames' whole contents.

    const SYNC: [u8; 44] = [
        0x00, 0x08, 0x24, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, 0x07, 0x12, 0x20, 0x55, 0x55, 0x55,
        0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
        0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    ];
    const SYNC_ANSWER: [u8; 12] = [1, 8, 4, 0, 0x07, 0x07, 0x12, 0x20, 0, 0, 0, 0];

    fn words(w: &[u32]) -> Vec<u8> {
        w.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    /// The loader's answer to a command that went well.
    fn done(op: u8) -> [u8; 12] {
        [1, op, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    }

    /// The loader's answer to one that did not.
    fn failed(op: u8, reason: u8) -> [u8; 12] {
        [1, op, 4, 0, 0, 0, 0, 0, 1, reason, 0, 0]
    }

    /// An application of 9000 bytes with the version text across the
    /// border of two blocks and two sectors.
    fn app_image(text: &[u8]) -> Vec<u8> {
        let mut v: Vec<u8> = (0..9000u32).map(|i| (i * 7 + i / 256) as u8).collect();
        v[4090..4090 + text.len()].copy_from_slice(text);
        v
    }

    impl Rig {
        fn mode(&mut self, mode: u8) {
            let f = self.rd(3);
            self.wr(3, (f & 0x8F) | mode << 4);
        }

        fn slip(&mut self, on: bool) {
            let f = self.rd(3);
            self.wr(3, if on { f | 0x04 } else { f & !0x04 });
        }

        fn clear_rx(&mut self) {
            for _ in 0..2 {
                let f = self.rd(3);
                self.wr(3, f | 0x80);
            }
        }

        /// The firmware's way into the loader, up to the ROM's greeting,
        /// which is returned; SLIP is on afterwards.
        fn download_mode(&mut self) -> Vec<u8> {
            self.slip(false);
            self.mode(MODE_OFF);
            self.clear_rx();
            self.run(CLOCK_HZ * 3 / 4);
            self.clear_rx();
            self.slip(false);
            self.wr(2, 0x85);
            self.arm(0x2000);
            self.mode(MODE_BOOT);
            self.run(CLOCK_HZ / 2);
            assert_eq!(
                self.rd(2) & 0x20,
                0,
                "without SLIP the text is no packet yet"
            );
            // SLIP on and off again cuts what has arrived into a packet.
            self.slip(true);
            self.slip(false);
            let text = self.receive(0x2000);
            self.slip(true);
            text
        }

        /// One loader command with the header the firmware builds.
        fn loader(&mut self, op: u8, checksum: u8, data: &[u8]) -> Vec<u8> {
            let n = data.len();
            let mut frame = vec![0, op, n as u8, (n >> 8) as u8, checksum, 0, 0, 0];
            frame.extend_from_slice(data);
            self.rpc(&frame)
        }

        fn flash_begin(&mut self, len: usize, offset: u32) -> Vec<u8> {
            let blocks = len.div_ceil(1024) as u32;
            self.loader(
                rom::FLASH_BEGIN,
                0,
                &words(&[blocks * 1024, blocks, 1024, offset, 0]),
            )
        }

        /// A block of 1024 bytes, a short one filled with `$FF`.
        fn flash_block(&mut self, seq: u32, chunk: &[u8]) -> Vec<u8> {
            let mut data = words(&[1024, seq, 0, 0]);
            data.extend_from_slice(chunk);
            data.resize(16 + 1024, 0xFF);
            let checksum = data[16..].iter().fold(0xEF, |a, b| a ^ b);
            self.loader(rom::FLASH_DATA, checksum, &data)
        }

        fn flash(&mut self, image: &[u8], offset: u32) {
            assert_eq!(
                self.flash_begin(image.len(), offset),
                done(rom::FLASH_BEGIN)
            );
            for (i, chunk) in image.chunks(1024).enumerate() {
                assert_eq!(
                    self.flash_block(i as u32, chunk),
                    done(rom::FLASH_DATA),
                    "block {i}"
                );
            }
        }

        /// Sync and the set-up commands of `Esp32::Download`.
        fn loader_setup(&mut self) {
            self.arm(0x2000);
            self.send(0x1000, &SYNC);
            self.run(2 * REPLY_CLOCKS);
            for _ in 0..8 {
                assert_eq!(self.receive(0x2000), SYNC_ANSWER);
                self.arm(0x2000);
            }
            assert_eq!(self.rd(2) & 0x20, 0, "eight answers, no more");
            assert_eq!(
                self.loader(rom::SPI_ATTACH, 0, &words(&[0, 0])),
                done(rom::SPI_ATTACH)
            );
            assert_eq!(
                self.loader(rom::CHANGE_BAUDRATE, 0, &words(&[2_000_000, 0])),
                done(rom::CHANGE_BAUDRATE)
            );
            // 100 MHz / 2 Mbit/s - 1.
            self.wr(1, 0);
            self.wr(0, 49);
            assert_eq!(
                self.loader(
                    rom::SPI_SET_PARAMS,
                    0,
                    &words(&[0, 0x20_0000, 0x1_0000, 0x1000, 0x100, 0xFFFF])
                ),
                done(rom::SPI_SET_PARAMS)
            );
        }

        /// The firmware's way back: off, then the module's program
        /// started without SLIP, 1.5 s for its messages, SLIP on.
        fn run_mode(&mut self) {
            self.mode(MODE_OFF);
            self.clear_rx();
            self.run(CLOCK_HZ / 4);
            self.wr(1, 0x03);
            self.wr(0, 0x63);
            self.clear_rx();
            self.slip(false);
            self.wr(2, 0x85);
            self.mode(0);
            self.run(CLOCK_HZ * 3 / 2);
            assert_eq!(self.rd(2) & 0x20, 0);
            self.slip(true);
        }

        fn unanswered(&mut self, frame: &[u8]) {
            self.arm(0x2000);
            self.send(0x1000, frame);
            self.run(CLOCK_HZ / 10);
            assert_eq!(self.rd(2) & 0x20, 0, "an answer to {frame:02x?}");
        }
    }

    #[test]
    fn the_rom_greets_with_the_text_the_firmware_looks_for() {
        let mut r = Rig::new();
        r.init();
        let text = r.download_mode();
        assert_eq!(&text[..text.len() - 1], ROM_GREETING);
        assert_eq!(text.last(), Some(&0x0A));
        // The firmware tries the first hundred places.
        assert!(text[..107].windows(8).any(|w| w == b"DOWNLOAD"));
        // Said once.
        r.arm(0x2000);
        r.slip(false);
        r.run(CLOCK_HZ);
        r.slip(true);
        assert_eq!(r.rd(2) & 0x20, 0);
    }

    #[test]
    fn the_updaters_whole_sequence_gives_the_module_the_new_version() {
        let mut r = Rig::new();
        r.init();
        assert_eq!(r.rpc(&[cmd::IDENTIFY, 0, 0, 0])[4..8], [1, 0, 30, 0]);
        r.download_mode();
        r.loader_setup();
        // Boot loader, partition table, application: where the updater
        // puts them.
        let bootloader: Vec<u8> = (0..20_500u32).map(|i| (i % 251) as u8).collect();
        let table = [0xAA, 0x50, 0x01, 0x02, 0x00, 0x90];
        let app = app_image(b"ESP32 WiFi Bridge V1.14\0");
        r.flash(&bootloader, 0x00_0000);
        r.flash(&table, 0x00_8000);
        r.flash(&app, 0x01_0000);
        // What is in the flash: the last block of each filled with $FF.
        let sector = |r: &Rig, n: u32| r.wifi.loader.flash[&n].clone();
        assert_eq!(sector(&r, 0), bootloader[..4096]);
        assert_eq!(sector(&r, 5)[..20], bootloader[20_480..]);
        assert!(sector(&r, 5)[20..1024].iter().all(|&b| b == 0xFF));
        assert_eq!(sector(&r, 8)[..6], table);
        assert_eq!(sector(&r, 0x11), app[4096..8192]);
        assert_eq!(r.wifi.loader.flash.len(), 6 + 1 + 3);
        // Still the loader: nothing has changed yet.
        assert_eq!(r.wifi.identity(), (1, 30));
        assert_eq!(r.wifi.take_identity_changed(), None);
        r.run_mode();
        let reply = r.rpc(&[cmd::IDENTIFY, 0, 0, 0]);
        assert_eq!(&reply[4..8], &[1, 0, 14, 0]);
        assert_eq!(&reply[8..31], b"ESP32 WiFi Bridge V1.14");
        assert_eq!(r.wifi.take_identity_changed(), Some((1, 14)));
        assert_eq!(r.wifi.take_identity_changed(), None);
        assert!(r.wifi.loader.flash.is_empty(), "the bytes are not kept");
        // The same software once more: nothing to tell the machine.
        r.download_mode();
        r.loader_setup();
        r.flash(&app, 0x01_0000);
        r.run_mode();
        assert_eq!(r.wifi.identity(), (1, 14));
        assert_eq!(r.wifi.take_identity_changed(), None);
    }

    #[test]
    fn a_flash_without_the_version_text_leaves_the_version() {
        let mut r = Rig::new();
        r.init();
        r.download_mode();
        r.loader_setup();
        r.flash(&[0x55; 3000], 0x00_8000);
        r.run_mode();
        assert_eq!(r.wifi.identity(), (1, 30));
        assert_eq!(r.wifi.take_identity_changed(), None);
    }

    #[test]
    fn the_board_reset_keeps_a_flash_that_was_not_started() {
        let mut r = Rig::new();
        r.init();
        r.download_mode();
        r.loader_setup();
        r.flash(&app_image(b"ESP32 WiFi Bridge V3.02\0"), 0x01_0000);
        r.wifi.reset();
        // The firmware's start: the UART's reset, which also selects run
        // mode.
        r.wr(3, 0x80);
        assert_eq!(r.wifi.take_identity_changed(), Some((3, 2)));
        r.init();
        assert_eq!(r.rpc(&[cmd::IDENTIFY, 0, 0, 0])[4..8], [3, 0, 2, 0]);
    }

    #[test]
    fn the_loader_refuses_what_the_rom_refuses() {
        let mut r = Rig::new();
        r.init();
        r.download_mode();
        r.loader_setup();
        // Data before a transfer is open.
        assert_eq!(
            r.flash_block(0, &[1, 2, 3]),
            failed(rom::FLASH_DATA, rom::ERR_FAILED)
        );
        assert_eq!(r.flash_begin(2048, 0x1_0000), done(rom::FLASH_BEGIN));
        // A wrong checksum, a block beyond the announced ones, a length
        // that is not the data's.
        let mut data = words(&[4, 0, 0, 0]);
        data.extend_from_slice(&[1, 2, 3, 4]);
        assert_eq!(
            r.loader(rom::FLASH_DATA, 0, &data),
            failed(rom::FLASH_DATA, rom::ERR_CHECKSUM)
        );
        assert_eq!(
            r.flash_block(2, &[1]),
            failed(rom::FLASH_DATA, rom::ERR_FAILED)
        );
        let mut data = words(&[5, 0, 0, 0]);
        data.extend_from_slice(&[1, 2, 3, 4]);
        assert_eq!(
            r.loader(rom::FLASH_DATA, 0xEF ^ 4, &data),
            failed(rom::FLASH_DATA, rom::ERR_INVALID)
        );
        assert!(r.wifi.loader.flash.is_empty());
        // Beyond the flash.
        assert_eq!(
            r.flash_begin(1024, FLASH_SIZE),
            failed(rom::FLASH_BEGIN, rom::ERR_FAILED)
        );
        assert_eq!(
            r.flash_begin(2048, FLASH_SIZE - 1024),
            failed(rom::FLASH_BEGIN, rom::ERR_FAILED)
        );
        assert_eq!(
            r.loader(rom::FLASH_BEGIN, 0, &words(&[0, u32::MAX, u32::MAX, 0, 0])),
            failed(rom::FLASH_BEGIN, rom::ERR_FAILED)
        );
        // Not in the ROM as modelled (MD5), wrong sizes, a length field
        // that is not the frame's.
        assert_eq!(r.loader(0x13, 0, &words(&[0, 16, 0, 0])), failed(0x13, 5));
        assert_eq!(
            r.loader(rom::SPI_ATTACH, 0, &[0]),
            failed(rom::SPI_ATTACH, 5)
        );
        assert_eq!(
            r.rpc(&[0, rom::FLASH_END, 9, 0, 0, 0, 0, 0, 1, 0, 0, 0]),
            failed(rom::FLASH_END, 5)
        );
        // The chip's kind and the end of a transfer.
        let reply = r.loader(rom::READ_REG, 0, &words(&[rom::CHIP_MAGIC_REG]));
        assert_eq!(reply, [1, 0x0A, 4, 0, 9, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            r.loader(rom::FLASH_END, 0, &words(&[1])),
            done(rom::FLASH_END)
        );
        assert_eq!(
            r.flash_block(0, &[1]),
            failed(rom::FLASH_DATA, rom::ERR_FAILED)
        );
        // No request at all: too short, an answer's direction byte; and
        // a request that is no frame.
        r.unanswered(&[0, rom::SYNC, 0, 0]);
        r.unanswered(&[1, rom::FLASH_END, 4, 0, 0, 0, 0, 0, 1, 0, 0, 0]);
        r.slip(false);
        r.unanswered(&SYNC);
        r.slip(true);
        assert_eq!(r.rd(2) & 0x20, 0);
    }

    #[test]
    fn flash_begin_erases_and_programming_only_clears_bits() {
        let mut r = Rig::new();
        r.init();
        r.download_mode();
        r.loader_setup();
        r.flash(&[0xF0; 1024], 0x2_0000);
        // Without an erase in between the old bytes show through.
        assert_eq!(r.flash_block(0, &[0x3C; 1024]), done(rom::FLASH_DATA));
        assert!(r.wifi.loader.flash[&0x20][..1024]
            .iter()
            .all(|&b| b == 0x30));
        assert_eq!(r.flash_begin(1024, 0x2_0000), done(rom::FLASH_BEGIN));
        assert!(r.wifi.loader.flash.is_empty());
        assert_eq!(r.flash_block(0, &[0x3C; 1024]), done(rom::FLASH_DATA));
        assert!(r.wifi.loader.flash[&0x20][..1024]
            .iter()
            .all(|&b| b == 0x3C));
        assert!(r.wifi.loader.flash[&0x20][1024..]
            .iter()
            .all(|&b| b == 0xFF));
    }

    #[test]
    fn without_slip_bytes_come_as_they_are_in_pieces_of_1536() {
        let mut r = Rig::new();
        r.init();
        r.slip(false);
        // Loopback without SLIP: plain bytes come back.
        let f = r.rd(3);
        r.wr(3, f | 0x02);
        let piece: Vec<u8> = (0..1000u32).map(|i| i as u8).collect();
        r.arm(0x2000);
        r.arm(0x3000);
        r.send(0x1000, &piece);
        r.run(2 * REPLY_CLOCKS);
        assert_eq!(r.rd(2) & 0x20, 0, "1000 bytes are no packet");
        r.send(0x1000, &piece);
        r.run(2 * REPLY_CLOCKS);
        let first = r.receive(0x2000);
        assert_eq!(first.len(), 1536);
        assert_eq!(first[..1000], piece[..]);
        assert_eq!(first[1000..], piece[..536]);
        assert_eq!(r.rd(2) & 0x20, 0);
        // Setting SLIP ends the rest with a line feed.
        r.slip(true);
        let rest = r.receive(0x3000);
        assert_eq!(rest.len(), 465);
        assert_eq!(rest[..464], piece[536..]);
        assert_eq!(rest[464], 0x0A);
        // A soft reset throws collected bytes away.
        r.slip(false);
        r.arm(0x2000);
        r.send(0x1000, &piece);
        r.run(2 * REPLY_CLOCKS);
        r.clear_rx();
        r.slip(true);
        assert_eq!(r.rd(2) & 0x20, 0);
    }

    #[test]
    fn without_slip_a_frame_shows_its_framing() {
        let mut r = Rig::new();
        r.init();
        r.download_mode();
        r.arm(0x2000);
        r.send(0x1000, &SYNC);
        r.slip(false);
        r.run(2 * REPLY_CLOCKS);
        r.slip(true);
        let bytes = r.receive(0x2000);
        assert_eq!(bytes.len(), 8 * 14 + 1);
        assert_eq!(bytes[0], 0xC0);
        assert_eq!(bytes[1..13], SYNC_ANSWER);
        assert_eq!(bytes[13..15], [0xC0, 0xC0]);
        assert_eq!(
            slip_encode(&[1, 0xC0, 0xDB, 2]),
            [0xC0, 1, 0xDB, 0xDC, 0xDB, 0xDD, 2, 0xC0]
        );
    }

    #[test]
    fn garbage_in_download_mode_never_panics() {
        let mut seed = 0x1541_0064u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed >> 8
        };
        let mut r = Rig::new();
        r.init();
        r.download_mode();
        let mut wrote = false;
        for i in 0..4000u32 {
            let len = next() as usize % 1200;
            let mut p: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            if i % 2 == 0 && len >= 8 {
                // A header the loader takes, around random data.
                const OPS: [u8; 10] = [2, 3, 4, 5, 8, 0x0A, 0x0B, 0x0D, 0x0F, 0x13];
                p[0] = 0;
                p[1] = OPS[next() as usize % OPS.len()];
                p[2] = (len - 8) as u8;
                p[3] = ((len - 8) >> 8) as u8;
                if p[1] == rom::FLASH_DATA && len >= 24 {
                    // And a block the loader takes.
                    p[8..12].copy_from_slice(&(len as u32 - 24).to_le_bytes());
                    p[12..16].copy_from_slice(&(next() % 0x2400).to_le_bytes());
                    let checksum = p[24..].iter().fold(0xEF, |a, b| a ^ b);
                    p[4..8].copy_from_slice(&u32::from(checksum).to_le_bytes());
                }
            }
            if i % 64 == 0 {
                // The whole flash as one transfer.
                p = vec![0, rom::FLASH_BEGIN, 16, 0, 0, 0, 0, 0];
                p.extend_from_slice(&words(&[next() % 0x90_0000, 0x2000, 0x400, 0]));
                r.mode(MODE_BOOT);
                r.slip(true);
            } else {
                match next() % 96 {
                    0 => r.mode(next() as u8 & 7),
                    1 => r.slip(next() & 1 != 0),
                    2 => r.clear_rx(),
                    3 => r.wifi.reset(),
                    _ => {}
                }
            }
            r.arm(0x2000);
            r.send(0x1000, &p);
            r.run(2 * REPLY_CLOCKS);
            while r.rd(2) & 0x20 != 0 {
                r.wr(0xB, 1);
                r.arm(0x2000);
            }
            assert!(r.wifi.loader.flash.len() <= (FLASH_SIZE / FLASH_SECTOR) as usize);
            assert!(r.wifi.ascii.len() < MAX_PACKET);
            wrote |= !r.wifi.loader.flash.is_empty();
        }
        assert!(wrote, "some of it reached the flash");
    }

    #[test]
    fn savestate_in_the_middle_of_a_flash() {
        let app = app_image(b"ESP32 WiFi Bridge V2.07\0");
        let mut r = Rig::new();
        r.init();
        r.download_mode();
        r.loader_setup();
        assert_eq!(r.flash_begin(app.len(), 0x1_0000), done(rom::FLASH_BEGIN));
        let mut blocks = app.chunks(1024).enumerate();
        for (i, chunk) in blocks.by_ref().take(4) {
            assert_eq!(r.flash_block(i as u32, chunk), done(rom::FLASH_DATA));
        }
        // An answer under way and text collected without SLIP as well.
        r.send(0x1000, &SYNC);
        let mut w = StateWriter::new();
        r.wifi.save(&mut w);
        let bytes = w.into_bytes();
        let mut other = Wifi::new();
        other.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(other, r.wifi);
        let mut w2 = StateWriter::new();
        other.save(&mut w2);
        assert_eq!(w2.into_bytes(), bytes);
        for n in 0..bytes.len() {
            let mut bad = Wifi::new();
            assert!(bad.load(&mut StateReader::new(&bytes[..n])).is_err());
        }
        for i in (10..bytes.len()).step_by(3) {
            let mut bad = bytes.clone();
            bad[i] ^= 0xFF;
            let mut rig = Rig::new();
            let _ = rig.wifi.load(&mut StateReader::new(&bad));
            rig.now = 5 * CLOCK_HZ;
            rig.with(|w, c| w.tick(c));
            rig.arm(0x2000);
            let mut block = vec![0, rom::FLASH_DATA, 20, 0, 0xEF, 0, 0, 0];
            block.extend_from_slice(&words(&[4, 5, 0, 0, 0]));
            rig.send(0x1000, &block);
            rig.run(2 * REPLY_CLOCKS);
            rig.mode(0);
            rig.slip(true);
        }
        // The copy finishes the flash.
        r.wifi = other;
        r.run(2 * REPLY_CLOCKS);
        for _ in 0..8 {
            r.arm(0x2000);
            assert_eq!(r.receive(0x2000), SYNC_ANSWER);
        }
        for (i, chunk) in blocks {
            assert_eq!(r.flash_block(i as u32, chunk), done(rom::FLASH_DATA));
        }
        r.run_mode();
        assert_eq!(r.wifi.take_identity_changed(), Some((2, 7)));
    }

    #[test]
    fn savestate_keeps_bytes_collected_without_slip() {
        let mut r = Rig::new();
        r.init();
        r.slip(false);
        r.mode(MODE_OFF);
        r.arm(0x2000);
        r.mode(MODE_BOOT);
        r.run(CLOCK_HZ / 2);
        assert_eq!(r.wifi.ascii, ROM_GREETING);
        let mut w = StateWriter::new();
        r.wifi.save(&mut w);
        let bytes = w.into_bytes();
        let mut other = Wifi::new();
        other.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(other, r.wifi);
        r.wifi = other;
        r.slip(true);
        assert_eq!(r.receive(0x2000).len(), ROM_GREETING.len() + 1);
    }
}
