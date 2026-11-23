//! The block driven the way the firmware's USB stack drives it
//! (`usb_base.cc`: `control_exchange`, `bulk_in`/`bulk_out`, autopipes, the
//! interrupt routine's FIFO drain; `usb_hub.cc`, `usb_scsi.cc`: the
//! requests and their lengths).

use super::*;
use crate::io::{unit, MemStorage, NullC64, SECTOR};

/// Firmware buffers in board memory (high word never 0).
const SETUP_BUF: u32 = 0x1_0000;
const IN_BUF: u32 = 0x1_1000;
const OUT_BUF: u32 = 0x2_0000;
const HUB_BUF: u32 = 0x3_0000;
/// One FreeRTOS tick (200 Hz).
const TICK: u64 = CLOCK_HZ / 200;
const MS: u64 = CLOCK_HZ / 1000;

struct Rig {
    usb: UsbHost,
    ram: Vec<u8>,
    irq: Irq,
    console: Vec<u8>,
    c64: NullC64,
    storage: MemStorage,
    now: u64,
    /// Every word the interrupt routine took from the FIFO.
    words: Vec<u16>,
    /// Words for the input task (not 0).
    queue: Vec<u16>,
    /// The command semaphore.
    sem: u32,
}

fn desc(n: usize) -> usize {
    at::PIPES + n * at::PIPE_SIZE
}

impl Rig {
    fn new() -> Self {
        Self {
            usb: UsbHost::new(),
            ram: vec![0; 0x10_0000],
            irq: Irq::default(),
            console: Vec::new(),
            c64: NullC64::default(),
            storage: MemStorage::default(),
            now: 0,
            words: Vec::new(),
            queue: Vec::new(),
            sem: 0,
        }
    }

