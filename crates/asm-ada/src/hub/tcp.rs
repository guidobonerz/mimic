//! TCP terminated in the gateway (SPEC.md 11.3, RFC 793): a machine's SYN
//! to an address outside the net makes the hub connect there through
//! [`Outside`]; once that works the hub answers the SYN and from then on
//! is the machine's TCP peer, carrying the bytes between the segments
//! and the host connection.
//!
//! The hub plays the passive side towards the machine. States (RFC 793
//! names, seen from the hub):
//!
//! - `Connecting`: LISTEN having taken the SYN, the outside connect is
//!   running (at most 5 s; failure or timeout: RST to the machine);
//! - `SynReceived`: SYN-ACK sent, waiting for the machine's ACK;
//! - `Established`;
//! - the machine closes first: `CloseWait` (its FIN acknowledged, the
//!   outside connection shut down for writing), then `LastAck` after the
//!   hub's own FIN, closed when that is acknowledged;
//! - the outside closes first: `FinWait1` (the hub's FIN sent),
//!   `FinWait2` (acknowledged), `Closing` (both FINs crossed), then
//!   `TimeWait` for 2 s to acknowledge a repeated FIN.
//!
//! Simplifications: segments out of order are dropped (and the expected
//! sequence number acknowledged again); the send window is taken from
//! every acceptable ACK (no SND.WL1/WL2 check); retransmission resends
//! the oldest unacknowledged segment after a fixed 1 s, at most 10
//! times; a zero window is probed with one byte a second; no urgent
//! data, no window scaling, no options but MSS.

use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddrV4;

use super::outside::{Connect, Outside, TcpRead};
use super::packet::{self, seq_le, seq_lt, Mac, ACK, FIN, PSH, RST, SYN};

/// The segment size the hub offers and sends at most.
pub const MSS: u16 = 1460;
/// Retransmission timeout.
pub const RTO_MS: u64 = 1000;
/// How long an outside connect may take.
pub const CONNECT_MS: u64 = 5000;
const MAX_RETRIES: u32 = 10;
/// Bytes from the machine waiting for the outside connection.
const RCV_BUF: usize = 65535;
/// Bytes from outside waiting to be sent or acknowledged.
const SND_BUF: usize = 65536;
const TIME_WAIT_MS: u64 = 2000;
/// A connection with no traffic for this long is reset.
const IDLE_MS: u64 = 30 * 60 * 1000;
const MAX_FLOWS: usize = 256;

/// A connection's state, RFC 793 names (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Connecting,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
}

/// A TCP segment for a machine: its MAC, the outside address it comes
/// from, the machine's address and the TCP header with data.
pub struct Seg {
    pub mac: Mac,
    pub from: SocketAddrV4,
    pub to: SocketAddrV4,
    pub tcp: Vec<u8>,
}

struct Flow {
    id: u32,
    mac: Mac,
    machine: SocketAddrV4,
    remote: SocketAddrV4,
    state: State,
    // Hub to machine.
    iss: u32,
    /// The oldest unacknowledged sequence number: that of `out_buf[0]`.
    snd_una: u32,
    snd_wnd: usize,
    mss: usize,
    /// Bytes from outside: `..sent` sent and unacknowledged, the rest
    /// waiting.
    out_buf: VecDeque<u8>,
    sent: usize,
    outside_eof: bool,
    fin_sent: bool,
    fin_acked: bool,
    rto_at: Option<u64>,
    retries: u32,
    // Machine to hub.
    irs: u32,
    rcv_nxt: u32,
    in_buf: Vec<u8>,
    fin_received: bool,
    shut_outside: bool,
    last_adv: usize,
    since: u64,
    last_seen: u64,
}

impl Flow {
    fn name(&self) -> String {
        format!("tcp {} > {}", self.machine, self.remote)
    }

    fn window(&self) -> u16 {
        (RCV_BUF - self.in_buf.len()).min(0xffff) as u16
    }

    fn snd_nxt(&self) -> u32 {
        self.snd_una
            .wrapping_add(self.sent as u32)
            .wrapping_add(u32::from(self.fin_sent && !self.fin_acked))
    }

    fn emit(&mut self, seq: u32, flags: u8, data: &[u8], res: &mut Vec<Seg>) {
        let w = self.window();
        self.last_adv = usize::from(w);
        let mss = (flags & SYN != 0).then_some(MSS);
        let ack = if flags & ACK != 0 { self.rcv_nxt } else { 0 };
        res.push(Seg {
            mac: self.mac,
            from: self.remote,
            to: self.machine,
            tcp: packet::tcp(self.remote, self.machine, seq, ack, flags, w, mss, data),
        });
    }

    fn ack_now(&mut self, res: &mut Vec<Seg>) {
        let seq = self.snd_nxt();
        self.emit(seq, ACK, &[], res);
    }

