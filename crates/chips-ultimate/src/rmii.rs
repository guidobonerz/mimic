//! The board's wired Ethernet: the MAC inside the FPGA ([`Rmii`], I/O
//! `$060800`–`$0608FF`) and the PHY chip behind the two management pins
//! the firmware wiggles by hand ([`MdioPhy`]).
//!
//! # The MAC
//!
//! Three small blocks share the address range; bits 5–4 of the address
//! pick the block, bits 3–0 the register (the range repeats every `$40`,
//! the fourth quarter is empty). All registers are bytes; wider values
//! are little-endian. What is not listed reads 0 and ignores writes.
//!
//! | Address | R/W | Register |
//! |---|---|---|
//! | `$060800`–`05` | W | own MAC address, octet 0 first (receive filter) |
//! | `$060807` | W | bit 0: promiscuous, the filter lets everything pass |
//! | `$060808` | W | bit 0: receiver on. Off holds the receiver in reset |
//! | `$060810`–`13` | W | address of the frame to send, 26 bits |
//! | `$060814`–`15` | W | its length, 12 bits |
//! | `$060818` | W | any write: send (ignored while busy) |
//! | `$060819` | W | any write: drop the "sent" interrupt |
//! | `$06081A` | R | bit 0: busy, the frame is still being fetched |
//! | `$060821`–`23` | W | base of the receive buffers, bits 25–8 (bits 7–0 are always 0, a write to `$060820` does nothing) |
//! | `$060824` | W | hand a buffer (its number) to the receiver |
//! | `$060824` | R | bit 1: the queue of free buffers is full |
//! | `$060828` | R | number of the buffer with the received frame shown; `$FF` after it was taken |
//! | `$06082A`–`2B` | R | that frame's length, 12 bits |
//! | `$06082E` | W | any write: empty both queues (the frame shown stays) |
//! | `$06082F` | R | bit 0: a received frame is shown |
//! | `$06082F` | W | any write: take the frame shown; the next one waiting is shown |
//!
//! ## Receiving
//!
//! The firmware owns a pool of buffers of 1536 bytes, buffer `n` at
//! `base + 1536 * n`, and gives their numbers to the MAC (queue of free
//! buffers, 127 entries; a number that does not fit is lost without a
//! sign). For a frame coming in the receiver takes a buffer from that
//! queue, unless it still holds one from a frame it did not keep; with
//! the queue empty the frame is dropped. The frame is written to the
//! buffer **from offset 2** (the first two bytes of the buffer are not
//! the frame's), the frame check sequence behind it. A frame the filter
//! lets pass is entered in the queue of received frames with its buffer
//! number and its length; the head of that queue is shown in the
//! registers at `$060828`…, and while one is shown the MAC holds
//! interrupt source 5 of the ITU high (a level: it falls with the write
//! to `$06082F` and rises again at once when another frame waits).
//!
//! The length the MAC reports is the frame's from the destination address
//! to the last byte of payload, **without** the four bytes of the frame
//! check sequence: the transceiver checks the sequence, passes it on and
//! adds a byte that says whether it was right, and the filter takes those
//! five bytes off the count. A frame with a wrong sequence is dropped;
//! the cable of this model ([`crate::io::EtherLink`]) carries frames
//! without the sequence, so that cannot happen here, and the sequence
//! written behind the frame is computed.
//!
//! The filter compares the destination address as three 16-bit words;
//! each must be the same word of the own address or `$FFFF`. So the
//! board's own address and broadcast pass, group addresses (multicast)
//! do not, and oddities such as "own address with one word of ones" do.
//! Promiscuous mode lets every frame pass.
//!
//! Not kept: a frame longer than 1530 bytes (the receiver gives up at
//! the 1536th byte of frame, check sequence and status byte), a frame
//! arriving while the receiver is off, a frame that finds no free buffer.
//! There is no lower limit: a short frame is kept as it is. Switching
//! the receiver off loses the buffer the receiver holds at that moment
//! (its number is in neither queue afterwards); the frames already
//! received stay.
//!
//! ## Sending
//!
//! A write to `$060818` fetches as many bytes as the length register
//! names from the address register and sends them; the transceiver puts
//! preamble and frame check sequence around them. The MAC does not pad:
//! the firmware makes every frame at least 60 bytes long itself (a short
//! one it copies into a buffer of its own and fills with zeros). A length
//! of 0 counts as 4096. Busy reads 1 while the bytes are fetched; then
//! busy falls and interrupt source 6 rises, which stays up until a write
//! to `$060819`. The firmware never enables source 6 and never writes
//! that register; it only reads busy before a frame and gives the frame
//! up when it is set.
//!
//! ## What the model does about time
//!
//! - A frame to send is read from memory and given to the cable at the
//!   write to `$060818`; busy then lasts one clock per byte, which is the
//!   least the FPGA needs (its memory port adds to that by an amount not
//!   known). On a real network a frame shorter than 60 bytes or longer
//!   than 1518 is thrown away by whoever receives it; the model does not
//!   put such a frame on the cable.
//! - While the receiver is on, the MAC looks at the cable every
//!   millisecond of board clock ([`RX_POLL_CLOCKS`]) and takes one frame;
//!   after a frame it looks again when that frame's time on a 100 Mbit/s
//!   wire is over (8 clocks per byte, preamble, check sequence and gap
//!   included), so a burst arrives as fast as on the wire and no faster.
//!   Frames that queued up on the cable while the receiver was off are
//!   thrown away when it is switched on.
//! - The next waiting frame is shown at the write that takes the one
//!   before (the FPGA needs a few clocks).
//!
//! # The PHY
//!
//! The PHY's management interface (MDIO, IEEE 802.3 clause 22) hangs on
//! two pins of the board's pin block: `$10000A` sets the clock MDC,
//! `$10000B` the data line MDIO (1 lets it float high, 0 pulls it low),
//! `$100006` reads the line. The firmware clocks out by hand: at least 32
//! ones, start `01`, operation (`10` read, `01` write), five bits of PHY
//! address, five of register, then for a write `10` and 16 data bits, for
//! a read one more clock and then 16 clocks after each of which it reads
//! the line. [`MdioPhy`] takes each bit at the rise of MDC and answers a
//! read from the rise after the register number on.
//!
//! The firmware looks for the PHY at address 0, then 3: register 2 must
//! read `$0022`. It then writes registers 4, `$1B`, `$16` and 0, and
//! from then on reads only register 1, whose bit 2 says "link": every
//! 250 ms while there is none, and after each second without a received
//! frame while there is one.
//!
//! Which chip sits on the board is not documented; the identifier and
//! the two vendor registers the firmware writes fit Microchip's
//! KSZ8081 family, whose register `$1E` (speed and duplex found) is
//! modelled on that assumption. The firmware reads none of it.
//!
//! Sources: UE2-C64U-Emulator `docs/hw/08-network-rmii.md` (registers,
//! hazards H1–H10, the functional model), `00-memory-map.md` (interrupt
//! table, DMA widths); 1541 Ultimate `software/io/network/
//! rmii_interface.cc`/`.h` and `eth_tx_frame.h` (the driver's sequences,
//! the padding), `software/io/mdio/mdio.c` (the bit stream),
//! `fpga/io/rmii/vhdl_source/eth_filter.vhd`, `eth_transmit.vhd`,
//! `rmii_transceiver.vhd`, `fpga/ip/free_queue/vhdl_source/
//! free_queue.vhd` (queues, filter rule, limits, reset values) and
//! `fpga/altera/u2p_io.vhd` (the pins' reset levels); IEEE 802.3
//! clause 22 for the PHY's standard registers.

use crate::io::{IoCtx, IoDevice, CLOCK_HZ};
use mimic_core::{StateError, StateReader, StateWriter};

/// First address of the MAC.
pub const BASE: u32 = 0x06_0800;
/// Size of one receive buffer.
pub const BLOCK_SIZE: u32 = 1536;
/// The longest frame (without check sequence) the receiver keeps.
pub const MAX_RX_FRAME: usize = 1530;
/// The shortest and the longest frame the model puts on the cable.
pub const MIN_TX_FRAME: usize = 60;
pub const MAX_TX_FRAME: usize = 1518;
/// How often the receiver looks at the cable while it is on: 1 ms.
pub const RX_POLL_CLOCKS: u64 = CLOCK_HZ / 1000;
/// ITU low interrupt sources.
pub const IRQ_RX: u8 = 5;
pub const IRQ_TX: u8 = 6;

/// Clocks one byte takes on a 100 Mbit/s wire.
const WIRE_BYTE_CLOCKS: u64 = CLOCK_HZ / 12_500_000;
/// Preamble (8), check sequence (4) and gap (12) around a frame.
const WIRE_OVERHEAD: u64 = 24;
/// Clocks the transmitter is busy per byte it fetches.
const TX_BYTE_CLOCKS: u64 = 1;
/// The DMA addresses are 26 bits wide.
const DMA_MASK: u32 = 0x03FF_FFFF;
/// Entries of each queue's memory (7-bit pointers).
const QUEUE: usize = 128;
const PTR_MASK: u8 = 0x7F;
/// Frames thrown away at most when the receiver is switched on.
const FLUSH_LIMIT: usize = 4096;

/// The frame check sequence of a frame as it goes over the wire (CRC-32
/// of IEEE 802.3, least significant byte first).
fn frame_check(frame: &[u8]) -> [u8; 4] {
    let mut c = 0xFFFF_FFFF_u32;
    for &b in frame {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
    }
    (!c).to_le_bytes()
}

/// What became of a frame from the cable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxOutcome {
    /// The receiver is off.
    Disabled,
    /// No bytes at all.
    Empty,
    /// The queue of free buffers was empty.
    NoBuffer,
    /// Longer than [`MAX_RX_FRAME`].
    TooLong,
    /// The filter did not let it pass.
    NotForUs,
    /// Written to this buffer and queued for the firmware.
    Stored(u8),
}

/// Counters for the host (status display, tests); they wrap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Frames given to the cable.
    pub tx_frames: u32,
    /// Frames sent with no link, or of a length no receiver accepts.
    pub tx_discarded: u32,
    /// Frames queued for the firmware.
    pub rx_frames: u32,
    pub rx_filtered: u32,
    pub rx_no_buffer: u32,
    pub rx_too_long: u32,
}

/// The buffer the receiver holds for the frame coming in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Held {
    id: u8,
    /// Where it lies (worked out when it was taken from the queue).
    addr: u32,
}

/// The Ethernet MAC at `$060800`–`$0608FF`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rmii {
    // Receive filter, `$00`–`$0F`.
    mac: [u8; 6],
    promiscuous: bool,
    rx_enable: bool,
    held: Option<Held>,
    /// When the receiver looks at the cable next; `u64::MAX` while off.
    rx_at: u64,

    // Transmitter, `$10`–`$1F`.
    tx_addr: u32,
    tx_len: u16,
    tx_busy: bool,
    tx_irq: bool,
    /// When busy falls; `u64::MAX` while not busy.
    tx_done_at: u64,

    // Buffer queues, `$20`–`$2F`.
    base: u32,
    free: [u8; QUEUE],
    used_id: [u8; QUEUE],
    used_size: [u16; QUEUE],
    free_head: u8,
    free_tail: u8,
    used_head: u8,
    used_tail: u8,
    /// A received frame is shown (the interrupt level).
    used_valid: bool,
    pop_id: u8,
    pop_size: u16,

    stats: Stats,
}

