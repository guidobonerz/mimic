use super::*;
use chips_ultimate::NullC64;

/// The user's update file: `MIMIC_U64_UPDATE`, a `.ue2` or `.cfw`.
fn update_file() -> Option<Vec<u8>> {
    let path = std::env::var("MIMIC_U64_UPDATE").ok()?;
    std::fs::read(path).ok()
}

fn print_console(b: &mut Elite2) {
    let text = b.take_console();
    print!("{}", String::from_utf8_lossy(&text).replace('\r', ""));
}

/// A tiny program: count in a register, write a byte to the console.
#[test]
fn runs_a_program_and_prints() {
    let mut b = Elite2::new(Kind::Elite2);
    let mut c64 = NullC64::default();
    // lui a0,0x10000; li a1,'A'; sb a1,0x10(a0); j .
    let code: [u32; 4] = [0x1000_0537, 0x0410_0593, 0x00b5_0823, 0x0000_006f];
    let mut rec = Vec::new();
    rec.extend(0x3_0000u32.to_le_bytes());
    rec.extend(16u32.to_le_bytes());
    rec.extend(0x3_0000u32.to_le_bytes());
    for w in code {
        rec.extend(w.to_le_bytes());
    }
    b.power_on(Some(&rec), &mut c64);
    assert_eq!(b.halt(), Halt::Running);
    b.run(
        1000,
        &mut c64,
        &mut chips_ultimate::MemStorage::default(),
        &mut chips_ultimate::MemLink::default(),
    );
    assert_eq!(b.take_console(), b"A");
    assert_eq!(b.cpu.pc(), 0x3_000c);
}

#[test]
fn an_empty_flash_does_not_start() {
    let mut b = Elite2::new(Kind::C64Ultimate);
    let mut c64 = NullC64::default();
    b.power_on(None, &mut c64);
    assert_eq!(b.halt(), Halt::NoFirmware);
    assert_eq!(
        b.run(
            1_000_000,
            &mut c64,
            &mut chips_ultimate::MemStorage::default(),
            &mut chips_ultimate::MemLink::default()
        ),
        None
    );
    assert_eq!(b.now(), 1_000_000);
}

#[test]
fn the_commodore_board_is_told_apart() {
    let mut c64 = NullC64::default();
    let read = |b: &mut Elite2, c64: &mut NullC64, addr: u32| b.io_access(addr, None, c64);
    let mut e = Elite2::new(Kind::Elite2);
    assert_eq!(read(&mut e, &mut c64, 0x10_0407), 1);
    assert_eq!(read(&mut e, &mut c64, 0x10_0802) & 4, 0);
    let mut c = Elite2::new(Kind::C64Ultimate);
    assert_eq!(read(&mut c, &mut c64, 0x10_0407), 0);
    assert_eq!(read(&mut c, &mut c64, 0x10_0802) & 4, 4);
    // Power-on and a loaded state keep it.
    c.power_on(None, &mut c64);
    let mut w = StateWriter::new();
    c.save(&mut w);
    let bytes = w.into_bytes();
    let mut d = Elite2::new(Kind::C64Ultimate);
    d.load(&mut StateReader::new(&bytes)).unwrap();
    assert_eq!(read(&mut d, &mut c64, 0x10_0407), 0);
    assert_eq!(read(&mut d, &mut c64, 0x10_0802) & 4, 4);
}

/// The updater of the user's file, started as the factory would.
#[test]
#[ignore]
fn updater_boots() {
    let Some(file) = update_file() else {
        eprintln!("MIMIC_U64_UPDATE not set");
        return;
    };
    let secs: u64 = std::env::var("MIMIC_U64_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let mut b = Elite2::new(if std::env::var("MIMIC_U64_C64U").is_ok() {
        Kind::C64Ultimate
    } else {
        Kind::Elite2
    });
    let mut c64 = NullC64::default();
    b.power_on(Some(&file), &mut c64);
    for _ in 0..secs * 100 {
        let req = b.run(
            1_000_000,
            &mut c64,
            &mut chips_ultimate::MemStorage::default(),
            &mut chips_ultimate::MemLink::default(),
        );
        print_console(&mut b);
        if let Some(r) = req {
            println!("request: {r:?}");
            break;
        }
    }
    println!(
        "pc {:08x}, {} instructions, halt {:?}",
        b.cpu.pc(),
        b.cpu.instructions(),
        b.halt()
    );
    for (n, row) in c64.mem[0x400..0x7e8].chunks(40).enumerate() {
        let line: String = row
            .iter()
            .map(|&c| match c & 0x7f {
                c @ 0x01..=0x1a => (c + 0x40) as char,
                c @ 0x20..=0x3f => c as char,
                _ => '.',
            })
            .collect();
        println!("{n:2} {line}");
    }
}