    fn with<R>(&mut self, f: impl FnOnce(&mut UsbHost, &mut IoCtx<'_>) -> R) -> R {
        let mut ctx = IoCtx {
            now: self.now,
            ram: &mut self.ram,
            irq: &mut self.irq,
            console: &mut self.console,
            c64: &mut self.c64,
            storage: Some(&mut self.storage),
            net: None,
        };
        f(&mut self.usb, &mut ctx)
    }

    fn wr(&mut self, off: usize, v: u8) {
        self.with(|u, c| u.write(BASE + off as u32, v, c));
    }

    fn wr16(&mut self, off: usize, v: u16) {
        self.wr(off, v as u8);
        self.wr(off + 1, (v >> 8) as u8);
    }

    fn rd16(&mut self, off: usize) -> u16 {
        let lo = self.with(|u, c| u.read(BASE + off as u32, c));
        let hi = self.with(|u, c| u.read(BASE + off as u32 + 1, c));
        u16::from(lo) | u16::from(hi) << 8
    }

    /// The interrupt routine: takes the FIFO until head and tail meet.
    fn isr(&mut self) {
        if self.irq.active() & 4 == 0 || !self.irq.line() {
            return;
        }
        self.irq.clear(4);
        let mut tail = self.rd16(at::TAIL);
        loop {
            let head = self.rd16(at::HEAD);
            assert_eq!(head, self.rd16(at::HEAD), "HEAD reads stable (H5)");
            if tail == head {
                break;
            }
            let w = self.rd16(at::FIFO + 2 * usize::from(tail));
            tail = (tail + 1) % 16;
            self.wr16(at::TAIL, tail);
            // H6: only 0, a pipe 1-7, or a status word.
            assert!(w <= 7 || w & 0xFFF0 == 0xFFF0, "FIFO word {w:04x}");
            self.words.push(w);
            if w == 0 {
                self.sem += 1;
            } else {
                self.queue.push(w);
            }
        }
    }

    /// Runs the clock to `until`, ticking when the block asks.
    fn advance(&mut self, until: u64) {
        let mut guard = 0u32;
        loop {
            let ne = self.usb.next_event();
            if ne > until {
                break;
            }
            self.now = self.now.max(ne);
            self.with(|u, c| u.tick(c));
            self.isr();
            guard += 1;
            assert!(guard < 1_000_000, "next_event does not move on");
        }
        self.now = self.now.max(until);
    }

    fn run_for(&mut self, d: u64) {
        let t = self.now + d;
        self.advance(t);
    }

    /// `xSemaphoreTake(commandSemaphore, ticks)`.
    fn take(&mut self, ticks: u64) -> bool {
        let end = self.now + ticks * TICK;
        while self.sem == 0 && self.now < end {
            let t = (self.now + FRAME_CLOCKS).min(end);
            self.advance(t);
        }
        if self.sem > 0 {
            self.sem -= 1;
            true
        } else {
            false
        }
    }

    /// `complete_command`.
    fn complete(&mut self, ticks: u64) -> u16 {
        if !self.take(ticks) {
            let c = self.rd16(desc(0));
            self.wr16(desc(0), c | 0x0100);
            if !self.take(ticks) {
                return 0xFFFF;
            }
        }
        self.rd16(desc(0) + pipe::RESULT) & 0x7FFF
    }

    fn set_mem(&mut self, n: usize, a: u32) {
        self.wr16(desc(n) + pipe::MEM_HI, (a >> 16) as u16);
        self.wr16(desc(n) + pipe::MEM_LO, a as u16);
    }

    /// `control_exchange`: the bytes that came back, or the error.
    fn control(&mut self, dev_ep: u16, setup: [u8; 8], in_len: usize) -> Result<Vec<u8>, i32> {
        self.ram[SETUP_BUF as usize..][..8].copy_from_slice(&setup);
        let d = desc(0);
        self.wr16(d + pipe::DEV_EP, dev_ep);
        self.wr16(d + pipe::LENGTH, 8);
        self.wr16(d + pipe::MAX_TRANS, 64);
        self.set_mem(0, SETUP_BUF);
        self.wr16(d + 0x0C, 0);
        self.wr16(d + pipe::STARTED, 0);
        self.wr16(d, 0x8040);
        let r = self.complete(100);
        if r & 0xF000 != 0x1000 {
            return Err(-1);
        }
        self.wr16(d + pipe::LENGTH, in_len as u16);
        self.set_mem(0, IN_BUF);
        self.wr16(d + pipe::STARTED, 0);
        self.wr16(d, 0x4C42);
        let r = self.complete(100);
        let transferred = in_len - usize::from(self.rd16(d + pipe::LENGTH));
        if r & 0xF000 == 0x4000 {
            return Err(-4);
        }
        if transferred > 0 {
            self.wr16(d + pipe::LENGTH, 0);
            self.wr16(d + pipe::STARTED, 0);
            self.wr16(d + pipe::RESULT, 0xFFFF);
            self.wr16(d, 0x8C41);
            let r = self.complete(100);
            assert_eq!(r & 0xF000, 0x1000, "status stage");
        }
        Ok(self.ram[IN_BUF as usize..][..transferred].to_vec())
    }

    fn control_ok(&mut self, dev_ep: u16, setup: [u8; 8], in_len: usize) -> Vec<u8> {
        self.control(dev_ep, setup, in_len)
            .unwrap_or_else(|e| panic!("control {setup:02x?} failed: {e}"))
    }

    /// `bulk_out`: bytes moved, or an error.
    fn bulk_out(&mut self, dev_ep: u16, data: &[u8], toggle: &mut u16) -> i32 {
        self.ram[OUT_BUF as usize..][..data.len()].copy_from_slice(data);
        let d = desc(0);
        self.wr16(d + pipe::DEV_EP, dev_ep);
        self.wr16(d + pipe::MAX_TRANS, 512);
        self.set_mem(0, OUT_BUF);
        self.wr16(d + pipe::LENGTH, data.len() as u16);
        self.wr16(d + pipe::TIMEOUT, 20000);
        self.wr16(d + pipe::STARTED, 0);
        self.wr16(d, *toggle | 0x8441);
        let r = self.complete(100);
        *toggle = self.rd16(d) & 0x0800;
        if r & 0xF000 != 0x1000 {
            return -1;
        }
        (data.len() - usize::from(self.rd16(d + pipe::LENGTH))) as i32
    }

    /// `bulk_in` (one chunk): the data or the result word.
    fn bulk_in(
        &mut self,
        dev_ep: u16,
        len: usize,
        timeout: u16,
        toggle: &mut u16,
    ) -> Result<Vec<u8>, u16> {
        let d = desc(0);
        self.wr16(d + pipe::DEV_EP, dev_ep);
        self.wr16(d + pipe::MAX_TRANS, len.min(512) as u16);
        self.set_mem(0, IN_BUF);
        self.wr16(d + pipe::TIMEOUT, timeout);
        self.wr16(d + pipe::LENGTH, len as u16);
        self.wr16(d + pipe::STARTED, 0);
        self.wr16(d, *toggle | 0x4442);
        let r = self.complete(100);
        let transferred = len - usize::from(self.rd16(d + pipe::LENGTH));
        if r & 0xF000 != 0 {
            return Err(r);
        }
        // H10: a data report always moves something.
        assert!(transferred > 0, "bulk IN data report with nothing (H10)");
        *toggle = self.rd16(d) & 0x0800;
        Ok(self.ram[IN_BUF as usize..][..transferred].to_vec())
    }

    /// Power-on to an operational link, as `init` and `attach_root` do it.
    fn boot(&mut self) {
        for i in 0..1024 {
            self.wr16(2 * i, 0);
        }
        // The nano program's first words, as the firmware loads them.
        for (i, w) in [0x0A9Du16, 0x83EF, 0x1234].into_iter().enumerate() {
            self.wr16(2 * i, w);
        }
        self.irq.clear(4);
        self.irq.mask |= 4;
        self.wr(0x800, 1);
        assert!(self.usb.started());
        self.run_for(650 * MS);
        assert!(self.words.is_empty(), "no report before the start delay");
        self.run_for(100 * MS);
        assert_eq!(self.words, [0xFFF1]);
        assert_eq!(self.rd16(at::STATUS), 1);
        self.queue.clear();
        // bus_reset: request, then the status after 50 ms.
        self.wr16(at::DO_RESET, 1);
        self.run_for(50 * MS);
        assert_eq!(self.rd16(at::STATUS), 3, "H8");
        assert_eq!(self.rd16(at::DO_RESET), 0);
        assert_eq!(self.rd16(at::LINK_SPEED), 2, "H9: the root is high speed");
    }

    /// `attach_root` → hub driver `install`, with the status pipe on 1.
    fn enumerate_hub(&mut self) {
        let dd = self.control_ok(0x0000, [0x80, 6, 0, 1, 0, 0, 18, 0], 18);
        assert_eq!(dd.len(), 18, "H14");
        assert_eq!((dd[4], dd[6], dd[7]), (9, 1, 64));
        assert_eq!(&dd[8..12], &[0x24, 0x04, 0x13, 0x25]);
        assert_eq!(
            self.control(0x0000, [0, 5, 1, 0, 0, 0, 0, 0], 0),
            Ok(vec![])
        );
        // The default address is free now.
        assert_eq!(
            self.control(0x0000, [0x80, 6, 0, 1, 0, 0, 18, 0], 18),
            Err(-1)
        );
        let lang = self.control_ok(0x0100, [0x80, 6, 0, 3, 0, 0, 4, 0], 4);
        assert_eq!(lang, [4, 3, 9, 4]);
        let c9 = self.control_ok(0x0100, [0x80, 6, 0, 2, 0, 0, 9, 0], 9);
        let total = usize::from(c9[2]) | usize::from(c9[3]) << 8;
        let cfg = self.control_ok(0x0100, [0x80, 6, 0, 2, 0, 0, c9[2], c9[3]], total);
        assert_eq!(cfg.len(), total, "H14");
        assert_eq!(cfg[9 + 5], 9, "hub interface");
        assert_eq!(cfg[18 + 2] & 0x80 | cfg[18 + 3] & 3, 0x83, "interrupt IN");
        self.control_ok(0x0100, [0, 9, cfg[5], 0, 0, 0, 0, 0], 0);
        let hd = self.control_ok(0x0100, [0xA0, 6, 0, 0x29, 0, 0, 64, 0], 64);
        assert_eq!((hd.len(), hd[2], hd[5]), (9, 3, 0x32));
        assert_eq!(
            self.control_ok(0x0100, [0xA0, 0, 0, 0, 0, 0, 4, 0], 64)
                .len(),
            4
        );
        for p in 1..=3 {
            self.control_ok(0x0100, [0x23, 3, 8, 0, p, 0, 0, 0], 64);
        }
        // allocate_input_pipe + activate_autopipe + resume_input_pipe.
        self.wr16(at::NUM_PIPES, 2);
        let d = desc(1);
        self.wr16(d + pipe::DEV_EP, 0x0101);
        self.wr16(d + pipe::MAX_TRANS, 64);
        self.wr16(d + pipe::INTERVAL, 1160);
        self.wr16(d + 0x0C, 0);
        self.wr16(d, 0x4042 | 0x0200);
        self.resume_hub_pipe();
    }

    fn resume_hub_pipe(&mut self) {
        let d = desc(1);
        self.wr16(d + pipe::LENGTH, 2);
        self.set_mem(1, HUB_BUF);
        self.wr16(d + pipe::STARTED, 0);
        let c = self.rd16(d);
        self.wr16(d, c & !0x0200);
    }

    /// Waits for a report of the hub's status pipe: its bitmap.
    fn hub_report(&mut self, within: u64) -> Option<u8> {
        let end = self.now + within;
        while !self.queue.contains(&1) && self.now < end {
            let t = (self.now + MS).min(end);
            self.advance(t);
        }
        let i = self.queue.iter().position(|&w| w == 1)?;
        self.queue.remove(i);
        let received = 2 - usize::from(self.rd16(desc(1) + pipe::LENGTH));
        assert!(received <= 2, "H12");
        assert_eq!(received, 1);
        Some(self.ram[HUB_BUF as usize])
    }

    fn port_status(&mut self, port: u8) -> (u16, u16) {
        let s = self.control_ok(0x0100, [0xA3, 0, 0, 0, port, 0, 4, 0], 8);
        assert_eq!(s.len(), 4, "H14");
        (
            u16::from_le_bytes([s[0], s[1]]),
            u16::from_le_bytes([s[2], s[3]]),
        )
    }

    /// The hub driver's `handle_irqdata` for a new device on `port`, then
    /// the device's `init`, `init2` and the storage driver's `install`.
    fn enumerate_stick(&mut self, port: u8, addr: u8) {
        let bm = self.hub_report(500 * MS).expect("connect report");
        assert_eq!(bm, 1 << port);
        let (st, ch) = self.port_status(port);
        assert_eq!((st, ch), (0x0101, 0x0001));
        self.control_ok(0x0100, [0x23, 1, 0x10, 0, port, 0, 0, 0], 8);
        self.control_ok(0x0100, [0x23, 3, 4, 0, port, 0, 0, 0], 8);
        self.resume_hub_pipe();
        let bm = self.hub_report(500 * MS).expect("reset report");
        assert_eq!(bm, 1 << port);
        let (st, ch) = self.port_status(port);
        assert_eq!((st, ch), (0x0503, 0x0010), "enabled, high speed");
        // init: device descriptor at 0, then the address.
        let dd = self.control_ok(0x0000, [0x80, 6, 0, 1, 0, 0, 18, 0], 18);
        assert_eq!(dd.len(), 18);
        assert_eq!((dd[4], dd[7]), (0, 64));
        self.control_ok(0x0000, [0, 5, addr, 0, 0, 0, 0, 0], 0);
        self.control_ok(0x0100, [0x23, 1, 0x14, 0, port, 0, 0, 0], 8);
        self.resume_hub_pipe();
        let a = u16::from(addr) << 8;
        // init2: strings with a 256-byte buffer, configuration.
        self.control_ok(a, [0x80, 6, 0, 3, 0, 0, 4, 0], 4);
        for i in 1..=3 {
            let s = self.control_ok(a, [0x80, 6, i, 3, 9, 4, 0xFF, 0], 256);
            assert_eq!(usize::from(s[0]), s.len());
        }
        let c9 = self.control_ok(a, [0x80, 6, 0, 2, 0, 0, 9, 0], 9);
        let total = usize::from(c9[2]);
        let cfg = self.control_ok(a, [0x80, 6, 0, 2, 0, 0, c9[2], 0], total);
        assert_eq!(cfg.len(), 32, "H14");
        assert_eq!(&cfg[14..17], &[8, 6, 0x50]);
        // install: configuration, GET MAX LUN.
        self.control_ok(a, [0, 9, cfg[5], 0, 0, 0, 0, 0], 0);
        let lun = self.control_ok(a, [0xA1, 0xFE, 0, 0, 0, 0, 1, 0], 8);
        assert_eq!(lun, [0], "H11");
    }

    /// One SCSI command through the Bulk-Only Transport: (data, status).
    fn scsi(
        &mut self,
        addr: u8,
        toggles: &mut (u16, u16),
        cdb: &[u8],
        data_in: usize,
        data_out: Option<&[u8]>,
    ) -> (Vec<u8>, u8) {
        let a = u16::from(addr) << 8;
        let len = data_out.map_or(data_in, <[u8]>::len) as u32;
        let mut cbw = vec![0u8; 31];
        cbw[..4].copy_from_slice(b"USBC");
        cbw[4..8].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        cbw[8..12].copy_from_slice(&len.to_le_bytes());
        cbw[12] = if data_out.is_some() { 0 } else { 0x80 };
        cbw[14] = cdb.len() as u8;
        cbw[15..15 + cdb.len()].copy_from_slice(cdb);
        assert_eq!(self.bulk_out(a | 2, &cbw, &mut toggles.1), 31);
        let mut data = Vec::new();
        if let Some(out) = data_out {
            if !out.is_empty() {
                assert_eq!(self.bulk_out(a | 2, out, &mut toggles.1), out.len() as i32);
            }
        } else {
            while data.len() < data_in {
                let chunk = (data_in - data.len()).min(49152);
                let got = self
                    .bulk_in(a | 1, chunk, 24000, &mut toggles.0)
                    .expect("data stage");
                data.extend_from_slice(&got);
            }
        }
        let csw = self.bulk_in(a | 1, 13, 24000, &mut toggles.0).expect("CSW");
        assert_eq!(csw.len(), 13);
        assert_eq!(&csw[..4], b"USBS");
        assert_eq!(&csw[4..8], &0x1234_5678u32.to_le_bytes());
        (data, csw[12])
    }

    fn with_stick(sectors: usize) -> Self {
        let mut r = Self::new();
        let img: Vec<u8> = (0..sectors * SECTOR)
            .map(|i| (i * 7 + i / 512) as u8)
            .collect();
        r.storage.set(unit::USB0, Some(img));
        r.boot();
        r.enumerate_hub();
        r.enumerate_stick(1, 2);
        r
    }
}

#[test]
fn t0_window_takes_every_width_and_never_interrupts_unstarted() {
    let mut r = Rig::new();
    assert!(UsbHost::claims(0x08_0000) && UsbHost::claims(0x08_0FFF));
    assert!(!UsbHost::claims(0x08_1000) && !UsbHost::claims(0x07_FFFF));
    // H1: the pre-scheduler clear, every width.
    r.wr(0x800, 0);
    for i in 0..1024 {
        r.wr16(2 * i, 0);
    }
    for (i, b) in [0x9D, 0x0A, 0xEF, 0x83].into_iter().enumerate() {
        r.wr(i, b);
    }
    let dw: u32 = (0..4).fold(0, |a, i| {
        a | u32::from(r.with(|u, c| u.read(BASE + i, c))) << (8 * i)
    });
    assert_eq!(dw, 0x83EF_0A9D, "H4: first DW");
    assert_eq!(r.with(|u, c| u.read(BASE + 0x800, c)), 0);
    assert_eq!(r.usb.peek(BASE + 0xFFF), 0);
    assert_eq!(r.usb.next_event(), u64::MAX);
    r.run_for(3 * CLOCK_HZ);
    assert_eq!(r.irq.flags, 0, "H5: never an interrupt while held");
    assert_eq!((r.rd16(at::HEAD), r.rd16(at::TAIL)), (0, 0));
    // Any address of the upper half is the run register.
    r.wr(0xABC, 1);
    assert!(r.usb.started());
    assert_eq!(r.rd16(at::NUM_PIPES), 1);
    r.wr(0x801, 0);
    assert!(!r.usb.started());
    assert_eq!(r.usb.next_event(), u64::MAX);
}

#[test]
fn start_attach_and_bus_reset() {
    let mut r = Rig::new();
    r.boot();
    // H15: every push latches the edge again.
    assert_eq!(r.words, [0xFFF1]);
    assert_eq!(r.irq.flags & 4, 0);
}

#[test]
fn no_hub_no_attach_until_it_comes() {
    let mut r = Rig::new();
    r.usb.set_root_attached(false);
    r.irq.mask |= 4;
    r.wr(0x800, 1);
    r.run_for(2 * CLOCK_HZ);
    assert!(r.words.is_empty());
    assert_eq!(r.rd16(at::STATUS), 0);
    r.usb.set_root_attached(true);
    r.run_for(800 * MS);
    assert_eq!(r.words, [0xFFF1]);
}

#[test]
fn hub_enumerates_like_the_firmware() {
    let mut r = Rig::new();
    r.boot();
    r.enumerate_hub();
    // No device: the status pipe stays quiet (H13).
    assert_eq!(r.hub_report(2 * CLOCK_HZ), None);
    // A request to an address nobody has: error, no hang.
    assert_eq!(r.control(0x0500, [0x80, 6, 0, 1, 0, 0, 18, 0], 18), Err(-1));
    // Hub GET_STATUS(port) for a port that does not exist: STALL.
    assert_eq!(r.control(0x0100, [0xA3, 0, 0, 0, 4, 0, 4, 0], 8), Err(-4));
}

#[test]
fn stick_enumerates_and_reads_and_writes() {
    let mut r = Rig::with_stick(64);
    let mut t = (0, 0);
    let (inq, st) = r.scsi(2, &mut t, &[0x12, 0, 0, 0, 36, 0], 36, None);
    assert_eq!((inq.len(), st, inq[0], inq[1]), (36, 0, 0, 0x80));
    assert_eq!(&inq[8..13], b"mimic");
    let (sense, st) = r.scsi(2, &mut t, &[0x03, 0, 0, 0, 18, 0], 18, None);
    assert_eq!((sense.len(), st, sense[0], sense[2]), (18, 0, 0x70, 0));
    let (_, st) = r.scsi(2, &mut t, &[0; 6], 0, Some(&[]));
    assert_eq!(st, 0, "TEST UNIT READY");
    let (cap, st) = r.scsi(2, &mut t, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], 8, None);
    assert_eq!(st, 0);
    assert_eq!(cap, [0, 0, 0, 63, 0, 0, 2, 0]);
    // WRITE(10) two sectors at 5, READ(10) them back.
    let data: Vec<u8> = (0..1024).map(|i| (i as u8) ^ 0x5A).collect();
    let (_, st) = r.scsi(
        2,
        &mut t,
        &[0x2A, 0, 0, 0, 0, 5, 0, 0, 2, 0],
        0,
        Some(&data),
    );
    assert_eq!(st, 0);
    let img = r.storage.units[usize::from(unit::USB0)].as_ref().unwrap();
    assert_eq!(&img[5 * SECTOR..7 * SECTOR], &data[..]);
    let (back, st) = r.scsi(2, &mut t, &[0x28, 0, 0, 0, 0, 5, 0, 0, 2, 0], 1024, None);
    assert_eq!((st, back), (0, data));
    // A larger read across bulk chunks.
    let (big, st) = r.scsi(
        2,
        &mut t,
        &[0x28, 0, 0, 0, 0, 0, 0, 0, 64, 0],
        64 * 512,
        None,
    );
    assert_eq!(st, 0);
    let img = r.storage.units[usize::from(unit::USB0)].as_ref().unwrap();
    assert_eq!(&big[..], &img[..]);
    // Past the end: padded data, failed status, sense 05/21.
    let (pad, st) = r.scsi(2, &mut t, &[0x28, 0, 0, 0, 0, 63, 0, 0, 2, 0], 1024, None);
    assert_eq!((pad.len(), st), (1024, 1));
    assert!(pad.iter().all(|&b| b == 0));
    let (sense, _) = r.scsi(2, &mut t, &[0x03, 0, 0, 0, 18, 0], 18, None);
    assert_eq!((sense[2], sense[12]), (5, 0x21));
    // An unknown command fails, the sense says so, the next one is fine.
    let (_, st) = r.scsi(2, &mut t, &[0xE7, 0, 0, 0, 0, 0], 0, Some(&[]));
    assert_eq!(st, 1);
    let (sense, _) = r.scsi(2, &mut t, &[0x03, 0, 0, 0, 18, 0], 18, None);
    assert_eq!((sense[2], sense[12]), (5, 0x20));
    let (sense, _) = r.scsi(2, &mut t, &[0x03, 0, 0, 0, 18, 0], 18, None);
    assert_eq!(sense[2], 0);
}

