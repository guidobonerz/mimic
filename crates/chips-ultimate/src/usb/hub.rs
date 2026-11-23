//! The board's USB2513 hub: the controller's only directly attached
//! device, a high-speed hub with three downstream ports (USB 2.0 chapter
//! 11), each of which can hold a USB stick on one of the board's storage
//! units ([`crate::io::unit::USB0`] + port − 1).
//!
//! Descriptors: device class 9, protocol 1 (one transaction translator),
//! VID `$0424` PID `$2513` release `$0BA0` (what the firmware writes into
//! the hub's configuration over I2C), one configuration with one interface
//! (class 9) and the status change endpoint `$81` (interrupt IN, one
//! byte). Hub descriptor: 3 ports, individual power switching and
//! over-current protection, 100 ms power-on to power-good, 1 mA for the
//! hub controller, no non-removable devices.
//!
//! The status change endpoint answers NAK while no port has a change bit,
//! and else one byte with bit p set for each changed port p: the
//! firmware's hub driver re-arms its status pipe only after a non-zero
//! bitmap (UE2-C64U-Emulator `docs/hw/09-usb.md` H13) and asserts on more
//! than four bytes (H12).
//!
//! Port state machine: a port is connected while it is powered and holds a
//! device; any change of that sets C_PORT_CONNECTION. SET_FEATURE
//! (PORT_RESET) on a connected port drives a reset for 10 ms
//! ([`PORT_RESET_CLOCKS`]); then the port is enabled, its device is back at
//! address 0, C_PORT_RESET is set and the port status says high speed (all
//! devices here are high-speed devices). Disconnecting disables the port.
//!
//! Hot-plug: [`Hub::poll_media`] compares each unit's medium with the
//! stick on its port. A unit with a medium gets a stick, a unit whose
//! medium went away or changed (its generation differs) loses it. A new
//! stick comes only after the firmware has taken note of the last
//! disconnect (its C_PORT_CONNECTION is clear) and [`REPLUG_GAP_CLOCKS`]
//! after it, so the firmware sees one change at a time: its hub driver
//! mishandles a device that is replaced before it has seen the
//! disconnect (1541 Ultimate `usb_hub.cc`, "device already present").
//!
//! Sources (facts only): USB 2.0 specification chapters 9 and 11 (request
//! codes, port status bits, hub descriptor); 1541 Ultimate
//! `software/io/usb/usb_hub.cc` (the requests the driver sends, the
//! lengths it expects), `usb_hwinit.cc` (VID, PID, release, power-on time);
//! UE2-C64U-Emulator `docs/hw/09-usb.md` §F8, H12–H14, H16.

use super::device::{Core, Env, Function, Response, Setup};
use super::storage::MassStorage;
use crate::io::{unit, CLOCK_HZ};
use mimic_core::{StateError, StateReader, StateWriter};

/// Downstream ports.
pub const PORTS: usize = unit::USB_PORTS as usize;
/// How long a port reset lasts: 10 ms (USB 2.0, TDRSTR).
pub const PORT_RESET_CLOCKS: u64 = CLOCK_HZ / 100;
/// The least time between a stick going and the next one coming on the
/// same port: 200 ms.
pub const REPLUG_GAP_CLOCKS: u64 = CLOCK_HZ / 5;

/// Hub class features (USB 2.0, table 11-17).
mod feature {
    pub const PORT_ENABLE: u16 = 1;
    pub const PORT_SUSPEND: u16 = 2;
    pub const PORT_RESET: u16 = 4;
    pub const PORT_POWER: u16 = 8;
    pub const C_PORT_CONNECTION: u16 = 0x10;
    pub const C_PORT_RESET: u16 = 0x14;
}

/// Port status bits (USB 2.0, table 11-21).
mod status {
    pub const CONNECTION: u16 = 0x0001;
    pub const ENABLE: u16 = 0x0002;
    pub const SUSPEND: u16 = 0x0004;
    pub const RESET: u16 = 0x0010;
    pub const POWER: u16 = 0x0100;
    pub const HIGH_SPEED: u16 = 0x0400;
}

/// Port change bits (USB 2.0, table 11-22).
const C_CONNECTION: u16 = 0x0001;
const C_RESET: u16 = 0x0010;

/// What can sit on a port. Extension point: a HID keyboard or mouse is
/// another variant here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Child {
    Storage(MassStorage),
}

impl Child {
    pub fn function(&mut self) -> &mut dyn Function {
        match self {
            Child::Storage(s) => s,
        }
    }

    fn address(&self) -> u8 {
        match self {
            Child::Storage(s) => s.core().address,
        }
    }

    fn bus_reset(&mut self) {
        match self {
            Child::Storage(s) => s.bus_reset(),
        }
    }

    fn save(&self, w: &mut StateWriter) {
        match self {
            Child::Storage(s) => {
                w.u8(1);
                s.save(w);
            }
        }
    }