    fn reset(&mut self, res: &mut Vec<Seg>) {
        let seq = if self.state == State::Connecting {
            0
        } else {
            self.snd_nxt()
        };
        self.emit(seq, RST | ACK, &[], res);
    }

    fn data(&self, from: usize, n: usize) -> Vec<u8> {
        self.out_buf.range(from..from + n).copied().collect()
    }

    /// RFC 793 "SEGMENT ARRIVES" for a known connection; false when the
    /// connection is over.
    fn arrive(
        &mut self,
        s: &packet::Tcp<'_>,
        now: u64,
        log: &mut Vec<String>,
        res: &mut Vec<Seg>,
    ) -> bool {
        self.last_seen = now;
        if self.state == State::Connecting {
            // Repeated SYNs wait for the outside connect.
            return !s.has(RST);
        }
        let wnd = u32::from(self.window()).max(1);
        let in_window =
            seq_le(self.rcv_nxt, s.seq) && seq_lt(s.seq, self.rcv_nxt.wrapping_add(wnd));
        if s.has(RST) {
            if in_window || s.seq == self.rcv_nxt {
                log.push(format!("{}: reset by the machine", self.name()));
                return false;
            }
            return true;
        }
        if s.has(SYN) {
            if self.state == State::SynReceived && !s.has(ACK) && s.seq == self.irs {
                // Our SYN-ACK was lost.
                let iss = self.iss;
                self.emit(iss, SYN | ACK, &[], res);
            } else {
                self.ack_now(res);
            }
            return true;
        }
        if !s.has(ACK) {
            return true;
        }
        if self.state == State::SynReceived {
            if s.ack != self.iss.wrapping_add(1) {
                res.push(Seg {
                    mac: self.mac,
                    from: self.remote,
                    to: self.machine,
                    tcp: packet::tcp(self.remote, self.machine, s.ack, 0, RST, 0, None, &[]),
                });
                return true;
            }
            self.state = State::Established;
            self.snd_una = s.ack;
            self.snd_wnd = usize::from(s.window);
            self.rto_at = None;
            self.retries = 0;
            log.push(format!("{}: established", self.name()));
        } else {
            let nxt = self.snd_nxt();
            if seq_lt(nxt, s.ack) {
                // Acknowledges something never sent.
                self.ack_now(res);
                return true;
            }
            if seq_le(self.snd_una, s.ack) {
                self.snd_wnd = usize::from(s.window);
                let acked = s.ack.wrapping_sub(self.snd_una) as usize;
                if acked > 0 {
                    let d = acked.min(self.sent);
                    self.out_buf.drain(..d);
                    self.sent -= d;
                    self.snd_una = self.snd_una.wrapping_add(d as u32);
                    if acked > d && self.fin_sent {
                        self.fin_acked = true;
                        self.snd_una = self.snd_una.wrapping_add(1);
                    }
                    self.retries = 0;
                    let in_flight = self.sent > 0 || (self.fin_sent && !self.fin_acked);
                    self.rto_at = in_flight.then_some(now + RTO_MS);
                }
            }
        }
        if self.fin_acked {
            match self.state {
                State::FinWait1 => self.state = State::FinWait2,
                State::Closing => self.enter_time_wait(now),
                State::LastAck => {
                    log.push(format!("{}: closed", self.name()));
                    return false;
                }
                _ => {}
            }
        }
        let mut consumed = s.seq_len() > 0;
        if matches!(
            self.state,
            State::Established | State::FinWait1 | State::FinWait2
        ) {
            let mut data = s.data;
            let mut seq = s.seq;
            if seq_lt(seq, self.rcv_nxt) {
                let skip = self.rcv_nxt.wrapping_sub(seq) as usize;
                data = data.get(skip..).unwrap_or(&[]);
                seq = self.rcv_nxt;
            }
            if seq == self.rcv_nxt && !data.is_empty() {
                let take = data.len().min(RCV_BUF - self.in_buf.len());
                self.in_buf.extend_from_slice(&data[..take]);
                self.rcv_nxt = self.rcv_nxt.wrapping_add(take as u32);
            }
            let fin_seq = s.seq.wrapping_add(s.data.len() as u32);
            if s.has(FIN) && fin_seq == self.rcv_nxt {
                self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                self.fin_received = true;
                match self.state {
                    State::Established => self.state = State::CloseWait,
                    State::FinWait1 => self.state = State::Closing,
                    _ => self.enter_time_wait(now),
                }
                consumed = true;
            }
        }
        if consumed {
            self.ack_now(res);
        }
        true
    }

    fn enter_time_wait(&mut self, now: u64) {
        self.state = State::TimeWait;
        self.rto_at = Some(now + TIME_WAIT_MS);
    }