#[test]
fn write_protected_stick_refuses_writes() {
    let mut r = Rig::with_stick(16);
    r.storage.protected[usize::from(unit::USB0)] = true;
    let mut t = (0, 0);
    let (ms, st) = r.scsi(2, &mut t, &[0x1A, 0, 0x3F, 0, 4, 0], 4, None);
    assert_eq!((st, ms[2] & 0x80), (0, 0x80), "MODE SENSE: write protected");
    let before = r.storage.units[usize::from(unit::USB0)].clone();
    let (_, st) = r.scsi(
        2,
        &mut t,
        &[0x2A, 0, 0, 0, 0, 1, 0, 0, 1, 0],
        0,
        Some(&[0xEE; 512]),
    );
    assert_eq!(st, 1);
    assert_eq!(r.storage.units[usize::from(unit::USB0)], before);
    let (sense, _) = r.scsi(2, &mut t, &[0x03, 0, 0, 0, 18, 0], 18, None);
    assert_eq!((sense[2], sense[12]), (7, 0x27));
    r.storage.protected[usize::from(unit::USB0)] = false;
    let (ms, _) = r.scsi(2, &mut t, &[0x1A, 0, 0x3F, 0, 4, 0], 4, None);
    assert_eq!(ms[2] & 0x80, 0);
}

