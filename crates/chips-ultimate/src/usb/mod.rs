//! The USB host (`$080000`–`$080FFF`): the FPGA's USB 2.0 core with its
//! small "nano" processor, the board's USB2513 hub behind it and USB
//! sticks on the hub's ports.
//!
//! The firmware never drives USB itself. It loads a program into the
//! nano's 2 KB of memory, lets the nano run, and from then on talks to that
//! program only through the shared memory: pipe descriptors it fills in,
//! a FIFO of reports the nano fills, status words, and one interrupt per
//! report. This model runs no nano code; it does what that program does
//! with the shared memory (high-level emulation), and below it a device
//! layer answers the transactions the program would put on the wire.
//!
//! # Registers
//!
//! The memory is byte addressed from the processor; the nano works on
//! 16-bit words, little endian: nano word `w` is bytes `2w` and `2w+1`.
//! Bit 11 of the address selects the memory (`$080000`–`$0807FF`) or the
//! run register (`$080800`–`$080FFF`, every address the same):
//!
//! | Address | Word | Meaning |
//! |---|---|---|
//! | `$080000`–`$0805FF` | | program and data of the nano (kept, not run) |
//! | `$080600` + `$18`·n | | pipe descriptor n (0–7), twelve words, see below |
//! | `$080700`–`$08071F` | `$380` | report FIFO, 16 words |
//! | `$0807CC` | `$3E6` | link status: bit 0 connected, bit 1 operational (bus reset done); `$8000` after a disconnect |
//! | `$0807D4` | `$3EA` | frame number of the last report of pipe 0 |
//! | `$0807D6` | `$3EB` | simulation switch of the program (not modelled) |
//! | `$0807D8` | `$3EC` | suspend request (not modelled) |
//! | `$0807DA` | `$3ED` | bus reset request: the firmware writes 1, the nano clears it and resets the bus |
//! | `$0807DC` | `$3EE` | link speed after the reset: 0 low, 1 full, 2 high |
//! | `$0807DE` | `$3EF` | number of pipes the nano looks at (1–8); the nano sets 1 at start |
//! | `$0807F0` | `$3F8` | FIFO tail, written by the firmware only |
//! | `$0807F2` | `$3F9` | FIFO head, written by the nano only |
//! | `$080800`–`$080FFF` | | write: bit 0 = 1 lets the nano run, 0 holds it in reset; reads 0 |
//!
//! Pipe descriptor, words: 0 command, 1 address << 8 | endpoint, 2 bytes
//! still to move, 3 packet size, 4 interval (frames of 125 µs), 5 frame of
//! the last attempt, 6 split control, 7 result, 8–9 memory address (low,
//! high; high 0: the pipe is skipped), 10 frame of the first attempt, 11
//! timeout in frames (0: retry a NAK for ever).
//!
//! Command bits: 15 data from memory (and the OUT/SETUP path), 14 data to
//! memory, 13 ping (nano's own), 11 data toggle, 10 retry on NAK (no
//! effect), 9 done / paused, 8 abort request, 6 data stage, 1–0 the token
//! (0 SETUP, 1 OUT, 2 IN, 3 PING). The command word 0 means: pipe free.
//!
//! Result word: 15 done, 14–12 code (0 data, 1 ACK, 2 NAK, 3 NYET, 4
//! STALL, 5 error, 6 aborted), 11 toggle of the received data (also the
//! timeout mark), 10 zero-length packet, 9–0 length of the last packet.
//!
//! # What the nano's program does, and so this model
//!
//! - **Start** (run bit 0 → 1): pipe count 1, pipe 0 command 0. About
//!   700 ms later, when the hub is there: link status 1, report `$FFF1`.
//!   While no hub is there, nothing.
//! - **Bus reset** (the firmware writes the reset request): request and
//!   status cleared, the bus is reset (the hub starts again unconfigured),
//!   10 ms later link speed 2 and status 3, without a report. The firmware
//!   checks 50 ms after its request (09 H8).
//! - **Disconnect** of the hub: status `$8000`, report `$FFF0`, then the
//!   start delay again.
//! - **Pipes**, while the link is operational: pipes 0 to count−1 are
//!   looked at; a pipe with command 0 or with the done bit is skipped, and
//!   so is one whose interval has not passed since its last attempt, one
//!   whose memory address high word is 0, and all while the FIFO is full.
//!   The frame of the first attempt is stamped when it is 0.
//!   - SETUP / OUT: packets of up to the packet size from memory; on ACK
//!     the toggle flips and address and count move on; the report comes
//!     when the count is 0.
//!   - IN: packets to memory (the data is cut to the count); on data the
//!     toggle flips, address and count move on, and the next packet
//!     follows while the count is not 0 and the packet was full; then the
//!     report.
//!   - NAK: with the abort request set the result becomes "aborted" and is
//!     reported; with a timeout that has run out the result gets bit 11
//!     and is reported; else the pipe is tried again later.
//!   - STALL or no answer at all (no device at that address: "error"): the
//!     report.
//! - **Report**: the pipe number into the FIFO at the head, head + 1, the
//!   done bit into the command, for pipe 0 the frame number into `$0807D4`,
//!   and an interrupt: a pulse on low source 2 of the ITU ([`IRQ_LOW`]),
//!   an edge the ITU latches (09 H15). A status change pushes `$FFF0` |
//!   status the same way.
//!
//! The firmware drains the FIFO in its interrupt routine (pipe 0 word 0
//! wakes the waiting command, others go to their drivers) and writes the
//! tail; the FIFO words are only ever 0, a pipe number 1–7 or `$FFFx`
//! (09 H5, H6).
//!
//! # Deviations from the real program
//!
//! - A whole transfer runs at one scan; the real program sends one OUT
//!   packet per pass. The firmware only looks at the result, the count and
//!   the toggle bit afterwards, so it cannot tell.
//! - A pipe is looked at no earlier than one frame (125 µs) after the
//!   firmware last wrote its descriptor (09 H16: the hub driver re-arms its
//!   status pipe before it clears the bitmap buffer); the real program
//!   needs about that long for a pass anyway.
//! - An IN token asks the device for at most the remaining count (a real
//!   device may send up to its packet size, the program would cut it);
//!   so the count never goes below 0 (09 H12).
//! - Split transactions, PING, NYET, suspend, the 3 retries on a bus error
//!   and data toggle mismatches do not happen: every device here is a
//!   high-speed device that answers at once with the toggle expected. The
//!   split control word and the ping bit are ignored. The root device
//!   reports high speed (09 H9: the firmware computes a split for a
//!   full-speed root from a null pointer).
//! - The ULPI transceiver, VBUS switching and line states are not there;
//!   the start delay is a plain 700 ms.
//! - A NAK with neither timeout nor abort is retried after the pipe's
//!   interval, at least 1 ms later.
//!
//! The hub and the sticks are in [`hub`] and [`storage`]; a HID keyboard or
//! mouse would be another [`device::Function`] on a hub port (not built).
//!
//! The firmware uses USB only when the ITU's capability word has bit 23
//! (`CAPAB_USB_HOST2`, [`CAPAB_USB_HOST2`]) set; then it loads the program
//! at boot.
//!
//! Sources (facts only): UE2-C64U-Emulator `docs/hw/09-usb.md` (address
//! map, descriptor and result layout, hazards H1–H16, the program's
//! behaviour F1–F8), `docs/hw/00-memory-map.md` (USB rows, C31, M10),
//! `docs/specs/S11-S14-later.md` §S13, `docs/specs/S33-usb-plug.md`,
//! `docs/status/usb.md`; 1541 Ultimate `software/io/usb/usb_nano.h`
//! (layout, bits), `usb_base.cc`/`.h` (the firmware's side of every
//! transfer, FIFO drain), `usb_device.cc`, `usb_hub.cc`, `usb_scsi.cc`
//! (requests and lengths), `nano_minimal.nan` (the program: start, reset,
//! pipe scan, NAK/timeout/abort rules, reports), `fpga/ip/nano_cpu/
//! vhdl_source/nano.vhd` (bit-11 split, run register).

