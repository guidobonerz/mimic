//! Contract test for the native debug protocol (JSON lines over TCP),
//! end to end against the sim6502 reference machine. A protocol change that
//! breaks this test needs a version bump in the handshake.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use emu_core::registry::MachineConfig;
use emu_debug::protocol::PROTOCOL_VERSION;
use emu_debug::{DebugServer, ServerConfig, Session};
use serde_json::{json, Value};

struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
}

impl Client {
    fn connect(server: &DebugServer) -> Self {
        let stream = TcpStream::connect(server.addr()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        Self {
            reader: BufReader::new(stream.try_clone().unwrap()),
            writer: stream,
            next_id: 1,
        }
    }

    fn recv(&mut self) -> Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line:?}"))
    }

    fn send_raw(&mut self, line: &str) {
        self.writer.write_all(line.as_bytes()).unwrap();
        self.writer.write_all(b"\n").unwrap();
    }

    /// Send a command and return its response, skipping pushed events.
    fn call(&mut self, mut req: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        req["id"] = json!(id);
        self.send_raw(&req.to_string());
        loop {
            let v = self.recv();
            if v.get("event").is_none() {
                assert_eq!(v["id"], id, "responses arrive in order");
                return v;
            }
        }
    }

    fn ok(&mut self, req: Value) -> Value {
        let v = self.call(req);
        assert_eq!(v["ok"], true, "{v}");
        v["result"].clone()
    }
}

fn server() -> DebugServer {
    let machine = emu_machines::registry()
        .create("sim6502", &MachineConfig::default())
        .unwrap();
    DebugServer::start(
        Session::new(machine, "sim6502"),
        ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            run: false,
            realtime: false,
        },
    )
    .unwrap()
}

#[test]
fn handshake_and_inspection() {
    let server = server();
    let mut c = Client::connect(&server);
    assert_eq!(
        c.recv(),
        json!({"event": "hello", "protocol": "polytrace-debug",
               "version": PROTOCOL_VERSION, "machine": "sim6502"})
    );

    let info = c.ok(json!({"cmd": "info"}));
    assert_eq!(info["devices"], json!(["cpu"]));
    assert_eq!(info["memory_spaces"][0]["addr_bits"], 16);
    assert_eq!(info["running"], false);

    // Reset sequence, then the demo's first instruction.
    let r = c.ok(json!({"cmd": "step_instr"}));
    assert_eq!(r["pc"], 0x0800);
    let r = c.ok(json!({"cmd": "step_instr"}));
    assert_eq!(r["reason"]["kind"], "step_done");
    assert_eq!(r["pc"], 0x0803);

    let regs = c.ok(json!({"cmd": "regs"}));
    let names: Vec<&str> = regs["regs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["PC", "A", "X", "Y", "S", "P"]);

    let d = c.ok(json!({"cmd": "disasm", "addr": 0x0800, "count": 3}));
    let text: Vec<&str> = d["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["text"].as_str().unwrap())
        .collect();
    assert_eq!(text, ["LDA $D010", "STA $D000", "JMP $0800"]);

    let p = c.ok(json!({"cmd": "peek", "addr": 0x0800, "len": 3}));
    assert_eq!(p["bytes"], json!([0xad, 0x10, 0xd0]));

    c.ok(json!({"cmd": "set_reg", "name": "A", "value": 0x42}));
    let regs = c.ok(json!({"cmd": "regs", "device": "cpu"}));
    assert_eq!(regs["regs"][1]["value"], 0x42);

    server.shutdown();
}

#[test]
fn stepping_by_cycle_line_and_frame() {
    let server = server();
    let mut c = Client::connect(&server);
    c.recv();
    let b0 = c.ok(json!({"cmd": "beam"}));
    let r = c.ok(json!({"cmd": "step_cycle"}));
    assert_eq!(r["beam"]["cycle"], b0["cycle"].as_u64().unwrap() + 1);
    let r = c.ok(json!({"cmd": "step_line"}));
    assert_eq!(r["beam"]["line"], 1);
    assert_eq!(r["beam"]["cycle"], 0);
    let r = c.ok(json!({"cmd": "step_frame"}));
    assert_eq!(r["reason"]["kind"], "frame_done");
    assert_eq!(r["beam"]["frame"], 1);
    server.shutdown();
}

#[test]
fn resume_until_breakpoint_pushes_stopped_event() {
    let server = server();
    let mut c = Client::connect(&server);
    c.recv();
    let id = c.ok(json!({"cmd": "break_add", "addr": 0x0806}))["bp"].clone();
    c.ok(json!({"cmd": "resume"}));
    let ev = c.recv();
    assert_eq!(ev["event"], "stopped", "{ev}");
    assert_eq!(ev["reason"], json!({"kind": "breakpoint", "bp": id}));
    assert_eq!(ev["pc"], 0x0806);
    let info = c.ok(json!({"cmd": "info"}));
    assert_eq!(info["running"], false);

    c.ok(json!({"cmd": "break_remove", "bp": id}));
    c.ok(json!({"cmd": "resume"}));
    let p = c.ok(json!({"cmd": "pause"}));
    assert_eq!(p["reason"]["kind"], "step_done");
    server.shutdown();
}

#[test]
fn errors_do_not_close_the_connection() {
    let server = server();
    let mut c = Client::connect(&server);
    c.recv();
    c.send_raw("this is not json");
    let e = c.recv();
    assert_eq!(e["ok"], false);
    assert!(e["id"].is_null());
    let e = c.call(json!({"cmd": "regs", "device": "z80"}));
    assert_eq!(e["ok"], false);
    assert_eq!(e["error"], "unknown device 'z80'");
    let e = c.call(json!({"cmd": "teleport"}));
    assert_eq!(e["ok"], false);
    c.ok(json!({"cmd": "info"}));
    server.shutdown();
}

#[test]
fn refuses_non_loopback_bind() {
    let machine = emu_machines::registry()
        .create("sim6502", &MachineConfig::default())
        .unwrap();
    let r = DebugServer::start(
        Session::new(machine, "sim6502"),
        ServerConfig {
            bind: "0.0.0.0:0".parse().unwrap(),
            run: false,
            realtime: false,
        },
    );
    assert!(r.is_err());
}