#[test]
fn bulk_in_outside_a_stage_naks_into_timeout_and_abort() {
    let mut r = Rig::with_stick(16);
    let mut tog = 0;
    // No command: NAK; the timeout of 80 frames (10 ms) ends it.
    let t0 = r.now;
    let e = r.bulk_in(0x0201, 13, 80, &mut tog).unwrap_err();
    assert_eq!(e & 0xF800, 0x2800, "NAK with the timeout mark");
    assert!(r.now - t0 >= 80 * FRAME_CLOCKS);
    // Timeout 0: NAK for ever until the abort request (H7).
    let e = r.bulk_in(0x0201, 13, 0, &mut tog).unwrap_err();
    assert_eq!(e & 0xF000, 0x6000, "aborted");
    // The device still works afterwards.
    let mut t = (tog, 0);
    let (_, st) = r.scsi(2, &mut t, &[0; 6], 0, Some(&[]));
    assert_eq!(st, 0);
}

#[test]
fn hot_plug_unplug_and_replug() {
    let mut r = Rig::with_stick(16);
    assert!(r.usb.port_has_device(1));
    // Unplug.
    r.storage.set(unit::USB0, None);
    let bm = r.hub_report(500 * MS).expect("disconnect report");
    assert_eq!(bm, 2);
    assert_eq!(r.port_status(1), (0x0100, 0x0001));
    assert!(!r.usb.port_has_device(1));
    // A new medium before the firmware has cleared the change: waits.
    r.storage.set(unit::USB0, Some(vec![0; 8 * SECTOR]));
    r.run_for(400 * MS);
    assert!(!r.usb.port_has_device(1));
    r.control_ok(0x0100, [0x23, 1, 0x10, 0, 1, 0, 0, 0], 8);
    r.resume_hub_pipe();
    // Then it comes, and enumerates again at a new address.
    r.enumerate_stick(1, 3);
    let mut t = (0, 0);
    let (cap, _) = r.scsi(3, &mut t, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], 8, None);
    assert_eq!(cap[3], 7);
    // The old address is gone.
    assert_eq!(r.control(0x0200, [0x80, 6, 0, 1, 0, 0, 18, 0], 18), Err(-1));
    // A replaced medium (same unit, new generation) is a new stick.
    r.storage.set(unit::USB0, Some(vec![0; 4 * SECTOR]));
    assert_eq!(r.hub_report(500 * MS), Some(2));
    assert_eq!(r.port_status(1).0 & 1, 0);
}

