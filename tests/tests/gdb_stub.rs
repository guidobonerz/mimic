//! GDB remote protocol stub, end to end over TCP: the packet sequence GDB
//! sends when it connects (qSupported, target.xml, ?, g, m), breakpoints,
//! watchpoints, stepping, Ctrl-C, the monitor and the mimic picture
//! extension (`qMimic.frame`, `qMimic.beam`, `beam:` in stop replies).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use emu_core::registry::MachineConfig;
use emu_debug::gdb;
use emu_debug::{DebugServer, ServerConfig, Session};

struct Gdb {
    stream: TcpStream,
    buf: Vec<u8>,
}

impl Gdb {
    fn connect(server: &DebugServer) -> Self {
        let stream = TcpStream::connect(server.gdb_addr().unwrap()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        Self {
            stream,
            buf: Vec::new(),
        }
    }

    fn send_raw(&mut self, data: &[u8]) {
        self.stream.write_all(data).unwrap();
    }

    /// The next packet's payload (acks skipped).
    fn recv(&mut self) -> Vec<u8> {
        loop {
            match gdb::parse(&self.buf).unwrap() {
                Some((gdb::Packet::Data(d), used)) => {
                    self.buf.drain(..used);
                    self.send_raw(b"+");
                    return d;
                }
                Some((gdb::Packet::Corrupt, _)) => panic!("bad checksum from the stub"),
                Some((_, used)) => {
                    self.buf.drain(..used);
                }
                None => {
                    let mut chunk = [0u8; 65536];
                    let n = self.stream.read(&mut chunk).unwrap();
                    assert!(n > 0, "connection closed");
                    self.buf.extend_from_slice(&chunk[..n]);
                }
            }
        }
    }

    fn cmd(&mut self, text: &str) -> String {
        self.send_raw(&gdb::frame(text.as_bytes()));
        String::from_utf8_lossy(&self.recv()).into_owned()
    }

    fn cmd_bytes(&mut self, text: &str) -> Vec<u8> {
        self.send_raw(&gdb::frame(text.as_bytes()));
        self.recv()
    }
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn server(machine: &str) -> DebugServer {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    let m = emu_machines::registry().create(machine, &config).unwrap();
    DebugServer::start(
        Session::new(m, machine),
        ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            vice: None,
            gdb: Some("127.0.0.1:0".parse().unwrap()),
            run: false,
            realtime: false,
        },
    )
    .unwrap()
}

/// The 32-bit big-endian register `n` of a 68000 `g` reply.
fn m68k_reg(g: &str, n: usize) -> u32 {
    u32::from_str_radix(&g[n * 8..n * 8 + 8], 16).unwrap()
}

#[test]
fn gdb_debugs_the_amiga_with_its_picture() {
    let server = server("amiga");
    let mut g = Gdb::connect(&server);

    let sup = g.cmd("qSupported:multiprocess+;swbreak+;hwbreak+;xmlRegisters=i386");
    for feature in ["qXfer:features:read+", "mimic-frame+", "QStartNoAckMode+"] {
        assert!(sup.contains(feature), "{feature} in {sup}");
    }
    assert_eq!(g.cmd("QStartNoAckMode"), "OK");
    let xml = g.cmd("qXfer:features:read:target.xml:0,ffb");
    assert!(xml.starts_with('l'), "whole description in one reply");
    assert!(xml.contains("<architecture>m68k</architecture>"));
    assert!(xml.contains("org.gnu.gdb.m68k.core"));
    assert_eq!(g.cmd("qfThreadInfo"), "m1");
    assert_eq!(g.cmd("qsThreadInfo"), "l");
    assert_eq!(g.cmd("qAttached"), "1");

    let stop = g.cmd("?");
    assert!(stop.starts_with("T05thread:1;"), "{stop}");

    // 18 core + 2 stack registers, 32 bits each; PC at the reset vector.
    let regs = g.cmd("g");
    assert_eq!(regs.len(), 20 * 8);
    assert_eq!(m68k_reg(&regs, 17), 0x00f8_0008);
    assert_eq!(g.cmd("p11"), "00f80008");
    assert_eq!(g.cmd("mf80008,4"), "13fc0003");
    assert_eq!(g.cmd("mfffffff,4"), "E14", "beyond 24 address bits");

    // Ctrl-C stops a free run with SIGINT.
    g.send_raw(&gdb::frame(b"c"));
    std::thread::sleep(Duration::from_millis(50));
    g.send_raw(&[0x03]);
    let stop = String::from_utf8(g.recv()).unwrap();
    assert!(stop.starts_with("T02thread:1;"), "{stop}");
    assert!(stop.contains(";beam:"), "{stop}");

    // The vertical blank handler at $F80100 runs every field.
    assert_eq!(g.cmd("Z0,f80100,2"), "OK");
    let stop = g.cmd("c");
    assert!(stop.starts_with("T05thread:1;swbreak:;beam:"), "{stop}");
    assert_eq!(m68k_reg(&g.cmd("g"), 17), 0x00f8_0100);
    assert_eq!(g.cmd("z0,f80100,2"), "OK");

    // A step moves on by one instruction (addq.l #1,$100.w is 4 bytes).
    let stop = g.cmd("s");
    assert!(stop.starts_with("T05"), "{stop}");
    assert_eq!(g.cmd("p11"), "00f80104");

    // The handler counts fields at $100: a write watchpoint catches it.
    assert_eq!(g.cmd("Z2,100,4"), "OK");
    let stop = g.cmd("vCont;c");
    assert!(stop.contains(";watch:10"), "{stop}");
    assert_eq!(g.cmd("z2,100,4"), "OK");

    // Memory writes are debugger pokes; X carries escaped binary data.
    assert_eq!(g.cmd("M2000,2:1234"), "OK");
    assert_eq!(g.cmd("m2000,2"), "1234");
    let mut x = b"X2000,2:".to_vec();
    x.extend(gdb::escape(&[b'#', 0x7d]));
    g.send_raw(&gdb::frame(&x));
    assert_eq!(g.recv(), b"OK");
    assert_eq!(g.cmd("m2000,2"), "237d");

    // The picture: an escaped PNG of the current frame.
    let png = gdb::unescape(&g.cmd_bytes("qMimic.frame"));
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    let qoi = gdb::unescape(&g.cmd_bytes("qMimic.frame:format=qoi;scale=2"));
    assert_eq!(&qoi[..4], b"qoif");
    assert_eq!(g.cmd("qMimic.frame:scale=99"), "E01");
    let beam = g.cmd("qMimic.beam");
    assert_eq!(beam.split(',').count(), 3, "{beam}");

    // monitor (qRcmd) answers in hex.
    let out = unhex(&g.cmd(&format!("qRcmd,{}", hex("beam"))));
    assert!(String::from_utf8(out).unwrap().starts_with("frame "));
    let out = unhex(&g.cmd(&format!("qRcmd,{}", hex("devices"))));
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("thread 1: cpu"), "{out}");
    assert!(out.contains("(not a thread): copper"), "{out}");
    let file = std::env::temp_dir().join(format!("mimic-gdb-{}.png", std::process::id()));
    let out = unhex(&g.cmd(&format!(
        "qRcmd,{}",
        hex(&format!("snapshot {}", file.display()))
    )));
    assert!(String::from_utf8(out).unwrap().starts_with("wrote "));
    let written = std::fs::read(&file).unwrap();
    let _ = std::fs::remove_file(&file);
    assert_eq!(&written[..4], b"\x89PNG");
    // No preview server in a bare session: a message, not an error.
    let out = unhex(&g.cmd(&format!("qRcmd,{}", hex("preview"))));
    assert!(!out.is_empty());

    assert_eq!(g.cmd("D"), "OK");
    server.shutdown();
}

#[test]
fn gdb_sees_the_6502_and_a_corrupt_packet_is_refused() {
    let server = server("sim6502");
    let mut g = Gdb::connect(&server);
    // Wrong checksum: the stub asks again with '-'.
    g.send_raw(b"$g#00");
    let mut b = [0u8; 1];
    g.stream.read_exact(&mut b).unwrap();
    assert_eq!(&b, b"-");
    let xml = g.cmd("qXfer:features:read:target.xml:0,ffb");
    assert!(xml.contains("org.mimic.6502"), "{xml}");
    // a x y s p (8 bits) and pc (16 bits, little-endian).
    assert_eq!(g.cmd("g").len(), 7 * 2);
    assert!(g.cmd("?").starts_with("T05thread:1;"));
    assert_eq!(g.cmd("qC"), "QC1");
    assert_eq!(g.cmd("qThreadExtraInfo,1"), hex("cpu"));
    server.shutdown();
}