mod device;
mod hub;
mod storage;
#[cfg(test)]
mod tests;

use crate::io::{IoCtx, IoDevice, CLOCK_HZ};
use crate::irq::Irq;
use device::{Env, Response};
use hub::Hub;
use mimic_core::{StateError, StateReader, StateWriter};

pub use hub::{PORTS, PORT_RESET_CLOCKS, REPLUG_GAP_CLOCKS};

/// First address of the block.
pub const BASE: u32 = 0x08_0000;
/// The ITU's low interrupt source of the block (an edge).
pub const IRQ_LOW: u8 = 2;
/// The capability bit the firmware needs to use USB at all (ITU
/// capability word, bit 23).
pub const CAPAB_USB_HOST2: u32 = 1 << 23;

/// Size of the shared memory.
const RAM: usize = 0x800;
/// One USB frame (micro-frame) of the nano's counter: 125 µs.
pub const FRAME_CLOCKS: u64 = CLOCK_HZ / 8_000;
/// From start (or a disconnect) to the attach report: 700 ms.
pub const SETTLE_CLOCKS: u64 = CLOCK_HZ / 1_000 * 700;
/// A bus reset of the root port: 10 ms.
pub const BUS_RESET_CLOCKS: u64 = CLOCK_HZ / 100;
/// How soon the program sees a bus reset request: 10 µs.
const REQUEST_CLOCKS: u64 = 1_000;
/// How often the media of the storage units are compared: 10 ms.
pub const MEDIA_POLL_CLOCKS: u64 = CLOCK_HZ / 100;
/// A NAK without interval is tried again after 8 frames (1 ms).
const NAK_RETRY_FRAMES: u64 = 8;
/// The pipe descriptors.
const PIPES: usize = 8;

