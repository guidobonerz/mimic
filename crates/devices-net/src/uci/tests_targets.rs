//! The targets that reach the machine (control, DOS mount/REU, Software
//! IEC) and the HTTP target.

use std::sync::{Arc, Mutex};

use super::siec::{code, ftype};
use super::tests::{command, command_m, state, text, wait, CMD, STATUS};
use super::*;
use crate::testing::FakeNet;
use emu_core::host::{MemFileHost, SharedFiles, SharedNet};

/// A machine with 64 KB of RAM, an optional REU, drive 8 and a palette.
struct TestMachine {
    ram: Vec<u8>,
    reu: Option<Vec<u8>>,
    drives: Vec<UciDrive>,
    /// IEC number → (image name, bytes).
    mounted: BTreeMap<u8, (String, Vec<u8>)>,
    reboots: u32,
    palette: [u8; 48],
    erased: Vec<(u8, bool)>,
}

impl TestMachine {
    fn new() -> Self {
        Self {
            ram: (0..65536).map(|i| (i % 251) as u8).collect(),
            reu: Some(vec![0; 128 * 1024]),
            drives: vec![UciDrive {
                kind: drive_kind::C1541,
                address: 8,
                powered: true,
            }],
            mounted: BTreeMap::new(),
            reboots: 0,
            palette: [0x11; 48],
            erased: Vec::new(),
        }
    }
}

impl UciMachine for TestMachine {
    fn dma_read(&mut self, addr: u16) -> u8 {
        self.ram[usize::from(addr)]
    }
    fn dma_write(&mut self, addr: u16, v: u8) {
        self.ram[usize::from(addr)] = v;
    }
    fn ram(&self) -> Option<&[u8]> {
        Some(&self.ram)
    }
    fn reu(&mut self) -> Option<&mut [u8]> {
        self.reu.as_deref_mut()
    }
    fn drives(&self) -> Vec<UciDrive> {
        self.drives.clone()
    }
    fn set_drive_power(&mut self, index: usize, on: bool) -> bool {
        match self.drives.get_mut(index) {
            Some(d) => {
                d.powered = on;
                true
            }
            None => false,
        }
    }
    fn mount(&mut self, address: u8, mut media: Media) -> Result<(), MediaError> {
        if !media.name.ends_with(".d64") {
            return Err(MediaError::Format("not a D64".into()));
        }
        let mut data = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut data)?;
        // Writes reach the folder through the backend's commit.
        media.backend.write(0, b"W")?;
        media.backend.commit()?;
        self.mounted.insert(address, (media.name, data));
        Ok(())
    }
    fn unmount(&mut self, address: u8) -> bool {
        self.mounted.remove(&address).is_some()
    }
    fn reboot(&mut self) {
        self.reboots += 1;
    }
    fn erase_flash(&mut self, bank: u8, high: bool) -> bool {
        self.erased.push((bank, high));
        true
    }
    fn sids(&self) -> Vec<UciSid> {
        vec![UciSid {
            base: 0xd400,
            second: 0,
            kind: 1,
        }]
    }
    fn palette(&self) -> Option<[u8; 48]> {
        Some(self.palette)
    }
    fn set_palette(&mut self, rgb: &[u8; 48]) -> bool {
        self.palette = *rgb;
        true
    }
    fn reset_palette(&mut self) -> bool {
        self.palette = [0x11; 48];
        true
    }
}

/// A UCI on `files` and a FakeNet both kept for inspection.
fn setup(files: MemFileHost, net: FakeNet) -> (Uci, SharedFiles, Arc<Mutex<FakeNet>>) {
    let net = Arc::new(Mutex::new(net));
    let shared: SharedFiles = Arc::new(Mutex::new(files));
    let mut u = Uci::new();
    u.attach_host(HostServices {
        net: Some(net.clone() as SharedNet),
        files: Some(shared.clone()),
    });
    (u, shared, net)
}

fn file(files: &SharedFiles, path: &str) -> Vec<u8> {
    read_file(&mut *lock(files), path).unwrap()
}

fn with_name(head: &[u8], name: &str) -> Vec<u8> {
    let mut v = head.to_vec();
    v.extend(name.bytes());
    v
}

