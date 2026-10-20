//! VICE binary monitor adapter, end to end over TCP. The client below
//! follows the protocol description in the VICE manual and the command
//! sequence VS64 uses: registers/banks available, checkpoint list/set, reset,
//! autostart, exit, advance, execute-until-return.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use emu_core::registry::MachineConfig;
use emu_debug::vice::{cmd, error, event, EVENT_ID};
use emu_debug::{DebugServer, ServerConfig, Session};

#[derive(Debug)]
struct Frame {
    kind: u8,
    error: u8,
    id: u32,
    body: Vec<u8>,
}

struct Vice {
    stream: TcpStream,
    next: u32,
    events: VecDeque<Frame>,
}

impl Vice {
    fn connect(server: &DebugServer) -> Self {
        let stream = TcpStream::connect(server.vice_addr().unwrap()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        Self {
            stream,
            next: 1,
            events: VecDeque::new(),
        }
    }

    fn read_frame(&mut self) -> Frame {
        let mut h = [0u8; 12];
        self.stream.read_exact(&mut h).unwrap();
        assert_eq!((h[0], h[1]), (0x02, 0x02), "STX and API version");
        let len = u32::from_le_bytes([h[2], h[3], h[4], h[5]]) as usize;
        let mut body = vec![0; len];
        self.stream.read_exact(&mut body).unwrap();
        Frame {
            kind: h[6],
            error: h[7],
            id: u32::from_le_bytes([h[8], h[9], h[10], h[11]]),
            body,
        }
    }

    fn send(&mut self, command: u8, body: &[u8]) -> u32 {
        let id = self.next;
        self.next += 1;
        let mut msg = vec![0x02, 0x02];
        msg.extend_from_slice(&(body.len() as u32).to_le_bytes());
        msg.extend_from_slice(&id.to_le_bytes());
        msg.push(command);
        msg.extend_from_slice(body);
        self.stream.write_all(&msg).unwrap();
        id
    }

    /// Send a command; returns the frames answering it (the last one is the
    /// response proper). Events in between are queued.
    fn call(&mut self, command: u8, body: &[u8], response: u8) -> Vec<Frame> {
        let id = self.send(command, body);
        let mut frames = Vec::new();
        loop {
            let f = self.read_frame();
            if f.id == EVENT_ID {
                self.events.push_back(f);
                continue;
            }
            assert_eq!(f.id, id, "{f:?}");
            let done = f.kind == response || f.error != error::OK;
            frames.push(f);
            if done {
                return frames;
            }
        }
    }

    fn ok(&mut self, command: u8, body: &[u8]) -> Vec<u8> {
        let response = match command {
            cmd::CHECKPOINT_SET | cmd::CHECKPOINT_GET => event::CHECKPOINT,
            cmd::REGISTERS_SET => event::REGISTERS,
            c => c,
        };
        let f = self.call(command, body, response).pop().unwrap();
        assert_eq!(f.error, error::OK, "command {command:#x}: {f:?}");
        assert_eq!(f.kind, response);
        f.body
    }

    /// Next event of `kind`; events of other kinds before it are dropped.
    fn event(&mut self, kind: u8) -> Frame {
        loop {
            let f = match self.events.pop_front() {
                Some(f) => f,
                None => self.read_frame(),
            };
            assert_eq!(f.id, EVENT_ID, "unexpected response {f:?}");
            if f.kind == kind {
                return f;
            }
        }
    }

    /// Registers by name from the 0x83 listing and a 0x31 body.
    fn registers(&mut self, regs_body: &[u8]) -> Vec<(String, u16)> {
        let names = self.ok(cmd::REGISTERS_AVAILABLE, &[0]);
        let mut by_id = std::collections::BTreeMap::new();
        let mut i = 2;
        while i < names.len() {
            let size = usize::from(names[i]);
            let (id, len) = (names[i + 1], usize::from(names[i + 3]));
            let name = String::from_utf8(names[i + 4..i + 4 + len].to_vec()).unwrap();
            by_id.insert(id, name);
            i += 1 + size;
        }
        let mut out = Vec::new();
        let mut i = 2;
        while i < regs_body.len() {
            let id = regs_body[i + 1];
            let v = u16::from_le_bytes([regs_body[i + 2], regs_body[i + 3]]);
            out.push((by_id[&id].clone(), v));
            i += 1 + usize::from(regs_body[i]);
        }
        out
    }

    fn reg(&mut self, regs_body: &[u8], name: &str) -> u16 {
        self.registers(regs_body)
            .into_iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("no register {name}"))
            .1
    }

