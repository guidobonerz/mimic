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

impl Client {
    /// Skip events until one named `name` arrives.
    fn event(&mut self, name: &str) -> Value {
        loop {
            let v = self.recv();
            if v["event"] == name {
                return v;
            }
            assert!(v.get("event").is_some(), "unexpected response {v}");
        }
    }
}

fn server() -> DebugServer {
    server_with(MachineConfig::default())
}

/// Subroutines for stepping tests, loaded at $0800:
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

fn server_with(config: MachineConfig) -> DebugServer {
    let machine = emu_machines::registry().create("sim6502", &config).unwrap();
    DebugServer::start(
        Session::new(machine, "sim6502"),
        ServerConfig {
            vice: None,
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
    assert_eq!(c.ok(json!({"cmd": "resume"})), json!({"running": true}));
    let ev = c.recv();
    assert_eq!(ev["event"], "resumed");
    assert!(ev["pc"].is_u64(), "{ev}");
    let ev = c.recv();
    assert_eq!(ev["event"], "stopped", "{ev}");
    assert_eq!(
        ev["reason"],
        json!({"kind": "breakpoint", "bp": id, "hits": 1})
    );
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
            vice: None,
            bind: "0.0.0.0:0".parse().unwrap(),
            run: false,
            realtime: false,
        },
    );
    assert!(r.is_err());
}

#[test]
fn step_over_step_out_and_run_to() {
    let server = server_with(subroutines());
    let mut c = Client::connect(&server);
    c.recv();
    assert_eq!(c.ok(json!({"cmd": "step_instr"}))["pc"], 0x0800, "reset");

    // Over the JSR at $0800: the whole subroutine runs.
    assert_eq!(c.ok(json!({"cmd": "step_over"})), json!({"running": true}));
    let ev = c.event("stopped");
    assert_eq!(ev["reason"]["kind"], "step_done");
    assert_eq!(ev["pc"], 0x0803);
    let p = c.ok(json!({"cmd": "peek", "addr": 0x0400, "len": 2}));
    assert_eq!(p["bytes"], json!([1, 1]), "subroutines ran");

    // Into it, then out of the nested call and the outer one.
    c.ok(json!({"cmd": "step_instr"})); // JMP $0800
    assert_eq!(c.ok(json!({"cmd": "step_instr", "count": 4}))["pc"], 0x0820);
    c.ok(json!({"cmd": "step_out"}));
    assert_eq!(c.event("stopped")["pc"], 0x0818);
    c.ok(json!({"cmd": "step_out"}));
    assert_eq!(c.event("stopped")["pc"], 0x0803);

    c.ok(json!({"cmd": "run_to", "addr": 0x0815}));
    let ev = c.event("stopped");
    assert_eq!(ev["pc"], 0x0815);
    assert_eq!(c.ok(json!({"cmd": "break_list"})), json!([]));

    let d = c.ok(json!({"cmd": "disasm", "addr": 0x0815, "count": 2}));
    assert_eq!(d["lines"][0]["flow"], "call");
    assert_eq!(d["lines"][1]["flow"], "return");
    server.shutdown();
}

#[test]
fn watchpoints_and_conditions() {
    let server = server_with(subroutines());
    let mut c = Client::connect(&server);
    c.recv();
    let w = c.ok(json!({"cmd": "break_add", "kind": "write", "addr": 0x0401}))["bp"].clone();
    c.ok(json!({"cmd": "resume"}));
    let ev = c.event("stopped");
    assert_eq!(
        ev["reason"],
        json!({"kind": "watchpoint", "bp": w, "space": 0, "addr": 0x0401, "write": true, "hits": 1})
    );
    c.ok(json!({"cmd": "break_remove", "bp": w}));

    // Stop at $0812 only once $0401 has reached 3.
    let b = c.ok(json!({
        "cmd": "break_add", "addr": 0x0812, "condition": "[$0401] == 3 && A == 1",
    }))["bp"]
        .clone();
    c.ok(json!({"cmd": "resume"}));
    let ev = c.event("stopped");
    assert_eq!(ev["reason"]["bp"], b);
    assert_eq!(ev["pc"], 0x0812);
    let p = c.ok(json!({"cmd": "peek", "addr": 0x0401, "len": 1}));
    assert_eq!(p["bytes"], json!([3]));

    let e = c.call(json!({"cmd": "break_add", "addr": 1, "condition": "A =="}));
    assert_eq!(e["ok"], false);
    server.shutdown();
}