    /// Moves bytes between the outside connection and the buffers and
    /// sends what the window allows; false when the connection broke.
    fn service(
        &mut self,
        now: u64,
        out: &mut dyn Outside,
        log: &mut Vec<String>,
        res: &mut Vec<Seg>,
    ) -> bool {
        if matches!(self.state, State::Connecting | State::SynReceived) {
            return true;
        }
        if !self.in_buf.is_empty() {
            match out.tcp_send(self.id, &self.in_buf) {
                None => {
                    log.push(format!("{}: outside connection broke", self.name()));
                    self.reset(res);
                    return false;
                }
                Some(n) => {
                    self.in_buf.drain(..n.min(self.in_buf.len()));
                    // Reopen a window the machine saw (nearly) closed.
                    if n > 0 && self.last_adv < self.mss {
                        self.ack_now(res);
                    }
                }
            }
        }
        if self.fin_received && self.in_buf.is_empty() && !self.shut_outside {
            out.tcp_shutdown(self.id);
            self.shut_outside = true;
        }
        if matches!(self.state, State::Established | State::CloseWait) && !self.outside_eof {
            let mut buf = [0u8; 4096];
            while self.out_buf.len() < SND_BUF {
                let room = (SND_BUF - self.out_buf.len()).min(buf.len());
                match out.tcp_recv(self.id, &mut buf[..room]) {
                    TcpRead::Data(n) => {
                        self.out_buf.extend(&buf[..n.min(room)]);
                        self.last_seen = now;
                    }
                    TcpRead::Nothing => break,
                    TcpRead::Closed => {
                        self.outside_eof = true;
                        break;
                    }
                    TcpRead::Failed => {
                        log.push(format!("{}: outside connection broke", self.name()));
                        self.reset(res);
                        return false;
                    }
                }
            }
        }
        self.transmit(now, res);
        true
    }

    /// Sends waiting bytes within the machine's window, then the FIN.
    fn transmit(&mut self, now: u64, res: &mut Vec<Seg>) {
        if !matches!(self.state, State::Established | State::CloseWait) {
            return;
        }
        while self.sent < self.out_buf.len() && self.sent < self.snd_wnd {
            let n = self
                .mss
                .min(self.snd_wnd - self.sent)
                .min(self.out_buf.len() - self.sent);
            let d = self.data(self.sent, n);
            let seq = self.snd_una.wrapping_add(self.sent as u32);
            self.emit(seq, ACK | PSH, &d, res);
            self.sent += n;
            self.rto_at.get_or_insert(now + RTO_MS);
        }
        if self.sent == self.out_buf.len() && self.outside_eof && !self.fin_sent {
            let seq = self.snd_nxt();
            self.emit(seq, FIN | ACK, &[], res);
            self.fin_sent = true;
            self.state = if self.state == State::Established {
                State::FinWait1
            } else {
                State::LastAck
            };
            self.rto_at.get_or_insert(now + RTO_MS);
        }
        if self.sent == 0 && !self.out_buf.is_empty() && self.snd_wnd == 0 {
            // Persist: probe the closed window.
            self.rto_at.get_or_insert(now + RTO_MS);
        }
    }

    /// The retransmission (and TIME-WAIT) timer; false when the
    /// connection is over.
    fn timer(&mut self, now: u64, log: &mut Vec<String>, res: &mut Vec<Seg>) -> bool {
        let Some(t) = self.rto_at else {
            return true;
        };
        if now < t {
            return true;
        }
        if self.state == State::TimeWait {
            log.push(format!("{}: closed", self.name()));
            return false;
        }
        let probe = self.snd_wnd == 0 && self.sent <= 1 && !self.out_buf.is_empty();
        if !probe {
            self.retries += 1;
            if self.retries > MAX_RETRIES {
                log.push(format!("{}: the machine stopped answering", self.name()));
                self.reset(res);
                return false;
            }
        }
        if self.state == State::SynReceived {
            let iss = self.iss;
            self.emit(iss, SYN | ACK, &[], res);
        } else if self.sent > 0 || probe {
            let n = self.mss.min(self.sent.max(1)).min(self.out_buf.len());
            let d = self.data(0, n);
            let seq = self.snd_una;
            self.emit(seq, ACK | PSH, &d, res);
            self.sent = self.sent.max(n);
        } else if self.fin_sent && !self.fin_acked {
            let seq = self.snd_una;
            self.emit(seq, FIN | ACK, &[], res);
        } else {
            self.rto_at = None;
            return true;
        }
        self.rto_at = Some(now + RTO_MS);
        true
    }

