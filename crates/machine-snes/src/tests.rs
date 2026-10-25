use super::*;
use emu_core::input::Dirs;
use emu_core::media::{Media, MediaBackend, MediaError};

struct Bytes(Vec<u8>);

impl MediaBackend for Bytes {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let o = offset as usize;
        buf.copy_from_slice(&self.0[o..o + buf.len()]);
        Ok(())
    }
    fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

/// A 32 KB LoROM image: `code` at $8000, `nmi` at $9000, `irq` at $9100,
/// `data` at $A000; native NMI/IRQ vectors.
fn lorom(code: &[u8], nmi: &[u8], irq: &[u8], data: &[u8]) -> Vec<u8> {
    let mut rom = vec![0xea; 0x8000];
    rom[..code.len()].copy_from_slice(code);
    rom[0x1000..0x1000 + nmi.len()].copy_from_slice(nmi);
    rom[0x1100..0x1100 + irq.len()].copy_from_slice(irq);
    rom[0x2000..0x2000 + data.len()].copy_from_slice(data);
    rom[0x7fc0..0x7fd5].copy_from_slice(b"MIMIC TEST           ");
    rom[0x7fd5] = 0x20;
    rom[0x7fd6..0x7fdc].copy_from_slice(&[0x00, 0x05, 0x00, 0x01, 0x00, 0x00]);
    rom[0x7fdc..0x7fe0].copy_from_slice(&[0xff, 0xff, 0x00, 0x00]);
    rom[0x7fea..0x7fec].copy_from_slice(&[0x00, 0x90]); // NMI (native)
    rom[0x7fee..0x7ff0].copy_from_slice(&[0x00, 0x91]); // IRQ (native)
    rom[0x7ffc..0x7ffe].copy_from_slice(&[0x00, 0x80]); // RESET
    rom
}