impl Default for Rmii {
    fn default() -> Self {
        Self::new()
    }
}

impl Rmii {
    pub fn new() -> Self {
        Self {
            mac: [0; 6],
            promiscuous: false,
            rx_enable: false,
            held: None,
            rx_at: u64::MAX,
            tx_addr: 0,
            tx_len: 0,
            tx_busy: false,
            tx_irq: false,
            tx_done_at: u64::MAX,
            base: 0,
            free: [0; QUEUE],
            used_id: [0; QUEUE],
            used_size: [0; QUEUE],
            free_head: 0,
            free_tail: 0,
            used_head: 0,
            used_tail: 0,
            used_valid: false,
            pop_id: 0,
            pop_size: 0,
            stats: Stats::default(),
        }
    }

    /// The addresses this block answers.
    pub fn claims(addr: u32) -> bool {
        addr >> 8 == BASE >> 8
    }

    /// The MAC address the firmware gave the filter.
    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    pub fn promiscuous(&self) -> bool {
        self.promiscuous
    }

    /// The receiver is on (the firmware saw a link and set it up).
    pub fn rx_enabled(&self) -> bool {
        self.rx_enable
    }

    /// A received frame is shown to the firmware (interrupt source 5).
    pub fn rx_waiting(&self) -> bool {
        self.used_valid
    }

    /// Buffers the receiver can still take.
    pub fn free_buffers(&self) -> usize {
        usize::from(self.free_head.wrapping_sub(self.free_tail) & PTR_MASK)
    }

    /// Received frames queued behind the one shown.
    pub fn queued_frames(&self) -> usize {
        usize::from(self.used_head.wrapping_sub(self.used_tail) & PTR_MASK)
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// A frame from the cable (destination address first, no check
    /// sequence) as the receiver treats it now. [`IoDevice::tick`] feeds
    /// the frames of `ctx.net` through here; the host may also call it.
    pub fn deliver(&mut self, frame: &[u8], ctx: &mut IoCtx<'_>) -> RxOutcome {
        let out = self.receive(frame, ctx.ram);
        self.sync_irq(ctx);
        out
    }

    fn receive(&mut self, frame: &[u8], ram: &mut [u8]) -> RxOutcome {
        if !self.rx_enable {
            return RxOutcome::Disabled;
        }
        if frame.is_empty() {
            return RxOutcome::Empty;
        }
        // The receiver wants a buffer as soon as bytes come, before it
        // knows whether it keeps the frame.
        let held = match self.held {
            Some(h) => h,
            None => {
                if self.free_tail == self.free_head {
                    self.stats.rx_no_buffer = self.stats.rx_no_buffer.wrapping_add(1);
                    return RxOutcome::NoBuffer;
                }
                let id = self.free[usize::from(self.free_tail)];
                self.free_tail = (self.free_tail + 1) & PTR_MASK;
                let h = Held {
                    id,
                    addr: self.base.wrapping_add(BLOCK_SIZE * u32::from(id)) & DMA_MASK,
                };
                self.held = Some(h);
                h
            }
        };
        if frame.len() > MAX_RX_FRAME {
            self.stats.rx_too_long = self.stats.rx_too_long.wrapping_add(1);
            return RxOutcome::TooLong;
        }
        // The stream the filter sees: frame, check sequence, status byte.
        let fcs = frame_check(frame);
        let stream = |i: usize| -> u8 {
            match i.checked_sub(frame.len()) {
                None => frame[i],
                Some(k) if k < 4 => fcs[k],
                Some(_) => 0xFF,
            }
        };
        let for_me = (0..3).all(|k| {
            let w = [stream(2 * k), stream(2 * k + 1)];
            w == [0xFF, 0xFF] || w == [self.mac[2 * k], self.mac[2 * k + 1]]
        });
        if !(for_me || self.promiscuous) {
            self.stats.rx_filtered = self.stats.rx_filtered.wrapping_add(1);
            return RxOutcome::NotForUs;
        }
        if !ram.is_empty() {
            let mask = ram.len() - 1;
            let at = |i: usize| (held.addr.wrapping_add(2 + i as u32) & DMA_MASK) as usize & mask;
            for i in 0..frame.len() + 4 {
                ram[at(i)] = stream(i);
            }
        }
        self.held = None;
        let head = usize::from(self.used_head);
        self.used_id[head] = held.id;
        self.used_size[head] = frame.len() as u16;
        self.used_head = (self.used_head + 1) & PTR_MASK;
        self.show_next();
        self.stats.rx_frames = self.stats.rx_frames.wrapping_add(1);
        RxOutcome::Stored(held.id)
    }

    /// With nothing shown and a frame waiting: show it.
    fn show_next(&mut self) {
        if !self.used_valid && self.used_tail != self.used_head {
            let tail = usize::from(self.used_tail);
            self.pop_id = self.used_id[tail];
            self.pop_size = self.used_size[tail];
            self.used_tail = (self.used_tail + 1) & PTR_MASK;
            self.used_valid = true;
        }
    }

    fn set_rx_enable(&mut self, on: bool, ctx: &mut IoCtx<'_>) {
        if on && !self.rx_enable {
            // What was on the wire while the receiver was off is gone.
            if let Some(net) = ctx.net.as_mut() {
                for _ in 0..FLUSH_LIMIT {
                    if net.receive().is_none() {
                        break;
                    }
                }
            }
            self.rx_at = ctx.now.saturating_add(RX_POLL_CLOCKS);
        }
        if !on {
            self.held = None;
            self.rx_at = u64::MAX;
        }
        self.rx_enable = on;
    }

    fn tx_start(&mut self, ctx: &mut IoCtx<'_>) {
        if self.tx_busy {
            return;
        }
        let len = match self.tx_len {
            0 => 4096,
            n => usize::from(n),
        };
        self.tx_busy = true;
        self.tx_done_at = ctx.now.saturating_add(len as u64 * TX_BYTE_CLOCKS);
        let sent = (MIN_TX_FRAME..=MAX_TX_FRAME).contains(&len)
            && match ctx.net.as_mut() {
                Some(net) if net.link_up() => {
                    let mask = ctx.ram.len().wrapping_sub(1);
                    let frame: Vec<u8> = (0..len)
                        .map(|i| {
                            let a = self.tx_addr.wrapping_add(i as u32) & DMA_MASK;
                            ctx.ram.get(a as usize & mask).copied().unwrap_or(0)
                        })
                        .collect();
                    net.transmit(&frame);
                    true
                }
                _ => false,
            };
        if sent {
            self.stats.tx_frames = self.stats.tx_frames.wrapping_add(1);
        } else {
            self.stats.tx_discarded = self.stats.tx_discarded.wrapping_add(1);
        }
    }

    /// Busy falls when the frame's bytes are fetched.
    fn tx_settle(&mut self, now: u64) {
        if self.tx_busy && now >= self.tx_done_at {
            self.tx_busy = false;
            self.tx_irq = true;
            self.tx_done_at = u64::MAX;
        }
    }

    fn sync_irq(&self, ctx: &mut IoCtx<'_>) {
        ctx.irq.set_level(IRQ_RX, self.used_valid);
        ctx.irq.set_level(IRQ_TX, self.tx_irq);
    }
}

/// Replace byte `n` of a little-endian value.
fn set_byte(value: u32, n: u32, v: u8) -> u32 {
    value & !(0xFF << (8 * n)) | u32::from(v) << (8 * n)
}

impl IoDevice for Rmii {
    fn read(&mut self, addr: u32, ctx: &mut IoCtx<'_>) -> u8 {
        self.tx_settle(ctx.now);
        self.sync_irq(ctx);
        self.peek(addr)
    }

    fn peek(&self, addr: u32) -> u8 {
        match addr & 0x3F {
            0x1A => u8::from(self.tx_busy),
            0x24 => {
                let full = (self.free_head + 1) & PTR_MASK == self.free_tail;
                u8::from(full) << 1
            }
            0x28 => self.pop_id,
            0x2A => self.pop_size as u8,
            0x2B => (self.pop_size >> 8) as u8 & 0x0F,
            0x2F => u8::from(self.used_valid),
            _ => 0,
        }
    }

    fn write(&mut self, addr: u32, v: u8, ctx: &mut IoCtx<'_>) {
        self.tx_settle(ctx.now);
        match addr & 0x3F {
            n @ 0x00..=0x05 => self.mac[n as usize] = v,
            0x07 => self.promiscuous = v & 1 != 0,
            0x08 => self.set_rx_enable(v & 1 != 0, ctx),
            n @ 0x10..=0x13 => self.tx_addr = set_byte(self.tx_addr, n - 0x10, v) & DMA_MASK,
            n @ 0x14..=0x15 => {
                self.tx_len = set_byte(u32::from(self.tx_len), n - 0x14, v) as u16 & 0x0FFF;
            }
            0x18 => self.tx_start(ctx),
            0x19 => self.tx_irq = false,
            n @ 0x21..=0x23 => self.base = set_byte(self.base, n - 0x20, v) & DMA_MASK,
            0x24 => {
                if (self.free_head + 1) & PTR_MASK != self.free_tail {
                    self.free[usize::from(self.free_head)] = v;
                    self.free_head = (self.free_head + 1) & PTR_MASK;
                }
            }
            0x2E => {
                self.free_head = 0;
                self.free_tail = 0;
                self.used_head = 0;
                self.used_tail = 0;
            }
            0x2F => {
                if self.used_valid {
                    self.used_valid = false;
                    self.pop_id = 0xFF;
                }
                self.show_next();
            }
            _ => {}
        }
        self.sync_irq(ctx);
    }

    /// The end of a frame being sent, and while the receiver is on the
    /// next look at the cable (every [`RX_POLL_CLOCKS`], sooner after a
    /// frame).
    fn next_event(&self) -> u64 {
        self.tx_done_at.min(self.rx_at)
    }

    fn tick(&mut self, ctx: &mut IoCtx<'_>) {
        self.tx_settle(ctx.now);
        if self.rx_enable && ctx.now >= self.rx_at {
            let frame = match ctx.net.as_mut() {
                Some(net) if net.link_up() => net.receive(),
                _ => None,
            };
            let wait = match frame {
                Some(f) => {
                    self.receive(&f, ctx.ram);
                    (f.len().max(MIN_TX_FRAME) as u64 + WIRE_OVERHEAD) * WIRE_BYTE_CLOCKS
                }
                None => RX_POLL_CLOCKS,
            };
            self.rx_at = ctx.now.saturating_add(wait);
        }
        self.sync_irq(ctx);
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"URMI", 1, |w| {
            w.raw(&self.mac);
            w.bool(self.promiscuous);
            w.bool(self.rx_enable);
            w.bool(self.held.is_some());
            let held = self.held.unwrap_or(Held { id: 0, addr: 0 });
            w.u8(held.id);
            w.u32(held.addr);
            w.u64(self.rx_at);
            w.u32(self.tx_addr);
            w.u16(self.tx_len);
            w.bool(self.tx_busy);
            w.bool(self.tx_irq);
            w.u64(self.tx_done_at);
            w.u32(self.base);
            w.raw(&self.free);
            w.raw(&self.used_id);
            for &s in &self.used_size {
                w.u16(s);
            }
            for p in [
                self.free_head,
                self.free_tail,
                self.used_head,
                self.used_tail,
            ] {
                w.u8(p);
            }
            w.bool(self.used_valid);
            w.u8(self.pop_id);
            w.u16(self.pop_size);
            let s = &self.stats;
            for v in [
                s.tx_frames,
                s.tx_discarded,
                s.rx_frames,
                s.rx_filtered,
                s.rx_no_buffer,
                s.rx_too_long,
            ] {
                w.u32(v);
            }
        });
    }