/// Byte offsets in the shared memory.
mod at {
    pub const PIPES: usize = 0x600;
    pub const PIPE_SIZE: usize = 0x18;
    pub const FIFO: usize = 0x700;
    pub const STATUS: usize = 0x7CC;
    pub const REPORT_FRAME: usize = 0x7D4;
    pub const DO_RESET: usize = 0x7DA;
    pub const LINK_SPEED: usize = 0x7DC;
    pub const NUM_PIPES: usize = 0x7DE;
    pub const TAIL: usize = 0x7F0;
    pub const HEAD: usize = 0x7F2;
}

/// Byte offsets in a pipe descriptor.
mod pipe {
    pub const COMMAND: usize = 0x00;
    pub const DEV_EP: usize = 0x02;
    pub const LENGTH: usize = 0x04;
    pub const MAX_TRANS: usize = 0x06;
    pub const INTERVAL: usize = 0x08;
    pub const LAST_FRAME: usize = 0x0A;
    pub const RESULT: usize = 0x0E;
    pub const MEM_LO: usize = 0x10;
    pub const MEM_HI: usize = 0x12;
    pub const STARTED: usize = 0x14;
    pub const TIMEOUT: usize = 0x16;
}

/// Command bits.
mod cmd {
    pub const MEMREAD: u16 = 0x8000;
    pub const MEMWRITE: u16 = 0x4000;
    pub const TOGGLE: u16 = 0x0800;
    pub const DONE: u16 = 0x0200;
    pub const ABORT_REQ: u16 = 0x0100;
}

/// Result words as the core gives them (bit 15: done).
mod res {
    pub const DATA: u16 = 0x8000;
    pub const ACK: u16 = 0x9000;
    pub const NAK: u16 = 0xA000;
    pub const STALL: u16 = 0xC000;
    pub const ERROR: u16 = 0xD000;
    pub const ABORTED: u16 = 0x6000;
    pub const TIMEOUT: u16 = 0x0800;
    pub const NO_DATA: u16 = 0x0400;
}

/// The DMA reaches 26 bits.
const DMA_MASK: u32 = 0x03FF_FFFF;

/// Where the program is with the link to the hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Link {
    /// Held in reset.
    Off,
    /// The start delay; at its end the hub is looked for.
    Settling,
    /// No hub; waiting for one.
    WaitAttach,
    /// Attached and reported, waiting for the bus reset request.
    Attached,
    /// Bus reset under way.
    Resetting,
    /// Pipes are served.
    Operational,
}

impl Link {
    const ALL: [Link; 6] = [
        Link::Off,
        Link::Settling,
        Link::WaitAttach,
        Link::Attached,
        Link::Resetting,
        Link::Operational,
    ];
}

/// What one look at a pipe ended with.
enum Outcome {
    Report,
    /// Try again at this time.
    Later(u64),
}