#[test]
fn control_target_reaches_the_machine() {
    let mut fs = MemFileHost::new();
    fs.insert("/image.reu", (0..200u8).collect()).unwrap();
    let (mut u, files, _) = setup(fs, FakeNet::new());
    let mut m = TestMachine::new();
    // REBOOT: no status, the machine resets.
    let (_, s) = command_m(&mut u, &mut m, &[4, 0x06]);
    assert_eq!((s.as_str(), m.reboots), ("", 1));
    // Drives: type, address, power.
    assert_eq!(command_m(&mut u, &mut m, &[4, 0x29, 0]).0[0], [1, 0, 8, 1]);
    assert_eq!(command_m(&mut u, &mut m, &[4, 0x31]).1, "00,OK");
    let (p, _) = command_m(&mut u, &mut m, &[4, 0x34]);
    assert_eq!(p[0], b"off");
    command_m(&mut u, &mut m, &[4, 0x30]);
    assert_eq!(command_m(&mut u, &mut m, &[4, 0x34]).0[0], b"on");
    assert_eq!(
        command_m(&mut u, &mut m, &[4, 0x32]).1,
        "90,DRIVE NOT PRESENT"
    );
    assert_eq!(command_m(&mut u, &mut m, &[4, 0x35]).0[0], b"off");
    // SIDs: base LSB first, second MSB first, type.
    assert_eq!(
        command_m(&mut u, &mut m, &[4, 0x28, 1]).0[0],
        [1, 0x00, 0xd4, 0, 0, 1]
    );
    // Palette.
    assert_eq!(command_m(&mut u, &mut m, &[4, 0x51]).0[0], [0x11; 48]);
    assert_eq!(
        command_m(&mut u, &mut m, &[4, 0x51, 0]).1,
        "81,INVALID PARAMS"
    );
    assert_eq!(
        command_m(&mut u, &mut m, &[4, 0x53, 2, 0xaa, 0xbb, 0xcc]).1,
        "00,OK"
    );
    assert_eq!(m.palette[6..9], [0xaa, 0xbb, 0xcc]);
    assert_eq!(
        command_m(&mut u, &mut m, &[4, 0x53, 16, 0, 0, 0]).1,
        "81,INVALID PARAMS"
    );
    let mut set = vec![4, 0x52];
    set.extend([0x22; 48]);
    assert_eq!(command_m(&mut u, &mut m, &set).1, "00,OK");
    assert_eq!(m.palette, [0x22; 48]);
    command_m(&mut u, &mut m, &[4, 0x54]);
    assert_eq!(m.palette, [0x11; 48]);
    // SAVEMEM to the default file, creating /temp.
    assert_eq!(command_m(&mut u, &mut m, &[4, 0x0f]).1, "00,OK");
    assert_eq!(file(&files, "/temp/c64_memory.bin"), m.ram);
    // EasyFlash erase: sub-command 0, bank, ROML/ROMH base.
    assert_eq!(command_m(&mut u, &mut m, &[4, 0x20, 0, 9, 0xa0]).1, "00,OK");
    assert_eq!(
        command_m(&mut u, &mut m, &[4, 0x20, 0, 9, 0xc0]).1,
        "81,INVALID PARAMS"
    );
    assert_eq!(m.erased, [(9, true)]);
    // REU images.
    let (p, s) = command_m(&mut u, &mut m, &with_name(&[4, 0x08], "image.reu"));
    assert_eq!(
        (p[0].as_slice(), s.as_str()),
        (&200i32.to_le_bytes()[..], "00,OK")
    );
    assert_eq!(m.reu.as_ref().unwrap()[199], 199);
    let (p, _) = command_m(&mut u, &mut m, &with_name(&[4, 0x09], "out.reu"));
    assert_eq!(p[0], (128 * 1024i32).to_le_bytes());
    assert_eq!(file(&files, "/out.reu").len(), 128 * 1024);
    let (p, s) = command_m(&mut u, &mut m, &with_name(&[4, 0x08], "missing.reu"));
    assert_eq!(
        (p[0].as_slice(), s.as_str()),
        (&(-1i32).to_le_bytes()[..], "85,REU FILE CANNOT BE OPENED.")
    );
    m.reu = None;
    assert_eq!(
        command_m(&mut u, &mut m, &with_name(&[4, 0x08], "image.reu")).1,
        "84,REU NOT ENABLED"
    );
    assert_eq!(command_m(&mut u, &mut m, &[4, 0x40]).0[0], [0; 8]);
    assert_eq!(
        command_m(&mut u, &mut m, &[4, 0x05]).1,
        "99,FUNCTION NOT IMPLEMENTED"
    );
}

