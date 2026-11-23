//! The router's TCP endpoint (RFC 793 essentials, RFC 1122 section 4.2):
//! one [`Conn`] ends a guest connection locally and relays its byte stream
//! to a host socket.
//!
//! - Sequence space: SYN and FIN take one number each. `tx` holds the
//!   bytes from the host the guest has not acknowledged; its first byte
//!   has the number `snd_una`.
//! - Sending: at most the guest's window is in flight, in segments of at
//!   most the guest's MSS. One timer per connection: when it runs out,
//!   everything unacknowledged is sent again (go-back-N) and the time-out
//!   doubles; after [`DATA_RETRIES`] rounds the connection is reset. With
//!   a closed window the same timer sends one byte beyond the window
//!   (zero-window probe) without ever giving up.
//! - Receiving: only the segment that continues the stream is taken (as
//!   far as the buffer has room); everything else is answered with an ACK
//!   and dropped — the guest sends it again. Every data segment is
//!   acknowledged at once. The window offered is the room in `rx`; when
//!   the host has taken data and the window grew by a segment from below
//!   one, or by half the buffer, a window update goes out.
//! - Closing: when the host closes, a FIN follows the last byte; the guest
//!   may go on sending (half-close). When the guest sends FIN, the host
//!   socket is closed as soon as everything was passed on —
//!   [`mimic_core::host::NetBackend`] has no way to close one direction —
//!   and the FIN to the guest follows. A finished connection lingers for
//!   [`LINGER_US`] to acknowledge a repeated FIN.

use std::collections::VecDeque;

use mimic_core::host::{ConnState, NetHandle, Recv};
use mimic_core::{StateError, StateReader, StateWriter};

use super::wire::{self, ACK, FIN, PSH, RST, SYN};
use super::{Link, Net};

/// A connection as the guest sees it: its own port, the far address, the
/// far port.
pub(super) type Key = (u16, [u8; 4], u16);

/// Bytes from the guest waiting for the host socket (the window offered).
pub(super) const RX_CAP: usize = 32 * 1024;
/// Bytes from the host waiting for the guest's acknowledgement.
pub(super) const TX_CAP: usize = 32 * 1024;
/// The MSS the router announces: 1500 - 20 (IPv4) - 20 (TCP).
const OUR_MSS: u16 = 1460;
/// The guest's MSS without an option (RFC 1122, 4.2.2.6).
const DEFAULT_MSS: u16 = 536;
const MIN_MSS: u16 = 48;
/// First retransmission time-out (RFC 6298's initial value; the link has
/// no delay to measure), doubled per round up to [`RTO_MAX_US`].
const RTO_INIT_US: u64 = 1_000_000;
const RTO_MAX_US: u64 = 8_000_000;
/// Retransmission rounds for data and FIN before the connection is reset.
const DATA_RETRIES: u8 = 8;
/// Repeats of a SYN or SYN-ACK.
const SYN_RETRIES: u8 = 5;
/// How long the host may take to connect before the guest gets a RST.
const CONNECT_TIMEOUT_US: u64 = 30_000_000;
/// How long a finished connection still acknowledges a repeated FIN.
const LINGER_US: u64 = 2_000_000;
/// A connection the host closed whose guest neither sends nor closes.
const HALF_CLOSED_IDLE_US: u64 = 120_000_000;
/// Most bytes handed to the host socket in one `send`.
const SEND_CHUNK: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Guest SYN seen, the host socket is connecting.
    Connecting,
    /// Host connected, SYN-ACK sent.
    SynAckSent,
    /// A forwarded connection: SYN sent to the guest.
    SynSent,
    Established,
    /// Both directions closed.
    Linger,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    Keep,
    /// The connection is over: close its host socket and forget it.
    Remove,
}

#[derive(Clone, Debug)]
pub(super) struct Conn {
    phase: Phase,
    host: Option<NetHandle>,
    /// Loaded from a savestate with a host socket that no longer exists.
    stale: bool,
    iss: u32,
    snd_una: u32,
    snd_nxt: u32,
    /// Highest number sent so far (`snd_nxt` falls back on a time-out).
    snd_max: u32,
    snd_wnd: u16,
    mss: u16,
    tx: VecDeque<u8>,
    /// The host closed (or was closed): a FIN follows `tx`.
    host_eof: bool,
    fin_acked: bool,
    rcv_nxt: u32,
    rx: VecDeque<u8>,
    guest_fin: bool,
    /// The window last offered.
    adv_wnd: u16,
    rto_us: u64,
    rto_at: Option<u64>,
    retries: u8,
    /// Connecting: give up; Linger: forget.
    deadline: u64,
    /// Last segment from the guest.
    last_us: u64,
}

