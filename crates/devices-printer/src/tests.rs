use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use emu_core::host::{DirEntry, FileHandle, FileHost, FileStat, FsError};

use super::*;

/// Files in memory.
#[derive(Default)]
struct Memory {
    files: BTreeMap<String, Vec<u8>>,
    open: Option<String>,
}

impl FileHost for Memory {
    fn open(&mut self, path: &str, mode: OpenMode) -> Result<FileHandle, FsError> {
        let f = self.files.entry(path.into()).or_default();
        if mode.truncate {
            f.clear();
        }
        self.open = Some(path.into());
        Ok(1)
    }
    fn read(&mut self, _: FileHandle, _: &mut [u8]) -> Result<usize, FsError> {
        Ok(0)
    }
    fn write(&mut self, _: FileHandle, data: &[u8]) -> Result<usize, FsError> {
        let path = self.open.clone().expect("open");
        self.files.get_mut(&path).unwrap().extend_from_slice(data);
        Ok(data.len())
    }
    fn seek(&mut self, _: FileHandle, _: u64) -> Result<(), FsError> {
        Ok(())
    }
    fn fstat(&mut self, _: FileHandle) -> Result<FileStat, FsError> {
        Ok(FileStat::default())
    }
    fn close(&mut self, _: FileHandle) {}
    fn stat(&mut self, path: &str) -> Result<FileStat, FsError> {
        if self.files.contains_key(path) {
            Ok(FileStat::default())
        } else {
            Err(FsError::NotFound)
        }
    }
    fn list_dir(&mut self, _: &str) -> Result<Vec<DirEntry>, FsError> {
        Ok(Vec::new())
    }
    fn mkdir(&mut self, _: &str) -> Result<(), FsError> {
        Ok(())
    }
    fn delete(&mut self, _: &str) -> Result<(), FsError> {
        Ok(())
    }
    fn rename(&mut self, _: &str, _: &str) -> Result<(), FsError> {
        Ok(())
    }
}

fn host(memory: &Arc<Mutex<Memory>>) -> HostServices {
    HostServices {
        net: None,
        files: Some(memory.clone()),
    }
}

fn attached() -> (Printer, Arc<Mutex<Memory>>) {
    let memory = Arc::new(Mutex::new(Memory::default()));
    let mut p = Printer::new();
    p.attach(&host(&memory));
    (p, memory)
}

#[test]
fn a_pause_writes_the_job() {
    let (mut p, memory) = attached();
    b"HI".iter().for_each(|&b| p.feed(b));
    (0..IDLE_FRAMES - 1).for_each(|_| p.frame());
    assert!(memory.lock().unwrap().files.is_empty(), "still coming");
    p.frame();
    let m = memory.lock().unwrap();
    assert_eq!(m.files["/Printer/print-001.prn"], b"HI");
    let png = &m.files["/Printer/print-001-1.png"];
    assert_eq!(&png[1..4], b"PNG");
}

#[test]
fn pages_and_jobs_get_their_own_files() {
    let (mut p, memory) = attached();
    b"A\x0cB".iter().for_each(|&b| p.feed(b));
    p.flush();
    let m = memory.lock().unwrap();
    assert!(m.files.contains_key("/Printer/print-001-1.png"));
    assert!(m.files.contains_key("/Printer/print-001-2.png"));
    drop(m);
    // A second run finds the first job's file.
    let mut q = Printer::new();
    q.attach(&host(&memory));
    q.feed(b'C');
    q.flush();
    let m = memory.lock().unwrap();
    assert!(m.files.contains_key("/Printer/print-002.prn"));
}

#[test]
fn without_files_the_printer_still_works() {
    let mut p = Printer::new();
    p.feed(b'A');
    assert!(p.epson().page().is_some());
    p.flush();
    assert!(p.epson().page().is_none(), "the page came out");
}

fn translated(t: Translation, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    bytes.iter().for_each(|&b| t.apply(b, |c| out.push(c)));
    out
}

#[test]
fn petscii_becomes_ascii() {
    assert_eq!(translated(Translation::Upper, b"HELLO 1\r"), b"HELLO 1\r\n");
    assert_eq!(translated(Translation::Lower, b"HELLO"), b"hello");
    assert_eq!(translated(Translation::Lower, &[0xc1, 0x42]), b"Ab");
    assert_eq!(translated(Translation::Upper, &[0xc1]), b"?", "no ROM");
    assert_eq!(translated(Translation::Upper, &[0xa0, 0xa6]), b" ?");
    assert_eq!(translated(Translation::Upper, &[0x93, 0x05]), b"");
    assert_eq!(
        translated(Translation::Transparent, &[0x1b, 0xff]),
        [0x1b, 0xff]
    );
    assert_eq!(Translation::of_secondary(7), Translation::Lower);
    assert_eq!(Translation::of_secondary(1), Translation::Transparent);
    assert_eq!(Translation::of_secondary(0), Translation::Upper);
}

/// A controller on the serial bus, as the KERNAL works.
struct Host {
    atn: bool,
    clk: bool,
    data: bool,
}

impl Host {
    fn new() -> Self {
        Self {
            atn: false,
            clk: false,
            data: false,
        }
    }

    fn run(&self, p: &mut CbmPrinter, us: u32) {
        for _ in 0..us {
            p.iec_tick(self.atn, self.clk, self.data);
        }
    }

    fn byte(&mut self, p: &mut CbmPrinter, b: u8, eoi: bool) {
        // Ready to send: the listener lets DATA go.
        self.clk = false;
        self.run(p, 80);
        assert!(!p.iec_data(), "listener ready");
        if eoi {
            self.run(p, 300);
        }
        for bit in 0..8 {
            self.clk = true;
            self.data = b & (1 << bit) == 0;
            self.run(p, 60);
            self.clk = false;
            self.run(p, 60);
        }
        self.clk = true;
        self.data = false;
        self.run(p, 60);
        assert!(p.iec_data(), "listener took the byte");
    }