    /// Wait for a stop: optional checkpoint event, registers, stopped.
    /// Returns (checkpoint body if any, registers body, PC).
    fn stopped(&mut self) -> (Option<Vec<u8>>, Vec<u8>, u16) {
        let mut cp = None;
        loop {
            let f = self.event_any();
            match f.kind {
                event::CHECKPOINT => cp = Some(f.body),
                event::REGISTERS => {
                    let stop = self.event(event::STOPPED);
                    let pc = u16::from_le_bytes([stop.body[0], stop.body[1]]);
                    return (cp, f.body, pc);
                }
                _ => {}
            }
        }
    }

    fn event_any(&mut self) -> Frame {
        match self.events.pop_front() {
            Some(f) => f,
            None => self.read_frame(),
        }
    }
}

fn exec_checkpoint(start: u16, end: u16) -> Vec<u8> {
    let mut b = start.to_le_bytes().to_vec();
    b.extend_from_slice(&end.to_le_bytes());
    b.extend_from_slice(&[1, 1, 4, 0, 0]);
    b
}

fn mem_get(start: u16, end: u16) -> Vec<u8> {
    let mut b = vec![0];
    b.extend_from_slice(&start.to_le_bytes());
    b.extend_from_slice(&end.to_le_bytes());
    b.extend_from_slice(&[0, 0, 0]);
    b
}

/// The subroutine program of the native protocol test:
///
/// ```text
/// 0800 JSR $0810      0810 LDA #$01      0820 INC $0401
/// 0803 JMP $0800      0812 STA $0400     0823 RTS
///                     0815 JSR $0820
///                     0818 RTS
/// ```
fn subroutines() -> MachineConfig {
    let mut mem = vec![0xea; 0x24];
    mem[0x00..0x06].copy_from_slice(&[0x20, 0x10, 0x08, 0x4c, 0x00, 0x08]);
    mem[0x10..0x19].copy_from_slice(&[0xa9, 0x01, 0x8d, 0x00, 0x04, 0x20, 0x20, 0x08, 0x60]);
    mem[0x20..0x24].copy_from_slice(&[0xee, 0x01, 0x04, 0x60]);
    let mut program = vec![0x00, 0x08];
    program.extend_from_slice(&mem);
    let mut config = MachineConfig::default();
    config.roms.insert("program".into(), program);
    config
}

fn server(machine: &str, config: MachineConfig) -> DebugServer {
    let m = emu_machines::registry().create(machine, &config).unwrap();
    DebugServer::start(
        Session::new(m, machine),
        ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            vice: Some("127.0.0.1:0".parse().unwrap()),
            run: true,
            realtime: false,
        },
    )
    .unwrap()
}