#[test]
fn dos_target_moves_reu_data_and_mounts_disks() {
    let mut fs = MemFileHost::new();
    fs.insert("/data.bin", (0..100u8).collect()).unwrap();
    fs.insert("/disks/game-1.d64", vec![1; 10]).unwrap();
    fs.insert("/disks/game-2.d64", vec![2; 10]).unwrap();
    fs.insert("/disks/other.d64", vec![3; 10]).unwrap();
    fs.insert("/disks/notes.txt", vec![4; 10]).unwrap();
    let (mut u, files, _) = setup(fs, FakeNet::new());
    let mut m = TestMachine::new();
    command_m(&mut u, &mut m, &with_name(&[1, 0x02, 0x01], "data.bin"));
    // 100 bytes to REU $000010; the top bytes of address and length are
    // masked.
    let load = [1, 0x21, 0x10, 0, 0, 0xff, 0x64, 0, 0, 0xff];
    let (p, s) = command_m(&mut u, &mut m, &load);
    assert_eq!(
        String::from_utf8_lossy(&p[0]),
        "$000064 BYTES LOADED TO REU $000010"
    );
    assert_eq!(s, "00,OK");
    assert_eq!(
        m.reu.as_ref().unwrap()[0x10..0x10 + 100],
        (0..100u8).collect::<Vec<_>>()[..]
    );
    // Past the end of a 128 KB REU: truncated.
    command_m(&mut u, &mut m, &[1, 0x03]);
    command_m(&mut u, &mut m, &with_name(&[1, 0x02, 0x0e], "out.bin"));
    let save = [1, 0x22, 0xf0, 0xff, 0x01, 0, 0x20, 0, 0, 0];
    let (p, s) = command_m(&mut u, &mut m, &save);
    assert_eq!(
        String::from_utf8_lossy(&p[0]),
        "$000010 BYTES SAVED FROM REU $01FFF0"
    );
    assert_eq!(s, "02,REQUEST TRUNCATED");
    command_m(&mut u, &mut m, &[1, 0x03]);
    assert_eq!(file(&files, "/out.bin").len(), 16);
    // Mount, swap within the set, unmount.
    command_m(&mut u, &mut m, &with_name(&[1, 0x11], "disks"));
    assert_eq!(
        command_m(&mut u, &mut m, &with_name(&[1, 0x23, 8], "game-1.d64")).1,
        "00,OK"
    );
    assert_eq!(m.mounted[&8].1, [1; 10]);
    // The image's write reached the folder.
    assert_eq!(file(&files, "/disks/game-1.d64")[0], b'W');
    assert_eq!(
        command_m(&mut u, &mut m, &with_name(&[1, 0x23, 8], "notes.txt")).1,
        "89,NOT A DISK IMAGE"
    );
    // Unknown id: the drive mounted on last.
    assert_eq!(command_m(&mut u, &mut m, &[1, 0x25, 11]).1, "00,OK");
    assert_eq!(m.mounted[&8].0, "game-2.d64");
    command_m(&mut u, &mut m, &[1, 0x25, 8]);
    assert_eq!(m.mounted[&8].0, "game-1.d64");
    assert_eq!(command_m(&mut u, &mut m, &[1, 0x24, 8]).1, "00,OK");
    assert!(m.mounted.is_empty());
    m.reu = None;
    assert_eq!(command_m(&mut u, &mut m, &load).1, "85,NO FILE OPEN");
}

fn siec_status(s: &str) -> u8 {
    s.chars().next().map_or(0xff, |c| c as u32 as u8)
}