    fn command(&mut self, p: &mut CbmPrinter, bytes: &[u8]) {
        self.atn = true;
        self.clk = true;
        self.run(p, 100);
        assert!(p.iec_data(), "ATN answered");
        for &b in bytes {
            self.byte(p, b, false);
        }
        self.atn = false;
        self.run(p, 100);
    }
}

fn column_inked(p: &CbmPrinter, x: usize) -> bool {
    (0..30).any(|y| p.printer.epson().dot(x, y))
}

#[test]
fn iec_print_to_device_4() {
    let mut p = CbmPrinter::new(4);
    let mut h = Host::new();
    h.run(&mut p, 10);
    // OPEN 4,4,7 with a file name, which is not printed.
    h.command(&mut p, &[0x24, 0xf7]);
    h.byte(&mut p, b'X', true);
    h.command(&mut p, &[0x3f]);
    assert!(p.printer.epson().page().is_none(), "the name");
    // PRINT#4,"AA"
    h.command(&mut p, &[0x24, 0x67]);
    h.byte(&mut p, 0x41, false);
    h.byte(&mut p, 0x41, true);
    h.command(&mut p, &[0x3f]);
    assert!((0..24).any(|x| column_inked(&p, x)), "first letter");
    assert!((24..48).any(|x| column_inked(&p, x)), "second letter");
    assert!(!(48..96).any(|x| column_inked(&p, x)));
    h.clk = false;
    h.run(&mut p, 10);
    assert!(!p.iec_data(), "bus released");
}

#[test]
fn iec_device_5_ignores_data_for_device_8() {
    let mut p = CbmPrinter::new(5);
    let mut h = Host::new();
    h.run(&mut p, 10);
    h.command(&mut p, &[0x28, 0x62]);
    assert!(!p.iec_data(), "not its turn");
    assert!(p.printer.epson().page().is_none());
}

#[test]
fn ieee_print_to_device_4() {
    let send = |p: &mut CbmPrinter, atn: bool, b: u8| {
        p.ieee_tick(atn, false, 0);
        p.ieee_tick(atn, false, 0);
        assert_eq!(p.ieee_lines(), (true, false), "ready");
        p.ieee_tick(atn, true, b);
        assert_eq!(p.ieee_lines(), (false, true), "accepted");
        p.ieee_tick(atn, false, 0);
    };
    let mut p = CbmPrinter::new(4);
    send(&mut p, true, 0x24);
    send(&mut p, true, 0x60);
    p.ieee_tick(false, false, 0);
    send(&mut p, false, b'A');
    assert!((0..24).any(|x| column_inked(&p, x)));
    // UNLISTEN lets the bus go.
    send(&mut p, true, 0x3f);
    p.ieee_tick(false, false, 0);
    assert_eq!(p.ieee_lines(), (false, false));
}

#[test]
fn savestate_round_trip() {
    let mut p = CbmPrinter::new(4);
    p.command(0x24);
    p.command(0x67);
    p.received(b'A');
    let mut w = StateWriter::new();
    p.save(&mut w);
    let bytes = w.into_bytes();
    let mut q = CbmPrinter::new(5);
    q.load(&mut StateReader::new(&bytes)).unwrap();
    assert_eq!((q.device, q.listening, q.secondary), (4, true, 7));
    assert_eq!(q.printer.epson().page(), p.printer.epson().page());
    assert!(CbmPrinter::new(4)
        .load(&mut StateReader::new(&bytes[..9]))
        .is_err());
}

#[test]
fn petscii_graphics_map_to_screen_codes() {
    let up = Translation::Upper;
    assert_eq!(up.graphic(0xc1), Some((0x41, false)), "spade");
    assert_eq!(up.graphic(0x61), Some((0x41, false)));
    assert_eq!(up.graphic(0xa6), Some((0x66, false)));
    assert_eq!(up.graphic(0xe6), Some((0x66, false)));
    assert_eq!(up.graphic(0x5c), Some((0x1c, false)), "pound");
    assert_eq!(up.graphic(0xff), Some((0x5e, false)), "pi");
    assert_eq!(up.graphic(0x41), None);
    assert_eq!(up.graphic(0xa0), None, "shifted space");
    let low = Translation::Lower;
    assert_eq!(low.graphic(0xc1), None, "a capital here");
    assert_eq!(low.graphic(0xa6), Some((0x66, true)));
    assert_eq!(Translation::Transparent.graphic(0xc1), None);
}

#[test]
fn graphics_print_from_the_character_rom() {
    let mut p = CbmPrinter::new(4);
    let mut rom = vec![0u8; 0x1000];
    rom[0x41 * 8] = 0xff; // the capitals' set: a top bar
    rom[0x800 + 0x66 * 8 + 7] = 0xff; // the other set: a bottom bar
    p.charset = rom;
    p.command(0x24);
    p.command(0x60);
    p.received(0xc1);
    assert!(p.printer.epson().dot(0, 0) && p.printer.epson().dot(23, 2));
    assert!(!p.printer.epson().dot(0, 3));
    p.command(0x67);
    p.received(0xa6);
    assert!(p.printer.epson().dot(24, 21) && !p.printer.epson().dot(24, 0));
}

#[test]
fn atari_records_end_at_their_eol() {
    let mut out = Vec::new();
    atari_line(&[b'H', 0xc9, 0x9b, b'X'], |c| out.push(c));
    assert_eq!(out, b"HI\r\n");
}