#[test]
fn connect_checkpoints_and_stepping_like_vs64() {
    let server = server("sim6502", subroutines());
    let mut v = Vice::connect(&server);

    // Connect: the first command stops the running machine.
    let names = v.ok(cmd::REGISTERS_AVAILABLE, &[0]);
    assert!(u16::from_le_bytes([names[0], names[1]]) >= 6);
    let (_, regs, _) = v.stopped();
    let all: Vec<String> = v.registers(&regs).into_iter().map(|(n, _)| n).collect();
    for name in ["PC", "A", "X", "Y", "SP", "FL", "LIN", "CYC"] {
        assert!(all.contains(&name.to_string()), "{name} in {all:?}");
    }
    let banks = v.ok(cmd::BANKS_AVAILABLE, &[]);
    assert!(banks.windows(7).any(|w| w == b"default"));
    assert!(banks.windows(3).any(|w| w == b"cpu"));

    // No checkpoints yet, then one at $0820.
    let list = v.call(cmd::CHECKPOINT_LIST, &[], cmd::CHECKPOINT_LIST);
    assert_eq!(list.last().unwrap().body, 0u32.to_le_bytes());
    let cp = v.ok(cmd::CHECKPOINT_SET, &exec_checkpoint(0x0820, 0x0820));
    let n = u32::from_le_bytes([cp[0], cp[1], cp[2], cp[3]]);
    assert_eq!(cp[4], 0, "not hit yet");
    assert_eq!(u16::from_le_bytes([cp[5], cp[6]]), 0x0820);

    v.ok(cmd::EXIT, &[]);
    v.event(event::RESUMED);
    let (hit, regs, pc) = v.stopped();
    let hit = hit.expect("checkpoint event");
    assert_eq!(u32::from_le_bytes([hit[0], hit[1], hit[2], hit[3]]), n);
    assert_eq!(hit[4], 1, "currently hit");
    assert_eq!(
        u32::from_le_bytes([hit[13], hit[14], hit[15], hit[16]]),
        1,
        "hit count"
    );
    assert_eq!(pc, 0x0820);
    assert_eq!(v.reg(&regs, "PC"), 0x0820);

    // Full memory read: length field 0 means 64 KB.
    let mem = v.ok(cmd::MEMORY_GET, &mem_get(0, 0xffff));
    assert_eq!(&mem[..2], &[0, 0]);
    assert_eq!(mem.len(), 2 + 0x10000);
    assert_eq!(mem[2 + 0x0820], 0xee);

    // Step into: response, then registers and stopped.
    v.ok(cmd::ADVANCE_INSTRUCTIONS, &[0, 1, 0]);
    let (_, _, pc) = v.stopped();
    assert_eq!(pc, 0x0823);
    // Return: back in the caller after its JSR.
    v.ok(cmd::EXECUTE_UNTIL_RETURN, &[]);
    let (_, _, pc) = v.stopped();
    assert_eq!(pc, 0x0818);

    // Step over the JSR at $0815: the nested call runs completely.
    v.ok(cmd::CHECKPOINT_DELETE, &n.to_le_bytes());
    v.ok(cmd::CHECKPOINT_SET, &exec_checkpoint(0x0815, 0x0815));
    v.ok(cmd::EXIT, &[]);
    let (_, _, pc) = v.stopped();
    assert_eq!(pc, 0x0815);
    let before = v.ok(cmd::MEMORY_GET, &mem_get(0x0401, 0x0401))[2];
    v.ok(cmd::ADVANCE_INSTRUCTIONS, &[1, 1, 0]);
    let (_, _, pc) = v.stopped();
    assert_eq!(pc, 0x0818);
    let after = v.ok(cmd::MEMORY_GET, &mem_get(0x0401, 0x0401))[2];
    assert_eq!(after, before.wrapping_add(1));

    // Registers and memory set.
    let ids = v.ok(cmd::REGISTERS_AVAILABLE, &[0]);
    let a_id = {
        let mut i = 2;
        loop {
            if &ids[i + 4..i + 4 + usize::from(ids[i + 3])] == b"A" {
                break ids[i + 1];
            }
            i += 1 + usize::from(ids[i]);
        }
    };
    let regs = v.ok(cmd::REGISTERS_SET, &[0, 1, 0, 3, a_id, 0x42, 0x00]);
    assert_eq!(v.reg(&regs, "A"), 0x42);
    let mut set = vec![0, 0x00, 0x05, 0x02, 0x05, 0, 0, 0];
    set.extend_from_slice(&[7, 8, 9]);
    v.ok(cmd::MEMORY_SET, &set);
    assert_eq!(
        &v.ok(cmd::MEMORY_GET, &mem_get(0x0500, 0x0502))[2..],
        &[7, 8, 9]
    );
    server.shutdown();
}