#[test]
fn second_port_and_quiet_status_pipe() {
    let mut r = Rig::with_stick(16);
    // H13: nothing changed, no report for seconds.
    assert_eq!(r.hub_report(2 * CLOCK_HZ), None);
    r.storage.set(unit::USB0 + 1, Some(vec![1; 16 * SECTOR]));
    r.enumerate_stick(2, 4);
    let mut t = (0, 0);
    let (d, st) = r.scsi(4, &mut t, &[0x28, 0, 0, 0, 0, 0, 0, 0, 1, 0], 512, None);
    assert_eq!((st, d[0]), (0, 1));
}

#[test]
fn h16_a_pipe_waits_a_frame_after_its_last_write() {
    let mut r = Rig::with_stick(16);
    // Arm the hub pipe as the hub driver does, with a change pending.
    r.storage.set(unit::USB0 + 2, Some(vec![0; 16 * SECTOR]));
    r.run_for(20 * MS);
    r.run_for(200 * MS);
    // The pipe is paused now (it reported or waits); re-arm it.
    r.queue.clear();
    r.ram[HUB_BUF as usize] = 0;
    r.resume_hub_pipe();
    let armed = r.now;
    r.advance(armed + FRAME_CLOCKS - 1);
    assert!(!r.queue.contains(&1), "no answer within the frame");
    assert_eq!(r.rd16(desc(1)) & 0x0200, 0);
    // The driver clears its buffer after resuming: the bitmap survives.
    r.ram[HUB_BUF as usize] = 0;
    assert_eq!(r.hub_report(500 * MS), Some(8));
}

