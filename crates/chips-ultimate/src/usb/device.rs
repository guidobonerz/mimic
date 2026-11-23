//! The device side of the bus: what every USB function behind the
//! controller shares. A function answers transactions (a SETUP packet, an
//! IN token, an OUT packet) addressed to it; endpoint 0 is the default
//! control endpoint, handled here for every function with the standard
//! requests of the USB 2.0 specification, chapter 9; class requests and
//! the other endpoints are the function's own.
//!
//! The model works on packets: the controller hands over one packet at a
//! time and gets one handshake (or one data packet) back, so a function
//! sees the same sequence a real one would, without the wire.

use crate::io::Storage;
use mimic_core::{StateError, StateReader, StateWriter};

/// What a function answers to one transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Response {
    /// The packet was taken (SETUP, OUT).
    Ack,
    /// Not now; the host tries again later.
    Nak,
    /// The endpoint is halted or the request is not supported.
    Stall,
    /// An IN token answered with this many bytes, put into the buffer the
    /// caller gave (0: a zero-length packet).
    Data(usize),
}

/// What a function may reach while it answers: the time and the media.
pub(crate) struct Env<'s> {
    /// The board's clock (100 MHz).
    pub now: u64,
    /// The board's storage, `None`: there is none.
    pub storage: Option<&'s mut dyn Storage>,
}

impl<'s> Env<'s> {
    pub fn new(now: u64, storage: &'s mut Option<&mut dyn Storage>) -> Self {
        Self {
            now,
            storage: match storage {
                Some(s) => Some(&mut **s),
                None => None,
            },
        }
    }
}

/// A SETUP packet, decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Setup {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

impl Setup {
    pub fn parse(p: &[u8; 8]) -> Self {
        Self {
            request_type: p[0],
            request: p[1],
            value: u16::from_le_bytes([p[2], p[3]]),
            index: u16::from_le_bytes([p[4], p[5]]),
            length: u16::from_le_bytes([p[6], p[7]]),
        }
    }

    /// Data flows from the device to the host.
    pub fn device_to_host(&self) -> bool {
        self.request_type & 0x80 != 0
    }

    /// Bits 6–5 of bmRequestType: 0 standard, 1 class, 2 vendor.
    pub fn kind(&self) -> u8 {
        (self.request_type >> 5) & 3
    }
}

/// Standard request codes (USB 2.0, table 9-4).
pub(crate) mod req {
    pub const GET_STATUS: u8 = 0;
    pub const CLEAR_FEATURE: u8 = 1;
    pub const SET_FEATURE: u8 = 3;
    pub const SET_ADDRESS: u8 = 5;
    pub const GET_DESCRIPTOR: u8 = 6;
    pub const GET_CONFIGURATION: u8 = 8;
    pub const SET_CONFIGURATION: u8 = 9;
    pub const GET_INTERFACE: u8 = 10;
    pub const SET_INTERFACE: u8 = 11;
}

/// Descriptor types (USB 2.0, table 9-5).
pub(crate) mod descr {
    pub const DEVICE: u8 = 1;
    pub const CONFIGURATION: u8 = 2;
    pub const STRING: u8 = 3;
}

/// Where the default control endpoint is in a control transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// No transfer; waiting for a SETUP packet.
    Idle,
    /// Device-to-host data waits in the buffer; the host's OUT packet is
    /// the status stage.
    DataIn,
    /// Host-to-device data goes into the buffer; the host's IN token is
    /// the status stage, where the request is carried out.
    DataOut,
    /// A request without data was accepted; the host's IN token (a
    /// zero-length packet back) ends it.
    StatusIn,
    /// The request is not supported: everything up to the next SETUP is
    /// answered with STALL.
    Stalled,
}

impl Stage {
    const ALL: [Stage; 5] = [
        Stage::Idle,
        Stage::DataIn,
        Stage::DataOut,
        Stage::StatusIn,
        Stage::Stalled,
    ];
}

/// The longest control transfer data this model keeps (descriptors are far
/// shorter; a host can ask for up to 64 KB, the answer is never longer than
/// what the function has).
const MAX_CONTROL_DATA: usize = 4096;

/// The part of every function the standard requests act on: its address,
/// its configuration and its default control endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Core {
    /// The function's address on the bus; 0 after a bus reset.
    pub address: u8,
    /// The selected configuration; 0: not configured.
    pub configuration: u8,
    stage: Stage,
    setup: [u8; 8],
    /// Data of the transfer under way.
    data: Vec<u8>,
    /// Bytes of `data` already sent (DataIn).
    sent: usize,
    /// SET_ADDRESS takes effect after its status stage.
    pending_address: Option<u8>,
}

impl Default for Core {
    fn default() -> Self {
        Self {
            address: 0,
            configuration: 0,
            stage: Stage::Idle,
            setup: [0; 8],
            data: Vec::new(),
            sent: 0,
            pending_address: None,
        }
    }
}