#[test]
fn conditions_implicit_stops_and_errors() {
    let server = server("sim6502", subroutines());
    let mut v = Vice::connect(&server);
    v.ok(cmd::PING, &[]);
    v.stopped();

    // Stop at $0812 only once $0401 reached 5 (VICE syntax: bare hex).
    let cp = v.ok(cmd::CHECKPOINT_SET, &exec_checkpoint(0x0812, 0x0812));
    let n = &cp[..4];
    let cond = b"@cpu:0401 == 5 && .A == 1";
    let mut body = n.to_vec();
    body.push(cond.len() as u8);
    body.extend_from_slice(cond);
    v.ok(cmd::CONDITION_SET, &body);
    let listed = v.call(cmd::CHECKPOINT_LIST, &[], cmd::CHECKPOINT_LIST);
    assert_eq!(listed.len(), 2, "one checkpoint item, then the count");
    assert_eq!(listed[0].body[21], 1, "has condition");
    v.ok(cmd::EXIT, &[]);
    let (_, _, pc) = v.stopped();
    assert_eq!(pc, 0x0812);
    assert_eq!(v.ok(cmd::MEMORY_GET, &mem_get(0x0401, 0x0401))[2], 5);

    // A command while running stops the machine (VICE semantics).
    v.ok(cmd::CHECKPOINT_DELETE, n);
    v.ok(cmd::EXIT, &[]);
    v.event(event::RESUMED);
    v.ok(cmd::PING, &[]);
    v.stopped();

    // Errors keep the connection usable.
    let e = v.call(
        cmd::CHECKPOINT_DELETE,
        &99u32.to_le_bytes(),
        cmd::CHECKPOINT_DELETE,
    );
    assert_eq!(e.last().unwrap().error, error::NOT_FOUND);
    let e = v.call(cmd::MEMORY_GET, &[0, 2, 0, 1, 0, 9, 0, 0], cmd::MEMORY_GET);
    assert_eq!(
        e.last().unwrap().error,
        error::BAD_PARAMETER,
        "end before start"
    );
    let e = v.call(cmd::KEYBOARD_FEED, &[1, b'A'], cmd::KEYBOARD_FEED);
    assert_eq!(e.last().unwrap().error, error::BAD_COMMAND);
    let e = v.call(0x77, &[], 0x77);
    assert_eq!(e.last().unwrap().error, error::BAD_COMMAND);

    let display = v.ok(cmd::DISPLAY_GET, &[1, 0]);
    let (w, h) = (
        u16::from_le_bytes([display[4], display[5]]),
        u16::from_le_bytes([display[6], display[7]]),
    );
    let len = u32::from_le_bytes([display[17], display[18], display[19], display[20]]);
    assert_eq!(len, u32::from(w) * u32::from(h));
    assert_eq!(display.len(), 21 + len as usize);
    let palette = v.ok(cmd::PALETTE_GET, &[1]);
    assert!(u16::from_le_bytes([palette[0], palette[1]]) > 0);
    v.ok(cmd::RESET, &[1]);
    v.ok(cmd::VICE_INFO, &[]);
    server.shutdown();
}

#[test]
fn reset_and_autostart_a_prg_like_vs64() {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    let server = server("c64", config);
    let mut v = Vice::connect(&server);
    v.ok(cmd::REGISTERS_AVAILABLE, &[0]);
    v.ok(cmd::BANKS_AVAILABLE, &[]);

    // A program at $C000 (not BASIC: injected, not RUN).
    let dir = std::env::temp_dir().join(format!("polytrace-vice-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("prog.prg");
    std::fs::write(&path, [0x00, 0xc0, 0xa9, 0x2a, 0x60]).unwrap();
    let name = path.to_str().unwrap().as_bytes();
    v.ok(cmd::RESET, &[0]);
    let mut body = vec![1, 0, 0, name.len() as u8];
    body.extend_from_slice(name);
    v.ok(cmd::AUTOSTART, &body);

    // The program arrives once the KERNAL has booted.
    let mut loaded = false;
    for _ in 0..200 {
        v.ok(cmd::EXIT, &[]);
        std::thread::sleep(Duration::from_millis(20));
        v.ok(cmd::PING, &[]);
        let mem = v.ok(cmd::MEMORY_GET, &mem_get(0xc000, 0xc002));
        if mem[2..] == [0xa9, 0x2a, 0x60] {
            loaded = true;
            break;
        }
    }
    assert!(loaded, "program injected after boot");

    let e = v.call(
        cmd::AUTOSTART,
        &[1, 0, 0, 4, b'x', b'.', b'd', b'6'],
        cmd::AUTOSTART,
    );
    assert_ne!(e.last().unwrap().error, error::OK);
    let _ = std::fs::remove_dir_all(&dir);
    server.shutdown();
}