#[test]
fn software_iec_loads_saves_and_streams() {
    let mut fs = MemFileHost::new();
    let mut prg = vec![0x00, 0xc0];
    prg.extend(1..=10u8);
    fs.insert("/hello.prg", prg).unwrap();
    fs.insert("/text.seq", (0..100u8).collect()).unwrap();
    fs.insert("/games/zork.prg", vec![0x01, 0x08, 0xea])
        .unwrap();
    let (mut u, files, _) = setup(fs, FakeNet::new());
    let mut m = TestMachine::new();
    assert_eq!(text(&mut u, &[5, 0x01]).0, "SOFTWARE IEC TARGET V1.0");
    // LOAD"HELLO",8,1: the file's address.
    let (p, s) = command_m(&mut u, &mut m, &with_name(&[5, 0x10, 1, 0, 0, 0], "HEL*"));
    assert_eq!((p[0].as_slice(), siec_status(&s)), (&[0x00, 0xc0][..], 0));
    let (p, s) = command_m(&mut u, &mut m, &[5, 0x11, 1, 0]);
    assert_eq!(
        (p[0].as_slice(), siec_status(&s)),
        (&[0, 0x0a, 0xc0][..], 0)
    );
    assert_eq!(m.ram[0xc000..0xc00a], [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    // Verify: equal, then different.
    command_m(&mut u, &mut m, &with_name(&[5, 0x10, 1, 1, 0, 0], "hello"));
    assert_eq!(command_m(&mut u, &mut m, &[5, 0x11, 1, 1]).0[0][0], 0);
    m.ram[0xc005] = 0;
    command_m(&mut u, &mut m, &with_name(&[5, 0x10, 1, 1, 0, 0], "hello"));
    assert_eq!(command_m(&mut u, &mut m, &[5, 0x11, 1, 1]).0[0][0], 0x80);
    // LOAD"..",8 at the given address, with partition and folder.
    command_m(
        &mut u,
        &mut m,
        &with_name(&[5, 0x10, 0, 0, 0x00, 0x20], "0/games/:ZORK"),
    );
    command_m(&mut u, &mut m, &[5, 0x11, 0, 0]);
    assert_eq!(m.ram[0x2000], 0xea);
    assert_eq!(
        siec_status(&command_m(&mut u, &mut m, &with_name(&[5, 0x10, 0, 0, 0, 0], "NONE")).1),
        code::FILE_NOT_FOUND
    );
    // SAVE $C000–$C003 (end exclusive); again only with @.
    let save = with_name(&[5, 0x12, 0, 1, 0x00, 0xc0, 0x03, 0xc0], "OUT");
    assert_eq!(siec_status(&command_m(&mut u, &mut m, &save).1), 0);
    assert_eq!(file(&files, "/OUT.prg"), [0x00, 0xc0, 1, 2, 3]);
    assert_eq!(
        siec_status(&command_m(&mut u, &mut m, &save).1),
        code::SAVE_ERROR
    );
    let resave = with_name(&[5, 0x12, 0, 1, 0x00, 0xc0, 0x01, 0xc0], "@0:OUT");
    assert_eq!(siec_status(&command_m(&mut u, &mut m, &resave).1), 0);
    assert_eq!(file(&files, "/OUT.prg"), [0x00, 0xc0, 1]);
    // OPEN 2,8,2,"TEXT,S,R": CHKIN streams packets of 32, 64, then the rest.
    assert_eq!(
        siec_status(&command_m(&mut u, &mut m, &with_name(&[5, 0x13, 2, 0], "TEXT,S,R")).1),
        0
    );
    let (p, _) = command_m(&mut u, &mut m, &[5, 0x15, 2]);
    assert_eq!(p.iter().map(Vec::len).collect::<Vec<_>>(), [32, 64, 4]);
    assert_eq!(p.concat(), (0..100u8).collect::<Vec<_>>());
    // A command during the stream: the channel keeps the unread bytes.
    command_m(&mut u, &mut m, &[5, 0x14, 2]);
    command_m(&mut u, &mut m, &with_name(&[5, 0x13, 2, 0], "TEXT"));
    for &b in &[5, 0x15, 2] {
        u.write(CMD, b);
    }
    u.write(STATUS, ctrl::PUSH_CMD);
    wait(&mut u);
    for _ in 0..10 {
        u.read(super::tests::RESP);
    }
    // CHKIN again: the next byte is the 11th.
    for &b in &[5, 0x15, 2] {
        u.write(CMD, b);
    }
    u.write(STATUS, ctrl::PUSH_CMD);
    wait(&mut u);
    assert_eq!(u.peek(super::tests::RESP), 10);
    u.write(STATUS, ctrl::ABORT);
    wait(&mut u);
    while state(&u) != State::Idle as u8 {
        u.tick();
    }
    // CHKOUT: open by $F0+sa, write, close by $E0+sa.
    command_m(&mut u, &mut m, &with_name(&[5, 0x16, 0xf3, 0], "LOG,S,W"));
    command_m(&mut u, &mut m, &[5, 0x16, 3, 0, b'H', b'I']);
    command_m(&mut u, &mut m, &[5, 0x16, 3, 0, b'!']);
    command_m(&mut u, &mut m, &[5, 0x16, 0xe3]);
    assert_eq!(file(&files, "/LOG.seq"), b"HI!");
    assert_eq!(
        siec_status(&command_m(&mut u, &mut m, &[5, 0x15, 3]).1),
        code::NO_INPUT_CHANNEL
    );
    // Partitions and names.
    let add = with_name(&[5, 0x20, 3], "GAMES:/games");
    assert_eq!(siec_status(&command_m(&mut u, &mut m, &add).1), 0);
    assert_eq!(
        siec_status(&command_m(&mut u, &mut m, &[5, 0x20, 3, b'X']).1),
        code::INVALID_PARAMETERS
    );
    assert_eq!(
        text(&mut u, &with_name(&[5, 0x22, 0], "3:ZORK")).0,
        "/games/zork.prg"
    );
    assert_eq!(
        text(&mut u, &with_name(&[5, 0x22, 1], "3:NEW")).0,
        "/games/NEW.prg"
    );
    assert_eq!(
        text(&mut u, &with_name(&[5, 0x22, 2], "3:NEW,W")).0,
        "/games/NEW.seq"
    );
    assert_eq!(text(&mut u, &with_name(&[5, 0x22, 0], "$3:")).0, "/games");
    assert_eq!(
        text(&mut u, &with_name(&[5, 0x22, 0], "$=P")).0,
        "/partitions"
    );
    assert_eq!(
        siec_status(&command_m(&mut u, &mut m, &with_name(&[5, 0x22, 0], "7:X")).1),
        code::INVALID_PARTITION
    );
    let (p, _) = command(&mut u, &with_name(&[5, 0x23], "notes.seq"));
    assert_eq!(p[0], [&[ftype::SEQ][..], b"notes"].concat());
    command_m(&mut u, &mut m, &[5, 0x21, 3]);
    // LOAD"$",8: a BASIC listing at $0401.
    command_m(
        &mut u,
        &mut m,
        &with_name(&[5, 0x10, 0, 0, 0x01, 0x04], "$"),
    );
    let (p, _) = command_m(&mut u, &mut m, &[5, 0x11, 0, 0]);
    let end = usize::from(u16::from_le_bytes([p[0][1], p[0][2]]));
    let listing = &m.ram[0x0401..end];
    let words = String::from_utf8_lossy(listing);
    assert!(words.contains("\"hello\""), "{words}");
    assert!(words.contains("BLOCKS FREE."));
    assert!(!words.contains("text.seq"));
    // The first line's link points at the second line.
    let link = usize::from(u16::from_le_bytes([listing[0], listing[1]]));
    assert_eq!(listing[link - 0x0401 - 1], 0);
}

#[test]
fn http_target_builds_requests_and_reads_answers() {
    let net = FakeNet::new()
        .json_page(
            "http://api.test/user",
            r#"{"name":"Ada","cars":["VW",{"model":"Toyota"}],"age":36,"pi":3.14,"none":null}"#,
        )
        .page("http://api.test/big", &[7; 2000]);
    let (mut u, _, net) = setup(MemFileHost::new(), net);
    assert_eq!(text(&mut u, &[6, 0x01]).0, "ULTIMATE HTTP TARGET V1.0");
    assert_eq!(command(&mut u, &[6, 0x10]).1, "000 OK");
    // A header: verb, URL without scheme; fields replace by name.
    let (p, s) = command(&mut u, &with_name(&[6, 0x11, 3], "api.test/echo?x=1"));
    assert_eq!((p[0].as_slice(), s.as_str()), (&[0][..], "000 OK"));
    command(&mut u, &with_name(&[6, 0x13, 0], "X-Key: one"));
    command(&mut u, &with_name(&[6, 0x13, 0], "x-key: two"));
    assert_eq!(
        command(&mut u, &with_name(&[6, 0x13, 0], "bad")).1,
        "500 BAD FORMAT"
    );
    assert_eq!(text(&mut u, &with_name(&[6, 0x14, 0], "X-KEY")).0, "two");
    assert_eq!(
        command(&mut u, &with_name(&[6, 0x14, 0], "nope")).1,
        "404 KEY NOT PRESENT"
    );
    assert_eq!(
        text(&mut u, &[6, 0x15, 0, 0]).0,
        "POST /echo?x=1 HTTP/1.1\r\nHost: api.test\r\nX-Key: two\r\n"
    );
    assert_eq!(text(&mut u, &[6, 0x15, 0, 1]).0, "X-Key: two");
    assert_eq!(
        command(&mut u, &[6, 0x15, 0, 2]).1,
        "400 INDEX OUT OF BOUNDS"
    );
    // A JSON body: BODY_ADD, the helpers, nesting.
    let (p, _) = command(&mut u, &[6, 0x21, 2]);
    let b = p[0][0];
    let mut add = vec![6, 0x2d, b];
    add.extend([5, b'w', b'i', b'd', b't', b'h', 1, 0xf4, 0x01, 0, 0]);
    add.extend([
        4, b'u', b's', b'e', b'r', 4, 1, 4, b'n', b'a', b'm', b'e', 3, 2, b'J', b'o',
    ]);
    assert_eq!(command(&mut u, &add).1, "000 OK");
    command(
        &mut u,
        &[6, 0x24, b, 7, b'v', b'i', b's', b'i', b'b', b'l', b'e', 1],
    );
    command(&mut u, &[6, 0x23, b, 3, b'n', b'e', b'g', 0xfe]);
    command(&mut u, &[6, 0x27, b, 4, b'l', b'i', b's', b't']);
    command(&mut u, &[6, 0x25, b, 0, 2, b'h', b'i']);
    command(&mut u, &[6, 0x26, b, 0]);
    command(&mut u, &[6, 0x23, b, 1, b'k', 7]);
    command(&mut u, &[6, 0x28, b]);
    command(&mut u, &[6, 0x28, b]);
    command(&mut u, &[6, 0x25, b, 3, b'e', b'n', b'd', 1, b'.']);
    assert_eq!(
        command(&mut u, &with_name(&[6, 0x2a, b], "user/name")).0[0],
        [3, 2, b'J', b'o']
    );
    assert_eq!(
        command(&mut u, &with_name(&[6, 0x2a, b], "list%1")).0[0],
        [4, 1, 1, b'k', 1, 7, 0, 0, 0]
    );
    assert_eq!(
        command(&mut u, &with_name(&[6, 0x2a, b], "neg")).0[0],
        [1, 0xfe, 0xff, 0xff, 0xff]
    );
    assert_eq!(
        command(&mut u, &with_name(&[6, 0x29, b], "list%0")).1,
        "000 OK"
    );
    assert_eq!(
        command(&mut u, &with_name(&[6, 0x2a, b], "nothing")).1,
        "400 BAD FORMAT: PATH NOT FOUND"
    );
    // POST it: the fake echoes the body.
    let (p, s) = command(&mut u, &[6, 0x31, 0, b]);
    assert_eq!(s, "200 OK");
    let (h2, b2) = (p[0][0], p[0][1]);
    let sent = lock(&net).requests.last().unwrap().clone();
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.url, "http://api.test/echo?x=1");
    assert_eq!(sent.header("x-key"), Some("two"));
    assert_eq!(sent.header("content-type"), Some("application/json"));
    assert_eq!(
        String::from_utf8_lossy(&sent.body),
        r#"{"width":500,"user":{"name":"Jo"},"visible":true,"neg":-2,"list":[{"k":7}],"end":"."}"#
    );
    assert_eq!(
        text(&mut u, &with_name(&[6, 0x14, h2], "content-type")).0,
        "application/json"
    );
    assert_eq!(
        command(&mut u, &with_name(&[6, 0x2a, b2], "user/name")).0[0],
        [3, 2, b'J', b'o']
    );
    // GET a JSON page: values by path.
    command(&mut u, &[6, 0x10]);
    command(&mut u, &with_name(&[6, 0x11, 1], "http://api.test/user"));
    let (p, s) = command(&mut u, &[6, 0x31, 0, 0xff]);
    assert_eq!(s, "200 OK");
    let rb = p[0][1];
    assert_eq!(
        command(&mut u, &with_name(&[6, 0x2a, rb], "cars%1/model")).0[0],
        [&[3, 6][..], b"Toyota"].concat()
    );
    assert_eq!(
        command(&mut u, &with_name(&[6, 0x2a, rb], "age")).0[0],
        [1, 36, 0, 0, 0]
    );
    assert_eq!(
        command(&mut u, &with_name(&[6, 0x2a, rb], "pi")).0[0],
        [&[3, 4][..], b"3.14"].concat()
    );
    assert_eq!(
        command(&mut u, &with_name(&[6, 0x2a, rb], "none")).0[0],
        [0]
    );
    assert_eq!(
        command(&mut u, &with_name(&[6, 0x2b, rb], "cars")).1,
        "000 OK"
    );
    // Raw: the header on the status channel, the body in packets.
    command(&mut u, &with_name(&[6, 0x11, 1], "api.test/big"));
    let (p, s) = command(&mut u, &[6, 0x32, 2, 0xff]);
    assert!(s.starts_with("HTTP/1.1 200 OK\r\n"), "{s}");
    assert_eq!(p.iter().map(Vec::len).collect::<Vec<_>>(), [896, 896, 208]);
    // A 404 and a URL-encoded body.
    command(&mut u, &with_name(&[6, 0x11, 1], "api.test/missing"));
    assert_eq!(command(&mut u, &[6, 0x31, 3, 0xff]).1, "404 NOT FOUND");
    let (p, _) = command(&mut u, &[6, 0x21, 4]);
    let f = p[0][0];
    command(&mut u, &[6, 0x25, f, 1, b'q', 3, b'a', b' ', b'&']);
    command(&mut u, &[6, 0x23, f, 1, b'n', 5]);
    command(&mut u, &with_name(&[6, 0x11, 3], "api.test/form"));
    command(&mut u, &[6, 0x32, 5, f]);
    let sent = lock(&net).requests.last().unwrap().clone();
    assert_eq!(sent.body, b"q=a+%26&n=5");
    assert_eq!(
        sent.header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    // Binary bodies refuse JSON commands; bad handles.
    let (p, _) = command(&mut u, &[6, 0x21, 1]);
    let bin = p[0][0];
    assert_eq!(command(&mut u, &[6, 0x2c, bin, 1, 2]).1, "000 OK");
    assert_eq!(
        command(&mut u, &[6, 0x24, bin, 1, b'a', 1]).1,
        "400 BAD REQUEST"
    );
    assert_eq!(command(&mut u, &[6, 0x13, 15, b'a']).1, "400 BAD REQUEST");
    assert_eq!(command(&mut u, &[6, 0x11, 10]).1, "400 BAD COMMAND");
    // Sixteen header slots.
    command(&mut u, &[6, 0x10]);
    for _ in 0..16 {
        command(&mut u, &with_name(&[6, 0x11, 1], "a"));
    }
    assert_eq!(
        command(&mut u, &with_name(&[6, 0x11, 1], "a")).1,
        "507 NO HEADER SLOT"
    );
    // Handles survive a C64 reset.
    u.reset();
    assert_eq!(
        text(&mut u, &[6, 0x15, 15, 0]).0,
        "GET / HTTP/1.1\r\nHost: a\r\n"
    );
}

#[test]
fn http_without_network_and_state_round_trip() {
    let (mut u, _, net) = setup(
        MemFileHost::new(),
        FakeNet::new().json_page("http://h/j", "[1,2]"),
    );
    command(&mut u, &with_name(&[6, 0x11, 1], "h/j"));
    let (p, _) = command(&mut u, &[6, 0x21, 3]);
    command(&mut u, &[6, 0x2d, p[0][0], 3, 1, b'x']);
    // Mid-exchange savestate: the pending exchange and the handles.
    for &b in &[6, 0x31, 0, 0xff] {
        u.write(CMD, b);
    }
    u.write(STATUS, ctrl::PUSH_CMD);
    for _ in 0..100 {
        u.tick();
    }
    let mut w = StateWriter::new();
    u.save(&mut w);
    let bytes = w.into_bytes();
    let mut v = Uci::new();
    v.load(&mut StateReader::new(&bytes)).unwrap();
    assert_eq!(u, v);
    for cut in [bytes.len() / 3, bytes.len() - 1] {
        let before = v.clone();
        assert!(v.load(&mut StateReader::new(&bytes[..cut])).is_err());
        assert_eq!(v, before);
    }
    wait(&mut u);
    let (data, s) = super::tests::take(&mut u);
    u.write(STATUS, ctrl::DATA_ACC);
    assert_eq!(s, "200 OK");
    assert_eq!(
        command(&mut u, &with_name(&[6, 0x2a, data[1]], "%1")).0[0],
        [1, 2, 0, 0, 0]
    );
    lock(&net).online = false;
    assert_eq!(
        command(&mut u, &[6, 0x31, 0, 0xff]).1,
        "503 SERVICE UNAVAILABLE"
    );
}