    fn load(r: &mut StateReader<'_>) -> Result<Option<Self>, StateError> {
        match r.u8()? {
            0 => Ok(None),
            1 => Ok(Some(Child::Storage(MassStorage::load(r)?))),
            _ => Err(StateError::Invalid("USB port device")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Port {
    pub powered: bool,
    pub connected: bool,
    pub enabled: bool,
    pub suspended: bool,
    /// The end of a port reset under way; `u64::MAX`: none.
    pub reset_until: u64,
    /// Change bits (wPortChange).
    pub change: u16,
    pub device: Option<Child>,
    /// No new stick before this time.
    pub replug_at: u64,
}

impl Default for Port {
    fn default() -> Self {
        Self {
            powered: false,
            connected: false,
            enabled: false,
            suspended: false,
            reset_until: u64::MAX,
            change: 0,
            device: None,
            replug_at: 0,
        }
    }
}

impl Port {
    /// wPortStatus.
    pub fn status(&self) -> u16 {
        let mut s = 0;
        if self.connected {
            s |= status::CONNECTION;
        }
        if self.enabled {
            s |= status::ENABLE | status::HIGH_SPEED;
        }
        if self.suspended {
            s |= status::SUSPEND;
        }
        if self.reset_until != u64::MAX {
            s |= status::RESET;
        }
        if self.powered {
            s |= status::POWER;
        }
        s
    }

    /// Connected is: powered and a device in it. A change sets
    /// C_PORT_CONNECTION; a disconnect disables the port.
    fn update_connection(&mut self) {
        let c = self.powered && self.device.is_some();
        if c != self.connected {
            self.connected = c;
            self.change |= C_CONNECTION;
            if !c {
                self.enabled = false;
                self.suspended = false;
                self.reset_until = u64::MAX;
            }
        }
    }

    /// The state after the hub's own reset: unpowered, nothing pending.
    /// The device stays plugged in.
    fn hub_reset(&mut self) {
        let device = self.device.take();
        let replug_at = self.replug_at;
        *self = Self {
            device,
            replug_at,
            ..Self::default()
        };
        if let Some(d) = &mut self.device {
            d.bus_reset();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hub {
    core: Core,
    pub ports: [Port; PORTS],
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    pub fn new() -> Self {
        Self {
            core: Core::default(),
            ports: Default::default(),
        }
    }

    /// A reset on the bus between the controller and the hub: the hub
    /// starts again unconfigured with its ports switched off.
    pub fn bus_reset(&mut self) {
        self.core.reset();
        for p in &mut self.ports {
            p.hub_reset();
        }
    }

    /// The function at bus address `addr`: the hub itself or a device on
    /// an enabled port.
    pub fn route(&mut self, addr: u8) -> Option<&mut dyn Function> {
        if self.core.address == addr {
            return Some(self);
        }
        for p in &mut self.ports {
            if !p.enabled || p.reset_until != u64::MAX {
                continue;
            }
            if let Some(d) = &mut p.device {
                if d.address() == addr {
                    return Some(d.function());
                }
            }
        }
        None
    }

    /// The next time [`Hub::tick`] has something to do.
    pub fn next_event(&self) -> u64 {
        self.ports
            .iter()
            .map(|p| p.reset_until)
            .min()
            .unwrap_or(u64::MAX)
    }

    /// Ends port resets that are due.
    pub fn tick(&mut self, now: u64) {
        for p in &mut self.ports {
            if p.reset_until <= now {
                p.reset_until = u64::MAX;
                if p.connected {
                    p.enabled = true;
                    p.change |= C_RESET;
                    if let Some(d) = &mut p.device {
                        d.bus_reset();
                    }
                }
            }
        }
    }

    /// Follows the media of the board's USB units (see the module
    /// documentation).
    pub fn poll_media(&mut self, env: &mut Env<'_>) {
        for (i, p) in self.ports.iter_mut().enumerate() {
            let unit = unit::USB0 + i as u8;
            let (has, generation) = match env.storage.as_ref() {
                Some(s) => (s.sectors(unit).is_some_and(|n| n > 0), s.generation(unit)),
                None => (false, 0),
            };
            let stale = match &p.device {
                Some(Child::Storage(s)) => !has || s.generation != generation,
                None => false,
            };
            if stale {
                p.device = None;
                p.replug_at = env.now.saturating_add(REPLUG_GAP_CLOCKS);
                p.update_connection();
            }
            let noticed = p.change & C_CONNECTION == 0 || !p.powered;
            if p.device.is_none() && has && noticed && env.now >= p.replug_at {
                p.device = Some(Child::Storage(MassStorage::new(
                    unit,
                    generation,
                    i as u8 + 1,
                )));
                p.update_connection();
            }
        }
    }

    fn hub_descriptor(&self) -> Vec<u8> {
        vec![
            9,
            0x29,
            PORTS as u8,
            0x09,
            0x00, // individual power, individual over-current
            0x32, // 100 ms power-on to power-good
            0x01, // 1 mA
            0x00, // no non-removable devices
            0xFF, // port power control mask (USB 1.1 field)
        ]
    }

    pub fn save(&self, w: &mut StateWriter) {
        self.core.save(w);
        for p in &self.ports {
            w.bool(p.powered);
            w.bool(p.connected);
            w.bool(p.enabled);
            w.bool(p.suspended);
            w.u64(p.reset_until);
            w.u16(p.change);
            w.u64(p.replug_at);
            match &p.device {
                Some(d) => d.save(w),
                None => w.u8(0),
            }
        }
    }

    pub fn load(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let core = Core::load(r)?;
        let mut hub = Self {
            core,
            ports: Default::default(),
        };
        for p in &mut hub.ports {
            p.powered = r.bool()?;
            p.connected = r.bool()?;
            p.enabled = r.bool()?;
            p.suspended = r.bool()?;
            p.reset_until = r.u64()?;
            p.change = r.u16()? & 0x1F;
            p.replug_at = r.u64()?;
            p.device = Child::load(r)?;
            if p.connected && (!p.powered || p.device.is_none()) {
                return Err(StateError::Invalid("USB hub port state"));
            }
        }
        Ok(hub)
    }
}

impl Function for Hub {
    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn device_descriptor(&self) -> [u8; 18] {
        [
            18, 1, 0x00, 0x02, // USB 2.0
            0x09, 0x00, 0x01, 64, // hub, single TT, EP0 64 bytes
            0x24, 0x04, 0x13, 0x25, // VID $0424, PID $2513
            0xA0, 0x0B, // release $0BA0
            0, 0, 0, // no strings
            1, // one configuration
        ]
    }

    fn configuration_descriptor(&self) -> Vec<u8> {
        vec![
            9, 2, 25, 0, 1, 1, 0, 0xE0, 1, // self powered, remote wakeup
            9, 4, 0, 0, 1, 0x09, 0, 0, 0, // hub interface
            7, 5, 0x81, 3, 1, 0, 12, // status change: interrupt IN, 1 byte, 256 ms
        ]
    }

    fn class_in(&mut self, s: &Setup, _env: &mut Env<'_>) -> Option<Vec<u8>> {
        match (s.request_type, s.request) {
            // GET_DESCRIPTOR (hub).
            (0xA0, 6) if s.value >> 8 == 0x29 => Some(self.hub_descriptor()),
            // GET_STATUS (hub): local power good, no over-current.
            (0xA0, 0) => Some(vec![0; 4]),
            // GET_STATUS (port).
            (0xA3, 0) => {
                let p = self.ports.get(usize::from(s.index).wrapping_sub(1))?;
                let mut v = p.status().to_le_bytes().to_vec();
                v.extend_from_slice(&p.change.to_le_bytes());
                Some(v)
            }
            _ => None,
        }
    }

    fn class_out(&mut self, s: &Setup, _data: &[u8], env: &mut Env<'_>) -> bool {
        match (s.request_type, s.request) {
            // SET_FEATURE / CLEAR_FEATURE (hub): nothing to switch.
            (0x20, 1 | 3) => true,
            (0x23, r @ (1 | 3)) => {
                let Some(p) = self
                    .ports
                    .get_mut(usize::from(s.index & 0xFF).wrapping_sub(1))
                else {
                    return false;
                };
                let set = r == 3;
                match (set, s.value) {
                    (true, feature::PORT_POWER) => {
                        p.powered = true;
                        p.update_connection();
                    }
                    (false, feature::PORT_POWER) => {
                        p.powered = false;
                        p.update_connection();
                    }
                    (true, feature::PORT_RESET) => {
                        if p.connected {
                            p.enabled = false;
                            p.suspended = false;
                            p.reset_until = env.now.saturating_add(PORT_RESET_CLOCKS);
                        }
                    }
                    (true, feature::PORT_SUSPEND) => p.suspended = p.enabled,
                    (false, feature::PORT_SUSPEND) => p.suspended = false,
                    (false, feature::PORT_ENABLE) => {
                        p.enabled = false;
                        p.suspended = false;
                    }
                    (false, f @ feature::C_PORT_CONNECTION..=feature::C_PORT_RESET) => {
                        p.change &= !(1 << (f - feature::C_PORT_CONNECTION));
                    }
                    // Indicator, test mode, enable by SET_FEATURE: ignored.
                    _ => {}
                }
                true
            }
            _ => false,
        }
    }

    fn endpoint_in(
        &mut self,
        ep: u8,
        max: usize,
        buf: &mut Vec<u8>,
        _env: &mut Env<'_>,
    ) -> Response {
        if ep != 1 {
            return Response::Stall;
        }
        let mut bitmap = 0u8;
        for (i, p) in self.ports.iter().enumerate() {
            if p.change != 0 {
                bitmap |= 2 << i;
            }
        }
        if bitmap == 0 || max == 0 {
            return Response::Nak;
        }
        buf.push(bitmap);
        Response::Data(1)
    }

    fn endpoint_out(&mut self, _ep: u8, _data: &[u8], _env: &mut Env<'_>) -> Response {
        Response::Stall
    }
}