    /// Corrupt data gives an error and leaves the state as it was.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut r) = r.chunk(*b"URMI", 1)?;
        let mut n = Self::new();
        r.raw_into(&mut n.mac)?;
        n.promiscuous = r.bool()?;
        n.rx_enable = r.bool()?;
        let has_held = r.bool()?;
        let held = Held {
            id: r.u8()?,
            addr: r.u32()?,
        };
        n.held = has_held.then_some(held);
        n.rx_at = r.u64()?;
        n.tx_addr = r.u32()?;
        n.tx_len = r.u16()?;
        n.tx_busy = r.bool()?;
        n.tx_irq = r.bool()?;
        n.tx_done_at = r.u64()?;
        n.base = r.u32()?;
        r.raw_into(&mut n.free)?;
        r.raw_into(&mut n.used_id)?;
        for s in &mut n.used_size {
            *s = r.u16()?;
        }
        n.free_head = r.u8()?;
        n.free_tail = r.u8()?;
        n.used_head = r.u8()?;
        n.used_tail = r.u8()?;
        n.used_valid = r.bool()?;
        n.pop_id = r.u8()?;
        n.pop_size = r.u16()?;
        n.stats = Stats {
            tx_frames: r.u32()?,
            tx_discarded: r.u32()?,
            rx_frames: r.u32()?,
            rx_filtered: r.u32()?,
            rx_no_buffer: r.u32()?,
            rx_too_long: r.u32()?,
        };

        if [n.free_head, n.free_tail, n.used_head, n.used_tail]
            .iter()
            .any(|&p| p > PTR_MASK)
        {
            return Err(StateError::Invalid("RMII queue pointer"));
        }
        if n.tx_addr > DMA_MASK || n.base > DMA_MASK || n.base & 0xFF != 0 || held.addr > DMA_MASK {
            return Err(StateError::Invalid("RMII DMA address"));
        }
        if n.tx_len > 0x0FFF
            || n.pop_size > 0x0FFF
            || n.used_size.iter().any(|&s| usize::from(s) > MAX_RX_FRAME)
        {
            return Err(StateError::Invalid("RMII frame length"));
        }
        if n.tx_busy != (n.tx_done_at != u64::MAX) {
            return Err(StateError::Invalid("RMII transmitter deadline"));
        }
        if n.rx_enable != (n.rx_at != u64::MAX) || (n.held.is_some() && !n.rx_enable) {
            return Err(StateError::Invalid("RMII receiver state"));
        }
        *self = n;
        Ok(())
    }
}

// ---------------------------------------------------------------------
// The PHY behind MDIO
// ---------------------------------------------------------------------

/// Register 0, basic control.
mod bmcr {
    /// Reset the PHY; clears itself.
    pub const RESET: u16 = 0x8000;
    /// 100 Mbit/s when autonegotiation is off.
    pub const SPEED_100: u16 = 0x2000;
    /// Autonegotiation on.
    pub const AUTONEG: u16 = 0x1000;
    /// Power down: no link.
    pub const POWER_DOWN: u16 = 0x0800;
    /// Start autonegotiation again; clears itself.
    pub const RESTART: u16 = 0x0200;
    /// Full duplex when autonegotiation is off.
    pub const FULL_DUPLEX: u16 = 0x0100;
    /// After power-on: autonegotiation, 100 Mbit/s, full duplex.
    pub const DEFAULT: u16 = AUTONEG | SPEED_100 | FULL_DUPLEX;
}

/// Register 1, basic status: able to do 100BASE-TX and 10BASE-T in both
/// duplex modes, takes frames without preamble, can autonegotiate, has
/// the extended registers.
const BMSR_ABILITIES: u16 = 0x7849;
/// Autonegotiation is complete.
const BMSR_AUTONEG_DONE: u16 = 0x0020;
/// There is a link. Once lost, the bit reads 0 until the register has
/// been read (IEEE 802.3 22.2.4.2.13).
const BMSR_LINK: u16 = 0x0004;
/// Register 2: the identifier the firmware looks for.
pub const PHY_ID1: u16 = 0x0022;
/// Register 3: the rest of the identifier (KSZ8081 family; assumed, the
/// firmware does not read it).
pub const PHY_ID2: u16 = 0x1560;
/// Register 4 after power-on, and what the far end advertises: IEEE
/// 802.3, 10 and 100 Mbit/s in half and full duplex.
const ADVERTISE_ALL: u16 = 0x01E1;
/// Register 5, bit 14: the far end acknowledged.
const LPA_ACK: u16 = 0x4000;

/// Where the decoder of the bit stream is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Counting the ones of the preamble.
    Idle,
    /// Start, operation, PHY address, register: 14 bits.
    Header,
    /// The clock between register number and the data of a read.
    ReadTurn,
    /// The PHY shows the 16 bits of a read.
    ReadData,
    /// The two turnaround bits of a write.
    WriteTurn,
    /// The 16 data bits of a write.
    WriteData,
}

/// The Ethernet PHY as its management interface shows it: the decoder of
/// the bit stream on MDC/MDIO and the registers behind it.
///
/// The board's pin block calls [`MdioPhy::set_mdc`] for a write to
/// `$10000A`, [`MdioPhy::set_mdio`] for one to `$10000B`, and returns
/// [`MdioPhy::get_mdio`] for a read of `$100006`.
///
/// Registers (IEEE 802.3 clause 22 unless noted):
/// - 0 control: bit 15 reset, 13 speed, 12 autonegotiation, 11 power
///   down, 9 restart autonegotiation, 8 duplex. Reads back what was
///   written, without the two bits that clear themselves.
/// - 1 status: `$7849` plus bit 2 link and bit 5 autonegotiation
///   complete. The link bit stays 0 after a loss until the register has
///   been read once.
/// - 2, 3 identifier: `$0022`, `$1560`.
/// - 4 advertisement, stored (`$01E1` after power-on).
/// - 5 what the far end advertises: `$41E1` with a link, else `$0001`.
/// - 6 bit 0: the far end autonegotiates.
/// - `$1E` (KSZ8081 family, assumed): bit 8 link, bits 2–0 the mode
///   found: 0 none yet, 1 10 half, 2 100 half, 5 10 full, 6 100 full.
/// - everything else, among them the `$16` and `$1B` the firmware
///   writes: stored and read back. The PHY's interrupt pin is not
///   connected to anything the firmware reads.
///
/// Autonegotiation takes no time here: it is complete as soon as there
/// is a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MdioPhy {
    /// The address the PHY answers at (its strapping).
    address: u8,
    /// MDC as the board drives it.
    mdc: bool,
    /// MDIO as the board drives it: `true` floats (high).
    mdio: bool,
    /// The PHY pulls MDIO low.
    pull_low: bool,
    phase: Phase,
    /// Ones of the preamble seen, up to 32.
    ones: u8,
    /// Bits taken (header, write) or still to show (read).
    count: u8,
    shift: u16,
    /// The frame under way names this PHY.
    selected: bool,
    reg: u8,
    regs: [u16; 32],
    /// The cable's state at the last clock.
    link: bool,
    /// The link was down since register 1 was read.
    link_lost: bool,
}

impl Default for MdioPhy {
    fn default() -> Self {
        Self::new(0)
    }
}

impl MdioPhy {
    /// A PHY strapped to `address` (0–31). The firmware finds one at 0
    /// or 3.
    pub fn new(address: u8) -> Self {
        Self {
            address: address & 0x1F,
            // The pin block's reset leaves both lines high.
            mdc: true,
            mdio: true,
            pull_low: false,
            phase: Phase::Idle,
            ones: 0,
            count: 0,
            shift: 0,
            selected: false,
            reg: 0,
            regs: Self::power_on_regs(),
            link: false,
            link_lost: true,
        }
    }

    fn power_on_regs() -> [u16; 32] {
        let mut regs = [0; 32];
        regs[0] = bmcr::DEFAULT;
        regs[4] = ADVERTISE_ALL;
        regs
    }

    pub fn address(&self) -> u8 {
        self.address
    }

    /// Power-on state of PHY and pins; the address stays.
    pub fn reset(&mut self) {
        *self = Self::new(self.address);
    }

    /// The board sets MDC. `link_up`: the cable has a link now
    /// (`EtherLink::link_up`, `false` with nothing plugged in). A rise
    /// of the clock moves the bit stream on.
    pub fn set_mdc(&mut self, level: bool, link_up: bool) {
        self.link = link_up;
        if !link_up {
            self.link_lost = true;
        }
        let rise = level && !self.mdc;
        self.mdc = level;
        if rise {
            self.clock();
        }
    }

    /// The board sets MDIO: `true` lets the line float high, `false`
    /// pulls it low.
    pub fn set_mdio(&mut self, level: bool) {
        self.mdio = level;
    }

    /// The level of MDIO, 0 or 1: low when the board or the PHY pulls.
    pub fn get_mdio(&self) -> u8 {
        u8::from(self.mdio && !self.pull_low)
    }

    /// The level of MDC as last set.
    pub fn mdc(&self) -> bool {
        self.mdc
    }

    /// A register as a read over MDIO would show it now, without the
    /// read's effect on the link bit (debugger).
    pub fn reg(&self, reg: u8) -> u16 {
        let up = self.link && self.regs[0] & bmcr::POWER_DOWN == 0;
        let autoneg = self.regs[0] & bmcr::AUTONEG != 0;
        match reg & 0x1F {
            1 => {
                let mut v = BMSR_ABILITIES;
                if up && !self.link_lost {
                    v |= BMSR_LINK;
                }
                if up && autoneg {
                    v |= BMSR_AUTONEG_DONE;
                }
                v
            }
            2 => PHY_ID1,
            3 => PHY_ID2,
            5 if up && autoneg => LPA_ACK | ADVERTISE_ALL,
            5 => 0x0001,
            6 => u16::from(up && autoneg),
            0x1E => {
                if !up {
                    return 0;
                }
                let (hundred, full) = if autoneg {
                    // The best mode both ends advertise; the far end
                    // offers all four.
                    match self.regs[4] {
                        a if a & 0x0100 != 0 => (true, true),
                        a if a & 0x0080 != 0 => (true, false),
                        a if a & 0x0040 != 0 => (false, true),
                        a if a & 0x0020 != 0 => (false, false),
                        _ => return 0x0100,
                    }
                } else {
                    (
                        self.regs[0] & bmcr::SPEED_100 != 0,
                        self.regs[0] & bmcr::FULL_DUPLEX != 0,
                    )
                };
                0x0100 | if hundred { 2 } else { 1 } | if full { 4 } else { 0 }
            }
            n => self.regs[usize::from(n)],
        }
    }

