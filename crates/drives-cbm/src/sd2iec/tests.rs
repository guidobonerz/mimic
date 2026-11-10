//! The SD2IEC over the serial bus, driven like the KERNAL does, on a
//! folder in memory.

use super::*;
use emu_core::host::{lock, HostServices, MemFileHost};
use media::D64;

const DEV: u8 = 9;

/// The host side of the serial bus; every wait ticks the device one
/// microsecond.
struct Host {
    sd: Sd2iec,
    files: SharedFiles,
    out: IecLines,
}

impl Host {
    fn new(setup: impl FnOnce(&mut MemFileHost)) -> Self {
        let mut mem = MemFileHost::new();
        setup(&mut mem);
        let files = HostServices::new(None, Some(Box::new(mem))).files.unwrap();
        let mut sd = Sd2iec::new(DEV, 1_000_000);
        sd.attach_files(Some(files.clone()));
        Self {
            sd,
            files,
            out: IecLines::default(),
        }
    }

    fn bus(&self) -> IecLines {
        IecLines::combine(&[self.out, self.sd.iec_out()])
    }

    fn tick(&mut self, n: u32) {
        for _ in 0..n {
            self.sd.run(self.out);
        }
    }

    fn wait(&mut self, what: &str, cond: impl Fn(IecLines) -> bool) {
        for _ in 0..5000 {
            if cond(self.bus()) {
                return;
            }
            self.tick(1);
        }
        panic!("timeout waiting for {what}");
    }

    fn send(&mut self, byte: u8, eoi: bool) {
        self.out.clk = false;
        self.wait("listener ready", |b| !b.data);
        if eoi {
            self.wait("EOI ack", |b| b.data);
            self.wait("EOI ack end", |b| !b.data);
        }
        self.out.clk = true;
        for bit in 0..8 {
            self.out.data = byte & (1 << bit) == 0;
            self.tick(20);
            self.out.clk = false;
            self.tick(30);
            self.out.clk = true;
            self.out.data = false;
        }
        self.wait("frame ack", |b| b.data);
    }

    fn receive(&mut self) -> Option<(u8, bool)> {
        for _ in 0..5000 {
            if !self.bus().clk {
                break;
            }
            self.tick(1);
        }
        if self.bus().clk {
            return None;
        }
        self.out.data = false;
        let mut eoi = false;
        let mut waited = 0;
        while !self.bus().clk {
            self.tick(1);
            waited += 1;
            if waited == 250 && !eoi {
                eoi = true;
                self.out.data = true;
                self.tick(60);
                self.out.data = false;
            }
            if waited > 3000 {
                return None;
            }
        }
        let mut byte = 0u8;
        for bit in 0..8 {
            self.wait("bit valid", |b| !b.clk);
            if !self.bus().data {
                byte |= 1 << bit;
            }
            self.wait("bit end", |b| b.clk);
        }
        self.out.data = true;
        self.tick(30);
        Some((byte, eoi))
    }

    fn atn(&mut self, bytes: &[u8]) {
        self.out.atn = true;
        self.out.clk = true;
        self.out.data = false;
        self.tick(1000);
        assert!(self.bus().data, "device present");
        for &b in bytes {
            self.send(b, false);
        }
    }

    fn release_atn(&mut self) {
        self.out.atn = false;
        self.tick(20);
    }

    fn listen_send(&mut self, secondary: u8, data: &[u8]) {
        self.atn(&[0x20 | DEV, secondary]);
        self.release_atn();
        for (i, &b) in data.iter().enumerate() {
            self.send(b, i + 1 == data.len());
        }
        self.atn(&[0x3f]);
        self.release_atn();
        self.out = IecLines::default();
        self.tick(100);
    }

    fn talk_receive(&mut self, secondary: u8) -> Vec<u8> {
        self.atn(&[0x40 | DEV, secondary]);
        self.out.data = true;
        self.out.clk = false;
        self.out.atn = false;
        let mut got = Vec::new();
        for _ in 0..500 {
            if self.bus().clk {
                break;
            }
            self.tick(1);
        }
        if self.bus().clk {
            while let Some((b, eoi)) = self.receive() {
                got.push(b);
                if eoi {
                    break;
                }
            }
        }
        self.atn(&[0x5f]);
        self.release_atn();
        self.out = IecLines::default();
        self.tick(100);
        got
    }

    fn open(&mut self, ch: u8, name: &[u8]) {
        self.listen_send(0xf0 | ch, name);
    }

    fn close(&mut self, ch: u8) {
        self.atn(&[0x20 | DEV, 0xe0 | ch, 0x3f]);
        self.release_atn();
        self.out = IecLines::default();
        self.tick(100);
    }

    /// LOAD"name": the file's bytes (empty if not found).
    fn load(&mut self, name: &[u8]) -> Vec<u8> {
        self.open(0, name);
        let data = self.talk_receive(0x60);
        self.close(0);
        data
    }

    /// SAVE"name" with `data` (load address included).
    fn save(&mut self, name: &[u8], data: &[u8]) {
        self.open(1, name);
        self.listen_send(0x61, data);
        self.close(1);
    }