#[test]
fn full_fifo_holds_reports_back() {
    let mut r = Rig::new();
    r.boot();
    // The interrupt routine does not run: mask the source.
    r.irq.mask = 0;
    r.irq.clear(0xFF);
    let d = desc(0);
    let mut done = 0;
    for _ in 0..20 {
        r.ram[SETUP_BUF as usize..][..8].copy_from_slice(&[0x80, 6, 0, 1, 0, 0, 18, 0]);
        r.wr16(d + pipe::DEV_EP, 0);
        r.wr16(d + pipe::LENGTH, 8);
        r.wr16(d + pipe::MAX_TRANS, 64);
        r.set_mem(0, SETUP_BUF);
        r.wr16(d, 0x8040);
        r.run_for(2 * MS);
        if r.rd16(d) & 0x0200 != 0 {
            done += 1;
        }
    }
    // 15 entries fit; the 16th waits.
    assert_eq!(done, 15);
    let tail = r.rd16(at::TAIL);
    assert_eq!(r.rd16(at::HEAD), (tail + 15) % 16);
    assert_ne!(r.irq.flags & 4, 0, "the edge latched while masked");
    // Draining lets it go.
    r.irq.mask = 4;
    r.isr();
    r.run_for(2 * MS);
    assert_ne!(r.rd16(d) & 0x0200, 0);
}