fn seq_lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

fn clamp_mss(mss: Option<u16>) -> u16 {
    mss.unwrap_or(DEFAULT_MSS).clamp(MIN_MSS, OUR_MSS)
}

impl Conn {
    fn new(phase: Phase, host: NetHandle, iss: u32, now: u64) -> Self {
        let first = iss.wrapping_add(1);
        Self {
            phase,
            host: Some(host),
            stale: false,
            iss,
            snd_una: first,
            snd_nxt: first,
            snd_max: first,
            snd_wnd: 0,
            mss: DEFAULT_MSS,
            tx: VecDeque::new(),
            host_eof: false,
            fin_acked: false,
            rcv_nxt: 0,
            rx: VecDeque::new(),
            guest_fin: false,
            adv_wnd: 0,
            rto_us: RTO_INIT_US,
            rto_at: None,
            retries: 0,
            deadline: now.saturating_add(CONNECT_TIMEOUT_US),
            last_us: now,
        }
    }

    /// The guest's SYN `seg` opened a connection; `host` is connecting.
    pub(super) fn outbound(seg: &wire::Tcp<'_>, host: NetHandle, iss: u32, now: u64) -> Self {
        let mut c = Self::new(Phase::Connecting, host, iss, now);
        c.rcv_nxt = seg.seq.wrapping_add(1);
        c.mss = clamp_mss(seg.mss);
        c.snd_wnd = seg.window;
        c
    }

    /// A connection accepted on a forwarded port: send the SYN.
    pub(super) fn inbound(host: NetHandle, iss: u32, link: &mut Link, key: Key, now: u64) -> Self {
        let mut c = Self::new(Phase::SynSent, host, iss, now);
        c.send_syn(link, key);
        c.rto_at = Some(now.saturating_add(c.rto_us));
        c
    }

    pub(super) fn is_lingering(&self) -> bool {
        self.phase == Phase::Linger
    }

    pub(super) fn deadline(&self) -> u64 {
        self.deadline
    }

    pub(super) fn close_host(&mut self, net: &mut Net<'_>) {
        if let Some(h) = self.host.take() {
            net.close(h);
        }
    }

    /// Close the host socket for good (a loaded state, another host); the
    /// connection is reset at the next poll.
    pub(super) fn orphan(&mut self, net: &mut Net<'_>) {
        if let Some(h) = self.host.take() {
            net.close(h);
            self.stale = true;
        }
    }

    fn window(&self) -> u16 {
        RX_CAP.saturating_sub(self.rx.len()).min(0xffff) as u16
    }

    fn send(
        &mut self,
        link: &mut Link,
        key: Key,
        flags: u8,
        seq: u32,
        mss: Option<u16>,
        data: &[u8],
    ) {
        self.adv_wnd = self.window();
        link.tcp(
            key.1,
            &wire::Tcp {
                sport: key.2,
                dport: key.0,
                seq,
                ack: if flags & ACK != 0 { self.rcv_nxt } else { 0 },
                flags,
                window: self.adv_wnd,
                mss,
                data,
            },
        );
    }

    fn send_syn(&mut self, link: &mut Link, key: Key) {
        let flags = if self.phase == Phase::SynSent {
            SYN
        } else {
            SYN | ACK
        };
        self.send(link, key, flags, self.iss, Some(OUR_MSS), &[]);
    }

    fn send_ack(&mut self, link: &mut Link, key: Key) {
        self.send(link, key, ACK, self.snd_max, None, &[]);
    }

    /// Reset the guest's side.
    fn send_rst(&mut self, link: &mut Link, key: Key) {
        match self.phase {
            // No SYN-ACK went out: answer the SYN itself.
            Phase::Connecting => self.send(link, key, RST | ACK, 0, None, &[]),
            Phase::SynSent => self.send(link, key, RST, self.snd_max, None, &[]),
            _ => self.send(link, key, RST | ACK, self.snd_max, None, &[]),
        }
    }

    fn abort(&mut self, link: &mut Link, key: Key, net: &mut Net<'_>) -> Step {
        self.send_rst(link, key);
        self.close_host(net);
        Step::Remove
    }

    fn backoff(&mut self) {
        self.rto_us = self.rto_us.saturating_mul(2).clamp(RTO_INIT_US, RTO_MAX_US);
    }