#[test]
fn poke_savestate_reset_and_chips() {
    let server = server_with(subroutines());
    let mut c = Client::connect(&server);
    c.recv();
    c.ok(json!({"cmd": "poke", "addr": 0x0400, "bytes": [7, 8, 9]}));
    let p = c.ok(json!({"cmd": "peek", "addr": 0x0400, "len": 3}));
    assert_eq!(p["bytes"], json!([7, 8, 9]));

    c.ok(json!({"cmd": "step_instr", "count": 3}));
    let saved = c.ok(json!({"cmd": "save_state"}))["data"].clone();
    let at = c.ok(json!({"cmd": "regs"}));
    c.ok(json!({"cmd": "step_frame"}));
    let r = c.ok(json!({"cmd": "load_state", "data": saved}));
    assert_eq!(r["pc"], at["regs"][0]["value"]);
    assert_eq!(c.ok(json!({"cmd": "regs"})), at);
    let e = c.call(json!({"cmd": "load_state", "data": "AAAA"}));
    assert_eq!(e["ok"], false, "corrupt state is rejected");
    assert_eq!(c.ok(json!({"cmd": "regs"})), at, "and changes nothing");

    let r = c.ok(json!({"cmd": "reset", "kind": "hard"}));
    assert_eq!(r["reason"]["kind"], "step_done");
    assert_eq!(c.ok(json!({"cmd": "step_instr"}))["pc"], 0x0800);
    let p = c.ok(json!({"cmd": "peek", "addr": 0x0400, "len": 1}));
    assert_eq!(p["bytes"], json!([0]), "hard reset restores RAM");

    let chips = c.ok(json!({"cmd": "chips"}));
    assert_eq!(chips[0]["name"], "video");
    assert_eq!(chips[0]["regs"][0]["addr"], 0xd000);
    server.shutdown();
}

