use super::*;
use crate::testing::FakeNet;
use emu_core::host::Journal;

/// The C64's side of the user port, driven at pin level like a program
/// using CIA2: PA2 for the direction, PB writes/reads with PC2 pulses,
/// FLAG2 edges as the handshake.
struct C64Side {
    dev: Wic64,
    now: u64,
    pa2_high: bool,
    pb_out: u8,
    flag: bool,
    flag_level: bool,
}

impl C64Side {
    fn new(net: Option<Box<dyn emu_core::host::NetBackend>>) -> Self {
        let mut dev = Wic64::new();
        dev.attach_host(&HostServices::new(net, None));
        Self::with(dev)
    }

    fn with(dev: Wic64) -> Self {
        Self {
            dev,
            now: 1,
            pa2_high: true,
            pb_out: 0xff,
            flag: false,
            flag_level: true,
        }
    }

    fn step(&mut self, pc2: bool) {
        let mut low = if self.pa2_high { 0 } else { user_port::PA2 };
        if pc2 {
            low |= user_port::PC2;
        }
        // Receiving: DDRB is input, the C64 drives nothing.
        low |= user_port::pb_lines(if self.pa2_high { self.pb_out } else { 0xff });
        self.dev.write_lines(
            PortLines {
                pulled_low: low,
                analog: [0; 2],
            },
            Cycle(self.now),
        );
        let l = self.dev.read_lines(Cycle(self.now));
        let level = l.pulled_low & user_port::FLAG2 == 0;
        if self.flag_level && !level {
            self.flag = true;
        }
        self.flag_level = level;
        self.now += 1;
    }

    fn wait_flag(&mut self, limit: u64) -> bool {
        for _ in 0..limit {
            if std::mem::take(&mut self.flag) {
                return true;
            }
            self.step(false);
        }
        false
    }

    fn send(&mut self, bytes: &[u8]) {
        self.pa2_high = true;
        self.step(false);
        for &b in bytes {
            self.pb_out = b;
            self.step(true);
            assert!(self.wait_flag(1000), "no handshake for a sent byte");
        }
    }

    fn recv(&mut self, limit: u64) -> Option<u8> {
        if !self.wait_flag(limit) {
            return None;
        }
        let pb = user_port::pb(self.dev.read_lines(Cycle(self.now)).pulled_low);
        self.step(true); // the read of $DD01 pulses PC2
        Some(pb)
    }

    /// A protocol-2 request; returns status and payload.
    fn request(&mut self, command: u8, payload: &[u8]) -> (u8, Vec<u8>) {
        let mut req = vec![b'R', command];
        req.extend((payload.len() as u16).to_le_bytes());
        req.extend(payload);
        self.send(&req);
        self.pa2_high = false;
        let limit = 10_000_000;
        let status = self.recv(limit).expect("response header");
        let lo = self.recv(1000).unwrap();
        let hi = self.recv(1000).unwrap();
        let n = usize::from(u16::from_le_bytes([lo, hi]));
        let data = (0..n).map(|_| self.recv(1000).unwrap()).collect();
        (status, data)
    }

    fn legacy(&mut self, command: u8, payload: &[u8]) -> Vec<u8> {
        let mut req = vec![b'W'];
        req.extend(((payload.len() + 4) as u16).to_le_bytes());
        req.push(command);
        req.extend(payload);
        self.send(&req);
        self.pa2_high = false;
        let hi = self.recv(10_000_000).expect("legacy header");
        let lo = self.recv(1000).unwrap();
        let n = usize::from(u16::from_be_bytes([hi, lo]));
        (0..n).map(|_| self.recv(1000).unwrap()).collect()
    }
}

fn net() -> FakeNet {
    FakeNet::new()
        .page("http://example.org/hello", b"Hello, C64!")
        .page("http://x.wic64.net/prg/menu", b"MENU")
        .page("http://example.org/?d=0102FF", b"encoded")
}