#[test]
fn root_detach_reports_and_reattach_starts_again() {
    let mut r = Rig::new();
    r.boot();
    r.enumerate_hub();
    r.usb.set_root_attached(false);
    r.run_for(MS);
    assert_eq!(r.words.last(), Some(&0xFFF0));
    assert_eq!(r.rd16(at::STATUS), 0x8000);
    r.usb.set_root_attached(true);
    r.run_for(800 * MS);
    assert_eq!(r.words.last(), Some(&0xFFF1));
    // The hub starts again at address 0 after the next bus reset.
    r.wr16(at::DO_RESET, 1);
    r.run_for(50 * MS);
    assert_eq!(r.rd16(at::STATUS), 3);
    let dd = r.control_ok(0x0000, [0x80, 6, 0, 1, 0, 0, 18, 0], 18);
    assert_eq!(dd[4], 9);
}

#[test]
fn bus_reset_while_operational_resets_the_hub() {
    let mut r = Rig::with_stick(16);
    r.wr16(at::DO_RESET, 1);
    r.run_for(50 * MS);
    assert_eq!(r.rd16(at::STATUS), 3);
    assert_eq!(r.control(0x0100, [0x80, 6, 0, 1, 0, 0, 18, 0], 18), Err(-1));
    assert_eq!(r.usb.port_status(1), (0, 0), "ports switched off");
    assert!(r.usb.port_has_device(1), "the stick stays plugged in");
}