#[test]
fn capabilities_subscriptions_and_trace() {
    let server = server();
    let mut c = Client::connect(&server);
    c.recv();
    let caps = c.ok(json!({"cmd": "capabilities"}));
    assert_eq!(caps["version"], PROTOCOL_VERSION);
    for cmd in [
        "step_over",
        "run_to",
        "break_add",
        "save_state",
        "trace_read",
    ] {
        assert!(
            caps["commands"].as_array().unwrap().contains(&json!(cmd)),
            "{cmd}"
        );
    }
    assert!(caps["breakpoint_kinds"]
        .as_array()
        .unwrap()
        .contains(&json!("raster")));

    let e = c.call(json!({"cmd": "subscribe", "events": ["teleported"]}));
    assert_eq!(e["ok"], false);
    c.ok(json!({"cmd": "subscribe", "events": ["frame_ready"]}));
    c.ok(json!({"cmd": "trace_start", "instructions": true, "capacity": 16}));
    c.ok(json!({"cmd": "resume"}));
    let f = c.event("frame_ready");
    assert!(f["beam"]["frame"].as_u64().unwrap() >= 1);
    c.ok(json!({"cmd": "pause"}));
    c.event("stopped");
    let t = c.ok(json!({"cmd": "trace_read"}));
    let entries = t["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 16, "ring buffer keeps the newest");
    assert_eq!(entries[0]["kind"], "instr");
    assert!(t["dropped"].as_u64().unwrap() > 0);
    c.ok(json!({"cmd": "trace_stop"}));
    c.ok(json!({"cmd": "subscribe", "events": []}));
    server.shutdown();
}

#[test]
fn symbols_label_disassembly_and_breakpoints() {
    let server = server_with(subroutines());
    let mut c = Client::connect(&server);
    c.recv();
    let kick = ".label main=$0800\n.namespace sub {\n  .label inner=$0820\n}\n.const N=3\n";
    let r = c.ok(json!({"cmd": "symbols_load", "text": kick}));
    assert_eq!(
        r,
        json!({"format": "kickass", "loaded": 2, "skipped": 0, "total": 2})
    );
    let r = c.ok(json!({"cmd": "symbols_load", "text": "al C:0810 .outer\n", "format": "vice"}));
    assert_eq!(r["total"], 3);

    let d = c.ok(json!({"cmd": "disasm", "addr": 0x0800, "count": 1}));
    assert_eq!(d["lines"][0]["label"], "main");
    assert_eq!(d["lines"][0]["target_label"], "outer", "JSR $0810");

    let bp = c.ok(json!({"cmd": "break_add", "symbol": "sub.inner"}))["bp"].clone();
    assert_eq!(c.ok(json!({"cmd": "break_list"}))[0]["addr"], 0x0820);
    c.ok(json!({"cmd": "resume"}));
    let ev = c.event("stopped");
    assert_eq!(ev["reason"]["bp"], bp);
    assert_eq!(ev["pc"], 0x0820);

    let list = c.ok(json!({"cmd": "symbols_list"}));
    assert_eq!(list[0], json!({"name": "main", "space": 0, "addr": 0x0800}));
    let e = c.call(json!({"cmd": "break_add", "symbol": "nowhere"}));
    assert_eq!(e["ok"], false);
    let e = c.call(json!({"cmd": "symbols_load", "text": "garbage"}));
    assert_eq!(e["ok"], false);
    c.ok(json!({"cmd": "symbols_clear"}));
    assert_eq!(c.ok(json!({"cmd": "symbols_list"})), json!([]));
    server.shutdown();
}

/// Devices from the `emu-machines` catalogue, images from files: what a
/// frontend lends the session.
struct TestHost;

impl emu_debug::host::HostServices for TestHost {
    fn device_names(&self, kind: emu_core::port::PortKind) -> Vec<&'static str> {
        emu_machines::device_names(kind)
    }

    fn create_device(
        &self,
        name: &str,
        kind: emu_core::port::PortKind,
    ) -> Option<Box<dyn emu_core::port::PortDevice>> {
        emu_machines::create_device(name, kind)
    }

    fn open_media(&self, path: &str) -> Result<emu_core::media::Media, String> {
        media_backends::image::open(std::path::Path::new(path)).map_err(|e| e.to_string())
    }
}

fn c64_server() -> DebugServer {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    let machine = emu_machines::registry().create("c64", &config).unwrap();
    let mut session = Session::new(machine, "c64");
    session.set_host(Box::new(TestHost));
    DebugServer::start(
        session,
        ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            vice: None,
            run: false,
            realtime: false,
        },
    )
    .unwrap()
}

