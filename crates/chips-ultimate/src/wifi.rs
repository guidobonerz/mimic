//! The WiFi DMA UART (`$060900`–`$06090F`) and, behind it, a stand-in for
//! the board's ESP32 control module ("u64ctrl").
//!
//! The UART moves whole packets between the board's memory and the serial
//! line by itself: the firmware hands it the address and length of a
//! packet to send, and addresses of buffers to receive into. The processor
//! never sees single bytes, so this model works on packets too; SLIP
//! framing, baud rate and flow control have no effect the firmware could
//! observe beyond reading the bits back.
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
//! Not modelled: the ESP32's serial boot loader (module mode "boot"), with
//! which an updater writes new software into the module. In boot mode the
//! stand-in is silent; the updater then reports that it could not reach
//! the loader and goes on. It does not try when the module's version is
//! the one it carries: [`Wifi::set_identity`], [`identity_in_image`].
//!
//! Sources (facts only): UE2-C64U-Emulator `docs/hw/04-esp32-wifi.md`
//! (registers, hazards H1–H11, request and reply layouts),
//! `docs/specs/S05-wifi-u64ctrl.md`, `S42-esp32-rocker-events.md`,
//! `docs/status/install.md` §5; 1541 Ultimate
//! `fpga/io/uart_lite/vhdl_source/uart_dma.vhd`, `rx_dma.vhd`,
//! `tx_dma.vhd`, `software/io/uart/dma_uart.h`/`.cc`,
//! `software/u64ctrl/main/rpc_calls.h`, `rpc_dispatch.h`,
//! `software/io/wifi/esp32.h`, `wifi_cmd.h`,
//! `software/application/update_u2p/update_u64ii.cc`.

use crate::io::{IoCtx, IoDevice, CLOCK_HZ};
use mimic_core::{StateError, StateReader, StateWriter};
use std::collections::VecDeque;

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
/// The longest packet the receiver writes; a longer one is cut.
const MAX_PACKET: usize = 1536;
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
    data: Vec<u8>,
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
    ctrl: U64Ctrl,
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
            ctrl: U64Ctrl::new(),
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
    /// this is the version it carries; see [`identity_in_image`].
    pub fn set_identity(&mut self, major: u16, minor: u16) {
        self.ctrl.major = major;
        self.ctrl.minor = minor;
    }

    pub fn identity(&self) -> (u16, u16) {
        (self.ctrl.major, self.ctrl.minor)
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

    /// The module's own program runs (module mode in bits 6–4).
    fn module_runs(&self) -> bool {
        !matches!((self.flow >> 4) & 7, MODE_BOOT | MODE_OFF | MODE_BOOT_UART)
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
            // Loopback: what is sent comes back.
            self.enqueue(due, data);
        } else if self.module_runs() {
            for answer in self.ctrl.request(&data, ctx.now) {
                self.enqueue(due, answer);
            }
        }
    }

    fn enqueue(&mut self, due: u64, data: Vec<u8>) {
        if self.queue.len() < MAX_QUEUE && !data.is_empty() {
            // The line keeps the order.
            let due = self.queue.back().map_or(due, |p| p.due.max(due));
            self.queue.push_back(Packet { due, data });
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
            if self.events {
                if self.queue.front().is_some_and(|p| p.data[0] & 0x80 != 0) {
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
                if !self.rx_addr_valid {
                    break;
                }
                self.rx_active = Some(self.rx_addr);
                self.rx_addr_valid = false;
            }
            let Some(addr) = self.rx_active else {
                break;
            };
            let Some(p) = self.queue.pop_front() else {
                break;
            };
            let n = p.data.len().min(MAX_PACKET);
            if !ctx.ram.is_empty() {
                let mask = ctx.ram.len() - 1;
                let base = (addr & DMA_MASK) as usize;
                for (i, &b) in p.data[..n].iter().enumerate() {
                    ctx.ram[(base + i) & mask] = b;
                }
            }
            self.rx_active = None;
            self.rx_held = Some(n as u16);
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
                self.flow = v & 0x77;
                if v & 0x80 != 0 {
                    self.soft_reset(ctx.now);
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
    /// its settings and identity; the host's choices (event decode, MAC)
    /// stay as well.
    fn reset(&mut self) {
        let mut fresh = Self::new();
        fresh.events = self.events;
        fresh.ctrl = self.ctrl.clone();
        fresh.ctrl.restart();
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
            let data = r.bytes()?;
            if data.is_empty() || data.len() > 0x1_0000 {
                return Err(StateError::Invalid("WiFi UART packet"));
            }
            n.queue.push_back(Packet {
                due,
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
    fn a_module_in_boot_mode_is_silent() {
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
}