    /// Send what the guest's window allows; `probe`: one byte even into a
    /// closed window. Arms or stops the timer.
    pub(super) fn output(&mut self, link: &mut Link, key: Key, now: u64, probe: bool) {
        if self.phase != Phase::Established {
            return;
        }
        let mss = usize::from(self.mss.max(1));
        let wnd = usize::from(self.snd_wnd).max(usize::from(probe));
        let mut seg = Vec::with_capacity(mss);
        loop {
            let off = self.snd_nxt.wrapping_sub(self.snd_una) as usize;
            if off < self.tx.len() {
                if off >= wnd || link.congested() {
                    break;
                }
                let n = mss.min(self.tx.len() - off).min(wnd - off);
                seg.clear();
                seg.extend(self.tx.range(off..off + n));
                let flags = if off + n == self.tx.len() {
                    ACK | PSH
                } else {
                    ACK
                };
                self.send(link, key, flags, self.snd_nxt, None, &seg);
                self.snd_nxt = self.snd_nxt.wrapping_add(n as u32);
            } else if off == self.tx.len() && self.host_eof && !self.fin_acked {
                self.send(link, key, FIN | ACK, self.snd_nxt, None, &[]);
                self.snd_nxt = self.snd_nxt.wrapping_add(1);
            } else {
                break;
            }
        }
        if seq_lt(self.snd_max, self.snd_nxt) {
            self.snd_max = self.snd_nxt;
        }
        if !self.tx.is_empty() || (self.host_eof && !self.fin_acked) {
            if self.rto_at.is_none() {
                self.rto_at = Some(now.saturating_add(self.rto_us));
            }
        } else {
            self.rto_at = None;
        }
    }

    /// The retransmission timer; `true`: the guest does not answer, give up.
    fn timer(&mut self, link: &mut Link, key: Key, now: u64) -> bool {
        match self.rto_at {
            Some(at) if now >= at => {}
            _ => return false,
        }
        self.rto_at = None;
        match self.phase {
            Phase::SynAckSent | Phase::SynSent => {
                self.retries = self.retries.saturating_add(1);
                if self.retries > SYN_RETRIES {
                    return true;
                }
                self.backoff();
                self.send_syn(link, key);
                self.rto_at = Some(now.saturating_add(self.rto_us));
            }
            Phase::Established => {
                let probe = self.snd_wnd == 0 && !self.tx.is_empty();
                if !probe {
                    self.retries = self.retries.saturating_add(1);
                    if self.retries > DATA_RETRIES {
                        return true;
                    }
                }
                self.backoff();
                self.snd_nxt = self.snd_una;
                self.output(link, key, now, probe);
            }
            Phase::Connecting | Phase::Linger => {}
        }
        false
    }

    /// A segment from the guest for this connection.
    pub(super) fn on_segment(
        &mut self,
        link: &mut Link,
        key: Key,
        seg: &wire::Tcp<'_>,
        now: u64,
    ) -> Step {
        self.last_us = now;
        match self.phase {
            Phase::Connecting => {
                // A repeated SYN waits with the first one.
                if seg.has(RST) {
                    return Step::Remove;
                }
                Step::Keep
            }
            Phase::SynAckSent => {
                if seg.has(RST) {
                    return Step::Remove;
                }
                if seg.has(SYN) {
                    self.send_syn(link, key);
                    return Step::Keep;
                }
                if !seg.has(ACK) || seg.ack != self.snd_una {
                    return Step::Keep;
                }
                self.established(seg.window);
                self.on_established(link, key, seg)
            }
            Phase::SynSent => {
                if !seg.has(ACK) || seg.ack != self.snd_una {
                    return Step::Keep;
                }
                if seg.has(RST) {
                    // The guest's port is closed.
                    return Step::Remove;
                }
                if seg.has(SYN) {
                    self.rcv_nxt = seg.seq.wrapping_add(1);
                    self.mss = clamp_mss(seg.mss);
                    self.established(seg.window);
                    self.send_ack(link, key);
                }
                Step::Keep
            }
            Phase::Established => self.on_established(link, key, seg),
            Phase::Linger => {
                if seg.has(RST) {
                    return Step::Remove;
                }
                if seg.has(FIN) || !seg.data.is_empty() {
                    self.send_ack(link, key);
                }
                Step::Keep
            }
        }
    }

    fn established(&mut self, window: u16) {
        self.phase = Phase::Established;
        self.snd_wnd = window;
        self.rto_us = RTO_INIT_US;
        self.rto_at = None;
        self.retries = 0;
    }