/// The USB host block.
#[derive(Clone, Debug)]
pub struct UsbHost {
    ram: Vec<u8>,
    /// The run bit.
    run: bool,
    link: Link,
    /// When the link state has its next step; `u64::MAX`: never.
    link_at: u64,
    /// Per pipe: not looked at before this time (one frame after the last
    /// write to its descriptor).
    armed: [u64; PIPES],
    /// The next pass over the pipes.
    scan_at: u64,
    /// The next comparison of the storage units' media.
    media_at: u64,
    /// The hub is connected (host side: not held in reset).
    root_attached: bool,
    /// `root_attached` changed since the last tick.
    root_changed: bool,
    hub: Hub,
    /// Packet buffer (not state).
    packet: Vec<u8>,
}

impl Default for UsbHost {
    fn default() -> Self {
        Self::new()
    }
}

impl UsbHost {
    pub fn new() -> Self {
        Self {
            ram: vec![0; RAM],
            run: false,
            link: Link::Off,
            link_at: u64::MAX,
            armed: [0; PIPES],
            scan_at: u64::MAX,
            media_at: u64::MAX,
            root_attached: true,
            root_changed: false,
            hub: Hub::new(),
            packet: Vec::new(),
        }
    }

    /// The I/O addresses of this block.
    pub fn claims(addr: u32) -> bool {
        (BASE..=BASE + 0xFFF).contains(&addr)
    }

    /// Connect or disconnect the hub (e.g. while the board holds it in
    /// reset). Connected after power-on.
    pub fn set_root_attached(&mut self, on: bool) {
        if on != self.root_attached {
            self.root_attached = on;
            self.root_changed = true;
        }
    }

    pub fn root_attached(&self) -> bool {
        self.root_attached
    }

    /// The firmware lets the nano run.
    pub fn started(&self) -> bool {
        self.run
    }

    /// The link status word (`$0807CC`).
    pub fn link_status(&self) -> u16 {
        self.rd16(at::STATUS)
    }

    /// A stick sits on hub port `port` (1–3).
    pub fn port_has_device(&self, port: usize) -> bool {
        port.checked_sub(1)
            .and_then(|i| self.hub.ports.get(i))
            .is_some_and(|p| p.device.is_some())
    }

    /// wPortStatus and wPortChange of hub port `port` (1–3).
    pub fn port_status(&self, port: usize) -> (u16, u16) {
        port.checked_sub(1)
            .and_then(|i| self.hub.ports.get(i))
            .map_or((0, 0), |p| (p.status(), p.change))
    }

    /// The shared memory.
    pub fn ram(&self) -> &[u8] {
        &self.ram
    }

    fn rd16(&self, off: usize) -> u16 {
        u16::from_le_bytes([self.ram[off], self.ram[off + 1]])
    }

