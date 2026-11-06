//! Telegraph's network unit (SPEC.md 10.1): Ethernet-like frames through
//! a transmit and a receive ring of 16 descriptors each in chip RAM, the
//! CRC-32 in hardware, a MAC filter, interrupts.
//!
//! **[S]** (decided here, recorded in SPEC.md 10.1): the hardware works
//! by the descriptors' OWN bits; TXTAIL written starts sending the
//! descriptors from TXHEAD up to it that the CPU gave the hardware, RXTAIL
//! is the CPU's bookkeeping only. A frame moves at once (sent: into the
//! machine's outbox with its frame check sequence appended; received: into
//! the next receive descriptor the hardware owns, its length field set to
//! the frame's length without the FCS); its DMA slots (a slot a 4 bytes)
//! are charged to the following lines through [`Net::slots`], at most 8 a
//! line together with the SD card. A frame no descriptor can take is
//! dropped (NETIRQ bit 2). A received frame passes the filter when it is
//! for this MAC, a broadcast or multicast (bit 0 of the first byte), or
//! with NETCON bit 2 always; a wrong FCS sets descriptor flag bit 1 and
//! the frame is still delivered. Frames longer than a buffer are cut to
//! it with flag bit 0. NETIRQ bits raise Telegraph's network interrupt
//! when NETIRQEN enables them.

use emu_core::{StateError, StateReader, StateWriter};

use crate::lithograph::ChipRamMut;

pub const NETCON: u16 = 0x4000;
pub const NETMAC0: u16 = 0x4002;
pub const NETMAC1: u16 = 0x4004;
pub const NETMAC2: u16 = 0x4006;
pub const TXRSEG: u16 = 0x4008;
pub const TXROFS: u16 = 0x400a;
pub const RXRSEG: u16 = 0x400c;
pub const RXROFS: u16 = 0x400e;
pub const TXTAIL: u16 = 0x4010;
pub const TXHEAD: u16 = 0x4012;
pub const RXHEAD: u16 = 0x4014;
pub const RXTAIL: u16 = 0x4016;
pub const NETSTAT: u16 = 0x4018;
pub const NETIRQ: u16 = 0x401a;
pub const NETIRQEN: u16 = 0x401c;

pub const CON_RX: u16 = 1 << 0;
pub const CON_TX: u16 = 1 << 1;
pub const CON_PROMISC: u16 = 1 << 2;
pub const IRQ_RX: u16 = 1 << 0;
pub const IRQ_TX: u16 = 1 << 1;
pub const IRQ_FULL: u16 = 1 << 2;
pub const IRQ_LINK: u16 = 1 << 3;
pub const DESC_OWN: u16 = 1 << 15;
pub const DESC_ERROR: u16 = 1 << 0;
pub const DESC_CRC: u16 = 1 << 1;

/// The largest frame without its FCS.
pub const MAX_FRAME: usize = 1536;
const MIN_FRAME: usize = 14;
const CHIP_END: u32 = 0x0040_0000;
/// Network and SD together: 8 slots a line.
const SLOTS_PER_LINE: u16 = 8;

/// The Ethernet CRC-32 (IEEE 802.3: reflected polynomial $EDB88320,
/// initial and final value inverted).
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                c >> 1 ^ 0xedb8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Net {
    con: u16,
    mac: [u16; 3],
    txr: (u16, u16),
    rxr: (u16, u16),
    tx_tail: u16,
    tx_head: u16,
    rx_head: u16,
    rx_tail: u16,
    link: bool,
    irq: u16,
    irq_en: u16,
    /// DMA slots still to be charged.
    pending_slots: u32,
    /// Frames sent (with their FCS) for the machine to put on the wire.
    pub outbox: Vec<Vec<u8>>,
}

fn lin(p: (u16, u16)) -> u32 {
    u32::from(p.0) << 16 | u32::from(p.1)
}

impl Net {
    pub fn new(mac: [u8; 6]) -> Self {
        let mut n = Self::default();
        n.set_mac(mac);
        n
    }

    fn set_mac(&mut self, m: [u8; 6]) {
        for k in 0..3 {
            self.mac[k] = u16::from(m[2 * k]) << 8 | u16::from(m[2 * k + 1]);
        }
    }

