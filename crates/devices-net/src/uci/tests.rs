use super::*;
use crate::testing::FakeNet;
use emu_core::host::{Journal, MemFileHost};

const STATUS: u16 = 0xdf1c;
const CMD: u16 = 0xdf1d;
const RESP: u16 = 0xdf1e;
const STAT: u16 = 0xdf1f;

fn uci_with(files: MemFileHost, net: FakeNet) -> Uci {
    let mut u = Uci::new();
    u.attach_host(HostServices::new(
        Some(Box::new(net)),
        Some(Box::new(files)),
    ));
    u
}

fn uci() -> Uci {
    let mut fs = MemFileHost::new();
    fs.insert("/hello.txt", b"Hello, Ultimate!".to_vec())
        .unwrap();
    fs.insert("/games/zork.prg", vec![1, 8, 0xea]).unwrap();
    uci_with(fs, FakeNet::new())
}

fn state(u: &Uci) -> u8 {
    (u.peek(STATUS) >> 4) & 3
}

/// Run until the interface leaves "busy"; bounded.
fn wait(u: &mut Uci) {
    for _ in 0..100_000 {
        if state(u) != State::Busy as u8 {
            return;
        }
        u.tick();
    }
    panic!("UCI stays busy");
}

/// Read the response and status queues like a C64 program does.
fn take(u: &mut Uci) -> (Vec<u8>, String) {
    let mut data = Vec::new();
    while u.read(STATUS) & status::DATA_AV != 0 {
        data.push(u.read(RESP));
    }
    let mut stat = Vec::new();
    while u.read(STATUS) & status::STAT_AV != 0 {
        stat.push(u.read(STAT));
    }
    (data, String::from_utf8(stat).unwrap())
}

/// Send a command and collect all packets until "data last".
fn command(u: &mut Uci, bytes: &[u8]) -> (Vec<Vec<u8>>, String) {
    assert_eq!(state(u), State::Idle as u8, "not idle before {bytes:02x?}");
    for &b in bytes {
        u.write(CMD, b);
    }
    u.write(STATUS, ctrl::PUSH_CMD);
    assert_eq!(state(u), State::Busy as u8);
    assert_ne!(u.peek(STATUS) & status::CMD_BUSY, 0);
    let mut packets = Vec::new();
    let mut last = String::new();
    loop {
        wait(u);
        let more = state(u) == State::DataMore as u8;
        let (data, stat) = take(u);
        packets.push(data);
        if !stat.is_empty() {
            last = stat;
        }
        u.write(STATUS, ctrl::DATA_ACC);
        if !more {
            break;
        }
    }
    assert_eq!(state(u), State::Idle as u8);
    (packets, last)
}

fn text(u: &mut Uci, bytes: &[u8]) -> (String, String) {
    let (p, s) = command(u, bytes);
    (String::from_utf8_lossy(&p.concat()).into_owned(), s)
}

#[test]
fn registers_and_identification() {
    let mut u = uci();
    assert_eq!(u.peek(CMD), IDENT);
    assert_eq!(u.peek(STATUS), 0);
    // A push while busy sets ERROR; CLR_ERR clears it.
    u.write(CMD, 4);
    u.write(CMD, 1);
    u.write(STATUS, ctrl::PUSH_CMD);
    u.write(STATUS, ctrl::PUSH_CMD);
    assert_ne!(u.peek(STATUS) & status::ERROR, 0);
    u.write(STATUS, ctrl::CLR_ERR);
    assert_eq!(u.peek(STATUS) & status::ERROR, 0);
    wait(&mut u);
    assert_eq!(state(&u), State::DataLast as u8);
    // Peeks do not take bytes from the queue.
    let first = u.peek(RESP);
    assert_eq!(u.peek(RESP), first);
    let (data, stat) = take(&mut u);
    assert_eq!(data, b"CONTROL TARGET V1.1");
    assert_eq!(stat, "00,OK");
    u.write(STATUS, ctrl::DATA_ACC);
    assert_eq!(
        text(&mut u, &[3, 1]).0,
        "ULTIMATE-II NETWORK INTERFACE V1.0"
    );
    assert_eq!(text(&mut u, &[4, 0x28, 0]).0, "ULTIMATE 64");
    assert_eq!(text(&mut u, &[4, 0x06]).1, "21,UNKNOWN COMMAND");
    assert_eq!(text(&mut u, &[9, 1]).1, "21,UNKNOWN COMMAND");
}