    fn wr16(&mut self, off: usize, v: u16) {
        self.ram[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }

    fn start(&mut self, now: u64) {
        self.run = true;
        self.wr16(at::NUM_PIPES, 1);
        self.wr16(at::PIPES + pipe::COMMAND, 0);
        self.link = Link::Settling;
        self.link_at = now.saturating_add(SETTLE_CLOCKS);
        self.armed = [0; PIPES];
        self.scan_at = u64::MAX;
        self.media_at = now;
    }

    fn stop(&mut self) {
        self.run = false;
        self.link = Link::Off;
        self.link_at = u64::MAX;
        self.scan_at = u64::MAX;
        self.media_at = u64::MAX;
    }

    /// A word into the report FIFO and the interrupt pulse.
    fn push(&mut self, word: u16, irq: &mut Irq) {
        let head = usize::from(self.rd16(at::HEAD) & 0xF);
        self.wr16(at::FIFO + 2 * head, word);
        self.wr16(at::HEAD, ((head + 1) & 0xF) as u16);
        irq.pulse(IRQ_LOW);
    }

    fn fifo_full(&self) -> bool {
        (self.rd16(at::HEAD).wrapping_add(1) & 0xF) == self.rd16(at::TAIL)
    }

    fn root_event(&mut self, ctx: &mut IoCtx<'_>) {
        if !self.root_attached {
            if matches!(
                self.link,
                Link::Attached | Link::Resetting | Link::Operational
            ) {
                self.wr16(at::STATUS, 0x8000);
                self.push(0xFFF0, ctx.irq);
                self.hub.bus_reset();
                self.link = Link::Settling;
                self.link_at = ctx.now.saturating_add(SETTLE_CLOCKS);
            }
        } else if self.link == Link::WaitAttach {
            self.link = Link::Settling;
            self.link_at = ctx.now.saturating_add(SETTLE_CLOCKS);
        }
    }

    fn link_event(&mut self, ctx: &mut IoCtx<'_>) {
        let now = ctx.now;
        self.link_at = u64::MAX;
        match self.link {
            Link::Settling => {
                if self.root_attached {
                    self.wr16(at::STATUS, 1);
                    self.push(0xFFF1, ctx.irq);
                    self.link = Link::Attached;
                    // A request already there is seen at once.
                    self.link_at = now.saturating_add(REQUEST_CLOCKS);
                } else {
                    self.wr16(at::STATUS, 0);
                    self.link = Link::WaitAttach;
                }
            }
            Link::Attached | Link::Operational => {
                if self.rd16(at::DO_RESET) != 0 {
                    self.wr16(at::DO_RESET, 0);
                    self.wr16(at::STATUS, 0);
                    self.hub.bus_reset();
                    self.link = Link::Resetting;
                    self.link_at = now.saturating_add(BUS_RESET_CLOCKS);
                }
            }
            Link::Resetting => {
                self.wr16(at::LINK_SPEED, 2);
                self.wr16(at::STATUS, 3);
                self.link = Link::Operational;
                self.scan_at = self.scan_at.min(now);
                // A request written during the reset.
                if self.rd16(at::DO_RESET) != 0 {
                    self.link_at = now;
                }
            }
            Link::Off | Link::WaitAttach => {}
        }
    }

    /// One pass over the pipes.
    fn scan(&mut self, ctx: &mut IoCtx<'_>) {
        self.scan_at = u64::MAX;
        if self.link != Link::Operational {
            return;
        }
        let now = ctx.now;
        let frame = (now / FRAME_CLOCKS) as u16;
        let frame_start = now - now % FRAME_CLOCKS;
        let count = usize::from(self.rd16(at::NUM_PIPES)).clamp(1, PIPES);
        let mut wake = u64::MAX;
        for n in 0..count {
            let base = at::PIPES + n * at::PIPE_SIZE;
            let command = self.rd16(base + pipe::COMMAND);
            if command == 0 || command & cmd::DONE != 0 {
                continue;
            }
            if now < self.armed[n] {
                wake = wake.min(self.armed[n]);
                continue;
            }
            let interval = self.rd16(base + pipe::INTERVAL);
            let elapsed = frame.wrapping_sub(self.rd16(base + pipe::LAST_FRAME));
            if elapsed < interval {
                let t = frame_start + u64::from(interval - elapsed) * FRAME_CLOCKS;
                wake = wake.min(t);
                continue;
            }
            // A full FIFO and a missing address wait for a write.
            if self.fifo_full() || self.rd16(base + pipe::MEM_HI) == 0 {
                continue;
            }
            if self.rd16(base + pipe::STARTED) == 0 {
                self.wr16(base + pipe::STARTED, frame);
            }
            self.wr16(base + pipe::LAST_FRAME, frame);
            match self.execute(base, frame, ctx) {
                Outcome::Report => {
                    self.push(n as u16, ctx.irq);
                    if n == 0 {
                        self.wr16(at::REPORT_FRAME, frame);
                    }
                    let c = self.rd16(base + pipe::COMMAND);
                    self.wr16(base + pipe::COMMAND, c | cmd::DONE);
                }
                Outcome::Later(t) => wake = wake.min(t),
            }
        }
        self.scan_at = wake;
    }

    fn mem_addr(&self, base: usize) -> u32 {
        (u32::from(self.rd16(base + pipe::MEM_HI)) << 16
            | u32::from(self.rd16(base + pipe::MEM_LO)))
            & DMA_MASK
    }

    fn set_mem_addr(&mut self, base: usize, a: u32) {
        self.wr16(base + pipe::MEM_LO, a as u16);
        self.wr16(base + pipe::MEM_HI, (a >> 16) as u16);
    }

    /// One transfer on the pipe at `base`.
    fn execute(&mut self, base: usize, frame: u16, ctx: &mut IoCtx<'_>) -> Outcome {
        let mut command = self.rd16(base + pipe::COMMAND);
        let dev_ep = self.rd16(base + pipe::DEV_EP);
        let addr = ((dev_ep >> 8) & 0x7F) as u8;
        let ep = (dev_ep & 0xF) as u8;
        let max_trans = match usize::from(self.rd16(base + pipe::MAX_TRANS)) {
            0 => 1024,
            m => m.min(1024),
        };
        let token = command & 3;
        let mut env = Env::new(ctx.now, &mut ctx.storage);
        let ram_mask = ctx.ram.len().wrapping_sub(1);
        let mut packet = std::mem::take(&mut self.packet);

        let result = if token == 3 {
            // PING: every device here has room.
            res::ACK
        } else if command & cmd::MEMREAD != 0 {
            // SETUP / OUT.
            loop {
                let length = usize::from(self.rd16(base + pipe::LENGTH));
                let chunk = length.min(max_trans);
                let mem = self.mem_addr(base);
                packet.clear();
                if !ctx.ram.is_empty() {
                    packet.extend(
                        (0..chunk).map(|i| ctx.ram[(mem as usize).wrapping_add(i) & ram_mask]),
                    );
                }
                let r = self.hub.route(addr).map(|f| match token {
                    0 => device::setup(f, &packet, &mut env),
                    1 => device::token_out(f, ep, &packet, &mut env),
                    _ => Response::Stall,
                });
                match r {
                    Some(Response::Ack) => {
                        command ^= cmd::TOGGLE;
                        self.wr16(base + pipe::COMMAND, command);
                        self.set_mem_addr(base, mem.wrapping_add(chunk as u32) & DMA_MASK);
                        self.wr16(base + pipe::LENGTH, (length - chunk) as u16);
                        if length == chunk {
                            break res::ACK | if chunk == 0 { res::NO_DATA } else { 0 };
                        }
                    }
                    Some(Response::Nak) => break res::NAK,
                    Some(Response::Stall) => break res::STALL,
                    Some(Response::Data(_)) | None => break res::ERROR,
                }
            }
        } else if token == 2 {
            // IN.
            loop {
                let length = usize::from(self.rd16(base + pipe::LENGTH));
                packet.clear();
                let max = length.min(max_trans);
                let r = self
                    .hub
                    .route(addr)
                    .map(|f| device::token_in(f, ep, max, &mut packet, &mut env));
                match r {
                    Some(Response::Data(n)) => {
                        let n = n.min(packet.len()).min(max);
                        let mem = self.mem_addr(base);
                        if command & cmd::MEMWRITE != 0 && !ctx.ram.is_empty() {
                            for (i, b) in packet[..n].iter().enumerate() {
                                ctx.ram[(mem as usize).wrapping_add(i) & ram_mask] = *b;
                            }
                        }
                        let word = res::DATA
                            | (command & cmd::TOGGLE)
                            | if n == 0 { res::NO_DATA } else { 0 }
                            | n as u16;
                        command ^= cmd::TOGGLE;
                        self.wr16(base + pipe::COMMAND, command);
                        self.set_mem_addr(base, mem.wrapping_add(n as u32) & DMA_MASK);
                        self.wr16(base + pipe::LENGTH, (length - n) as u16);
                        if length == n || n != max_trans {
                            break word;
                        }
                    }
                    Some(Response::Nak) => break res::NAK,
                    Some(Response::Stall) => break res::STALL,
                    Some(Response::Ack) | None => break res::ERROR,
                }
            }
        } else {
            // A SETUP or OUT token without data from memory: nothing a
            // device answers.
            res::ERROR
        };
        self.packet = packet;
        self.wr16(base + pipe::RESULT, result);
        if result & 0x7000 != res::NAK & 0x7000 {
            return Outcome::Report;
        }

        // NAK: abort request, timeout, or later.
        if self.rd16(base + pipe::COMMAND) & cmd::ABORT_REQ != 0 {
            self.wr16(base + pipe::RESULT, (result & 0x0FFF) | res::ABORTED);
            return Outcome::Report;
        }
        let now = ctx.now;
        let frame_start = now - now % FRAME_CLOCKS;
        let interval = u64::from(self.rd16(base + pipe::INTERVAL));
        let retry = frame_start
            + if interval > 0 {
                interval
            } else {
                NAK_RETRY_FRAMES
            } * FRAME_CLOCKS;
        let timeout = self.rd16(base + pipe::TIMEOUT);
        if timeout == 0 {
            return Outcome::Later(retry);
        }
        let elapsed = frame.wrapping_sub(self.rd16(base + pipe::STARTED));
        if elapsed >= timeout {
            self.wr16(base + pipe::RESULT, result | res::TIMEOUT);
            return Outcome::Report;
        }
        let expiry = frame_start + u64::from(timeout - elapsed) * FRAME_CLOCKS;
        Outcome::Later(retry.min(expiry))
    }
}

impl IoDevice for UsbHost {
    fn read(&mut self, addr: u32, _ctx: &mut IoCtx<'_>) -> u8 {
        self.peek(addr)
    }