    pub fn mac(&self) -> [u8; 6] {
        let mut m = [0; 6];
        for k in 0..3 {
            m[2 * k] = (self.mac[k] >> 8) as u8;
            m[2 * k + 1] = self.mac[k] as u8;
        }
        m
    }

    /// The network interrupt is pending.
    pub fn irq(&self) -> bool {
        self.irq & self.irq_en != 0
    }

    pub fn peek(&self, off: u16) -> u16 {
        match off & !1 {
            NETCON => self.con,
            NETMAC0 => self.mac[0],
            NETMAC1 => self.mac[1],
            NETMAC2 => self.mac[2],
            TXRSEG => self.txr.0,
            TXROFS => self.txr.1,
            RXRSEG => self.rxr.0,
            RXROFS => self.rxr.1,
            TXTAIL => self.tx_tail,
            TXHEAD => self.tx_head,
            RXHEAD => self.rx_head,
            RXTAIL => self.rx_tail,
            NETSTAT => u16::from(self.link),
            NETIRQ => self.irq,
            NETIRQEN => self.irq_en,
            _ => 0,
        }
    }

    /// A register write; transmit work runs at once through `mem`.
    pub fn write(&mut self, off: u16, v: u16, mem: &mut dyn ChipRamMut) {
        match off & !1 {
            NETCON => self.con = v & 7,
            NETMAC0 => self.mac[0] = v,
            NETMAC1 => self.mac[1] = v,
            NETMAC2 => self.mac[2] = v,
            TXRSEG => self.txr.0 = v,
            TXROFS => self.txr.1 = v,
            RXRSEG => self.rxr.0 = v,
            RXROFS => self.rxr.1 = v,
            TXTAIL => {
                self.tx_tail = v & 15;
                self.transmit(mem);
            }
            RXTAIL => self.rx_tail = v & 15,
            NETIRQ => self.irq &= !v,
            NETIRQEN => self.irq_en = v & 15,
            _ => {}
        }
    }

    fn word(mem: &dyn ChipRamMut, a: u32) -> u16 {
        u16::from_le_bytes([mem.get(a), mem.get(a.wrapping_add(1))])
    }

    fn set_word(mem: &mut dyn ChipRamMut, a: u32, v: u16) {
        let [lo, hi] = v.to_le_bytes();
        mem.set(a, lo);
        mem.set(a.wrapping_add(1), hi);
    }

    /// Send the descriptors from TXHEAD up to TXTAIL the hardware owns.
    fn transmit(&mut self, mem: &mut dyn ChipRamMut) {
        if self.con & CON_TX == 0 {
            return;
        }
        while self.tx_head != self.tx_tail {
            let d = lin(self.txr).wrapping_add(8 * u32::from(self.tx_head));
            if d + 8 > CHIP_END {
                break;
            }
            let flags = Self::word(mem, d + 6);
            if flags & DESC_OWN == 0 {
                break;
            }
            let buf = u32::from(Self::word(mem, d)) << 16 | u32::from(Self::word(mem, d + 2));
            let len = usize::from(Self::word(mem, d + 4));
            let ok = (MIN_FRAME..=MAX_FRAME).contains(&len) && buf + len as u32 <= CHIP_END;
            if ok {
                let mut frame: Vec<u8> = (0..len as u32).map(|i| mem.get(buf + i)).collect();
                let fcs = crc32(&frame);
                frame.extend_from_slice(&fcs.to_le_bytes());
                if self.link {
                    self.outbox.push(frame);
                }
                self.pending_slots += len.div_ceil(4) as u32;
            }
            Self::set_word(mem, d + 6, if ok { 0 } else { DESC_ERROR });
            self.tx_head = (self.tx_head + 1) & 15;
            self.irq |= IRQ_TX;
        }
    }