#[test]
fn garbage_never_panics() {
    let mut r = Rig::with_stick(16);
    let mut x = 0x1234_5678_9ABC_DEF0u64;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for _ in 0..20_000 {
        let v = next();
        let off = (v % 0x1000) as usize;
        // Mostly the descriptors and the status words.
        let off = match v >> 60 {
            0..=9 => 0x600 + off % 0xC0,
            10..=12 => 0x7C0 + off % 0x40,
            _ => off,
        };
        r.wr(off, (v >> 20) as u8);
        if v & 0xF == 0 {
            r.run_for((v >> 40) % (3 * MS));
        }
        let _ = r.with(|u, c| u.read(BASE + ((v >> 32) as u32 & 0xFFF), c));
    }
    r.run_for(2 * CLOCK_HZ);
}

#[test]
fn savestate_round_trip_mid_transfer_and_truncated() {
    let mut r = Rig::with_stick(16);
    let mut t = (0, 0);
    // A READ(10) whose data stage is under way.
    let mut cbw = vec![0u8; 31];
    cbw[..4].copy_from_slice(b"USBC");
    cbw[8..12].copy_from_slice(&1024u32.to_le_bytes());
    cbw[12] = 0x80;
    cbw[14] = 10;
    cbw[15..25].copy_from_slice(&[0x28, 0, 0, 0, 0, 3, 0, 0, 2, 0]);
    assert_eq!(r.bulk_out(0x0202, &cbw, &mut t.1), 31);
    let first = r.bulk_in(0x0201, 512, 24000, &mut t.0).unwrap();

    let mut w = StateWriter::new();
    r.usb.save(&mut w);
    let bytes = w.into_bytes();
    let mut copy = UsbHost::new();
    copy.load(&mut StateReader::new(&bytes)).unwrap();
    let mut w2 = StateWriter::new();
    copy.save(&mut w2);
    assert_eq!(w2.into_bytes(), bytes, "the same state again");
    assert_eq!(copy.next_event(), r.usb.next_event());

    // Both go on alike.
    let mut r2 = Rig::new();
    r2.ram = r.ram.clone();
    r2.storage = r.storage.clone();
    r2.now = r.now;
    r2.irq = r.irq.clone();
    r2.usb = copy;
    let mut t2 = t;
    let rest = r.bulk_in(0x0201, 512, 24000, &mut t.0).unwrap();
    let rest2 = r2.bulk_in(0x0201, 512, 24000, &mut t2.0).unwrap();
    assert_eq!(rest, rest2);
    let img = r.storage.units[usize::from(unit::USB0)].as_ref().unwrap();
    assert_eq!(&img[3 * SECTOR..4 * SECTOR], &first[..]);
    assert_eq!(&img[4 * SECTOR..5 * SECTOR], &rest[..]);
    assert_eq!(
        r.bulk_in(0x0201, 13, 24000, &mut t.0).unwrap(),
        r2.bulk_in(0x0201, 13, 24000, &mut t2.0).unwrap()
    );

    let mut d = UsbHost::new();
    for cut in 0..bytes.len() {
        assert!(
            d.load(&mut StateReader::new(&bytes[..cut])).is_err(),
            "cut {cut}"
        );
    }
    // A link state that does not exist.
    let mut bad = bytes.clone();
    bad[10 + RAM + 1] = 9;
    assert!(d.load(&mut StateReader::new(&bad)).is_err());
}

#[test]
fn reset_stops_the_nano_and_keeps_the_hub_setting() {
    let mut r = Rig::with_stick(16);
    r.usb.set_root_attached(false);
    r.usb.reset();
    assert!(!r.usb.started());
    assert!(!r.usb.root_attached());
    assert_eq!(r.usb.next_event(), u64::MAX);
    assert!(r.usb.ram().iter().all(|&b| b == 0));
}