fn machine(options: &[(&str, &str)], model: &str, image: Vec<u8>) -> Snes {
    let mut config = MachineConfig {
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    let mut m = Snes::new(&config).unwrap();
    m.media_slot(SLOT_CARTRIDGE)
        .unwrap()
        .insert(Media {
            kind: MediaKind::Cartridge,
            name: "test.sfc".into(),
            backend: Box::new(Bytes(image)),
        })
        .unwrap();
    m
}

fn frames(m: &mut Snes, n: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

/// SEI; CLC; XCE (native mode, 8-bit registers); STZ $10, $12, $13 (the
/// work RAM powers up as $55).
const NATIVE: [u8; 9] = [0x78, 0x18, 0xfb, 0x64, 0x10, 0x64, 0x12, 0x64, 0x13];

#[test]
fn nmi_every_frame_and_frame_timing() {
    for (model, clocks) in [("ntsc", 357_366u64), ("pal", 425_568)] {
        let mut code = NATIVE.to_vec();
        code.extend([
            0xa9, 0x80, // LDA #$80
            0x8d, 0x00, 0x42, // STA $4200
            0x80, 0xfe, // BRA *
        ]);
        let nmi = [
            0xee, 0x10, 0x00, // INC $0010
            0xad, 0x10, 0x42, // LDA $4210
            0x40, // RTI
        ];
        let mut m = machine(&[], model, lorom(&code, &nmi, &[0x40], &[]));
        frames(&mut m, 3);
        let start = master_clocks(&m);
        frames(&mut m, 10);
        let per = (master_clocks(&m) - start) / 10;
        assert!(
            per.abs_diff(clocks) < 8,
            "{model}: {per} master clocks a frame"
        );
        let n = peek_cpu(&m, 0x7e_0010);
        assert!((11..=13).contains(&n), "{model}: {n} NMIs");
    }
}

#[test]
fn dma_copies_into_vram() {
    let mut code = NATIVE.to_vec();
    code.extend([
        0xa9, 0x80, 0x8d, 0x15, 0x21, // VMAIN = $80
        0x9c, 0x16, 0x21, 0x9c, 0x17, 0x21, // VMADD = 0
        0xa9, 0x01, 0x8d, 0x00, 0x43, // DMAP0 = 1
        0xa9, 0x18, 0x8d, 0x01, 0x43, // BBAD0 = $18
        0xa9, 0x00, 0x8d, 0x02, 0x43, // A1T0 = $A000
        0xa9, 0xa0, 0x8d, 0x03, 0x43, //
        0x9c, 0x04, 0x43, // A1B0 = 0
        0xa9, 0x20, 0x8d, 0x05, 0x43, // DAS0 = 32
        0x9c, 0x06, 0x43, //
        0xa9, 0x01, 0x8d, 0x0b, 0x42, // MDMAEN = 1
        0x85, 0x10, // STA $10 (done)
        0x80, 0xfe, // BRA *
    ]);
    let data: Vec<u8> = (0..32).collect();
    let mut m = machine(&[], "ntsc", lorom(&code, &[0x40], &[0x40], &data));
    frames(&mut m, 2);
    assert_eq!(peek_cpu(&m, 0x10), 1);
    let v = &ppu(&m).vram;
    assert_eq!(v[0], 0x0100);
    assert_eq!(v[15], 0x1f1e);
    assert_eq!(
        peek_cpu(&m, 0x4305) | peek_cpu(&m, 0x4306),
        0,
        "counter ran down"
    );
}

#[test]
fn v_irq_once_a_frame() {
    let mut code = NATIVE.to_vec();
    code.extend([
        0xa9, 0x64, 0x8d, 0x09, 0x42, // VTIME = 100
        0x9c, 0x0a, 0x42, //
        0xa9, 0x20, 0x8d, 0x00, 0x42, // NMITIMEN: V-IRQ
        0x58, // CLI
        0x80, 0xfe, // BRA *
    ]);
    let irq = [
        0xee, 0x12, 0x00, // INC $0012
        0xa5, 0x13, // LDA $13
        0xd0, 0x08, // BNE +8
        0xad, 0x37, 0x21, // LDA $2137 (latch H/V)
        0xad, 0x3d, 0x21, // LDA $213D
        0x85, 0x13, // STA $13
        0xad, 0x11, 0x42, // LDA $4211 (ack)
        0x40, // RTI
    ];
    let mut m = machine(&[], "ntsc", lorom(&code, &[0x40], &irq, &[]));
    frames(&mut m, 6);
    let n = peek_cpu(&m, 0x12);
    assert!((5..=7).contains(&n), "{n} IRQs in 6 frames");
    assert_eq!(peek_cpu(&m, 0x13), 100, "taken on line 100");
}

#[test]
fn automatic_joypad_read() {
    let mut code = NATIVE.to_vec();
    code.extend([
        0xa9, 0x01, 0x8d, 0x00, 0x42, // NMITIMEN: auto read
        0x80, 0xfe,
    ]);
    let mut m = machine(&[], "ntsc", lorom(&code, &[0x40], &[0x40], &[]));
    m.apply_input(TimedInput {
        at: Cycle(0),
        target: InputTarget::Port(PortId(0)),
        event: InputEvent::Digital {
            dirs: Dirs::RIGHT,
            buttons: 0b0000_0001, // B
        },
    });
    m.apply_input(TimedInput {
        at: Cycle(0),
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: MachineKey(keys::PAD2 + 3), // Start
            down: true,
        },
    });
    frames(&mut m, 3);
    let joy1 = u16::from(peek_cpu(&m, 0x4218)) | u16::from(peek_cpu(&m, 0x4219)) << 8;
    let joy2 = u16::from(peek_cpu(&m, 0x421a)) | u16::from(peek_cpu(&m, 0x421b)) << 8;
    assert_eq!(joy1, 0x8100, "B and Right");
    assert_eq!(joy2, 0x1000, "Start");
}

/// Upload a 6-byte SPC700 program with the boot loader's protocol; it
/// answers $5A in port 1.
fn upload_rom() -> Vec<u8> {
    let mut code = NATIVE.to_vec();
    code.extend([
        0xad, 0x40, 0x21, 0xc9, 0xaa, 0xd0, 0xf9, // wait $2140 = $AA
        0xad, 0x41, 0x21, 0xc9, 0xbb, 0xd0, 0xf9, // wait $2141 = $BB
        0xa9, 0x00, 0x8d, 0x42, 0x21, // address $0200
        0xa9, 0x02, 0x8d, 0x43, 0x21, //
        0xa9, 0x01, 0x8d, 0x41, 0x21, // command: transfer
        0xa9, 0xcc, 0x8d, 0x40, 0x21, // kick $CC
        0xcd, 0x40, 0x21, 0xd0, 0xfb, // wait for the echo
        0xa2, 0x00, // LDX #0
        0xbd, 0x00, 0xa0, // lp: LDA $A000,X
        0x8d, 0x41, 0x21, // STA $2141
        0x8a, // TXA
        0x8d, 0x40, 0x21, // STA $2140
        0xcd, 0x40, 0x21, 0xd0, 0xfb, // wait for the echo
        0xe8, // INX
        0xe0, 0x06, // CPX #6
        0xd0, 0xec, // BNE lp
        0xa9, 0x00, 0x8d, 0x42, 0x21, // entry $0200
        0xa9, 0x02, 0x8d, 0x43, 0x21, //
        0xa9, 0x00, 0x8d, 0x41, 0x21, // command: entry
        0x8a, 0x1a, 0x1a, // A = 6 + 2
        0x8d, 0x40, 0x21, //
        0xcd, 0x40, 0x21, 0xd0, 0xfb, // wait for the echo
        0xad, 0x41, 0x21, 0xc9, 0x5a, 0xd0, 0xf9, // wait $2141 = $5A
        0x85, 0x10, // STA $10
        0x80, 0xfe,
    ]);
    // SPC700: MOV A,#$5A; MOV $F5,A; BRA *
    let spc = [0xe8, 0x5a, 0xc4, 0xf5, 0x2f, 0xfe];
    lorom(&code, &[0x40], &[0x40], &spc)
}

#[test]
fn sound_program_uploads_through_the_stub_loader() {
    let mut m = machine(&[], "ntsc", upload_rom());
    frames(&mut m, 20);
    assert_eq!(peek_cpu(&m, 0x10), 0x5a, "the SPC700 program ran");
    assert_eq!(
        m.board.apu.smp.ram[0x200..0x206],
        [0xe8, 0x5a, 0xc4, 0xf5, 0x2f, 0xfe]
    );
}

#[test]
fn audio_comes_out_at_48_khz_stereo() {
    let mut m = machine(&[], "ntsc", upload_rom());
    frames(&mut m, 10);
    let mut out = Vec::new();
    m.audio(&mut out);
    let start = master_clocks(&m);
    frames(&mut m, 60);
    out.clear();
    let fmt = m.audio(&mut out);
    assert_eq!((fmt.sample_rate, fmt.channels), (48_000, 2));
    let secs = (master_clocks(&m) - start) as f64 / MASTER_HZ_NTSC as f64;
    let rate = out.len() as f64 / 2.0 / secs;
    assert!((rate - 48_000.0).abs() < 300.0, "{rate} samples a second");
}

#[test]
fn savestate_round_trip_and_determinism() {
    let mut m = machine(&[("port2", "mouse")], "ntsc", upload_rom());
    frames(&mut m, 3);
    for _ in 0..777 {
        m.step_cycle();
    }
    let state = m.save_state();
    let mut copy = machine(&[("port2", "mouse")], "ntsc", upload_rom());
    frames(&mut copy, 1);
    copy.load_state(&state).unwrap();
    frames(&mut m, 4);
    frames(&mut copy, 4);
    assert_eq!(m.save_state(), copy.save_state());
    let mut other = machine(&[], "pal", upload_rom());
    assert!(other.load_state(&state).is_err());
    assert!(copy.load_state(&state[..state.len() / 2]).is_err());
}

#[test]
fn options_and_debugger() {
    let bad = |k: &str, v: &str| {
        let mut c = MachineConfig::default();
        c.options.insert(k.into(), v.into());
        Snes::new(&c).is_err()
    };
    assert!(bad("port1", "superscope"));
    assert!(bad("port2", "lightgun"));
    assert!(bad("multitap", "maybe"));
    let mut m = machine(&[("multitap", "on")], "ntsc", upload_rom());
    assert_eq!(m.ports().len(), 5);
    frames(&mut m, 2);
    assert_eq!(m.devices(), vec![CPU, SPC]);
    assert!(m.poke(VRAM_SPACE, 0x10, Size::Word, 0xbeef));
    assert_eq!(m.peek(VRAM_SPACE, 0x10, Size::Word), Some(0xbeef));
    assert_eq!(m.disassemble(0x00_8000).mnemonic, "SEI");
    let spc = m.debug_target(SPC).unwrap();
    assert_eq!(spc.peek(ARAM_SPACE, 0x200, Size::Byte), Some(0xe8));
}

#[test]
fn gfx_views_show_the_ppu_memories() {
    let mut m = machine(&[], "ntsc", upload_rom());
    frames(&mut m, 2);
    let ids: Vec<_> = m.gfx_views().iter().map(|v| v.id).collect();
    assert_eq!(ids, ["tiles", "screen", "sprites", "palette"]);
    // A 2 bpp tile at VRAM byte $4000 with its top-left pixel colour 1.
    assert!(m.poke(VRAM_SPACE, 0x4000, Size::Byte, 0x80));
    let state = m.save_state();
    let opts = GfxOptions::new()
        .with("bpp", 2)
        .with("addr", 0x4000)
        .with("count", 1);
    let t = m.gfx_view("tiles", &opts).unwrap();
    assert_eq!((t.space, t.addr, t.raw.len()), (VRAM_SPACE, 0x4000, 16));
    assert_eq!(t.raw[0], 0x80);
    assert_ne!(t.frame().rgb_at(0, 0), t.frame().rgb_at(1, 0));
    for id in ["screen", "sprites", "palette"] {
        let v = m.gfx_view(id, &GfxOptions::new()).unwrap();
        assert!(v.width > 0 && !v.raw.is_empty(), "{id}");
    }
    assert_eq!(
        m.gfx_view("palette", &GfxOptions::new()).unwrap().space,
        CGRAM_SPACE
    );
    assert_eq!(m.save_state(), state, "views leave the machine as it was");
}

#[test]
fn overlays_show_hdma_lines_interrupts_and_sprites() {
    use emu_core::overlay::{colors, Shape};
    let mut m = machine(&[], "ntsc", upload_rom());
    frames(&mut m, 2);
    let ids: Vec<_> = m.overlays().iter().map(|o| o.id).collect();
    assert_eq!(ids, ["sprites", "hdma", "irq"]);
    // HDMA channel 2, one byte to CGADD ($2121), table at $7E:1000: one
    // transfer then 2 idle lines, then two transfers (repeat mode), end.
    m.board.wram[0x1000..0x1006].copy_from_slice(&[0x03, 0x0f, 0x82, 0x0a, 0x0b, 0x00]);
    let d = &mut m.board.dma[2];
    (d.dmap, d.bbad, d.a1t, d.a1b) = (0x00, 0x21, 0x1000, 0x7e);
    m.board.hdmaen = 0x04;
    // V-IRQ at line 100 and the NMI; object 3 at X = 60, Y = 49.
    (m.board.nmitimen, m.board.vtime) = (0xa0, 100);
    m.board.ppu.write(0x00, 0x0f);
    for i in 0..128 {
        m.board.ppu.oam[i * 4 + 1] = 0xf0;
    }
    m.board.ppu.oam[12..14].copy_from_slice(&[60, 49]);
    let state = m.save_state();
    frames(&mut m, 1);
    assert!(m.overlay("hdma").is_none(), "switched off");
    m.load_state(&state).unwrap();
    m.set_overlays(&["sprites", "hdma", "irq"]);
    frames(&mut m, 1);
    let band = |y: i32| Shape::Band {
        y,
        h: 1,
        color: colors::SERIES[2],
        label: Some("ch2 $2121".into()),
    };
    let hdma = m.overlay("hdma").unwrap().shapes;
    assert_eq!(hdma, [band(0), band(3), band(4)], "lines 1, 4 and 5");
    let irq = m.overlay("irq").unwrap().shapes;
    let labels: Vec<_> = irq
        .iter()
        .filter_map(|s| match s {
            Shape::Mark { label, .. } => label.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(labels, ["IRQ 100:2", "NMI 225:0"]);
    assert!(matches!(irq[0], Shape::Mark { x: 0, y: 99, .. }));
    let spr = m.overlay("sprites").unwrap().shapes;
    assert!(
        spr.contains(&Shape::Rect {
            x: 120,
            y: 49,
            w: 16,
            h: 8,
            color: colors::SERIES[3],
            label: Some("3".into()),
        }),
        "{spr:?}"
    );
    assert_eq!(m.frame_row(1), Some(0));
    assert_eq!(m.frame_row(0), None);
    // Recording changes nothing in the emulation.
    let on = m.save_state();
    m.load_state(&state).unwrap();
    m.set_overlays(&[]);
    frames(&mut m, 1);
    assert_eq!(m.save_state(), on);
    assert!(m.overlay("irq").is_none());
}

#[test]
fn beam_hpos_is_the_frame_column() {
    let mut m = machine(&[], "ntsc", upload_rom());
    frames(&mut m, 2);
    // Object 3 at X = 60, Y = 49: tile 0, its left pixel colour 1 (red).
    let p = &mut m.board.ppu;
    p.write(0x00, 0x0f);
    p.write(0x01, 0x00);
    p.write(0x2c, 0x10); // OBJ on the main screen
    for i in 0..128 {
        p.oam[i * 4 + 1] = 0xf0;
    }
    p.oam[12..16].copy_from_slice(&[60, 49, 0, 0x30]);
    p.vram[0] = 0x0080;
    p.cgram[129] = 0x001f;
    frames(&mut m, 1);
    let row = m.frame_row(50).unwrap();
    let f = m.framebuffer();
    let red = f.rgb_at(120, row);
    assert_ne!(red, f.rgb_at(119, row));
    assert_eq!(red, f.rgb_at(121, row), "two columns a pixel");
    // The dot of picture pixel 60 reports its frame column.
    m.board.h = (22 + 60) * 4;
    assert_eq!(m.beam().unwrap().hpos, 120);
    m.board.h = 0;
    assert_eq!(m.beam().unwrap().hpos, 0);
    m.board.h = 1300;
    assert_eq!(m.beam().unwrap().hpos, 511);
}

/// Development aid: run a cartridge (EMU_CART) for EMU_FRAMES frames and
/// print the S-PPU's registers.
#[test]
#[ignore]
fn trace_cart() {
    let Some(cart) = std::env::var_os("EMU_CART") else {
        return;
    };
    let mut m = machine(&[("roms", "stub")], "ntsc", std::fs::read(cart).unwrap());
    let n: usize = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600);
    frames(&mut m, n);
    if let Ok(b) = std::env::var("EMU_PRESS") {
        // Hold these pad buttons (bit 3 = Start) for 10 frames, run on,
        // and write the picture as a PPM.
        let buttons: u8 = b.parse().unwrap();
        let at = m.now();
        m.input(TimedInput {
            at,
            target: InputTarget::Port(PortId(0)),
            event: InputEvent::Digital {
                dirs: Dirs(0),
                buttons,
            },
        });
        for _ in 0..10 {
            frames(&mut m, 1);
            println!(
                "pad regs {:02x?} game held {:02x?} new {:02x?}",
                m.board.joy,
                &m.board.wram[0x2b7..0x2bb],
                &m.board.wram[0x2bb..0x2bf]
            );
        }
        let at = m.now();
        m.input(TimedInput {
            at,
            target: InputTarget::Port(PortId(0)),
            event: InputEvent::Digital {
                dirs: Dirs(0),
                buttons: 0,
            },
        });
        let after: usize = std::env::var("EMU_AFTER")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300);
        frames(&mut m, after);
        if let Ok(d) = std::env::var("EMU_THEN_RIGHT") {
            let hold: usize = d.parse().unwrap();
            let at = m.now();
            m.input(TimedInput {
                at,
                target: InputTarget::Port(PortId(0)),
                event: InputEvent::Digital {
                    dirs: Dirs::RIGHT,
                    buttons: 0,
                },
            });
            frames(&mut m, hold);
        }
        let f = m.framebuffer();
        let mut ppm = format!(
            "P6 {} {} 255
",
            f.width, f.height
        )
        .into_bytes();
        for y in 0..f.height {
            for x in 0..f.width {
                let c = f.rgb_at(x, y).unwrap_or_default();
                ppm.extend_from_slice(&[c.r, c.g, c.b]);
            }
        }
        std::fs::write("target/tmp/press.ppm", ppm).unwrap();
        println!("wrote target/tmp/press.ppm");
        return;
    }
    if std::env::var_os("EMU_HDMA_LINES").is_some() {
        let mut last = (u16::MAX, 0u16, 0u8, false);
        for _ in 0..200_000 {
            m.step_cycle();
            let b = m.beam().unwrap();
            let d = &m.board.dma[4];
            let now = (b.line, d.a2a, d.ntrl, d.terminated);
            if std::env::var_os("EMU_HDMAEN_ONLY").is_some() {
                if m.board.hdmaen != last.2 {
                    println!(
                        "E L{:3} c{:4} hdmaen {:02x} pc {:06x}",
                        b.line,
                        b.cycle,
                        m.board.hdmaen,
                        m.cpu.pc24()
                    );
                    last.2 = m.board.hdmaen;
                }
                continue;
            }
            if now.1 != last.1
                || now.2 != last.2
                || now.3 != last.3
                || (b.line < 3 && b.line != last.0)
            {
                println!(
                    "H L{:3} c{:4} a2a {:04x} ntrl {:02x} term {} hdmaen {:02x} bgmode {:x}",
                    b.line,
                    b.cycle,
                    d.a2a,
                    d.ntrl,
                    d.terminated,
                    m.board.hdmaen,
                    m.board.ppu.bgmode
                );
            }
            last = now;
        }
        return;
    }
    if std::env::var_os("EMU_PPU_LINES").is_some() {
        // Register changes during one frame, by raster line.
        let mut last = m.board.ppu.debug_regs();
        let start = m.beam().map_or(0, |b| b.frame);
        while m.beam().map_or(0, |b| b.frame) == start {
            m.step_cycle();
            let now = m.board.ppu.debug_regs();
            let changed: Vec<String> = now
                .iter()
                .zip(&last)
                .filter(|(a, b)| a.1 != b.1 && a.0 != "VMADD")
                .map(|(a, _)| format!("{}={:x}", a.0, a.1))
                .collect();
            if !changed.is_empty() {
                let b = m.beam().unwrap();
                println!("L{:3} c{:4} {}", b.line, b.cycle, changed.join(" "));
            }
            last = now;
        }
        return;
    }
    println!("wram 1f50: {:02x?}", &m.board.wram[0x1f50..0x1f90]);
    if let Ok(base) = std::env::var("EMU_MAP") {
        let base = usize::from_str_radix(&base, 16).unwrap();
        let v = m.board.ppu.vram_words();
        for row in 0..28 {
            let line: Vec<String> = (0..32)
                .map(|c| format!("{:03x}", v[(base + row * 32 + c) & 0x7fff] & 0x3ff))
                .collect();
            println!("map {row:2}: {}", line.join(" "));
        }
    }
    for (i, d) in m.board.dma.iter().enumerate() {
        println!(
            "dma{i}: dmap {:02x} bbad {:02x} a1 {:02x}:{:04x} das {:02x}:{:04x} a2a {:04x} ntrl {:02x}",
            d.dmap, d.bbad, d.a1b, d.a1t, d.dasb, d.das, d.a2a, d.ntrl
        );
    }
    for c in m.chips() {
        if std::env::var_os("EMU_ALL_CHIPS").is_some() || c.name.contains("ppu") {
            let regs: Vec<String> = c
                .regs
                .iter()
                .map(|r| format!("{}={:x}", r.name, r.value))
                .collect();
            println!("{}: {}", c.name, regs.join(" "));
        }
    }
}