    /// A frame (with its FCS) arrives from the wire.
    pub fn receive(&mut self, data: &[u8], mem: &mut dyn ChipRamMut) {
        if self.con & CON_RX == 0 || data.len() < MIN_FRAME + 4 {
            return;
        }
        let (frame, fcs) = data.split_at(data.len() - 4);
        let mine = frame[..6] == self.mac();
        let group = frame[0] & 1 != 0;
        if !(mine || group || self.con & CON_PROMISC != 0) {
            return;
        }
        let d = lin(self.rxr).wrapping_add(8 * u32::from(self.rx_head));
        if d + 8 > CHIP_END || Self::word(mem, d + 6) & DESC_OWN == 0 {
            self.irq |= IRQ_FULL;
            return;
        }
        let buf = u32::from(Self::word(mem, d)) << 16 | u32::from(Self::word(mem, d + 2));
        let room = usize::from(Self::word(mem, d + 4)).min(MAX_FRAME);
        let n = frame.len().min(room);
        for (i, b) in frame[..n].iter().enumerate() {
            let a = buf.wrapping_add(i as u32);
            if a < CHIP_END {
                mem.set(a, *b);
            }
        }
        let mut flags = 0;
        if n < frame.len() || frame.len() > MAX_FRAME {
            flags |= DESC_ERROR;
        }
        if u32::from_le_bytes(fcs.try_into().unwrap()) != crc32(frame) {
            flags |= DESC_CRC;
        }
        Self::set_word(mem, d + 4, n as u16);
        Self::set_word(mem, d + 6, flags);
        self.rx_head = (self.rx_head + 1) & 15;
        self.pending_slots += n.div_ceil(4) as u32;
        self.irq |= IRQ_RX;
    }

    /// The tunnel came up or went down.
    pub fn set_link(&mut self, up: bool) {
        if up != self.link {
            self.link = up;
            self.irq |= IRQ_LINK;
        }
    }

    pub fn link(&self) -> bool {
        self.link
    }

    /// Slot 640: the slots this line gives the network (after the SD
    /// card's `sd` slots).
    pub fn slots(&mut self, sd: u16) -> u16 {
        let free = u32::from(SLOTS_PER_LINE.saturating_sub(sd));
        let n = self.pending_slots.min(free);
        self.pending_slots -= n;
        n as u16
    }

    pub fn save(&self, w: &mut StateWriter) {
        for v in [
            self.con,
            self.mac[0],
            self.mac[1],
            self.mac[2],
            self.txr.0,
            self.txr.1,
            self.rxr.0,
            self.rxr.1,
            self.tx_tail,
            self.tx_head,
            self.rx_head,
            self.rx_tail,
            self.irq,
            self.irq_en,
        ] {
            w.u16(v);
        }
        w.bool(self.link);
        w.u32(self.pending_slots);
    }

    /// The outbox is not state (it is emptied every line).
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut n = Net {
            con: r.u16()? & 7,
            ..Net::default()
        };
        for k in 0..3 {
            n.mac[k] = r.u16()?;
        }
        n.txr = (r.u16()?, r.u16()?);
        n.rxr = (r.u16()?, r.u16()?);
        n.tx_tail = r.u16()? & 15;
        n.tx_head = r.u16()? & 15;
        n.rx_head = r.u16()? & 15;
        n.rx_tail = r.u16()? & 15;
        n.irq = r.u16()? & 15;
        n.irq_en = r.u16()? & 15;
        n.link = r.bool()?;
        n.pending_slots = r.u32()?.min(1 << 20);
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ram(Vec<u8>);
    impl ChipRamMut for Ram {
        fn get(&self, a: u32) -> u8 {
            self.0[a as usize]
        }
        fn set(&mut self, a: u32, v: u8) {
            self.0[a as usize] = v;
        }
    }

    const MAC: [u8; 6] = [2, 0x4c, 0x31, 0x36, 0, 1];

    fn desc(ram: &mut Ram, at: usize, buf: u32, len: u16, flags: u16) {
        let w = [(buf >> 16) as u16, buf as u16, len, flags];
        for (k, v) in w.iter().enumerate() {
            ram.0[at + 2 * k..at + 2 * k + 2].copy_from_slice(&v.to_le_bytes());
        }
    }

    fn setup() -> (Net, Ram) {
        let mut ram = Ram(vec![0; 0x2_0000]);
        let mut n = Net::new(MAC);
        n.write(TXRSEG, 1, &mut ram);
        n.write(TXROFS, 0, &mut ram);
        n.write(RXRSEG, 1, &mut ram);
        n.write(RXROFS, 0x80, &mut ram);
        n.write(NETCON, CON_RX | CON_TX, &mut ram);
        n.write(NETIRQEN, 15, &mut ram);
        n.set_link(true);
        n.write(NETIRQ, 15, &mut ram);
        (n, ram)
    }