    fn on_established(&mut self, link: &mut Link, key: Key, seg: &wire::Tcp<'_>) -> Step {
        if seg.has(RST) {
            // Accepted anywhere in the window offered.
            if seg.seq.wrapping_sub(self.rcv_nxt) as usize <= RX_CAP {
                return Step::Remove;
            }
            return Step::Keep;
        }
        if seg.has(SYN) {
            self.send_ack(link, key);
            return Step::Keep;
        }
        if !seg.has(ACK) {
            return Step::Keep;
        }
        // What the guest acknowledges.
        let n = seg.ack.wrapping_sub(self.snd_una);
        if n <= self.snd_max.wrapping_sub(self.snd_una) {
            if n > 0 {
                let data = (n as usize).min(self.tx.len());
                if n as usize > data && self.host_eof {
                    self.fin_acked = true;
                }
                self.tx.drain(..data);
                self.snd_una = seg.ack;
                if seq_lt(self.snd_nxt, self.snd_una) {
                    self.snd_nxt = self.snd_una;
                }
                self.rto_us = RTO_INIT_US;
                self.rto_at = None;
                self.retries = 0;
            } else if self.snd_wnd == 0 && seg.window > 0 {
                // The window opens: a probe byte the guest refused is
                // sent again with the rest.
                self.snd_nxt = self.snd_una;
                self.rto_us = RTO_INIT_US;
                self.rto_at = None;
            }
            self.snd_wnd = seg.window;
        }
        // What the guest sends.
        let mut data = if self.guest_fin { &[][..] } else { seg.data };
        let mut fin = seg.has(FIN) && !self.guest_fin;
        let mut reply = false;
        let behind = self.rcv_nxt.wrapping_sub(seg.seq) as usize;
        if behind != 0 {
            // Not the next byte: a repeat (its new part counts), a
            // keep-alive or a segment after a lost one.
            reply = true;
            if behind <= data.len() {
                data = &data[behind..];
            } else {
                data = &[];
                fin = false;
            }
        }
        if !data.is_empty() {
            reply = true;
            let take = data.len().min(RX_CAP.saturating_sub(self.rx.len()));
            self.rx.extend(&data[..take]);
            self.rcv_nxt = self.rcv_nxt.wrapping_add(take as u32);
            if take < data.len() {
                fin = false;
            }
        }
        if fin {
            self.guest_fin = true;
            self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
            reply = true;
        }
        if reply {
            self.send_ack(link, key);
        }
        Step::Keep
    }

