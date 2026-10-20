//! Cartridges with real ROMs (user-supplied via `EMU_ROM_DIR`): CRT images
//! built here start through the C64 KERNAL (CBM80, Ultimax reset vector),
//! switch banks like their hardware and, on EasyFlash, program the flash
//! and write it back into the image.
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --release --test cartridges -- --ignored

use std::sync::{Arc, Mutex};

use emu_core::machine::Machine;
use emu_core::media::{Media, MediaBackend, MediaError, MediaKind, SlotId};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Size};
use media::crt::{hw, Chip, Crt, CrtMachine};

struct Shared(Arc<Mutex<Vec<u8>>>);

impl MediaBackend for Shared {
    fn len(&self) -> u64 {
        self.0.lock().unwrap().len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let d = self.0.lock().unwrap();
        buf.copy_from_slice(&d[offset as usize..offset as usize + buf.len()]);
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        let mut d = self.0.lock().unwrap();
        d[offset as usize..offset as usize + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

fn machine(id: &str) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, id).expect("ROMs"),
        ..MachineConfig::default()
    };
    config.options.insert("drive8".into(), "none".into());
    emu_machines::registry().create(id, &config).unwrap()
}

fn slot(m: &dyn Machine) -> SlotId {
    m.media_slots()
        .into_iter()
        .find(|s| s.name == "cartridge")
        .expect("cartridge slot")
        .id
}

/// Insert `image`, run `frames`, return the machine.
fn run_with(id: &str, image: &Arc<Mutex<Vec<u8>>>, frames: u32) -> Box<dyn Machine> {
    let mut m = machine(id);
    let s = slot(m.as_ref());
    m.media_slot(s)
        .unwrap()
        .insert(Media {
            name: "t.crt".into(),
            kind: MediaKind::Cartridge,
            backend: Box::new(Shared(image.clone())),
        })
        .unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..frames {
        m.run_frame(&mut dbg);
    }
    m
}

/// A RAM byte (the RAM space, whatever the banking).
fn ram(m: &mut dyn Machine, addr: u32) -> u8 {
    space_byte(m, "ram", addr)
}

fn space_byte(m: &mut dyn Machine, name: &str, addr: u32) -> u8 {
    let space = m
        .memory_spaces()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap()
        .id;
    m.debug_target(DeviceId("cpu"))
        .unwrap()
        .peek(space, addr, Size::Byte)
        .unwrap() as u8
}

/// "CBM80" at `$8004`, cold and warm start at `$8009`.
fn cbm80(code: &[u8]) -> Vec<u8> {
    let mut rom = vec![0u8; 0x2000];
    rom[..9].copy_from_slice(&[0x09, 0x80, 0x09, 0x80, 0xc3, 0xc2, 0xcd, 0x38, 0x30]);
    rom[9..9 + code.len()].copy_from_slice(code);
    rom
}

fn crt(hardware: u16, exrom: bool, game: bool, chips: Vec<Chip>) -> Arc<Mutex<Vec<u8>>> {
    let crt = Crt {
        machine: CrtMachine::C64,
        version: 0x0100,
        hardware,
        exrom,
        game,
        subtype: 0,
        name: "TEST".into(),
        chips,
    };
    Arc::new(Mutex::new(crt.to_bytes()))
}

fn chip(bank: u16, load: u16, data: Vec<u8>) -> Chip {
    Chip {
        kind: 0,
        bank,
        load,
        data,
    }
}

#[test]
#[ignore = "needs C64 ROMs in EMU_ROM_DIR"]
fn normal_8k_starts_with_cbm80() {
    // LDA #$42; STA $C000; JMP *
    let code = [0xa9, 0x42, 0x8d, 0x00, 0xc0, 0x4c, 0x0e, 0x80];
    let image = crt(hw::NORMAL, false, true, vec![chip(0, 0x8000, cbm80(&code))]);
    let mut m = run_with("c64", &image, 30);
    assert_eq!(ram(m.as_mut(), 0xc000), 0x42);
}