    fn peek(&self, addr: u32) -> u8 {
        let off = (addr & 0xFFF) as usize;
        if off < RAM {
            self.ram[off]
        } else {
            0
        }
    }

    fn write(&mut self, addr: u32, v: u8, ctx: &mut IoCtx<'_>) {
        let off = (addr & 0xFFF) as usize;
        let now = ctx.now;
        if off >= RAM {
            match (v & 1 != 0, self.run) {
                (true, false) => self.start(now),
                (false, true) => self.stop(),
                _ => {}
            }
            return;
        }
        self.ram[off] = v;
        if !self.run {
            return;
        }
        let next_frame = now.saturating_add(FRAME_CLOCKS);
        if (at::PIPES..at::PIPES + PIPES * at::PIPE_SIZE).contains(&off) {
            let n = (off - at::PIPES) / at::PIPE_SIZE;
            self.armed[n] = next_frame;
        }
        if (at::DO_RESET..at::DO_RESET + 2).contains(&off)
            && matches!(self.link, Link::Attached | Link::Operational)
        {
            self.link_at = self.link_at.min(now.saturating_add(REQUEST_CLOCKS));
        }
        if self.link == Link::Operational {
            self.scan_at = self.scan_at.min(next_frame);
        }
    }

    fn next_event(&self) -> u64 {
        if !self.run {
            return u64::MAX;
        }
        let root = if self.root_changed { 0 } else { u64::MAX };
        root.min(self.link_at)
            .min(self.scan_at)
            .min(self.media_at)
            .min(self.hub.next_event())
    }