    #[test]
    fn the_crc_is_ieee_802_3() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn transmit_takes_owned_descriptors_up_to_the_tail() {
        let (mut n, mut ram) = setup();
        ram.0[0x1_1000..0x1_1000 + 20]
            .copy_from_slice(b"\xff\xff\xff\xff\xff\xff\x02\x4c\x31\x36\x00\x01\x4c\x31hello!");
        desc(&mut ram, 0x1_0000, 0x1_1000, 20, DESC_OWN);
        desc(&mut ram, 0x1_0008, 0x1_1000, 5, DESC_OWN); // too short
        desc(&mut ram, 0x1_0010, 0x1_1000, 20, 0); // the CPU's still
        n.write(TXTAIL, 3, &mut ram);
        assert_eq!(n.peek(TXHEAD), 2, "stops at a descriptor the CPU owns");
        assert_eq!(n.outbox.len(), 1);
        let f = &n.outbox[0];
        assert_eq!(f.len(), 24);
        assert_eq!(
            u32::from_le_bytes(f[20..].try_into().unwrap()),
            crc32(&f[..20])
        );
        assert_eq!(
            u16::from_le_bytes([ram.0[0x1_0006], ram.0[0x1_0007]]),
            0,
            "given back"
        );
        assert_eq!(
            u16::from_le_bytes([ram.0[0x1_000e], ram.0[0x1_000f]]),
            DESC_ERROR
        );
        assert_eq!(n.peek(NETIRQ), IRQ_TX);
        assert!(n.irq());
        assert_eq!(
            (n.slots(0), n.slots(6), n.slots(0)),
            (5, 0, 0),
            "20 bytes: 5 slots"
        );
    }

    #[test]
    fn receive_filters_checks_and_fills_the_ring() {
        let (mut n, mut ram) = setup();
        for k in 0..2u32 {
            desc(
                &mut ram,
                0x1_0080 + 8 * k as usize,
                0x1_2000 + 0x800 * k,
                1536,
                DESC_OWN,
            );
        }
        let frame = |dst: [u8; 6], body: &[u8], good: bool| {
            let mut f = dst.to_vec();
            f.extend_from_slice(&[2, 0, 0, 0, 0, 9, 0x4c, 0x31]);
            f.extend_from_slice(body);
            let fcs = crc32(&f) ^ u32::from(!good);
            f.extend_from_slice(&fcs.to_le_bytes());
            f
        };
        n.receive(&frame([2, 0, 0, 0, 0, 7], b"not mine", true), &mut ram);
        assert_eq!(n.peek(RXHEAD), 0, "filtered");
        n.receive(&frame(MAC, b"for me", true), &mut ram);
        n.receive(&frame([0xff; 6], b"to all", false), &mut ram);
        assert_eq!(n.peek(RXHEAD), 2);
        assert_eq!(&ram.0[0x1_2000 + 14..0x1_2000 + 20], b"for me");
        let w = |a: usize| u16::from_le_bytes([ram.0[a], ram.0[a + 1]]);
        assert_eq!((w(0x1_0084), w(0x1_0086)), (20, 0));
        assert_eq!(w(0x1_008e), DESC_CRC, "delivered with the CRC error");
        n.receive(&frame([0xff; 6], b"no room", true), &mut ram);
        assert_eq!(n.peek(NETIRQ), IRQ_RX | IRQ_FULL);
        n.write(NETIRQ, IRQ_RX, &mut ram);
        assert_eq!(n.peek(NETIRQ), IRQ_FULL, "write 1 clears");
        // Promiscuous takes everything; off, nothing comes in.
        let (mut n, mut ram) = setup();
        desc(&mut ram, 0x1_0080, 0x1_2000, 1536, DESC_OWN);
        n.write(NETCON, CON_RX | CON_PROMISC, &mut ram);
        n.receive(&frame([2, 0, 0, 0, 0, 7], b"x", true), &mut ram);
        assert_eq!(n.peek(RXHEAD), 1);
        n.write(NETCON, 0, &mut ram);
        n.receive(&frame([0xff; 6], b"x", true), &mut ram);
        assert_eq!(n.peek(RXHEAD), 1);
        let mut w = StateWriter::new();
        n.save(&mut w);
        let bytes = w.into_bytes();
        let mut m = Net::default();
        m.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(n, m);
    }
}