#[test]
#[ignore = "needs C64 ROMs in EMU_ROM_DIR"]
fn ultimax_starts_from_romh() {
    // At $E000: LDA #$43; STA $0800; JMP *; vectors at $FFFA.
    let mut romh = vec![0u8; 0x2000];
    romh[..8].copy_from_slice(&[0xa9, 0x43, 0x8d, 0x00, 0x08, 0x4c, 0x05, 0xe0]);
    romh[0x1ffa..].copy_from_slice(&[0x00, 0xe0, 0x00, 0xe0, 0x00, 0xe0]);
    let image = crt(hw::NORMAL, true, false, vec![chip(0, 0xe000, romh)]);
    let mut m = run_with("c64", &image, 10);
    assert_eq!(ram(m.as_mut(), 0x0800), 0x43);
}

/// Every bank holds the same code (switching under the running code is
/// harmless) and its number at `$9000`; the code selects bank 5 with
/// `select` and stores what `$9000` holds then at `$C000`.
fn banked(hardware: u16, select: &[u8], banks: u16, size: usize) {
    let mut code = select.to_vec();
    // LDA $9000; STA $C000; JMP *
    let jmp = 0x8009 + code.len() as u16 + 6;
    code.extend([0xad, 0x00, 0x90, 0x8d, 0x00, 0xc0, 0x4c]);
    code.extend(jmp.to_le_bytes());
    let chips = (0..banks)
        .map(|b| {
            let mut rom = cbm80(&code);
            rom[0x1000] = b as u8;
            rom.resize(size, b as u8);
            chip(b, 0x8000, rom)
        })
        .collect();
    let (exrom, game) = (false, size == 0x2000);
    let image = crt(hardware, exrom, game, chips);
    let mut m = run_with("c64", &image, 30);
    assert_eq!(ram(m.as_mut(), 0xc000), 5, "type {hardware}");
}

#[test]
#[ignore = "needs C64 ROMs in EMU_ROM_DIR"]
fn bank_switching_types() {
    // Ocean: STA $DE00 with the bank.
    banked(hw::OCEAN, &[0xa9, 0x05, 0x8d, 0x00, 0xde], 16, 0x2000);
    // System 3: a write to $DE00 + bank.
    banked(hw::SYSTEM_3, &[0x8d, 0x05, 0xde], 64, 0x2000);
    // Dinamic: a read of $DE00 + bank.
    banked(hw::DINAMIC, &[0xad, 0x05, 0xde], 16, 0x2000);
    // Magic Desk: STA $DE00.
    banked(hw::MAGIC_DESK, &[0xa9, 0x05, 0x8d, 0x00, 0xde], 32, 0x2000);
    // Fun Play: bank 5 = bits 3–5 = 5.
    banked(hw::FUN_PLAY, &[0xa9, 0x28, 0x8d, 0x00, 0xde], 16, 0x2000);
}

#[test]
#[ignore = "needs C64 ROMs in EMU_ROM_DIR"]
fn super_games_16k_banks() {
    // STA $DF00 with bank 1 (16 KB banks); marker in ROML at $9000.
    let code = [
        0xa9, 0x01, 0x8d, 0x00, 0xdf, // LDA #1; STA $DF00
        0xad, 0x00, 0x90, 0x8d, 0x00, 0xc0, // LDA $9000; STA $C000
        0x4c, 0x14, 0x80, // JMP *
    ];
    let chips = (0..4u16)
        .map(|b| {
            let mut rom = cbm80(&code);
            rom[0x1000] = 0x10 + b as u8;
            rom.resize(0x4000, 0);
            chip(b, 0x8000, rom)
        })
        .collect();
    let image = crt(hw::SUPER_GAMES, false, false, chips);
    let mut m = run_with("c64", &image, 30);
    assert_eq!(ram(m.as_mut(), 0xc000), 0x11);
}