    fn read_reg(&mut self, reg: u8) -> u16 {
        let v = self.reg(reg);
        if reg == 1 {
            // Reading lets the bit follow the link again.
            self.link_lost = !(self.link && self.regs[0] & bmcr::POWER_DOWN == 0);
        }
        v
    }

    fn write_reg(&mut self, reg: u8, v: u16) {
        match reg {
            0 if v & bmcr::RESET != 0 => {
                self.regs = Self::power_on_regs();
                self.link_lost = true;
            }
            0 => {
                self.regs[0] = v & !bmcr::RESTART;
                if v & bmcr::POWER_DOWN != 0 {
                    self.link_lost = true;
                }
            }
            1..=3 | 5 | 6 | 0x1E => {}
            n => self.regs[usize::from(n & 0x1F)] = v,
        }
    }

    /// A bit while no frame is under way: 32 ones, then a 0 starts one.
    fn idle_bit(&mut self, bit: bool) {
        if bit {
            self.ones = (self.ones + 1).min(32);
        } else if self.ones >= 32 {
            self.phase = Phase::Header;
            self.shift = 0;
            self.count = 1;
        } else {
            self.ones = 0;
        }
    }

    fn go_idle(&mut self) {
        self.phase = Phase::Idle;
        self.ones = 0;
        self.count = 0;
    }

    /// MDC rose: take the board's bit, or show the next one.
    fn clock(&mut self) {
        let bit = self.mdio;
        match self.phase {
            Phase::Idle => self.idle_bit(bit),
            Phase::Header => {
                self.shift = self.shift << 1 | u16::from(bit);
                self.count += 1;
                if self.count == 2 && !bit {
                    // `00` is not the start of a clause 22 frame.
                    self.go_idle();
                } else if self.count == 14 {
                    self.reg = (self.shift & 0x1F) as u8;
                    self.selected = (self.shift >> 5) & 0x1F == u16::from(self.address);
                    self.count = 0;
                    match (self.shift >> 10) & 3 {
                        0b10 => self.phase = Phase::ReadTurn,
                        0b01 => self.phase = Phase::WriteTurn,
                        _ => self.go_idle(),
                    }
                }
            }
            Phase::ReadTurn => {
                if self.selected {
                    self.shift = self.read_reg(self.reg);
                    // The second turnaround bit is a 0 from the PHY.
                    self.pull_low = true;
                }
                self.count = 16;
                self.phase = Phase::ReadData;
            }
            Phase::ReadData => {
                if self.count == 0 {
                    // The clock after the last bit: let go of the line.
                    self.pull_low = false;
                    self.go_idle();
                    self.idle_bit(bit);
                } else {
                    self.count -= 1;
                    if self.selected {
                        self.pull_low = self.shift >> self.count & 1 == 0;
                    }
                }
            }
            Phase::WriteTurn => {
                self.count += 1;
                if self.count == 2 {
                    self.phase = Phase::WriteData;
                    self.count = 0;
                    self.shift = 0;
                }
            }
            Phase::WriteData => {
                self.shift = self.shift << 1 | u16::from(bit);
                self.count += 1;
                if self.count == 16 {
                    if self.selected {
                        self.write_reg(self.reg, self.shift);
                    }
                    self.go_idle();
                }
            }
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"UPHY", 1, |w| {
            w.u8(self.address);
            w.bool(self.mdc);
            w.bool(self.mdio);
            w.bool(self.pull_low);
            w.u8(match self.phase {
                Phase::Idle => 0,
                Phase::Header => 1,
                Phase::ReadTurn => 2,
                Phase::ReadData => 3,
                Phase::WriteTurn => 4,
                Phase::WriteData => 5,
            });
            w.u8(self.ones);
            w.u8(self.count);
            w.u16(self.shift);
            w.bool(self.selected);
            w.u8(self.reg);
            for &v in &self.regs {
                w.u16(v);
            }
            w.bool(self.link);
            w.bool(self.link_lost);
        });
    }

    /// Corrupt data gives an error and leaves the state as it was.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut r) = r.chunk(*b"UPHY", 1)?;
        let address = r.u8()?;
        let mdc = r.bool()?;
        let mdio = r.bool()?;
        let pull_low = r.bool()?;
        let phase = match r.u8()? {
            0 => Phase::Idle,
            1 => Phase::Header,
            2 => Phase::ReadTurn,
            3 => Phase::ReadData,
            4 => Phase::WriteTurn,
            5 => Phase::WriteData,
            _ => return Err(StateError::Invalid("MDIO phase")),
        };
        let ones = r.u8()?;
        let count = r.u8()?;
        let shift = r.u16()?;
        let selected = r.bool()?;
        let reg = r.u8()?;
        let mut regs = [0; 32];
        for v in &mut regs {
            *v = r.u16()?;
        }
        let link = r.bool()?;
        let link_lost = r.bool()?;

        if address > 0x1F || reg > 0x1F {
            return Err(StateError::Invalid("MDIO address"));
        }
        let limit = match phase {
            Phase::Idle => 0,
            Phase::Header => 13,
            Phase::WriteTurn => 1,
            Phase::ReadTurn => 0,
            Phase::ReadData => 16,
            Phase::WriteData => 15,
        };
        if ones > 32 || count > limit || (phase == Phase::Header && count == 0) {
            return Err(StateError::Invalid("MDIO bit count"));
        }
        if pull_low && phase != Phase::ReadData {
            return Err(StateError::Invalid("MDIO line"));
        }
        *self = Self {
            address,
            mdc,
            mdio,
            pull_low,
            phase,
            ones,
            count,
            shift,
            selected,
            reg,
            regs,
            link,
            link_lost,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{EtherLink, MemLink, NullC64};
    use crate::irq::Irq;

    const RX_MAC: u32 = BASE;
    const RX_PROMISC: u32 = BASE + 0x07;
    const RX_ENABLE: u32 = BASE + 0x08;
    const TX_ADDRESS: u32 = BASE + 0x10;
    const TX_LENGTH: u32 = BASE + 0x14;
    const TX_START: u32 = BASE + 0x18;
    const TX_IRQACK: u32 = BASE + 0x19;
    const TX_BUSY: u32 = BASE + 0x1A;
    const FREE_BASE: u32 = BASE + 0x20;
    const FREE_PUT: u32 = BASE + 0x24;
    const ALLOC_ID: u32 = BASE + 0x28;
    const ALLOC_SIZE: u32 = BASE + 0x2A;
    const FREE_RESET: u32 = BASE + 0x2E;
    const ALLOC_VALID: u32 = BASE + 0x2F;
    const ALLOC_POP: u32 = BASE + 0x2F;

    const MAC: [u8; 6] = [0x02, 0x15, 0x41, 0x71, 0x67, 0x42];
    const OTHER: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
    const BROADCAST: [u8; 6] = [0xFF; 6];
    /// Where the firmware's buffer pool lies in these tests.
    const POOL: u32 = 0x0004_0100;

    // ----- the PHY ---------------------------------------------------

    /// The firmware's way of clocking one bit out.
    fn mdio_bit(p: &mut MdioPhy, v: bool, link: bool) {
        p.set_mdio(v);
        p.set_mdc(true, link);
        p.set_mdc(false, link);
    }

    /// The header of the firmware's frames: 33 ones, start, operation,
    /// `000AA` with both A set for any address but 0, the register.
    fn mdio_header(p: &mut MdioPhy, read: bool, reg: u8, addr: u8, link: bool) {
        for _ in 0..33 {
            mdio_bit(p, true, link);
        }
        mdio_bit(p, false, link);
        mdio_bit(p, true, link);
        mdio_bit(p, read, link);
        mdio_bit(p, !read, link);
        for _ in 0..3 {
            mdio_bit(p, false, link);
        }
        mdio_bit(p, addr != 0, link);
        mdio_bit(p, addr != 0, link);
        for i in (0..5).rev() {
            mdio_bit(p, reg >> i & 1 != 0, link);
        }
    }

    /// The firmware's read of a PHY register.
    fn mdio_read(p: &mut MdioPhy, reg: u8, addr: u8, link: bool) -> u16 {
        mdio_header(p, true, reg, addr, link);
        mdio_bit(p, true, link);
        let mut result = 0;
        for _ in 0..16 {
            result <<= 1;
            mdio_bit(p, true, link);
            result |= u16::from(p.get_mdio());
        }
        result
    }

    /// The firmware's write of a PHY register.
    fn mdio_write(p: &mut MdioPhy, reg: u8, data: u16, addr: u8, link: bool) {
        mdio_header(p, false, reg, addr, link);
        mdio_bit(p, true, link);
        mdio_bit(p, false, link);
        for i in (0..16).rev() {
            mdio_bit(p, data >> i & 1 != 0, link);
        }
        mdio_bit(p, true, link);
    }

    #[test]
    fn the_firmware_finds_the_phy_at_address_0() {
        // H2: register 2 reads $0022 at the first address tried, also as
        // the very first frame after reset (MDC starts high, so the first
        // of the 33 ones makes no edge).
        let mut p = MdioPhy::default();
        assert_eq!(mdio_read(&mut p, 2, 0, false), 0x0022);
        assert_eq!(mdio_read(&mut p, 3, 0, false), PHY_ID2);
        // Nobody answers at address 3: the line floats high.
        assert_eq!(mdio_read(&mut p, 2, 3, false), 0xFFFF);
        assert_eq!(mdio_read(&mut p, 2, 0, false), 0x0022);
    }

    #[test]
    fn a_phy_strapped_to_3_is_found_on_the_second_try() {
        let mut p = MdioPhy::new(3);
        assert_eq!(p.address(), 3);
        assert_eq!(mdio_read(&mut p, 2, 0, false), 0xFFFF);
        assert_eq!(mdio_read(&mut p, 2, 3, false), 0x0022);
    }

    #[test]
    fn the_firmware_setup_writes_are_kept() {
        let mut p = MdioPhy::default();
        mdio_write(&mut p, 0x04, 0x01E1, 0, false);
        mdio_write(&mut p, 0x1B, 0x0500, 0, false);
        mdio_write(&mut p, 0x16, 0x0002, 0, false);
        mdio_write(&mut p, 0x00, 0x1200, 0, false);
        assert_eq!(mdio_read(&mut p, 0x04, 0, false), 0x01E1);
        assert_eq!(mdio_read(&mut p, 0x1B, 0, false), 0x0500);
        assert_eq!(mdio_read(&mut p, 0x16, 0, false), 0x0002);
        // "Restart autonegotiation" clears itself.
        assert_eq!(mdio_read(&mut p, 0x00, 0, false), 0x1000);
        // The identifier cannot be written.
        mdio_write(&mut p, 0x02, 0x1234, 0, false);
        assert_eq!(mdio_read(&mut p, 0x02, 0, false), 0x0022);
        // A write to another PHY changes nothing here.
        mdio_write(&mut p, 0x04, 0x0021, 3, false);
        assert_eq!(mdio_read(&mut p, 0x04, 0, false), 0x01E1);
    }

    #[test]
    fn the_link_bit_follows_the_cable() {
        // H3/H4: bit 2 of register 1 is the cable's state, stable while
        // it is plugged in.
        let mut p = MdioPhy::default();
        assert_eq!(mdio_read(&mut p, 1, 0, false), 0x7849);
        assert_eq!(mdio_read(&mut p, 1, 0, false), 0x7849);
        // The bit reads 0 once more after a loss, then follows.
        assert_eq!(mdio_read(&mut p, 1, 0, true), 0x7869);
        for _ in 0..5 {
            assert_eq!(mdio_read(&mut p, 1, 0, true), 0x786D);
        }
        assert_eq!(p.reg(1), 0x786D);
        assert_eq!(mdio_read(&mut p, 5, 0, true), 0x41E1);
        assert_eq!(mdio_read(&mut p, 6, 0, true), 1);
        assert_eq!(mdio_read(&mut p, 0x1E, 0, true), 0x0106);
        // Cable out.
        assert_eq!(mdio_read(&mut p, 1, 0, false), 0x7849);
        assert_eq!(mdio_read(&mut p, 5, 0, false), 0x0001);
        assert_eq!(mdio_read(&mut p, 0x1E, 0, false), 0);
        // A loss between two reads is seen at the next one.
        assert_eq!(mdio_read(&mut p, 1, 0, true), 0x7869);
        assert_eq!(mdio_read(&mut p, 1, 0, true), 0x786D);
        mdio_bit(&mut p, true, false);
        assert_eq!(mdio_read(&mut p, 1, 0, true), 0x7869);
        assert_eq!(mdio_read(&mut p, 1, 0, true), 0x786D);
    }

    #[test]
    fn speed_and_duplex_follow_the_advertisement() {
        let mut p = MdioPhy::default();
        mdio_write(&mut p, 4, 0x0061, 0, true);
        assert_eq!(mdio_read(&mut p, 0x1E, 0, true), 0x0105);
        mdio_write(&mut p, 4, 0x0081, 0, true);
        assert_eq!(mdio_read(&mut p, 0x1E, 0, true), 0x0102);
        mdio_write(&mut p, 4, 0x0021, 0, true);
        assert_eq!(mdio_read(&mut p, 0x1E, 0, true), 0x0101);
        // Forced 10 Mbit/s full duplex: no autonegotiation result.
        mdio_write(&mut p, 0, 0x0100, 0, true);
        assert_eq!(mdio_read(&mut p, 0x1E, 0, true), 0x0105);
        assert_eq!(mdio_read(&mut p, 1, 0, true) & 0x0020, 0);
        // Power down takes the link away.
        mdio_write(&mut p, 0, 0x1800, 0, true);
        assert_eq!(mdio_read(&mut p, 1, 0, true) & 0x0004, 0);
        // Reset brings the power-on values back.
        mdio_write(&mut p, 0, 0x8000, 0, true);
        assert_eq!(mdio_read(&mut p, 0, 0, true), 0x3100);
        assert_eq!(mdio_read(&mut p, 4, 0, true), 0x01E1);
    }

    #[test]
    fn the_line_is_the_boards_level_outside_a_read() {
        let mut p = MdioPhy::default();
        assert_eq!(p.get_mdio(), 1);
        p.set_mdio(false);
        assert_eq!(p.get_mdio(), 0);
        p.set_mdio(true);
        assert_eq!(p.get_mdio(), 1);
        // During the data of a read the PHY pulls for a 0 ...
        mdio_header(&mut p, true, 2, 0, false);
        mdio_bit(&mut p, true, false);
        assert_eq!(p.get_mdio(), 0);
        for _ in 0..16 {
            mdio_bit(&mut p, true, false);
        }
        // ... the last bit of $0022 is a 0 and stays until the next clock.
        assert_eq!(p.get_mdio(), 0);
        mdio_bit(&mut p, true, false);
        assert_eq!(p.get_mdio(), 1);
        assert!(!p.mdc());
    }

    #[test]
    fn a_short_preamble_or_a_wrong_start_is_no_frame() {
        let mut p = MdioPhy::default();
        // Bring the decoder to a known place.
        assert_eq!(mdio_read(&mut p, 2, 0, false), 0x0022);
        // 20 ones only, then what would be a read of register 2.
        mdio_bit(&mut p, false, false);
        for _ in 0..20 {
            mdio_bit(&mut p, true, false);
        }
        for b in [false, true, true, false] {
            mdio_bit(&mut p, b, false);
        }
        for b in [false; 5] {
            mdio_bit(&mut p, b, false);
        }
        for b in [false, false, false, true, false] {
            mdio_bit(&mut p, b, false);
        }
        let mut seen = 0_u16;
        for _ in 0..17 {
            mdio_bit(&mut p, true, false);
            seen = seen << 1 | u16::from(p.get_mdio());
        }
        assert_eq!(seen, 0xFFFF);
        // Start `00` (not clause 22) and the operations 00 and 11.
        for head in [[false, false, true, false], [false, true, false, false]] {
            for _ in 0..40 {
                mdio_bit(&mut p, true, false);
            }
            for b in head {
                mdio_bit(&mut p, b, false);
            }
            for _ in 0..40 {
                mdio_bit(&mut p, false, false);
                assert_eq!(p.get_mdio(), 0);
            }
        }
        // The next proper frame works.
        assert_eq!(mdio_read(&mut p, 2, 0, false), 0x0022);
    }

    #[test]
    fn any_bit_stream_is_taken_without_panic() {
        let mut p = MdioPhy::default();
        let mut x = 0x1234_5678_u32;
        for _ in 0..200_000 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            match x & 3 {
                0 => p.set_mdio(x & 4 != 0),
                1 => p.set_mdc(x & 4 != 0, x & 8 != 0),
                _ => {
                    assert!(p.get_mdio() <= 1);
                }
            }
        }
        for reg in 0..=255 {
            p.reg(reg);
        }
        p.reset();
        assert_eq!(p, MdioPhy::default());
    }