#[test]
fn abort_returns_to_idle() {
    let mut u = uci();
    u.write(CMD, 1);
    u.write(CMD, 1);
    u.write(STATUS, ctrl::PUSH_CMD);
    u.write(STATUS, ctrl::ABORT);
    assert_ne!(u.peek(STATUS) & status::ABORT_P, 0);
    wait(&mut u);
    assert_eq!(u.peek(STATUS), 0);
}

#[test]
fn dos_reads_writes_and_lists_files() {
    let mut u = uci();
    assert_eq!(text(&mut u, &[1, 0x12]).0, "/");
    let mut open = vec![1, 0x02, 0x01];
    open.extend(b"hello.txt");
    assert_eq!(command(&mut u, &open).1, "00,OK");
    // 16 bytes in chunks; a 600-byte request ends at the end of file.
    let (p, _) = command(&mut u, &[1, 0x04, 0x58, 0x02]);
    assert_eq!(p.concat(), b"Hello, Ultimate!");
    let (p, s) = command(&mut u, &[1, 0x07]);
    assert_eq!(s, "00,OK");
    assert_eq!(&p[0][..4], &16u32.to_le_bytes());
    assert_eq!(&p[0][8..11], b"TXT");
    assert_eq!(p[0][11], 0x20);
    assert_eq!(&p[0][12..], b"hello.txt");
    // Writing a file opened for reading is refused.
    assert_eq!(
        command(&mut u, &[1, 0x05, 0, 0, b'x']).1,
        "87,ACCESS DENIED"
    );
    assert_eq!(command(&mut u, &[1, 0x03]).1, "00,OK");
    assert_eq!(command(&mut u, &[1, 0x03]).1, "84,NO FILE TO CLOSE");
    assert_eq!(command(&mut u, &[1, 0x04, 1, 0]).1, "85,NO FILE OPEN");

    // A new file of 1300 bytes, read back in three chunks.
    let mut create = vec![1, 0x02, 0x0e];
    create.extend(b"games/big.bin");
    assert_eq!(command(&mut u, &create).1, "00,OK");
    let data: Vec<u8> = (0..650u32).map(|i| i as u8).collect();
    for _ in 0..2 {
        let mut w = vec![1, 0x05, 0, 0];
        w.extend(&data);
        assert_eq!(command(&mut u, &w).1, "00,OK");
    }
    assert_eq!(command(&mut u, &[1, 0x06, 0, 0, 0, 0]).1, "00,OK");
    // Reopen for reading through the second DOS target.
    let mut open2 = vec![2, 0x02, 0x01];
    open2.extend(b"/games/big.bin");
    assert_eq!(command(&mut u, &open2).1, "00,OK");
    let (p, _) = command(&mut u, &[2, 0x04, 0xff, 0xff]);
    assert_eq!(p.iter().map(Vec::len).collect::<Vec<_>>(), [512, 512, 276]);
    assert_eq!(p.concat(), [data.clone(), data].concat());

    // Directories.
    let mut cd = vec![1, 0x11];
    cd.extend(b"games");
    assert_eq!(command(&mut u, &cd).1, "00,OK");
    assert_eq!(text(&mut u, &[1, 0x12]).0, "/games/");
    assert_eq!(command(&mut u, &[1, 0x13]).1, "00,OK");
    let (p, _) = command(&mut u, &[1, 0x14]);
    assert_eq!(p.len(), 2);
    assert_eq!(p[0][0], 0x20);
    assert_eq!(&p[0][1..], b"big.bin");
    assert_eq!(&p[1][1..], b"zork.prg");
    assert_eq!(
        command(&mut u, &[1, 0x11, b'.', b'.', b'/', b'.', b'.']).1,
        "83,NO SUCH DIRECTORY"
    );
    assert_eq!(text(&mut u, &[1, 0x17]).0, "/");
    let mut md = vec![1, 0x16];
    md.extend(b"saves");
    assert_eq!(command(&mut u, &md).1, "00,OK");
    let mut rn = vec![1, 0x0a];
    rn.extend(b"hello.txt\0saves/hi.txt");
    assert_eq!(command(&mut u, &rn).1, "00,OK");
    let mut cp = vec![1, 0x0b];
    cp.extend(b"saves/hi.txt\0copy.txt");
    assert_eq!(command(&mut u, &cp).1, "00,OK");
    let mut st = vec![1, 0x08];
    st.extend(b"copy.txt");
    let (p, s) = command(&mut u, &st);
    assert_eq!(
        (s.as_str(), &p[0][..4]),
        ("00,OK", &16u32.to_le_bytes()[..])
    );
    let mut del = vec![1, 0x09];
    del.extend(b"copy.txt");
    assert_eq!(command(&mut u, &del).1, "00,OK");
    assert_eq!(command(&mut u, &st).1, "88,FILE NOT FOUND");
    // The sandbox cannot be left.
    let mut esc = vec![1, 0x02, 0x01];
    esc.extend(b"../../etc/passwd");
    assert_eq!(command(&mut u, &esc).1, "87,INVALID PATH");
    // Time from the host clock, mount refused, echo.
    assert_eq!(text(&mut u, &[1, 0x26]).0, "2026/10/25 14:14:15");
    assert_eq!(text(&mut u, &[1, 0x26, 1]).0, "Sun 2026/10/25 14:14:15");
    assert_eq!(
        command(&mut u, &[1, 0x23, 8, b'a']).1,
        "99,FUNCTION NOT IMPLEMENTED"
    );
    assert_eq!(command(&mut u, &[1, 0xf0, 7]).0[0], [1, 0xf0, 7]);
}