    fn tick(&mut self, ctx: &mut IoCtx<'_>) {
        if !self.run {
            return;
        }
        let now = ctx.now;
        if self.root_changed {
            self.root_changed = false;
            self.root_event(ctx);
        }
        if self.link_at <= now {
            self.link_event(ctx);
        }
        if self.hub.next_event() <= now {
            self.hub.tick(now);
        }
        if self.media_at <= now {
            self.media_at = now.saturating_add(MEDIA_POLL_CLOCKS);
            let mut env = Env::new(now, &mut ctx.storage);
            self.hub.poll_media(&mut env);
        }
        if self.scan_at <= now {
            self.scan(ctx);
        }
    }

    /// The FPGA's reset: the nano stopped, its memory cleared, the hub
    /// (reset with the board) unconfigured. Whether the hub is connected
    /// is the host's and stays.
    fn reset(&mut self) {
        let root = self.root_attached;
        *self = Self::new();
        self.root_attached = root;
    }

    fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"UUSB", 1, |w| {
            w.raw(&self.ram);
            w.bool(self.run);
            w.u8(self.link as u8);
            w.u64(self.link_at);
            for a in self.armed {
                w.u64(a);
            }
            w.u64(self.scan_at);
            w.u64(self.media_at);
            w.bool(self.root_attached);
            w.bool(self.root_changed);
            self.hub.save(w);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut r) = r.chunk(*b"UUSB", 1)?;
        let r = &mut r;
        let mut n = Self::new();
        r.raw_into(&mut n.ram)?;
        n.run = r.bool()?;
        n.link = *Link::ALL
            .get(usize::from(r.u8()?))
            .ok_or(StateError::Invalid("USB link state"))?;
        n.link_at = r.u64()?;
        for a in &mut n.armed {
            *a = r.u64()?;
        }
        n.scan_at = r.u64()?;
        n.media_at = r.u64()?;
        n.root_attached = r.bool()?;
        n.root_changed = r.bool()?;
        n.hub = Hub::load(r)?;
        if n.run == (n.link == Link::Off) {
            return Err(StateError::Invalid("USB run state"));
        }
        *self = n;
        Ok(())
    }
}