    #[test]
    fn phy_savestate_round_trip_mid_frame() {
        let mut p = MdioPhy::new(3);
        mdio_write(&mut p, 0x1B, 0x0500, 3, true);
        // Stop in the data of a read of register 1.
        mdio_header(&mut p, true, 1, 3, true);
        for _ in 0..6 {
            mdio_bit(&mut p, true, true);
        }
        let mut w = StateWriter::new();
        p.save(&mut w);
        let bytes = w.into_bytes();
        let mut q = MdioPhy::default();
        q.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(p, q);
        let rest = |p: &mut MdioPhy| {
            let mut v = 0_u16;
            for _ in 0..11 {
                mdio_bit(p, true, true);
                v = v << 1 | u16::from(p.get_mdio());
            }
            v
        };
        let a = rest(&mut p);
        assert_eq!(a, rest(&mut q));
        assert_eq!(a, 0x7869 & 0x07FF);
        assert_eq!(mdio_read(&mut q, 0x1B, 3, true), 0x0500);

        for cut in 0..bytes.len() {
            assert!(q.load(&mut StateReader::new(&bytes[..cut])).is_err());
        }
    }

    #[test]
    fn phy_savestate_rejects_impossible_values() {
        let mut p = MdioPhy::default();
        mdio_header(&mut p, true, 1, 0, true);
        let mut w = StateWriter::new();
        p.save(&mut w);
        let bytes = w.into_bytes();
        // Chunk header: tag 4, version 2, length 4.
        let body = 10;
        let mut q = MdioPhy::new(3);
        let before = q.clone();
        for (at, v) in [
            (body, 0x20_u8),   // address
            (body + 4, 6),     // phase
            (body + 5, 33),    // ones
            (body + 6, 17),    // count
            (body + 3, 1),     // the PHY pulls outside a read
            (body + 10, 0x20), // register number
        ] {
            let mut bad = bytes.clone();
            bad[at] = v;
            assert!(q.load(&mut StateReader::new(&bad)).is_err(), "byte {at}");
            assert_eq!(q, before);
        }
        q.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(q, p);
    }

    // ----- the MAC ---------------------------------------------------