    /// Everything that happens without a segment arriving.
    fn poll(
        &mut self,
        now: u64,
        out: &mut dyn Outside,
        log: &mut Vec<String>,
        res: &mut Vec<Seg>,
    ) -> bool {
        if self.state == State::Connecting {
            let state = out.tcp_connect_state(self.id);
            let late = now.saturating_sub(self.since) >= CONNECT_MS;
            if state == Connect::Open {
                self.state = State::SynReceived;
                self.last_seen = now;
                let iss = self.iss;
                self.emit(iss, SYN | ACK, &[], res);
                self.rto_at = Some(now + RTO_MS);
                return true;
            }
            if state == Connect::Failed || late {
                log.push(format!("{}: refused", self.name()));
                self.reset(res);
                return false;
            }
            return true;
        }
        if self.state != State::TimeWait && now.saturating_sub(self.last_seen) > IDLE_MS {
            log.push(format!("{}: idle, reset", self.name()));
            self.reset(res);
            return false;
        }
        self.service(now, out, log, res) && self.timer(now, log, res)
    }
}

/// The connections the gateway terminates, by (machine, outside)
/// address.
#[derive(Default)]
pub struct Tcp {
    flows: BTreeMap<(SocketAddrV4, SocketAddrV4), Flow>,
    next_id: u32,
}

/// The answer to a segment no connection takes (RFC 793 3.4, "Reset
/// Generation").
fn reset_for(mac: Mac, machine: SocketAddrV4, remote: SocketAddrV4, s: &packet::Tcp<'_>) -> Seg {
    let tcp = if s.has(ACK) {
        packet::tcp(remote, machine, s.ack, 0, RST, 0, None, &[])
    } else {
        let ack = s.seq.wrapping_add(s.seq_len());
        packet::tcp(remote, machine, 0, ack, RST | ACK, 0, None, &[])
    };
    Seg {
        mac,
        from: remote,
        to: machine,
        tcp,
    }
}

impl Tcp {
    /// The number of connections (any state).
    pub fn len(&self) -> usize {
        self.flows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.flows.is_empty()
    }

    /// The state of the connection between `machine` and `remote`.
    pub fn state(&self, machine: SocketAddrV4, remote: SocketAddrV4) -> Option<State> {
        self.flows.get(&(machine, remote)).map(|f| f.state)
    }

    /// A segment from `machine` (MAC `mac`) to the outside `remote`.
    #[allow(clippy::too_many_arguments)]
    pub fn segment(
        &mut self,
        mac: Mac,
        machine: SocketAddrV4,
        remote: SocketAddrV4,
        s: &packet::Tcp<'_>,
        now: u64,
        out: &mut dyn Outside,
        log: &mut Vec<String>,
        res: &mut Vec<Seg>,
    ) {
        let key = (machine, remote);
        let new_syn = s.has(SYN) && !s.has(ACK);
        if let Some(f) = self.flows.get(&key) {
            if f.state == State::TimeWait && new_syn {
                out.tcp_close(f.id);
                self.flows.remove(&key);
            }
        }
        if let Some(f) = self.flows.get_mut(&key) {
            f.mac = mac;
            let keep = f.arrive(s, now, log, res) && f.service(now, out, log, res);
            if !keep {
                out.tcp_close(f.id);
                self.flows.remove(&key);
            }
            return;
        }
        if s.has(RST) {
            return;
        }
        if !new_syn || self.flows.len() >= MAX_FLOWS {
            res.push(reset_for(mac, machine, remote, s));
            return;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let iss = (now as u32)
            .wrapping_mul(250)
            .wrapping_add(id.wrapping_mul(0x9e37_79b9));
        let mss = usize::from(s.mss.unwrap_or(536).clamp(64, MSS));
        let rcv_nxt = s.seq.wrapping_add(1);
        let f = Flow {
            id,
            mac,
            machine,
            remote,
            state: State::Connecting,
            iss,
            snd_una: iss,
            snd_wnd: usize::from(s.window),
            mss,
            out_buf: VecDeque::new(),
            sent: 0,
            outside_eof: false,
            fin_sent: false,
            fin_acked: false,
            rto_at: None,
            retries: 0,
            irs: s.seq,
            rcv_nxt,
            in_buf: Vec::new(),
            fin_received: false,
            shut_outside: false,
            last_adv: RCV_BUF,
            since: now,
            last_seen: now,
        };
        log.push(format!("{}: connecting", f.name()));
        out.tcp_connect(id, remote);
        self.flows.insert(key, f);
    }

    /// Outside connects and data, retransmissions and timeouts.
    pub fn poll(
        &mut self,
        now: u64,
        out: &mut dyn Outside,
        log: &mut Vec<String>,
        res: &mut Vec<Seg>,
    ) {
        let mut gone = Vec::new();
        for (key, f) in &mut self.flows {
            if !f.poll(now, out, log, res) {
                out.tcp_close(f.id);
                gone.push(*key);
            }
        }
        for k in gone {
            self.flows.remove(&k);
        }
    }
}