impl Core {
    /// The state after a bus reset: default address, not configured.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u8(self.address);
        w.u8(self.configuration);
        w.u8(self.stage as u8);
        w.raw(&self.setup);
        w.bytes(&self.data);
        w.u32(self.sent as u32);
        w.u8(self.pending_address.unwrap_or(0xFF));
    }

    pub fn load(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let address = r.u8()?;
        let configuration = r.u8()?;
        let stage = *Stage::ALL
            .get(usize::from(r.u8()?))
            .ok_or(StateError::Invalid("USB control stage"))?;
        let mut setup = [0; 8];
        r.raw_into(&mut setup)?;
        let data = r.bytes()?;
        if data.len() > MAX_CONTROL_DATA {
            return Err(StateError::Invalid("USB control data length"));
        }
        let sent = r.u32()? as usize;
        if sent > data.len() || address > 0x7F {
            return Err(StateError::Invalid("USB control position"));
        }
        let pending_address = match r.u8()? {
            0xFF => None,
            a @ 0..=0x7F => Some(a),
            _ => return Err(StateError::Invalid("USB pending address")),
        };
        Ok(Self {
            address,
            configuration,
            stage,
            setup,
            data: data.to_vec(),
            sent,
            pending_address,
        })
    }
}

/// A USB function as the controller sees it. Endpoint 0 is answered by the
/// shared code in this module ([`setup`], [`token_in`], [`token_out`]);
/// a function supplies its descriptors, its class requests and its other
/// endpoints.
///
/// Extension point: a HID keyboard or mouse is one more implementation of
/// this trait (an interrupt IN endpoint that answers NAK while nothing
/// changed) plus a variant of the hub's port device (`hub::Child`).
pub(crate) trait Function {
    fn core(&self) -> &Core;
    fn core_mut(&mut self) -> &mut Core;
    /// The 18-byte device descriptor.
    fn device_descriptor(&self) -> [u8; 18];
    /// The configuration descriptor with everything below it
    /// (`wTotalLength` bytes).
    fn configuration_descriptor(&self) -> Vec<u8>;
    /// String descriptor `index` (1…) as text; `None`: none.
    fn string(&self, _index: u8) -> Option<String> {
        None
    }
    /// A class request with data to the host: the answer (before it is cut
    /// to `wLength`), `None`: STALL.
    fn class_in(&mut self, _s: &Setup, _env: &mut Env<'_>) -> Option<Vec<u8>> {
        None
    }
    /// A class request without data or with data from the host: `false`:
    /// STALL.
    fn class_out(&mut self, _s: &Setup, _data: &[u8], _env: &mut Env<'_>) -> bool {
        false
    }
    /// An IN token on endpoint `ep` (1–15); at most `max` bytes go into
    /// `buf` (which is empty).
    fn endpoint_in(&mut self, ep: u8, max: usize, buf: &mut Vec<u8>, env: &mut Env<'_>)
        -> Response;
    /// An OUT packet on endpoint `ep` (1–15).
    fn endpoint_out(&mut self, ep: u8, data: &[u8], env: &mut Env<'_>) -> Response;
    /// CLEAR_FEATURE(ENDPOINT_HALT) on endpoint address `ep`.
    fn clear_halt(&mut self, _ep: u8) {}
    /// The configuration was set (0: unconfigured).
    fn configured(&mut self, _value: u8) {}
}

/// String descriptor of `text` (UTF-16LE, at most 126 characters).
fn string_descriptor(text: &str) -> Vec<u8> {
    let mut v = vec![0, descr::STRING];
    for c in text.encode_utf16().take(126) {
        v.extend_from_slice(&c.to_le_bytes());
    }
    v[0] = v.len() as u8;
    v
}

/// A standard or class request with data to the host.
fn request_in(f: &mut dyn Function, s: &Setup, env: &mut Env<'_>) -> Option<Vec<u8>> {
    if s.kind() == 1 {
        return f.class_in(s, env);
    }
    if s.kind() != 0 {
        return None;
    }
    match s.request {
        req::GET_DESCRIPTOR => {
            let [kind, index] = s.value.to_be_bytes();
            match kind {
                descr::DEVICE if index == 0 => Some(f.device_descriptor().to_vec()),
                descr::CONFIGURATION if index == 0 => Some(f.configuration_descriptor()),
                descr::STRING if index == 0 => Some(vec![4, descr::STRING, 0x09, 0x04]),
                descr::STRING => f.string(index).map(|t| string_descriptor(&t)),
                _ => None,
            }
        }
        req::GET_CONFIGURATION => Some(vec![f.core().configuration]),
        req::GET_STATUS => Some(vec![0, 0]),
        req::GET_INTERFACE if f.core().configuration != 0 => Some(vec![0]),
        _ => None,
    }
}

/// A standard or class request without data to the host.
fn request_out(f: &mut dyn Function, s: &Setup, data: &[u8], env: &mut Env<'_>) -> bool {
    if s.kind() == 1 {
        return f.class_out(s, data, env);
    }
    if s.kind() != 0 {
        return false;
    }
    match s.request {
        req::SET_ADDRESS if s.value <= 0x7F => {
            f.core_mut().pending_address = Some(s.value as u8);
            true
        }
        req::SET_CONFIGURATION => {
            let wanted = s.value as u8;
            let own = f.configuration_descriptor().get(5).copied().unwrap_or(1);
            if s.value > 0xFF || (wanted != 0 && wanted != own) {
                return false;
            }
            f.core_mut().configuration = wanted;
            f.configured(wanted);
            true
        }
        // Only alternate setting 0 exists.
        req::SET_INTERFACE => s.value == 0 && f.core().configuration != 0,
        req::CLEAR_FEATURE => {
            // Recipient endpoint, feature ENDPOINT_HALT.
            if s.request_type & 0x1F == 2 && s.value == 0 {
                f.clear_halt(s.index as u8);
            }
            true
        }
        req::SET_FEATURE => true,
        _ => false,
    }
}

/// A SETUP packet to the function's endpoint 0. A function always takes
/// a SETUP packet (USB 2.0, 8.5.3); a malformed one ends any transfer and
/// stalls the next stage.
pub(crate) fn setup(f: &mut dyn Function, packet: &[u8], env: &mut Env<'_>) -> Response {
    let Ok(raw) = <[u8; 8]>::try_from(packet) else {
        let c = f.core_mut();
        c.stage = Stage::Stalled;
        c.data.clear();
        return Response::Ack;
    };
    let s = Setup::parse(&raw);
    {
        let c = f.core_mut();
        c.setup = raw;
        c.data.clear();
        c.sent = 0;
        c.pending_address = None;
    }
    let stage = if s.device_to_host() {
        match request_in(f, &s, env) {
            Some(mut v) => {
                v.truncate(usize::from(s.length).min(MAX_CONTROL_DATA));
                f.core_mut().data = v;
                Stage::DataIn
            }
            None => Stage::Stalled,
        }
    } else if s.length == 0 {
        if request_out(f, &s, &[], env) {
            Stage::StatusIn
        } else {
            Stage::Stalled
        }
    } else {
        Stage::DataOut
    };
    f.core_mut().stage = stage;
    Response::Ack
}

/// An IN token to the function.
pub(crate) fn token_in(
    f: &mut dyn Function,
    ep: u8,
    max: usize,
    buf: &mut Vec<u8>,
    env: &mut Env<'_>,
) -> Response {
    if ep != 0 {
        if f.core().configuration == 0 {
            return Response::Stall;
        }
        return f.endpoint_in(ep, max, buf, env);
    }
    match f.core().stage {
        Stage::DataIn => {
            let c = f.core_mut();
            let n = (c.data.len() - c.sent).min(max);
            buf.extend_from_slice(&c.data[c.sent..c.sent + n]);
            c.sent += n;
            Response::Data(n)
        }
        Stage::StatusIn => {
            let c = f.core_mut();
            if let Some(a) = c.pending_address.take() {
                c.address = a;
            }
            c.stage = Stage::Idle;
            Response::Data(0)
        }
        Stage::DataOut => {
            let raw = f.core().setup;
            let data = std::mem::take(&mut f.core_mut().data);
            let ok = request_out(f, &Setup::parse(&raw), &data, env);
            f.core_mut().stage = if ok { Stage::Idle } else { Stage::Stalled };
            if ok {
                Response::Data(0)
            } else {
                Response::Stall
            }
        }
        Stage::Idle | Stage::Stalled => Response::Stall,
    }
}

/// An OUT packet to the function.
pub(crate) fn token_out(f: &mut dyn Function, ep: u8, data: &[u8], env: &mut Env<'_>) -> Response {
    if ep != 0 {
        if f.core().configuration == 0 {
            return Response::Stall;
        }
        return f.endpoint_out(ep, data, env);
    }
    let c = f.core_mut();
    match c.stage {
        Stage::DataOut => {
            let want = usize::from(Setup::parse(&c.setup).length).min(MAX_CONTROL_DATA);
            let room = want.saturating_sub(c.data.len());
            c.data.extend_from_slice(&data[..data.len().min(room)]);
            Response::Ack
        }
        // The status stage of a transfer to the host.
        Stage::DataIn => {
            c.stage = Stage::Idle;
            c.data.clear();
            c.sent = 0;
            Response::Ack
        }
        Stage::Idle => Response::Ack,
        Stage::StatusIn | Stage::Stalled => Response::Stall,
    }
}