    struct Rig {
        mac: Rmii,
        link: MemLink,
        /// `false`: nothing is plugged into the socket.
        plugged: bool,
        ram: Vec<u8>,
        irq: Irq,
        console: Vec<u8>,
        c64: NullC64,
        now: u64,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                mac: Rmii::new(),
                link: MemLink {
                    up: true,
                    ..MemLink::default()
                },
                plugged: true,
                ram: vec![0; 0x10_0000],
                irq: Irq::default(),
                console: Vec::new(),
                c64: NullC64::default(),
                now: 1000,
            }
        }

        fn with<R>(&mut self, f: impl FnOnce(&mut Rmii, &mut IoCtx<'_>) -> R) -> R {
            let mut ctx = IoCtx {
                now: self.now,
                ram: &mut self.ram,
                irq: &mut self.irq,
                console: &mut self.console,
                c64: &mut self.c64,
                storage: None,
                net: if self.plugged {
                    Some(&mut self.link)
                } else {
                    None
                },
            };
            f(&mut self.mac, &mut ctx)
        }

        fn rd(&mut self, addr: u32) -> u8 {
            self.with(|m, ctx| m.read(addr, ctx))
        }

        fn rd16(&mut self, addr: u32) -> u16 {
            u16::from_le_bytes([self.rd(addr), self.rd(addr + 1)])
        }

        fn wr(&mut self, addr: u32, v: u8) {
            self.with(|m, ctx| m.write(addr, v, ctx));
        }

        fn wr16(&mut self, addr: u32, v: u16) {
            for (i, b) in v.to_le_bytes().into_iter().enumerate() {
                self.wr(addr + i as u32, b);
            }
        }

        fn wr32(&mut self, addr: u32, v: u32) {
            for (i, b) in v.to_le_bytes().into_iter().enumerate() {
                self.wr(addr + i as u32, b);
            }
        }

        /// Let `clocks` pass, running the block's events when due.
        fn run(&mut self, clocks: u64) {
            let end = self.now + clocks;
            loop {
                let at = self.mac.next_event();
                if at > end {
                    break;
                }
                self.now = self.now.max(at);
                self.with(|m, ctx| m.tick(ctx));
            }
            self.now = end;
        }

        /// What the driver's task does first: the MAC address, the filter
        /// narrow, the receiver off; before that the pool's base.
        fn fw_start(&mut self) {
            self.wr32(FREE_BASE, POOL);
            for (i, b) in MAC.into_iter().enumerate() {
                self.wr(RX_MAC + i as u32, b);
            }
            self.wr(RX_PROMISC, 0);
            self.wr(RX_ENABLE, 0);
        }

        /// The driver's `initRx` after it saw a link.
        fn fw_init_rx(&mut self) {
            self.wr(RX_ENABLE, 0);
            self.run(CLOCK_HZ / 200);
            self.wr(FREE_RESET, 1);
            self.wr(ALLOC_POP, 1);
            self.wr(FREE_RESET, 1);
            for id in 0..32 {
                self.wr(FREE_PUT, id);
            }
            self.wr(RX_ENABLE, 1);
            self.irq.mask |= 1 << IRQ_RX;
        }

        fn up() -> Self {
            let mut r = Self::new();
            r.fw_start();
            r.fw_init_rx();
            r
        }

        /// The driver's interrupt handler: `None` when nothing is shown.
        fn fw_isr(&mut self) -> Option<(u8, u16)> {
            if self.rd(ALLOC_VALID) == 0 {
                return None;
            }
            let size = self.rd16(ALLOC_SIZE);
            let id = self.rd(ALLOC_ID);
            self.wr(ALLOC_POP, 1);
            Some((id, size))
        }

        /// The driver's `output_packet` for a frame lying at `addr`.
        fn fw_send(&mut self, addr: u32, len: u16) -> bool {
            if self.rd(TX_BUSY) != 0 {
                return false;
            }
            self.wr32(TX_ADDRESS, addr);
            self.wr16(TX_LENGTH, len);
            self.wr(TX_START, 1);
            true
        }

        /// The frame in buffer `id` as the firmware reads it.
        fn buffer(&self, id: u8, len: usize) -> &[u8] {
            let at = (POOL + BLOCK_SIZE * u32::from(id) + 2) as usize;
            &self.ram[at..at + len]
        }
    }

    fn frame(dst: [u8; 6], len: usize) -> Vec<u8> {
        let mut f = Vec::with_capacity(len);
        f.extend_from_slice(&dst);
        f.extend_from_slice(&OTHER);
        f.extend_from_slice(&[0x08, 0x00]);
        f.extend((14..len).map(|i| (i * 7 + len) as u8));
        f.truncate(len);
        f
    }

    #[test]
    fn frame_check_is_the_ethernet_crc() {
        assert_eq!(frame_check(b"123456789"), 0xCBF4_3926_u32.to_le_bytes());
        assert_eq!(frame_check(&[]), [0; 4]);
    }

    #[test]
    fn after_reset_everything_reads_zero_and_no_interrupt() {
        // T0 behaviour of the untouched block: not busy, nothing shown.
        let mut r = Rig::new();
        for a in 0..0x100 {
            assert_eq!(r.rd(BASE + a), 0, "{a:02x}");
            assert_eq!(r.mac.peek(BASE + a), 0);
        }
        assert_eq!(r.mac.next_event(), u64::MAX);
        assert_eq!(r.irq.level, 0);
        assert!(Rmii::claims(BASE) && Rmii::claims(BASE + 0xFF));
        assert!(!Rmii::claims(BASE + 0x100) && !Rmii::claims(BASE - 1));
    }

    #[test]
    fn the_firmware_init_sequence_leaves_32_buffers_and_the_receiver_on() {
        let mut r = Rig::new();
        r.fw_start();
        assert_eq!(r.mac.mac(), MAC);
        assert!(!r.mac.promiscuous());
        assert!(!r.mac.rx_enabled());
        assert_eq!(r.mac.next_event(), u64::MAX);
        r.fw_init_rx();
        assert!(r.mac.rx_enabled());
        assert_eq!(r.mac.free_buffers(), 32);
        assert_eq!(r.rd(ALLOC_VALID), 0);
        assert_eq!(r.rd(TX_BUSY), 0);
        assert!(!r.irq.line());
        // The receiver looks at the cable every millisecond from now on.
        assert_eq!(r.mac.next_event(), r.now + RX_POLL_CLOCKS);
        let t = r.mac.next_event();
        r.run(RX_POLL_CLOCKS);
        assert_eq!(r.mac.next_event(), t + RX_POLL_CLOCKS);
        // deinitRx.
        r.wr(RX_ENABLE, 0);
        assert_eq!(r.mac.next_event(), u64::MAX);
    }

    #[test]
    fn a_frame_from_memory_reaches_the_cable() {
        let mut r = Rig::up();
        let f = frame(BROADCAST, 342);
        // An odd address: the MAC fetches from any byte.
        let at = 0x0002_3457_u32;
        r.ram[at as usize..at as usize + f.len()].copy_from_slice(&f);
        assert!(r.fw_send(at, 342));
        assert_eq!(r.link.sent, vec![f.clone()]);
        // H7: busy while the bytes are fetched, then free again without
        // anybody acknowledging anything.
        assert_eq!(r.rd(TX_BUSY), 1);
        assert_eq!(r.irq.level & (1 << IRQ_TX), 0);
        assert_eq!(r.mac.next_event(), r.now + 342);
        r.run(341);
        assert_eq!(r.rd(TX_BUSY), 1);
        r.run(1);
        assert_eq!(r.mac.peek(TX_BUSY), 0);
        assert_eq!(r.rd(TX_BUSY), 0);
        // "Sent" is a level the firmware keeps masked: no interrupt.
        assert_ne!(r.irq.level & (1 << IRQ_TX), 0);
        assert!(!r.irq.line());
        assert!(r.fw_send(at, 342));
        assert_eq!(r.link.sent.len(), 2);
        r.run(1000);
        r.wr(TX_IRQACK, 0);
        assert_eq!(r.irq.level & (1 << IRQ_TX), 0);
        assert_eq!(r.mac.stats().tx_frames, 2);
    }

    #[test]
    fn busy_falls_on_the_read_even_before_the_tick() {
        let mut r = Rig::up();
        assert!(r.fw_send(0x1000, 60));
        r.now += 60;
        assert_eq!(r.rd(TX_BUSY), 0);
        assert_ne!(r.irq.level & (1 << IRQ_TX), 0);
    }

    #[test]
    fn a_start_while_busy_is_ignored() {
        let mut r = Rig::up();
        r.ram[0x1000..0x1040].fill(0x11);
        r.ram[0x2000..0x2040].fill(0x22);
        assert!(r.fw_send(0x1000, 64));
        assert!(!r.fw_send(0x2000, 64));
        r.wr32(TX_ADDRESS, 0x2000);
        r.wr(TX_START, 1);
        assert_eq!(r.link.sent, vec![vec![0x11; 64]]);
        r.run(64);
        r.wr(TX_START, 1);
        assert_eq!(r.link.sent.len(), 2);
        assert_eq!(r.link.sent[1], vec![0x22; 64]);
    }

    #[test]
    fn the_firmware_pads_and_the_mac_sends_what_it_is_told() {
        let mut r = Rig::up();
        // The firmware's padded copy of a 42-byte ARP request: 60 bytes.
        let mut f = frame(BROADCAST, 42);
        f.resize(60, 0);
        r.ram[0x3000..0x3000 + 60].copy_from_slice(&f);
        assert!(r.fw_send(0x3000, 60));
        assert_eq!(r.link.sent, vec![f]);
        r.run(100);
        // Unpadded it would be a runt no receiver takes: not on the cable.
        assert!(r.fw_send(0x3000, 42));
        assert_eq!(r.rd(TX_BUSY), 1);
        r.run(42);
        assert_eq!(r.rd(TX_BUSY), 0);
        // Longest frame with a VLAN tag passes, one more byte does not.
        assert!(r.fw_send(0x3000, 1518));
        r.run(1518);
        assert!(r.fw_send(0x3000, 1519));
        r.run(1519);
        // Length 0 counts as 4096 bytes.
        assert!(r.fw_send(0x3000, 0));
        assert_eq!(r.mac.next_event().min(r.now + 4096), r.now + 4096);
        r.run(4095);
        assert_eq!(r.rd(TX_BUSY), 1);
        r.run(1);
        assert_eq!(r.rd(TX_BUSY), 0);
        assert_eq!(r.link.sent.len(), 2);
        assert_eq!(r.link.sent[1].len(), 1518);
        assert_eq!(r.mac.stats().tx_frames, 2);
        assert_eq!(r.mac.stats().tx_discarded, 3);
    }

    #[test]
    fn sending_without_a_link_loses_the_frame_but_ends() {
        for plugged in [false, true] {
            let mut r = Rig::up();
            r.plugged = plugged;
            r.link.up = false;
            assert!(r.fw_send(0x1000, 60));
            assert_eq!(r.rd(TX_BUSY), 1);
            r.run(60);
            assert_eq!(r.rd(TX_BUSY), 0);
            assert!(r.link.sent.is_empty());
            assert_eq!(r.mac.stats().tx_discarded, 1);
        }
    }

    #[test]
    fn the_address_registers_are_26_and_12_bits_wide() {
        let mut r = Rig::up();
        r.ram[0x5000..0x5040].fill(0x5A);
        // Bits above 25 of the address and above 11 of the length are
        // not there.
        r.wr32(TX_ADDRESS, 0xFC00_5000);
        r.wr16(TX_LENGTH, 0xF040);
        r.wr(TX_START, 1);
        assert_eq!(r.link.sent, vec![vec![0x5A; 64]]);
    }

    #[test]
    fn a_frame_from_the_cable_lands_in_a_free_buffer() {
        let mut r = Rig::up();
        let f = frame(MAC, 98);
        r.link.incoming.push_back(f.clone());
        assert!(r.link.pending());
        assert!(!r.irq.line());
        r.run(RX_POLL_CLOCKS);
        // H5: the interrupt is the "frame shown" level.
        assert!(r.irq.line());
        assert_eq!(r.irq.active(), 1 << IRQ_RX);
        assert_eq!(r.mac.peek(ALLOC_VALID), 1);
        assert!(r.mac.rx_waiting());
        // The length is the frame's, without the check sequence.
        assert_eq!(r.rd16(ALLOC_SIZE), 98);
        assert_eq!(r.rd(ALLOC_ID), 0);
        // The frame starts two bytes into the buffer, its check sequence
        // follows it.
        assert_eq!(r.buffer(0, 98), &f[..]);
        let fcs_at = (POOL + 2 + 98) as usize;
        assert_eq!(r.ram[fcs_at..fcs_at + 4], frame_check(&f));
        assert_eq!(r.ram[POOL as usize..POOL as usize + 2], [0, 0]);
        // Reading does not acknowledge; the level stays until the pop.
        assert!(r.irq.line());
        r.run(10 * RX_POLL_CLOCKS);
        assert!(r.irq.line());
        r.wr(ALLOC_POP, 1);
        assert!(!r.irq.line());
        assert_eq!(r.rd(ALLOC_VALID), 0);
        // H6: after the pop the buffer number reads $FF.
        assert_eq!(r.rd(ALLOC_ID), 0xFF);
        assert_eq!(r.mac.free_buffers(), 31);
        assert_eq!(r.mac.stats().rx_frames, 1);
    }

    #[test]
    fn frames_queue_and_the_level_rises_again_after_each_pop() {
        // H10: a level, so a frame that comes while one is shown is not
        // lost.
        let mut r = Rig::up();
        for n in 0..3 {
            r.link.incoming.push_back(frame(MAC, 60 + n));
        }
        // One poll period and the wire time of two short frames.
        r.run(RX_POLL_CLOCKS + 2 * 90 * 8);
        assert!(r.link.incoming.is_empty());
        assert_eq!(r.mac.queued_frames(), 2);
        for n in 0..3_u8 {
            assert!(r.irq.line());
            assert_eq!(r.fw_isr(), Some((n, 60 + u16::from(n))));
        }
        assert!(!r.irq.line());
        assert_eq!(r.fw_isr(), None);
        // A pop with nothing shown changes nothing.
        r.wr(ALLOC_POP, 1);
        assert_eq!(r.rd(ALLOC_ID), 0xFF);
        assert!(!r.irq.line());
    }

    #[test]
    fn a_burst_arrives_at_the_speed_of_the_wire() {
        let mut r = Rig::up();
        for _ in 0..4 {
            r.link.incoming.push_back(frame(MAC, 1514));
        }
        r.run(RX_POLL_CLOCKS);
        assert_eq!(r.link.incoming.len(), 3);
        // 1514 bytes and 24 of preamble, check sequence and gap, 80 ns
        // each.
        let wire = (1514 + 24) * 8;
        r.run(wire - 1);
        assert_eq!(r.link.incoming.len(), 3);
        r.run(1);
        assert_eq!(r.link.incoming.len(), 2);
        r.run(2 * wire);
        assert!(r.link.incoming.is_empty());
        // Idle again: back to the millisecond.
        let t = r.mac.next_event();
        r.run(wire);
        assert_eq!(r.mac.next_event(), t + RX_POLL_CLOCKS);
    }

    #[test]
    fn recycled_buffers_keep_the_receiver_fed() {
        // H8: the numbers the firmware hands back are used again, in the
        // order they came.
        let mut r = Rig::up();
        let mut ids = Vec::new();
        for n in 0..100_usize {
            let f = frame(BROADCAST, 60 + n);
            r.link.incoming.push_back(f.clone());
            r.run(RX_POLL_CLOCKS);
            let (id, size) = r.fw_isr().unwrap();
            assert_eq!(usize::from(size), 60 + n);
            assert_eq!(r.buffer(id, 60 + n), &f[..]);
            ids.push(id);
            r.wr(FREE_PUT, id);
        }
        let expect: Vec<u8> = (0..100).map(|n| n % 32).collect();
        assert_eq!(ids, expect);
        assert_eq!(r.mac.stats().rx_no_buffer, 0);
    }

    #[test]
    fn without_a_free_buffer_the_frame_is_dropped() {
        let mut r = Rig::up();
        for n in 0..34 {
            r.link.incoming.push_back(frame(MAC, 64 + n));
        }
        r.run(40 * RX_POLL_CLOCKS);
        // All taken off the cable; 32 kept, two gone.
        assert!(r.link.incoming.is_empty());
        assert_eq!(r.mac.free_buffers(), 0);
        assert_eq!(r.mac.stats().rx_frames, 32);
        assert_eq!(r.mac.stats().rx_no_buffer, 2);
        for n in 0..32_u8 {
            assert_eq!(r.fw_isr(), Some((n, 64 + u16::from(n))));
        }
        assert_eq!(r.fw_isr(), None);
        // With a buffer back the next frame is kept.
        r.wr(FREE_PUT, 7);
        r.link.incoming.push_back(frame(MAC, 70));
        r.run(RX_POLL_CLOCKS);
        assert_eq!(r.fw_isr(), Some((7, 70)));
    }

    #[test]
    fn the_filter_passes_own_address_and_broadcast_only() {
        let mut r = Rig::up();
        let deliver = |r: &mut Rig, dst: [u8; 6]| {
            let f = frame(dst, 64);
            r.with(|m, ctx| m.deliver(&f, ctx))
        };
        assert_eq!(deliver(&mut r, OTHER), RxOutcome::NotForUs);
        assert!(!r.irq.line());
        // Group addresses do not pass: IPv4 and IPv6 multicast.
        assert_eq!(
            deliver(&mut r, [0x01, 0x00, 0x5E, 0x00, 0x00, 0xFB]),
            RxOutcome::NotForUs
        );
        assert_eq!(
            deliver(&mut r, [0x33, 0x33, 0x00, 0x00, 0x00, 0x01]),
            RxOutcome::NotForUs
        );
        // One byte off.
        let mut near = MAC;
        near[5] ^= 1;
        assert_eq!(deliver(&mut r, near), RxOutcome::NotForUs);
        // The buffer taken for the first of them serves the frame that
        // is kept: nothing was lost.
        assert_eq!(r.mac.free_buffers(), 31);
        assert_eq!(deliver(&mut r, MAC), RxOutcome::Stored(0));
        assert_eq!(deliver(&mut r, BROADCAST), RxOutcome::Stored(1));
        // The comparison is by 16-bit words, each the own or all ones.
        let mixed = [0xFF, 0xFF, MAC[2], MAC[3], 0xFF, 0xFF];
        assert_eq!(deliver(&mut r, mixed), RxOutcome::Stored(2));
        let half = [0xFF, MAC[1], MAC[2], MAC[3], MAC[4], MAC[5]];
        assert_eq!(deliver(&mut r, half), RxOutcome::NotForUs);
        assert_eq!(r.mac.stats().rx_filtered, 5);
        assert_eq!(r.mac.stats().rx_frames, 3);
        assert_eq!(r.mac.free_buffers(), 28);
    }

    #[test]
    fn promiscuous_takes_everything() {
        let mut r = Rig::up();
        r.wr(RX_PROMISC, 1);
        assert!(r.mac.promiscuous());
        for dst in [OTHER, [0x01, 0x00, 0x5E, 0x00, 0x00, 0xFB], MAC, BROADCAST] {
            r.link.incoming.push_back(frame(dst, 60));
        }
        r.run(2 * RX_POLL_CLOCKS);
        for n in 0..4 {
            assert_eq!(r.fw_isr(), Some((n, 60)));
        }
        r.wr(RX_PROMISC, 0);
        r.link.incoming.push_back(frame(OTHER, 60));
        r.run(RX_POLL_CLOCKS);
        assert_eq!(r.fw_isr(), None);
    }

    #[test]
    fn short_frames_are_kept_and_long_ones_dropped() {
        let mut r = Rig::up();
        // No lower limit: a runt is passed on as it is.
        let runt = frame(MAC, 20);
        assert_eq!(r.with(|m, ctx| m.deliver(&runt, ctx)), RxOutcome::Stored(0));
        assert_eq!(r.fw_isr(), Some((0, 20)));
        assert_eq!(r.buffer(0, 20), &runt[..]);
        // Shorter than an address: the filter compares what follows.
        assert_eq!(
            r.with(|m, ctx| m.deliver(&[0xFF, 0xFF, 0xFF], ctx)),
            RxOutcome::NotForUs
        );
        assert_eq!(r.with(|m, ctx| m.deliver(&[], ctx)), RxOutcome::Empty);
        // The longest frame fills the buffer to its last byte ...
        let long = frame(MAC, MAX_RX_FRAME);
        assert_eq!(r.with(|m, ctx| m.deliver(&long, ctx)), RxOutcome::Stored(1));
        assert_eq!(r.fw_isr(), Some((1, 1530)));
        assert_eq!(r.buffer(1, 1530), &long[..]);
        let next = (POOL + 2 * BLOCK_SIZE) as usize;
        assert_eq!(r.ram[next - 4..next], frame_check(&long));
        assert_eq!(r.ram[next..next + 2], [0, 0]);
        // ... one byte more is too long.
        let giant = frame(MAC, MAX_RX_FRAME + 1);
        assert_eq!(r.with(|m, ctx| m.deliver(&giant, ctx)), RxOutcome::TooLong);
        assert_eq!(r.fw_isr(), None);
        assert_eq!(r.mac.stats().rx_too_long, 1);
        // The buffer taken for it serves the next frame.
        assert_eq!(r.with(|m, ctx| m.deliver(&runt, ctx)), RxOutcome::Stored(2));
    }

    #[test]
    fn the_receiver_off_takes_nothing_and_loses_its_buffer() {
        let mut r = Rig::up();
        // A frame not for us leaves buffer 0 in the receiver's hand.
        let f = frame(OTHER, 60);
        assert_eq!(r.with(|m, ctx| m.deliver(&f, ctx)), RxOutcome::NotForUs);
        let kept = frame(MAC, 60);
        assert_eq!(r.with(|m, ctx| m.deliver(&kept, ctx)), RxOutcome::Stored(0));
        assert_eq!(r.with(|m, ctx| m.deliver(&f, ctx)), RxOutcome::NotForUs);
        r.wr(RX_ENABLE, 0);
        // What was received stays shown.
        assert!(r.irq.line());
        assert_eq!(r.with(|m, ctx| m.deliver(&kept, ctx)), RxOutcome::Disabled);
        r.link.incoming.push_back(kept.clone());
        r.run(5 * RX_POLL_CLOCKS);
        assert_eq!(r.link.incoming.len(), 1);
        // On again: what waited on the cable is gone, buffer 1 too.
        r.wr(RX_ENABLE, 1);
        assert!(r.link.incoming.is_empty());
        assert_eq!(r.fw_isr(), Some((0, 60)));
        assert_eq!(r.with(|m, ctx| m.deliver(&kept, ctx)), RxOutcome::Stored(2));
    }

    #[test]
    fn nothing_arrives_without_a_link() {
        let mut r = Rig::up();
        r.link.up = false;
        r.link.incoming.push_back(frame(MAC, 60));
        r.run(3 * RX_POLL_CLOCKS);
        assert_eq!(r.link.incoming.len(), 1);
        r.link.up = true;
        r.run(RX_POLL_CLOCKS);
        assert_eq!(r.fw_isr(), Some((0, 60)));
        // Nothing plugged in at all.
        r.plugged = false;
        r.run(3 * RX_POLL_CLOCKS);
        assert_eq!(r.fw_isr(), None);
        r.wr(RX_ENABLE, 0);
        r.wr(RX_ENABLE, 1);
        assert!(r.fw_send(0x1000, 60));
    }

    #[test]
    fn a_new_link_starts_from_clean_queues() {
        // The driver's second initRx: frames still queued and the one
        // shown are thrown away, all 32 numbers are entered again.
        let mut r = Rig::up();
        for _ in 0..3 {
            r.link.incoming.push_back(frame(MAC, 60));
        }
        r.run(2 * RX_POLL_CLOCKS);
        assert!(r.irq.line());
        assert_eq!(r.mac.queued_frames(), 2);
        r.fw_init_rx();
        assert!(!r.irq.line());
        assert_eq!(r.rd(ALLOC_VALID), 0);
        assert_eq!(r.mac.queued_frames(), 0);
        assert_eq!(r.mac.free_buffers(), 32);
        r.link.incoming.push_back(frame(MAC, 61));
        r.run(RX_POLL_CLOCKS);
        assert_eq!(r.fw_isr(), Some((0, 61)));
    }

    #[test]
    fn emptying_the_queues_keeps_the_frame_shown() {
        let mut r = Rig::up();
        r.link.incoming.push_back(frame(MAC, 60));
        r.link.incoming.push_back(frame(MAC, 61));
        r.run(2 * RX_POLL_CLOCKS);
        r.wr(FREE_RESET, 0);
        assert_eq!(r.mac.free_buffers(), 0);
        assert_eq!(r.mac.queued_frames(), 0);
        assert!(r.irq.line());
        assert_eq!(r.fw_isr(), Some((0, 60)));
        assert_eq!(r.fw_isr(), None);
    }

    #[test]
    fn the_free_queue_holds_127_numbers() {
        let mut r = Rig::new();
        r.fw_start();
        for id in 0..126 {
            r.wr(FREE_PUT, id);
            assert_eq!(r.rd(FREE_PUT), 0);
        }
        r.wr(FREE_PUT, 126);
        assert_eq!(r.rd(FREE_PUT), 2);
        assert_eq!(r.mac.free_buffers(), 127);
        // Full: further numbers are lost.
        r.wr(FREE_PUT, 200);
        assert_eq!(r.mac.free_buffers(), 127);
        // The same number twice is not noticed (as on the board).
        r.wr(FREE_RESET, 1);
        r.wr(FREE_PUT, 5);
        r.wr(FREE_PUT, 5);
        r.wr(RX_ENABLE, 1);
        let f = frame(MAC, 60);
        assert_eq!(r.with(|m, ctx| m.deliver(&f, ctx)), RxOutcome::Stored(5));
        assert_eq!(r.with(|m, ctx| m.deliver(&f, ctx)), RxOutcome::Stored(5));
        assert_eq!(r.with(|m, ctx| m.deliver(&f, ctx)), RxOutcome::NoBuffer);
    }

    #[test]
    fn the_pool_base_has_no_low_byte_and_any_buffer_number_stays_in_memory() {
        let mut r = Rig::new();
        r.fw_start();
        // The low byte of the base is not stored.
        r.wr32(FREE_BASE, 0xFC00_12FF);
        r.wr(FREE_PUT, 3);
        // A number the firmware never uses, with the base at the top of
        // the 26 bits: the address wraps, nothing panics.
        r.wr(FREE_PUT, 0xFF);
        r.wr(RX_ENABLE, 1);
        let f = frame(MAC, 100);
        assert_eq!(r.with(|m, ctx| m.deliver(&f, ctx)), RxOutcome::Stored(3));
        let at = 0x1200 + 3 * 1536 + 2;
        assert_eq!(r.ram[at..at + 100], f[..]);
        r.wr32(FREE_BASE, 0x03FF_FF00);
        assert_eq!(r.with(|m, ctx| m.deliver(&f, ctx)), RxOutcome::Stored(0xFF));
        assert_eq!(r.fw_isr(), Some((3, 100)));
        assert_eq!(r.fw_isr(), Some((0xFF, 100)));
        // Memory of no size at all.
        r.ram = Vec::new();
        r.wr(FREE_PUT, 1);
        assert_eq!(r.with(|m, ctx| m.deliver(&f, ctx)), RxOutcome::Stored(1));
        r.link.up = true;
        assert!(r.fw_send(0, 60));
        assert_eq!(r.link.sent, vec![vec![0; 60]]);
    }

    #[test]
    fn every_address_and_value_is_taken_without_panic() {
        let mut r = Rig::up();
        r.link.incoming.push_back(frame(BROADCAST, 200));
        let mut x = 0x2468_ACE1_u32;
        for round in 0..20_000_u32 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let addr = BASE + (x & 0xFF);
            if x & 0x100 != 0 {
                r.wr(addr, (x >> 16) as u8);
            } else {
                r.rd(addr);
                r.mac.peek(addr);
            }
            if round % 16 == 0 {
                r.link
                    .incoming
                    .push_back(frame(BROADCAST, 60 + (x >> 20) as usize % 1500));
                r.run(u64::from(x >> 12 & 0xFFFF));
            }
            // H5: the interrupt level is "a frame is shown", always.
            let shown = r.mac.peek(ALLOC_VALID) & 1 != 0;
            assert_eq!(r.irq.level & (1 << IRQ_RX) != 0, shown);
        }
        // The state so reached survives a round trip.
        let mut w = StateWriter::new();
        r.mac.save(&mut w);
        let bytes = w.into_bytes();
        let mut m = Rmii::new();
        m.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(m, r.mac);
    }

    #[test]
    fn peek_changes_nothing() {
        let mut r = Rig::up();
        r.link.incoming.push_back(frame(MAC, 60));
        r.run(RX_POLL_CLOCKS);
        assert!(r.fw_send(0x1000, 60));
        r.now += 1000;
        let before = r.mac.clone();
        for a in 0..0x100 {
            r.mac.peek(BASE + a);
        }
        assert_eq!(r.mac, before);
        // Busy is still shown to the debugger; a read lets it fall.
        assert_eq!(r.mac.peek(TX_BUSY), 1);
        assert_eq!(r.rd(TX_BUSY), 0);
    }

    #[test]
    fn reset_gives_the_power_on_state() {
        let mut r = Rig::up();
        r.link.incoming.push_back(frame(MAC, 60));
        r.run(RX_POLL_CLOCKS);
        assert!(r.fw_send(0x1000, 60));
        r.mac.reset();
        assert_eq!(r.mac, Rmii::new());
        assert_eq!(Rmii::default(), Rmii::new());
    }

    #[test]
    fn savestate_round_trip() {
        let mut r = Rig::up();
        // A frame shown, two queued, one buffer in the receiver's hand,
        // a frame being sent.
        for n in 0..3 {
            r.link.incoming.push_back(frame(MAC, 100 + n));
        }
        r.run(2 * RX_POLL_CLOCKS);
        let other = frame(OTHER, 60);
        r.with(|m, ctx| m.deliver(&other, ctx));
        assert!(r.fw_send(0x1000, 700));
        r.run(100);

        let mut w = StateWriter::new();
        r.mac.save(&mut w);
        let bytes = w.into_bytes();
        let mut loaded = Rmii::new();
        loaded.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(loaded, r.mac);
        assert_eq!(loaded.next_event(), r.mac.next_event());

        // Both go on alike.
        let mut twin = Rig::new();
        twin.mac = loaded;
        twin.now = r.now;
        twin.ram.clone_from(&r.ram);
        twin.irq = r.irq.clone();
        for rig in [&mut r, &mut twin] {
            assert_eq!(rig.rd(TX_BUSY), 1);
            rig.run(600);
            assert_eq!(rig.rd(TX_BUSY), 0);
            rig.link.incoming.push_back(frame(BROADCAST, 77));
            rig.run(RX_POLL_CLOCKS);
            let got: Vec<_> = std::iter::from_fn(|| rig.fw_isr()).collect();
            assert_eq!(got, vec![(0, 100), (1, 101), (2, 102), (3, 77)]);
        }
        assert_eq!(r.mac, twin.mac);
    }

    #[test]
    fn savestate_truncated_and_invalid() {
        let mut r = Rig::up();
        r.link.incoming.push_back(frame(MAC, 100));
        r.run(RX_POLL_CLOCKS);
        let other = frame(OTHER, 60);
        r.with(|m, ctx| m.deliver(&other, ctx));
        assert!(r.fw_send(0x1000, 700));
        let mut w = StateWriter::new();
        r.mac.save(&mut w);
        let bytes = w.into_bytes();

        let mut m = Rmii::new();
        m.wr_mac_for_test();
        let before = m.clone();
        for cut in 0..bytes.len() {
            assert!(m.load(&mut StateReader::new(&bytes[..cut])).is_err());
        }
        assert_eq!(m, before);
        assert!(m.load(&mut StateReader::new(b"junk")).is_err());

        // Chunk header: tag 4, version 2, length 4. Then the fields in
        // the order of `save`.
        let body = 10;
        let pointers = body + 6 + 3 + 5 + 8 + 4 + 2 + 2 + 8 + 4 + 128 + 128 + 256;
        for (at, v, what) in [
            (pointers, 0x80_u8, "free head"),
            (pointers + 3, 0xFF, "used tail"),
            (body + 6 + 3 + 5 + 8 + 3, 0x04, "send address"),
            (body + 6 + 3 + 5 + 8 + 4 + 1, 0x10, "send length"),
            (body + 6 + 3 + 5 + 8 + 4 + 2, 0, "busy without an end"),
            (body + 7, 0, "buffer held, receiver off"),
            (body + 6 + 3 + 5 + 8 + 4 + 2 + 2 + 8, 1, "base low byte"),
            (pointers + 4 + 1 + 1 + 1, 0x10, "length shown"),
            (pointers - 255, 0x06, "queued length"),
        ] {
            let mut bad = bytes.clone();
            bad[at] = v;
            assert!(m.load(&mut StateReader::new(&bad)).is_err(), "{what}");
            assert_eq!(m, before, "{what}");
        }
        m.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(m, r.mac);
    }

    impl Rmii {
        /// Something to tell a failed load from a fresh block.
        fn wr_mac_for_test(&mut self) {
            self.mac = [9; 6];
        }
    }

    #[test]
    fn the_whole_bring_up_as_the_firmware_does_it() {
        // C24 from end to end: probe, set up, wait for the link, receive
        // set-up, one frame each way, link loss.
        let mut r = Rig::new();
        let mut phy = MdioPhy::default();
        r.link.up = false;
        r.wr32(FREE_BASE, POOL);
        let mut addr = 0;
        if mdio_read(&mut phy, 2, addr, r.link.link_up()) != 0x0022 {
            addr = 3;
        }
        assert_eq!(addr, 0);
        for (reg, v) in [
            (0x04, 0x01E1),
            (0x1B, 0x0500),
            (0x16, 0x0002),
            (0x00, 0x1200),
        ] {
            mdio_write(&mut phy, reg, v, addr, r.link.link_up());
        }
        r.fw_start();
        // No cable: the driver polls every 250 ms and stays down.
        for _ in 0..4 {
            assert_eq!(mdio_read(&mut phy, 1, addr, r.link.link_up()) & 4, 0);
            r.run(CLOCK_HZ / 4);
        }
        assert!(!r.mac.rx_enabled());
        // Cable in: seen within two polls.
        r.link.up = true;
        let mut polls = 0;
        while mdio_read(&mut phy, 1, addr, r.link.link_up()) & 4 == 0 {
            polls += 1;
            assert!(polls < 3);
            r.run(CLOCK_HZ / 4);
        }
        r.fw_init_rx();
        // A DHCP discover goes out, an answer comes back.
        let mut out = frame(BROADCAST, 342);
        out[6..12].copy_from_slice(&MAC);
        r.ram[0x8000..0x8000 + 342].copy_from_slice(&out);
        assert!(r.fw_send(0x8000, 342));
        assert_eq!(r.link.sent.last(), Some(&out));
        let answer = frame(MAC, 342);
        r.link.incoming.push_back(answer.clone());
        r.run(RX_POLL_CLOCKS);
        assert!(r.irq.line());
        let (id, size) = r.fw_isr().unwrap();
        assert_eq!(r.buffer(id, usize::from(size)), &answer[..]);
        r.wr(FREE_PUT, id);
        assert!(!r.irq.line());
        // A second without frames, the cable is out: deinitRx.
        r.link.up = false;
        r.run(u64::from(CLOCK_HZ as u32));
        assert_eq!(mdio_read(&mut phy, 1, addr, r.link.link_up()) & 4, 0);
        r.wr(RX_ENABLE, 0);
        r.irq.mask &= !(1 << IRQ_RX);
        assert_eq!(r.mac.next_event(), u64::MAX);
    }
}