#[test]
fn network_sockets() {
    let mut u = uci();
    let (p, _) = command(&mut u, &[3, 0x05, 0]);
    assert_eq!(p[0], [192, 168, 1, 64, 255, 255, 255, 0, 192, 168, 1, 1]);
    assert_eq!(command(&mut u, &[3, 0x04, 0]).0[0].len(), 6);
    assert_eq!(
        command(&mut u, &[3, 0x04, 1]).1,
        "82,PARAMETER(S) OUT OF RANGE"
    );
    let mut open = vec![3, 0x07, 0x17, 0x00];
    open.extend(b"echo\0");
    let (p, s) = command(&mut u, &open);
    assert_eq!(s, "00,OK");
    let id = p[0][0];
    assert_eq!(id, 1);
    let (p, _) = command(&mut u, &[3, 0x11, id, b'h', b'i', b'!']);
    assert_eq!(p[0], [3, 0]);
    let (p, s) = command(&mut u, &[3, 0x10, id, 0x10, 0]);
    assert_eq!(
        (p[0].as_slice(), s.as_str()),
        (&[3, 0, b'h', b'i', b'!'][..], "00,OK")
    );
    let (p, _) = command(&mut u, &[3, 0x10, id, 0x10, 0]);
    assert_eq!(p[0], [0, 0]);
    assert_eq!(command(&mut u, &[3, 0x09, id]).1, "00,OK");
    assert_eq!(
        command(&mut u, &[3, 0x10, id, 1, 0]).1,
        "82,PARAMETER(S) OUT OF RANGE"
    );

    // A server that closes.
    let mut open = vec![3, 0x07, 80, 0];
    open.extend(b"greeter");
    let id = command(&mut u, &open).0[0][0];
    let (p, _) = command(&mut u, &[3, 0x10, id, 0x00, 0x01]);
    assert_eq!(&p[0][2..], b"HELLO\r\n");
    assert_eq!(
        command(&mut u, &[3, 0x10, id, 0x00, 0x01]).1,
        "01,CONNECTION CLOSED BY HOST"
    );

    let mut bad = vec![3, 0x07, 80, 0];
    bad.extend(b"nowhere.invalid");
    assert_eq!(command(&mut u, &bad).1, "84,UNRESOLVED HOST");
    let mut refused = vec![3, 0x08, 80, 0];
    refused.extend(b"10.0.0.1");
    assert_eq!(command(&mut u, &refused).1, "85,CONNECTION FAILED");

    // Listener.
    assert_eq!(command(&mut u, &[3, 0x12, 0x40, 0x1f]).1, "00,OK");
    assert_eq!(command(&mut u, &[3, 0x14]).0[0], [1]);
    assert_eq!(command(&mut u, &[3, 0x13]).1, "00,OK");
    assert_eq!(command(&mut u, &[3, 0x14]).0[0], [0]);
}