    /// A command on channel 15; returns the status afterwards.
    fn command(&mut self, cmd: &[u8]) -> String {
        self.listen_send(0x6f, cmd);
        self.status()
    }

    fn status(&mut self) -> String {
        String::from_utf8_lossy(&self.talk_receive(0x6f)).into_owned()
    }

    fn file(&self, path: &str) -> Option<Vec<u8>> {
        let mut fs = lock(&self.files);
        Sd2iecDos::read_all(&mut *fs, path).ok()
    }
}

fn listing_names(listing: &[u8]) -> Vec<String> {
    // The quoted names after the header line.
    let text = String::from_utf8_lossy(listing).into_owned();
    text.split('"')
        .skip(3)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

fn standard() -> Host {
    Host::new(|m| {
        m.insert("/hello", vec![0x01, 0x08, 0xaa, 0xbb]).unwrap();
        m.insert("/games/elite", vec![0x01, 0x08, 0xe1]).unwrap();
        m.insert("/.hidden", vec![1, 2]).unwrap();
    })
}

#[test]
fn powers_on_with_the_version_message() {
    let mut h = standard();
    assert_eq!(h.status(), format!("73,{},00,00\r", dos::VERSION));
    assert_eq!(h.status(), "00, OK,00,00\r");
}

#[test]
fn loads_a_file_from_the_folder() {
    let mut h = standard();
    assert_eq!(h.load(b"HELLO"), [0x01, 0x08, 0xaa, 0xbb]);
    assert_eq!(h.load(b"HE*"), [0x01, 0x08, 0xaa, 0xbb]);
    assert!(h.load(b"NOPE").is_empty());
    assert!(h.status().starts_with("62,FILE NOT FOUND"));
}

#[test]
fn directory_lists_files_and_subdirectories() {
    let mut h = standard();
    let listing = h.load(b"$");
    assert_eq!(&listing[..2], &[0x01, 0x04]);
    let text = String::from_utf8_lossy(&listing).into_owned();
    assert!(text.contains("\"GAMES\"            DIR"), "{text}");
    assert!(text.contains("\"HELLO\"            PRG"), "{text}");
    assert!(!text.contains("HIDDEN"));
    assert!(text.contains("BLOCKS FREE"));
    let only_dirs = listing_names(&h.load(b"$=D"));
    assert_eq!(only_dirs, ["GAMES"]);
    let with_hidden = String::from_utf8_lossy(&h.load(b"$:*=H")).into_owned();
    assert!(with_hidden.contains(".HIDDEN"), "{with_hidden}");
}

#[test]
fn saves_prg_plain_and_seq_as_s00() {
    let mut h = standard();
    h.save(b"NEW", &[0x01, 0x08, 1, 2, 3]);
    assert_eq!(h.file("/new").unwrap(), [0x01, 0x08, 1, 2, 3]);
    assert!(h.status().starts_with("00"));
    h.open(2, b"LOG,S,W");
    h.listen_send(0x62, b"TEXT");
    h.close(2);
    let s00 = h.file("/log.s00").unwrap();
    assert_eq!(&s00[..8], names::X00_MAGIC);
    assert_eq!(&s00[26..], b"TEXT");
    let text = String::from_utf8_lossy(&h.load(b"$")).into_owned();
    assert!(text.contains("\"LOG\"              SEQ"), "{text}");
    h.open(2, b"LOG,S,R");
    assert_eq!(h.talk_receive(0x62), b"TEXT");
    h.close(2);
    // Saving over an existing file needs `@`.
    h.save(b"NEW", &[0x01, 0x08, 9]);
    assert!(h.status().starts_with("63,FILE EXISTS"));
    h.save(b"@:NEW", &[0x01, 0x08, 9]);
    assert_eq!(h.file("/new").unwrap(), [0x01, 0x08, 9]);
}

#[test]
fn changes_directories_and_takes_paths() {
    let mut h = standard();
    assert_eq!(h.load(b"//GAMES/:ELITE"), [0x01, 0x08, 0xe1]);
    assert!(h.command(b"CD:GAMES").starts_with("00"));
    assert_eq!(h.sd.dos.cwd(), "/games");
    assert_eq!(h.load(b"ELITE"), [0x01, 0x08, 0xe1]);
    assert!(h.command(b"CD\x5f").starts_with("00"));
    assert_eq!(h.sd.dos.cwd(), "/");
    assert!(h.command(b"CD/GAMES/").starts_with("00"));
    assert_eq!(h.sd.dos.cwd(), "/games");
    assert!(h.command(b"CD//").starts_with("00"));
    assert_eq!(h.sd.dos.cwd(), "/");
    assert!(h.command(b"CD:NOWHERE").starts_with("39"));
    assert!(h.command(b"MD:NEWDIR").starts_with("00"));
    assert!(h.command(b"MD:NEWDIR").starts_with("63"));
    h.save(b"/NEWDIR/:INSIDE", &[0x01, 0x08]);
    assert!(h.file("/newdir/inside").is_some());
    assert!(h.command(b"RD:NEWDIR").starts_with("63"));
    assert!(h
        .command(b"S/NEWDIR/:INSIDE")
        .starts_with("01, FILES SCRATCHED,01"));
    assert!(h.command(b"RD:NEWDIR").starts_with("00"));
}

#[test]
fn mounts_a_disk_image_by_changing_into_it() {
    let mut d = D64::blank(b"IMAGE", *b"IM");
    d.write_file(b"INSIDE", 2, &[0x01, 0x08, 0x42]).unwrap();
    let bytes = d.to_bytes();
    let mut h = Host::new(move |m| m.insert("/disk.d64", bytes).unwrap());
    assert!(h.command(b"CD:DISK.D64").starts_with("00"));
    assert!(h.sd.dos.mounted());
    assert_eq!(h.load(b"INSIDE"), [0x01, 0x08, 0x42]);
    assert!(listing_names(&h.load(b"$")).contains(&"INSIDE".to_string()));
    // Saving goes into the image file in the folder.
    h.save(b"SAVED", &[0x01, 0x08, 7]);
    let back = D64::parse(&h.file("/disk.d64").unwrap()).unwrap();
    assert_eq!(back.read_file(b"SAVED").unwrap().1, [0x01, 0x08, 7]);
    assert!(h.command(b"CD:\x5f").starts_with("00"));
    assert!(!h.sd.dos.mounted());
    assert!(listing_names(&h.load(b"$")).contains(&"DISK.D64".to_string()));
}

#[test]
fn creates_and_formats_images() {
    let mut h = standard();
    assert!(h.command(b"N:WORK.D64,WK").starts_with("00"));
    let image = D64::parse(&h.file("/work.d64").unwrap()).unwrap();
    assert_eq!(image.id(), *b"WK");
    // Without an extension .D64 is added, but an existing one is kept.
    assert!(h.command(b"N:WORK,XX").starts_with("63"));
    assert!(h.command(b"N:BIG.D81,BG").starts_with("00"));
    assert_eq!(
        D64::parse(&h.file("/big.d81").unwrap()).unwrap().format,
        media::Format::D81
    );
}

#[test]
fn renames_and_copies() {
    let mut h = standard();
    assert!(h.command(b"R:GREETING=HELLO").starts_with("00"));
    assert!(h.file("/greeting").is_some());
    assert!(h
        .command(b"C:BOTH=GREETING,//GAMES/:ELITE")
        .starts_with("00"));
    assert_eq!(
        h.file("/both").unwrap(),
        [0x01, 0x08, 0xaa, 0xbb, 0x01, 0x08, 0xe1]
    );
    assert!(h.command(b"R:BOTH=GREETING").starts_with("63"));
}

#[test]
fn extension_hiding_and_long_names() {
    let mut h = Host::new(|m| {
        m.insert("/tool.prg", vec![0x01, 0x08]).unwrap();
        m.insert("/readme.seq", b"HI".to_vec()).unwrap();
        m.insert("/a rather long file name.prg", vec![0x01, 0x08])
            .unwrap();
    });
    let names = listing_names(&h.load(b"$"));
    assert_eq!(names, ["ARATHE~1.PRG", "README.SEQ", "TOOL.PRG"]);
    assert!(h.command(b"XE+").starts_with("00"));
    let text = String::from_utf8_lossy(&h.load(b"$")).into_owned();
    assert!(text.contains("\"README\"           SEQ"), "{text}");
    assert_eq!(h.load(b"TOOL"), [0x01, 0x08]);
    assert!(h.command(b"X").starts_with("03,J-:E01+:*+,09,00"));
}

#[test]
fn device_number_changes_by_command() {
    let mut h = standard();
    h.listen_send(0x6f, b"U0>\x0a");
    assert_eq!(h.sd.device(), 10);
}

#[test]
fn without_a_folder_the_drive_is_not_ready() {
    let mut h = standard();
    h.sd.attach_files(None);
    assert!(h.load(b"HELLO").is_empty());
    assert!(h.status().starts_with("74,DRIVE NOT READY"));
}

#[test]
fn savestate_keeps_directory_and_mount() {
    let mut d = D64::blank(b"IMAGE", *b"IM");
    d.write_file(b"INSIDE", 2, &[0x01, 0x08, 0x42]).unwrap();
    let bytes = d.to_bytes();
    let mut h = Host::new(move |m| {
        m.insert("/sub/disk.d64", bytes).unwrap();
    });
    assert!(h.command(b"CD:SUB").starts_with("00"));
    assert!(h.command(b"CD:DISK.D64").starts_with("00"));
    let mut w = StateWriter::new();
    h.sd.save(&mut w);
    let state = w.into_bytes();
    h.sd.reset(true);
    assert_eq!(h.sd.dos.cwd(), "/");
    assert!(!h.sd.dos.mounted());
    h.sd.load(&mut StateReader::new(&state)).unwrap();
    assert_eq!(h.sd.dos.cwd(), "/sub");
    assert_eq!(h.load(b"INSIDE"), [0x01, 0x08, 0x42]);
    // A damaged state changes nothing.
    assert!(h.sd.load(&mut StateReader::new(&state[..20])).is_err());
    assert_eq!(h.sd.dos.cwd(), "/sub");
}