#[test]
#[ignore = "needs C64 ROMs in EMU_ROM_DIR"]
fn easyflash_boots_programs_flash_and_writes_back() {
    let mut code = vec![];
    for (v, addr) in [
        (0x00u8, 0xde00u16),
        (0xaa, 0x8555),
        (0x55, 0x82aa),
        (0xa0, 0x8555),
        (0x01, 0xde00),
        (0x5a, 0x8010),
    ] {
        code.extend([0xa9, v, 0x8d]);
        code.extend(addr.to_le_bytes());
    }
    code.extend([0xad, 0x10, 0x80, 0x8d, 0x00, 0x08]); // LDA $8010; STA $0800
    code.extend([0xad, 0x20, 0x80, 0x8d, 0x01, 0x08]); // LDA $8020; STA $0801
    let jmp = 0xe000 + code.len() as u16;
    code.push(0x4c);
    code.extend(jmp.to_le_bytes());
    let mut crt = devices_cartridge::easyflash::blank_image(&[]);
    for c in &mut crt.chips {
        // The bank register switches ROMH too: the code is in banks 0 and 1
        // (real programs run such code from RAM).
        if c.bank <= 1 && c.load == 0xa000 {
            c.data[..code.len()].copy_from_slice(&code);
            c.data[0x1ffa..].copy_from_slice(&[0x00, 0xe0, 0x00, 0xe0, 0x00, 0xe0]);
        }
        if c.bank == 1 && c.load == 0x8000 {
            c.data[0x20] = 0x77;
        }
    }
    let image = Arc::new(Mutex::new(crt.to_bytes()));
    let mut m = run_with("c64", &image, 10);
    assert_eq!(ram(m.as_mut(), 0x0800), 0x5a, "programmed");
    assert_eq!(ram(m.as_mut(), 0x0801), 0x77, "bank 1");
    let s = slot(m.as_ref());
    m.media_slot(s).unwrap().eject().expect("cartridge");
    let back = Crt::parse(&image.lock().unwrap()).unwrap();
    assert_eq!(back.chip(1, 0x8000).unwrap().data[0x10], 0x5a);
    assert_eq!(back.chip(1, 0x8000).unwrap().data[0x11], 0xff);
}

#[test]
#[ignore = "needs C128 ROMs in EMU_ROM_DIR"]
fn c128_starts_c64_cartridges() {
    // The KERNAL sees GAME/EXROM in the MMU and switches to C64 mode.
    let code = [0xa9, 0x44, 0x8d, 0x00, 0xc0, 0x4c, 0x0e, 0x80];
    let image = crt(hw::NORMAL, false, true, vec![chip(0, 0x8000, cbm80(&code))]);
    let mut m = run_with("c128", &image, 60);
    assert_eq!(ram(m.as_mut(), 0xc000), 0x44, "8 KB");

    let mut romh = vec![0u8; 0x2000];
    romh[..8].copy_from_slice(&[0xa9, 0x45, 0x8d, 0x00, 0x08, 0x4c, 0x05, 0xe0]);
    romh[0x1ffa..].copy_from_slice(&[0x00, 0xe0, 0x00, 0xe0, 0x00, 0xe0]);
    let image = crt(hw::NORMAL, true, false, vec![chip(0, 0xe000, romh)]);
    let mut m = run_with("c128", &image, 60);
    assert_eq!(ram(m.as_mut(), 0x0800), 0x45, "Ultimax");
}

/// A VIC-20 cartridge image in its usual raw form: load address, then ROM.
fn vic20_cart(load: u16, rom: &[u8]) -> Arc<Mutex<Vec<u8>>> {
    let mut bytes = load.to_le_bytes().to_vec();
    bytes.extend(rom);
    Arc::new(Mutex::new(bytes))
}