#[test]
fn without_host_services_everything_fails_cleanly() {
    let mut u = Uci::new();
    let mut open = vec![1, 0x02, 0x01];
    open.extend(b"x");
    assert_eq!(command(&mut u, &open).1, "87,DISK ERR: NO FOLDER");
    let mut tcp = vec![3, 0x07, 80, 0];
    tcp.extend(b"echo");
    assert_eq!(command(&mut u, &tcp).1, "85,CONNECTION FAILED: NO NETWORK");
    assert_eq!(command(&mut u, &[3, 0x05, 0]).0[0], [0; 12]);
    let mut off = FakeNet::new();
    off.online = false;
    let mut u = uci_with(MemFileHost::new(), off);
    assert_eq!(command(&mut u, &tcp).1, "85,CONNECTION FAILED: NO NETWORK");
}

#[test]
fn savestate_round_trip_mid_transfer() {
    let mut u = uci();
    let mut open = vec![1, 0x02, 0x01];
    open.extend(b"hello.txt");
    command(&mut u, &open);
    command(&mut u, &[1, 0x13]);
    for &b in &[1u8, 0x04, 4, 0] {
        u.write(CMD, b);
    }
    u.write(STATUS, ctrl::PUSH_CMD);
    u.tick();
    let mut w = StateWriter::new();
    u.save(&mut w);
    let bytes = w.into_bytes();
    let mut v = Uci::new();
    v.load(&mut StateReader::new(&bytes)).unwrap();
    assert_eq!(u, v);
    // Corrupt data changes nothing.
    let before = v.clone();
    for cut in [0, 5, bytes.len() / 2, bytes.len() - 1] {
        assert!(v.load(&mut StateReader::new(&bytes[..cut])).is_err());
        assert_eq!(v, before);
    }
}

#[test]
fn journal_replays_a_network_session() {
    let journal = Journal::new(
        Some(Box::new(FakeNet::new())),
        Some(Box::new(MemFileHost::new())),
    );
    let run = |u: &mut Uci| {
        let mut open = vec![3, 0x07, 0x17, 0x00];
        open.extend(b"echo");
        let id = command(u, &open).0[0][0];
        command(u, &[3, 0x11, id, 1, 2, 3]);
        let a = command(u, &[3, 0x10, id, 9, 0]);
        let t = text(u, &[1, 0x26]);
        (a, t)
    };
    let mut u = Uci::new();
    u.attach_host(journal.services());
    journal.start_recording();
    let first = run(&mut u);
    let record = journal.stop_recording();
    // Replay on a UCI with no live network behind the journal.
    let replay = Journal::new(None, None);
    replay.start_replay(&record).unwrap();
    let mut v = Uci::new();
    v.attach_host(replay.services());
    assert_eq!(run(&mut v), first);
    assert_eq!(replay.replay_status().diverged, None);
}