#[test]
fn ports_devices_and_media() {
    let server = c64_server();
    let mut c = Client::connect(&server);
    c.recv();
    let ports = c.ok(json!({"cmd": "list_ports"}));
    assert_eq!(ports[1]["name"], "control port 2");
    assert_eq!(ports[1]["connected"], true, "default joystick");
    assert_eq!(ports[0]["devices"], json!(["joystick"]));

    c.ok(json!({"cmd": "connect_device", "port": 0, "device": "joystick"}));
    assert_eq!(c.event("device_connected")["port"], 0);
    let e = c.call(json!({"cmd": "connect_device", "port": 0, "device": "joystick"}));
    assert_eq!(e["ok"], false, "occupied");
    let e = c.call(json!({"cmd": "connect_device", "port": 1, "device": "lightgun"}));
    assert_eq!(e["ok"], false);
    let r = c.ok(json!({"cmd": "disconnect_device", "port": 1}));
    assert_eq!(r["disconnected"], true);
    assert!(c.event("device_connected")["device"].is_null());
    assert_eq!(c.ok(json!({"cmd": "list_ports"}))[1]["connected"], false);

    let slots = c.ok(json!({"cmd": "list_media_slots"}));
    let program = slots
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["kind"] == "Program")
        .unwrap()["id"]
        .clone();
    let dir = std::env::temp_dir().join(format!("polytrace-media-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let prg = dir.join("t.prg");
    std::fs::write(&prg, [0x00, 0xc0, 0x60]).unwrap();
    let r = c.ok(json!({"cmd": "insert_media", "slot": program, "path": prg.to_str().unwrap()}));
    assert_eq!(r["media"], "t.prg");
    let ev = c.event("media_changed");
    assert_eq!(ev["media"], "t.prg");
    assert_eq!(ev["name"], "program");
    let e = c.call(json!({"cmd": "insert_media", "slot": program, "path": "nope.prg"}));
    assert_eq!(e["ok"], false);
    c.ok(json!({"cmd": "eject_media", "slot": program}));
    assert!(c.event("media_changed")["media"].is_null());
    let e = c.call(json!({"cmd": "set_drive_mode", "slot": program, "mode": "virtual"}));
    assert_eq!(e["ok"], false, "not a drive");
    // Without a DOS ROM drive 8 is the virtual drive; the mode is fixed at
    // start.
    let drive8 = slots
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "drive 8")
        .expect("virtual drive 8")["id"]
        .clone();
    let r = c.ok(json!({"cmd": "set_drive_mode", "slot": drive8, "mode": "virtual"}));
    assert_eq!(r["mode"], "virtual");
    let e = c.call(json!({"cmd": "set_drive_mode", "slot": drive8, "mode": "true"}));
    assert_eq!(e["ok"], false);
    // The datasette's keys: refused without a tape and by slots without keys.
    let tape = slots
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "tape")
        .expect("tape slot")["id"]
        .clone();
    let e = c.call(json!({"cmd": "media_control", "slot": tape, "command": "play"}));
    assert_eq!(e["ok"], false, "no tape");
    let e = c.call(json!({"cmd": "media_control", "slot": program, "command": "play"}));
    assert_eq!(e["ok"], false, "no keys");
    let tap = dir.join("t.tap");
    let blank = media::tap::Tap::blank(media::tap::TapMachine::C64, media::tap::Video::Pal, 1000);
    std::fs::write(&tap, blank.to_bytes().unwrap()).unwrap();
    c.ok(json!({"cmd": "insert_media", "slot": tape, "path": tap.to_str().unwrap()}));
    for key in ["record", "stop", "rewind", "forward", "play"] {
        let r = c.ok(json!({"cmd": "media_control", "slot": tape, "command": key}));
        assert_eq!(r["command"], key);
    }
    let e = c.call(json!({"cmd": "media_control", "slot": tape, "command": "eject!"}));
    assert_eq!(e["ok"], false);
    let _ = std::fs::remove_dir_all(&dir);
    server.shutdown();
}

#[test]
fn recorded_input_replays_bit_exactly() {
    let server = c64_server();
    let mut c = Client::connect(&server);
    c.recv();
    c.ok(json!({"cmd": "step_frame"}));
    c.ok(json!({"cmd": "record_start"}));
    let r = c.ok(json!({"cmd": "input", "events": [
        {"target": {"port": 1}, "delay": 5000,
         "event": {"type": "joystick", "dirs": ["up", "left"], "buttons": 1}},
        {"target": "keyboard", "delay": 9000, "event": {"type": "key", "key": 7, "down": true}},
    ]}));
    assert_eq!(r["queued"], 2);
    for _ in 0..5 {
        c.ok(json!({"cmd": "step_frame"}));
    }
    let replay = c.ok(json!({"cmd": "record_stop"}));
    assert_eq!(replay["format"], "polytrace-replay");
    assert_eq!(replay["events"].as_array().unwrap().len(), 2);
    let recorded_state = c.ok(json!({"cmd": "save_state"}))["data"].clone();

    // Replaying from the start state ends in exactly the same state.
    let r = c.ok(json!({"cmd": "replay_input", "replay": replay}));
    assert_eq!(r["events"], 2);
    for _ in 0..5 {
        c.ok(json!({"cmd": "step_frame"}));
    }
    assert_eq!(c.ok(json!({"cmd": "save_state"}))["data"], recorded_state);

    // Without its events the run differs: the input had an effect.
    let mut silent = replay.clone();
    silent["events"] = json!([]);
    c.ok(json!({"cmd": "replay_input", "replay": silent}));
    for _ in 0..5 {
        c.ok(json!({"cmd": "step_frame"}));
    }
    assert_ne!(c.ok(json!({"cmd": "save_state"}))["data"], recorded_state);

    let e = c.call(json!({"cmd": "record_stop"}));
    assert_eq!(e["ok"], false, "not recording");
    let e = c.call(json!({"cmd": "input", "events": [{"target": "mouse", "event": {}}]}));
    assert_eq!(e["ok"], false);
    server.shutdown();
}