    /// Pass bytes between the buffers and the host socket; `Err`: the
    /// host connection broke.
    fn relay(
        &mut self,
        h: NetHandle,
        link: &mut Link,
        key: Key,
        net: &mut Net<'_>,
    ) -> Result<(), ()> {
        if !self.host_eof {
            let mut buf = [0u8; 2048];
            loop {
                let room = TX_CAP.saturating_sub(self.tx.len()).min(buf.len());
                if room == 0 {
                    break;
                }
                match net.recv(h, &mut buf[..room]) {
                    Ok(Recv::Data(0)) => break,
                    Ok(Recv::Data(n)) => self.tx.extend(&buf[..n.min(room)]),
                    Ok(Recv::Closed) => {
                        self.host_eof = true;
                        break;
                    }
                    Err(_) => return Err(()),
                }
            }
        }
        while !self.rx.is_empty() {
            let (front, _) = self.rx.as_slices();
            let chunk = &front[..front.len().min(SEND_CHUNK)];
            match net.send(h, chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let n = n.min(chunk.len());
                    self.rx.drain(..n);
                }
                // The far side is gone for good: nobody reads this.
                Err(_) if self.host_eof => self.rx.clear(),
                Err(_) => return Err(()),
            }
        }
        let wnd = usize::from(self.window());
        let adv = usize::from(self.adv_wnd);
        let mss = usize::from(self.mss);
        if (adv < mss && wnd >= mss) || wnd >= adv + RX_CAP / 2 {
            self.send_ack(link, key);
        }
        if self.guest_fin && self.rx.is_empty() {
            self.close_host(net);
            self.host_eof = true;
        }
        Ok(())
    }

    /// Let time pass: host socket, timers, output.
    pub(super) fn poll(&mut self, link: &mut Link, key: Key, net: &mut Net<'_>, now: u64) -> Step {
        if self.stale {
            self.send_rst(link, key);
            return Step::Remove;
        }
        match self.phase {
            Phase::Connecting => {
                let state = match self.host {
                    Some(h) => net.state(h),
                    None => return self.abort(link, key, net),
                };
                match state {
                    Ok(ConnState::Connecting) if now < self.deadline => {}
                    // Closed: it stood and the far side is done already.
                    Ok(ConnState::Connected | ConnState::Closed) => {
                        self.phase = Phase::SynAckSent;
                        self.send_syn(link, key);
                        self.rto_at = Some(now.saturating_add(self.rto_us));
                    }
                    _ => return self.abort(link, key, net),
                }
            }
            Phase::SynAckSent | Phase::SynSent => {
                if self.timer(link, key, now) {
                    return self.abort(link, key, net);
                }
            }
            Phase::Established => {
                match self.host {
                    Some(h) => {
                        if self.relay(h, link, key, net).is_err() {
                            return self.abort(link, key, net);
                        }
                    }
                    None => {
                        self.host_eof = true;
                        self.rx.clear();
                    }
                }
                if self.timer(link, key, now) {
                    return self.abort(link, key, net);
                }
                self.output(link, key, now, false);
                if self.fin_acked && self.guest_fin && self.host.is_none() {
                    self.phase = Phase::Linger;
                    self.rto_at = None;
                    self.deadline = now.saturating_add(LINGER_US);
                } else if self.host_eof
                    && self.fin_acked
                    && !self.guest_fin
                    && now.saturating_sub(self.last_us) >= HALF_CLOSED_IDLE_US
                {
                    return self.abort(link, key, net);
                }
            }
            Phase::Linger => {
                if now >= self.deadline {
                    return Step::Remove;
                }
            }
        }
        Step::Keep
    }

    pub(super) fn save(&self, w: &mut StateWriter) {
        w.u8(match self.phase {
            Phase::Connecting => 0,
            Phase::SynAckSent => 1,
            Phase::SynSent => 2,
            Phase::Established => 3,
            Phase::Linger => 4,
        });
        w.bool(self.host.is_some() || self.stale);
        w.u32(self.iss);
        w.u32(self.snd_una);
        w.u32(self.snd_nxt);
        w.u32(self.snd_max);
        w.u16(self.snd_wnd);
        w.u16(self.mss);
        let (a, b) = self.tx.as_slices();
        w.u32(self.tx.len() as u32);
        w.raw(a);
        w.raw(b);
        w.bool(self.host_eof);
        w.bool(self.fin_acked);
        w.u32(self.rcv_nxt);
        let (a, b) = self.rx.as_slices();
        w.u32(self.rx.len() as u32);
        w.raw(a);
        w.raw(b);
        w.bool(self.guest_fin);
        w.u16(self.adv_wnd);
        w.u64(self.rto_us);
        w.bool(self.rto_at.is_some());
        w.u64(self.rto_at.unwrap_or(0));
        w.u8(self.retries);
        w.u64(self.deadline);
        w.u64(self.last_us);
    }

    /// A connection from a savestate. Its host socket is gone: unless it
    /// had none any more, it is reset at the next poll.
    pub(super) fn load(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let bad = StateError::Invalid("router TCP connection");
        let phase = match r.u8()? {
            0 => Phase::Connecting,
            1 => Phase::SynAckSent,
            2 => Phase::SynSent,
            3 => Phase::Established,
            4 => Phase::Linger,
            _ => return Err(bad),
        };
        let had_host = r.bool()?;
        let iss = r.u32()?;
        let snd_una = r.u32()?;
        let snd_nxt = r.u32()?;
        let snd_max = r.u32()?;
        let snd_wnd = r.u16()?;
        let mss = r.u16()?;
        let tx = r.bytes()?;
        let host_eof = r.bool()?;
        let fin_acked = r.bool()?;
        let rcv_nxt = r.u32()?;
        let rx = r.bytes()?;
        let guest_fin = r.bool()?;
        let adv_wnd = r.u16()?;
        let rto_us = r.u64()?;
        let rto_armed = r.bool()?;
        let rto_at = r.u64()?;
        let retries = r.u8()?;
        let deadline = r.u64()?;
        let last_us = r.u64()?;
        if tx.len() > TX_CAP
            || rx.len() > RX_CAP
            || !(MIN_MSS..=OUR_MSS).contains(&mss)
            || !(RTO_INIT_US..=RTO_MAX_US).contains(&rto_us)
        {
            return Err(bad);
        }
        // Without a host socket only a connection whose guest side is
        // closed can go on (it waits for the last acknowledgement).
        let alone = phase == Phase::Linger || (phase == Phase::Established && guest_fin);
        Ok(Self {
            phase,
            host: None,
            stale: had_host || !alone,
            iss,
            snd_una,
            snd_nxt,
            snd_max,
            snd_wnd,
            mss,
            tx: tx.iter().copied().collect(),
            host_eof,
            fin_acked,
            rcv_nxt,
            rx: rx.iter().copied().collect(),
            guest_fin,
            adv_wnd,
            rto_us,
            rto_at: rto_armed.then_some(rto_at),
            retries,
            deadline,
            last_us,
        })
    }
}