#[test]
fn version_echo_and_info() {
    let mut c = C64Side::new(Some(Box::new(net())));
    assert_eq!(
        c.request(cmd::GET_VERSION_STRING, &[]),
        (0, b"2.1.0".to_vec())
    );
    assert_eq!(
        c.request(cmd::GET_VERSION_NUMBERS, &[]),
        (0, vec![2, 1, 0, 0])
    );
    assert_eq!(c.request(cmd::ECHO, b"abc"), (0, b"abc".to_vec()));
    assert_eq!(c.request(cmd::GET_IP, &[]).1, b"192.168.1.64\0");
    assert_eq!(c.request(cmd::GET_MAC, &[]).1, b"02:6D:69:6D:69:63\0");
    assert_eq!(c.request(cmd::GET_SSID, &[]).1, b"mimic\0");
    assert_eq!(c.request(cmd::GET_RSSI, &[]).1, b"-50dBm\0");
    assert_eq!(c.request(cmd::IS_CONNECTED, &[1, 0, 5]).0, status::SUCCESS);
    assert_eq!(c.request(cmd::IS_HARDWARE, &[]).0, status::INTERNAL_ERROR);
    assert_eq!(
        c.request(cmd::GET_STATUS_MESSAGE, &[1]).1,
        b"WIC64 IS EMULATED\0"
    );
    assert_eq!(c.request(cmd::FORCE_ERROR, &[]).0, status::INTERNAL_ERROR);
    assert_eq!(c.request(0x77, &[]).0, status::CLIENT_ERROR);
    assert_eq!(
        c.request(cmd::GET_SERVER, &[]).1,
        b"http://x.wic64.net/prg/\0"
    );
}

#[test]
fn time_and_time_zone() {
    let mut c = C64Side::new(Some(Box::new(net())));
    // The host's local time (UTC+1).
    assert_eq!(
        c.request(cmd::GET_LOCAL_TIME, &[]),
        (0, b"14:14:15 25-10-2026\0".to_vec())
    );
    assert_eq!(c.request(cmd::GET_TIMEZONE, &[]).1, b"3600\0");
    // Index 3 (two digits, low first): UTC+2.
    assert_eq!(c.request(cmd::SET_TIMEZONE, b"30").0, 0);
    assert_eq!(c.request(cmd::GET_TIMEZONE, &[]).1, b"7200\0");
    assert_eq!(
        c.request(cmd::GET_LOCAL_TIME, &[]).1,
        b"15:14:15 25-10-2026\0"
    );
}

#[test]
fn http_get_plain_encoded_and_errors() {
    let mut c = C64Side::new(Some(Box::new(net())));
    assert_eq!(
        c.request(cmd::HTTP_GET, b"http://example.org/hello"),
        (0, b"Hello, C64!".to_vec())
    );
    assert_eq!(c.request(cmd::HTTP_GET, b"!menu"), (0, b"MENU".to_vec()));
    let mut enc = b"http://example.org/?d=<$".to_vec();
    enc.extend([3, 0, 1, 2, 0xff]);
    assert_eq!(
        c.request(cmd::HTTP_GET_ENCODED, &enc),
        (0, b"encoded".to_vec())
    );
    assert_eq!(
        c.request(cmd::HTTP_GET, b"http://example.org/missing").0,
        status::SERVER_ERROR
    );
    assert_eq!(
        c.request(cmd::GET_STATUS_MESSAGE, &[0]).1,
        b"HTTP error 404\0"
    );
    assert_eq!(
        c.request(cmd::HTTP_GET, b"https://example.org/").0,
        status::NETWORK_ERROR
    );
    assert_eq!(c.request(cmd::HTTP_GET, b"").0, status::CLIENT_ERROR);
    // POST: the fake echoes the body.
    assert_eq!(
        c.request(cmd::HTTP_POST_URL, b"http://example.org/post").0,
        0
    );
    assert_eq!(c.request(cmd::HTTP_POST_DATA, b"\x01\x02"), (0, vec![1, 2]));
    // Legacy protocol: length MSB first, no status; errors read "!0".
    assert_eq!(
        c.legacy(cmd::HTTP_GET, b"http://example.org/hello"),
        b"Hello, C64!"
    );
    assert_eq!(c.legacy(cmd::HTTP_GET, b"http://example.org/nope"), b"!0");
    assert_eq!(c.legacy(cmd::GET_IP, &[]), b"192.168.1.64\0");
}

#[test]
fn extended_protocol() {
    let mut c = C64Side::new(Some(Box::new(net())));
    let url = b"http://example.org/hello";
    let mut req = vec![b'E', cmd::HTTP_GET];
    req.extend((url.len() as u32).to_le_bytes());
    req.extend(url);
    c.send(&req);
    c.pa2_high = false;
    let header: Vec<u8> = (0..5).map(|_| c.recv(10_000_000).unwrap()).collect();
    assert_eq!(header, [0, 11, 0, 0, 0]);
    let body: Vec<u8> = (0..11).map(|_| c.recv(1000).unwrap()).collect();
    assert_eq!(body, b"Hello, C64!");
}