#[test]
#[ignore = "needs VIC-20 ROMs in EMU_ROM_DIR"]
fn vic20_autostarts_a0cbm() {
    // $A000 cold start, $A002 warm start, "A0CBM" at $A004; then
    // LDA #$46; STA $1000; JMP *.
    let mut rom = vec![0u8; 0x2000];
    rom[..9].copy_from_slice(&[0x09, 0xa0, 0x09, 0xa0, 0x41, 0x30, 0xc3, 0xc2, 0xcd]);
    rom[9..17].copy_from_slice(&[0xa9, 0x46, 0x8d, 0x00, 0x10, 0x4c, 0x0e, 0xa0]);
    let image = vic20_cart(0xa000, &rom);
    let mut m = run_with("vic20", &image, 30);
    assert_eq!(space_byte(m.as_mut(), "cpu", 0x1000), 0x46);
    assert_eq!(space_byte(m.as_mut(), "cpu", 0xa004), 0x41, "ROM visible");
}

#[test]
#[ignore = "needs Plus/4 ROMs in EMU_ROM_DIR"]
fn plus4_starts_cartridge_c1() {
    // $8000: JMP $800A; module number 1 at $8006, "CBM" at $8007;
    // $800A: LDA #$47; STA $5000; JMP *.
    let mut rom = vec![0u8; 0x4000];
    rom[..3].copy_from_slice(&[0x4c, 0x0a, 0x80]);
    rom[6..10].copy_from_slice(&[0x01, 0x43, 0x42, 0x4d]);
    rom[10..18].copy_from_slice(&[0xa9, 0x47, 0x8d, 0x00, 0x50, 0x4c, 0x0f, 0x80]);
    let image = Arc::new(Mutex::new(rom));
    let mut m = run_with("plus4", &image, 30);
    assert_eq!(space_byte(m.as_mut(), "cpu", 0x5000), 0x47);
}

/// Save, run on, load, run on again: the same state both times (the
/// cartridge's registers and lines are part of the savestate).
fn round_trip(m: &mut dyn Machine) {
    let mut dbg = DebugHooks::new();
    let saved = m.save_state();
    for _ in 0..5 {
        m.run_frame(&mut dbg);
    }
    let first = m.save_state();
    m.load_state(&saved).unwrap();
    for _ in 0..5 {
        m.run_frame(&mut dbg);
    }
    assert!(m.save_state() == first, "savestate round trip");
}

#[test]
#[ignore = "needs C64, C128, VIC-20 and Plus/4 ROMs in EMU_ROM_DIR"]
fn savestates_with_cartridges() {
    let ocean = |id| {
        let chips = (0..16u16)
            .map(|b| chip(b, 0x8000, cbm80(&[0x8d, 0x03, 0xde, 0x4c, 0x09, 0x80])))
            .collect();
        let image = crt(hw::OCEAN, false, true, chips);
        run_with(id, &image, 30)
    };
    round_trip(ocean("c64").as_mut());
    round_trip(ocean("c128").as_mut());
    let image = devices_cartridge::easyflash::blank_image(&[]).to_bytes();
    let mut m = run_with("c64", &Arc::new(Mutex::new(image)), 10);
    round_trip(m.as_mut());

    let mut rom = vec![0u8; 0x2000];
    rom[..12].copy_from_slice(&[
        0x09, 0xa0, 0x09, 0xa0, 0x41, 0x30, 0xc3, 0xc2, 0xcd, 0x4c, 0x09, 0xa0,
    ]);
    round_trip(run_with("vic20", &vic20_cart(0xa000, &rom), 30).as_mut());
    let mut rom = vec![0u8; 0x4000];
    rom[..10].copy_from_slice(&[0x4c, 0x00, 0x80, 0, 0, 0, 0x01, 0x43, 0x42, 0x4d]);
    round_trip(run_with("plus4", &Arc::new(Mutex::new(rom)), 30).as_mut());

    // A state with a cartridge does not load into a machine without one.
    let saved = ocean("c64").save_state();
    assert!(machine("c64").load_state(&saved).is_err());
}