#[test]
fn cheat_search_finds_the_counter_and_a_freeze_pins_it() {
    let server = server_with(subroutines());
    let mut c = Client::connect(&server);
    c.recv();
    // One loop iteration of the program increments $0401 once.
    let iterate = |c: &mut Client| {
        c.ok(json!({"cmd": "run_to", "addr": 0x0812}));
        c.event("stopped");
    };
    iterate(&mut c);
    let r = c.ok(json!({"cmd": "search_start", "size": 1}));
    assert_eq!(r["candidates"], 0x10000);
    let mut left = 0x10000;
    for _ in 0..4 {
        iterate(&mut c);
        left = c.ok(json!({"cmd": "search_next", "op": "increased_by", "value": 1}))["candidates"]
            .as_u64()
            .unwrap();
    }
    assert!((1..10).contains(&left), "{left} candidates");
    let found = c.ok(json!({"cmd": "search_results"}));
    let hit = found["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["addr"] == 0x0401)
        .expect("counter found")
        .clone();
    assert_eq!(hit["value"], hit["last"]);
    let e = c.call(json!({"cmd": "search_next", "op": "eq"}));
    assert_eq!(e["ok"], false, "eq needs a value");
    assert_eq!(
        c.ok(json!({"cmd": "search_results", "max": 1}))["total"],
        left,
        "a bad request keeps the search"
    );

    // Freeze it: the program keeps incrementing, the value stays.
    let f = c.ok(json!({"cmd": "freeze_add", "addr": 0x0401, "value": 7, "label": "Lives"}));
    c.ok(json!({"cmd": "subscribe", "events": ["freeze_blocked_write"]}));
    c.ok(json!({"cmd": "resume"}));
    let ev = c.event("freeze_blocked_write");
    assert_eq!(
        (ev["addr"].clone(), ev["value"].clone()),
        (json!(0x0401), json!(8))
    );
    c.ok(json!({"cmd": "pause"}));
    let p = c.ok(json!({"cmd": "peek", "addr": 0x0400, "len": 3}));
    assert_eq!(p["bytes"][1], 7);
    assert_eq!(p["frozen"], json!([1]), "peek marks frozen bytes");

    let list = c.ok(json!({"cmd": "freeze_list"}));
    assert_eq!(list[0]["label"], "Lives");
    assert_eq!(list[0]["mode"], "block_writes");
    let state = c.ok(json!({"cmd": "save_state"}));

    // Cheat file round trip.
    let saved = c.ok(json!({"cmd": "cheats_save"}));
    let text = saved["text"].as_str().unwrap().to_string();
    assert!(
        text.contains("[[freeze]]") && text.contains("addr = 1025"),
        "{text}"
    );
    c.ok(json!({"cmd": "freeze_remove", "freeze": f["freeze"]}));
    assert_eq!(c.ok(json!({"cmd": "freeze_list"})), json!([]));
    let r = c.ok(json!({"cmd": "cheats_load", "text": text}));
    assert_eq!(r["freezes"].as_array().unwrap().len(), 1);
    let id = c.ok(json!({"cmd": "freeze_list"}))[0]["freeze"].clone();
    assert_eq!(
        c.ok(json!({"cmd": "freeze_toggle", "freeze": id}))["enabled"],
        false
    );
    let e = c.call(json!({"cmd": "cheats_load", "text": "[[freeze]]\nspace = \"nowhere\"\naddr = 1\nvalue = 1\n"}));
    assert_eq!(e["ok"], false);

    // A savestate brings its freezes back.
    c.ok(json!({"cmd": "load_state", "data": state["data"], "freezes": state["freezes"]}));
    let list = c.ok(json!({"cmd": "freeze_list"}));
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["enabled"], true);
    server.shutdown();
}