#[test]
fn tcp_session() {
    let mut c = C64Side::new(Some(Box::new(net())));
    assert_eq!(c.request(cmd::TCP_READ, &[]).0, status::NETWORK_ERROR);
    assert_eq!(c.request(cmd::TCP_OPEN, b"echo:6400").0, 0);
    assert_eq!(c.request(cmd::TCP_WRITE, b"ping"), (0, vec![]));
    assert_eq!(c.request(cmd::TCP_AVAILABLE, &[]), (0, vec![4, 0]));
    assert_eq!(c.request(cmd::TCP_READ, &[]), (0, b"ping".to_vec()));
    assert_eq!(c.request(cmd::TCP_READ, &[]), (0, vec![]));
    assert_eq!(c.request(cmd::TCP_CLOSE, &[]).0, 0);
    assert_eq!(c.request(cmd::TCP_OPEN, b"greeter:23").0, 0);
    assert_eq!(c.request(cmd::TCP_READ, &[]), (0, b"HELLO\r\n".to_vec()));
    assert_eq!(c.request(cmd::TCP_READ, &[]).0, status::NETWORK_ERROR);
    assert_eq!(
        c.request(cmd::TCP_OPEN, b"gone.invalid:23").0,
        status::NETWORK_ERROR
    );
    assert_eq!(c.request(cmd::TCP_OPEN, b"no port").0, status::CLIENT_ERROR);
}

#[test]
fn without_network_remote_commands_fail() {
    for net in [None, {
        let mut n = net();
        n.online = false;
        Some(Box::new(n) as Box<dyn emu_core::host::NetBackend>)
    }] {
        let mut c = C64Side::new(net);
        assert_eq!(
            c.request(cmd::HTTP_GET, b"http://example.org/hello").0,
            status::CONNECTION_ERROR
        );
        assert_eq!(
            c.request(cmd::GET_STATUS_MESSAGE, &[0]).1,
            b"WiFi not connected\0"
        );
        assert_eq!(
            c.request(cmd::TCP_OPEN, b"echo:1").0,
            status::CONNECTION_ERROR
        );
        assert_eq!(
            c.request(cmd::IS_CONNECTED, &[1, 0, 1]).0,
            status::CONNECTION_ERROR
        );
        assert_eq!(c.request(cmd::GET_IP, &[]).1, b"0.0.0.0\0");
        assert_eq!(
            c.request(cmd::GET_LOCAL_TIME, &[]).0,
            status::INTERNAL_ERROR
        );
        assert_eq!(c.request(cmd::ECHO, b"x"), (0, b"x".to_vec()));
    }
}

#[test]
fn savestate_round_trip_mid_request() {
    let mut c = C64Side::new(Some(Box::new(net())));
    c.request(cmd::TCP_OPEN, b"echo:1");
    // Half a request.
    c.send(&[b'R', cmd::ECHO, 3, 0, b'a']);
    let state = c.dev.save_state();
    let mut copy = Wic64::new();
    copy.load_state(&state).unwrap();
    assert_eq!(copy.save_state(), state);
    // Continue on the copy: the request completes as on the original.
    let mut d = C64Side::with(copy);
    d.now = c.now;
    d.send(b"bc");
    d.pa2_high = false;
    let got: Vec<u8> = (0..6).map(|_| d.recv(100_000).unwrap()).collect();
    assert_eq!(got, [0, 3, 0, b'a', b'b', b'c']);
    // Corrupt states change nothing.
    let before = d.dev.save_state();
    for cut in [0, 7, state.len() / 2, state.len() - 1] {
        assert!(d.dev.load_state(&state[..cut]).is_err());
    }
    assert_eq!(d.dev.save_state(), before);
}

#[test]
fn journal_replays_http_and_tcp_exactly() {
    let session = |c: &mut C64Side| {
        let a = c.request(cmd::HTTP_GET, b"http://example.org/hello");
        c.request(cmd::TCP_OPEN, b"echo:1");
        c.request(cmd::TCP_WRITE, b"x");
        let b = c.request(cmd::TCP_READ, &[]);
        let t = c.request(cmd::GET_LOCAL_TIME, &[]);
        (a, b, t, c.now)
    };
    let journal = Journal::new(Some(Box::new(net())), None);
    let mut dev = Wic64::new();
    dev.attach_host(&journal.services());
    let mut c = C64Side::with(dev);
    journal.start_recording();
    let first = session(&mut c);
    let record = journal.stop_recording();

    let replay = Journal::new(None, None);
    replay.start_replay(&record).unwrap();
    let mut dev = Wic64::new();
    dev.attach_host(&replay.services());
    let mut d = C64Side::with(dev);
    // Same answers at the same cycles.
    assert_eq!(session(&mut d), first);
    assert_eq!(replay.replay_status().diverged, None);
}
