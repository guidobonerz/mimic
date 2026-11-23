//! The small C64-side blocks that are present and idle: the sampler, the
//! ACIA 6551, the cartridge EEPROM and the two halves of the tape
//! interface. Each is its own struct so a working model can replace it.
//!
//! Source: UE2-C64U-Emulator `docs/hw/11-drives-iec-periph.md` (address
//! maps "ACIA 6551" and "C2N tape", hazards H8, H15, H16, the T0 rows),
//! `docs/hw/12-gaps.md` (region A, sampler) and `docs/hw/10-c64-machine.md`
//! (EEPROM rows); the ACIA's power-on values were checked in the
//! firmware's `fpga/io/acia/vhdl_source/acia6551.vhd`.

use mimic_core::{StateError, StateReader, StateWriter};

/// The sampler, `$048000`-`$049FFF`: 256 bytes of registers repeated over
/// the window, eight voices of 32 bytes each.
///
/// Per voice (offset in the voice): `$00` CONTROL (bit 0 on, bit 1
/// repeat, bit 2 interrupt at the end, bits 5:4 16-bit, bit 6
/// interleaved), `$01` VOLUME (6 bits), `$02` PAN (4 bits), then start
/// address, length, rate (`$0E`/`$0F`, big-endian) and the repeat points;
/// `$1F` W acknowledges the voice's interrupt. Reads do not return the
/// registers: an even address reads the voices' interrupt flags (0 here),
/// an odd one the version `$10`.
///
/// The firmware only writes: CONTROL = 0 for all voices after every C64
/// reset. The sampler's interrupt goes to the C64, not to the board's
/// processor.
///
/// To make it work later: the voices play from board memory (default
/// start `$01000000`, the REU's memory), the C64 reaches the registers of
/// voices 0-6 at `$DF20`-`$DFFF` while C64_SAMPLER_ENABLE is set, and the
/// mix feeds the audio mixer's channels 4 and 5.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sampler {
    regs: [u8; 0x100],
}

/// What an odd address of the sampler reads.
pub const SAMPLER_VERSION: u8 = 0x10;

impl Default for Sampler {
    fn default() -> Self {
        Self { regs: [0; 0x100] }
    }
}

impl Sampler {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn read(&self, off: u32) -> u8 {
        if off & 1 != 0 {
            SAMPLER_VERSION
        } else {
            0
        }
    }

    pub fn write(&mut self, off: u32, v: u8) {
        self.regs[(off & 0xff) as usize] = v;
    }

    /// The registers as written.
    pub fn regs(&self) -> &[u8; 0x100] {
        &self.regs
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.raw(&self.regs);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        r.raw_into(&mut self.regs)
    }
}

/// The firmware's side of the ACIA 6551 (the modem), `$04A000`-`$04ABFF`.
///
/// | Addr | Name | Bits |
/// |---|---|---|
/// | `$04A000` | rx_head | RW: where the firmware has written to in the ring towards the C64 |
/// | `$04A001` | rx_tail | R: how far the C64 has taken it |
/// | `$04A002` | tx_head | R: where the C64 has written to in its ring |
/// | `$04A003` | tx_tail | RW: how far the firmware has taken it (also acknowledges) |
/// | `$04A004`, `$04A005` | control, command | R: the 6551 registers the C64 wrote (command is `$02` after a reset) |
/// | `$04A006` | status | R: the 6551 status: bit 4 transmitter empty, bit 5 DCD and bit 6 DSR inactive |
/// | `$04A007` | enable | bit 0 visible to the C64, bits 1-4 interrupt enables: receive, transmit, control change, DTR change |
/// | `$04A008` | handsh | bit 0 CTS, bit 2 DSR, bit 4 DCD, bit 5 RTS off, bit 6 receive push-back (power-on set); R also bit 1 RTS, bit 3 DTR |
/// | `$04A009` | irq_source | R: what asks; W bits 3, 4 acknowledge. 0 here |
/// | `$04A00A` | slot_base | W: bit 0 turbo, bits 6:1 C64 I/O offset, bit 7 NMI instead of IRQ |
/// | `$04A800`-`$04A9FF` | C64 → firmware ring | 256 bytes, twice |
/// | `$04AA00`-`$04ABFF` | firmware → C64 ring | 256 bytes, twice |
///
/// Interrupt: the ITU's high source 0, never raised here.
///
/// To make it work later: the C64 needs the five 6551 registers at the
/// selected I/O address, bytes move between the rings at the baud rate,
/// and the interrupt follows the enables.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Acia {
    pub rx_head: u8,
    pub tx_tail: u8,
    /// 5 bits.
    pub enable: u8,
    /// The writable handshake bits (0, 2, 4, 5, 6).
    pub handsh: u8,
    pub slot_base: u8,
    tx_ring: [u8; 0x100],
    rx_ring: [u8; 0x100],
}

const HANDSH_MASK: u8 = 0x75;

impl Default for Acia {
    fn default() -> Self {
        Self {
            rx_head: 0,
            tx_tail: 0,
            enable: 0,
            handsh: 0x40,
            slot_base: 0x80,
            tx_ring: [0; 0x100],
            rx_ring: [0; 0x100],
        }
    }
}

impl Acia {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The C64 was reset: the ring positions start over.
    pub fn c64_reset(&mut self) {
        self.rx_head = 0;
        self.tx_tail = 0;
    }

    /// `off` counts from `$04A000`.
    pub fn read(&self, off: u32) -> u8 {
        let off = off & 0xfff;
        match off >> 9 {
            0..=3 => match off & 0xf {
                0x0 => self.rx_head,
                0x3 => self.tx_tail,
                0x5 => 0x02,
                // Transmitter empty; DSR and DCD are active low and
                // follow the handshake register.
                0x6 => {
                    0x10 | (if self.handsh & 0x04 == 0 { 0x40 } else { 0 })
                        | (if self.handsh & 0x10 == 0 { 0x20 } else { 0 })
                }
                0x7 => self.enable,
                0x8 => self.handsh,
                _ => 0,
            },
            4 => self.tx_ring[(off & 0xff) as usize],
            5 => self.rx_ring[(off & 0xff) as usize],
            _ => 0,
        }
    }

    pub fn write(&mut self, off: u32, v: u8) {
        let off = off & 0xfff;
        match off >> 9 {
            0..=3 => match off & 0xf {
                0x0 => self.rx_head = v,
                0x3 => self.tx_tail = v,
                0x7 => self.enable = v & 0x1f,
                0x8 => self.handsh = v & HANDSH_MASK,
                0xa => self.slot_base = v,
                _ => {}
            },
            5 => self.rx_ring[(off & 0xff) as usize] = v,
            // The C64's ring is written by the C64.
            _ => {}
        }
    }

    /// The ring towards the C64 as the firmware filled it.
    pub fn rx_ring(&self) -> &[u8; 0x100] {
        &self.rx_ring
    }

    pub fn save(&self, w: &mut StateWriter) {
        for v in [
            self.rx_head,
            self.tx_tail,
            self.enable,
            self.handsh,
            self.slot_base,
        ] {
            w.u8(v);
        }
        w.raw(&self.tx_ring);
        w.raw(&self.rx_ring);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        self.rx_head = r.u8()?;
        self.tx_tail = r.u8()?;
        self.enable = r.u8()? & 0x1f;
        self.handsh = r.u8()? & HANDSH_MASK;
        self.slot_base = r.u8()?;
        r.raw_into(&mut self.tx_ring)?;
        r.raw_into(&mut self.rx_ring)
    }
}

/// The serial EEPROM of a GMOD2 cartridge, `$04C000`-`$04CFFF`.
///
/// `$04C000`: R not 0 = the C64 has changed the contents; W 1 clears
/// that. `$04C800`-`$04CFFF`: the 2048 bytes. No cartridge writes here, so
/// "changed" reads 0; the bytes keep what the firmware loads.
///
/// To make it work later: the cartridge logic needs the 93C86's serial
/// protocol at `$DE00` on these bytes and sets "changed".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Eeprom {
    data: Vec<u8>,
}

impl Default for Eeprom {
    fn default() -> Self {
        Self {
            data: vec![0; 0x800],
        }
    }
}

impl Eeprom {
    pub fn reset(&mut self) {
        self.data.fill(0);
    }

    /// `off` counts from `$04C000`.
    pub fn read(&self, off: u32) -> u8 {
        let off = off & 0xfff;
        if off >= 0x800 {
            self.data[(off & 0x7ff) as usize]
        } else {
            0
        }
    }

    pub fn write(&mut self, off: u32, v: u8) {
        let off = off & 0xfff;
        if off >= 0x800 {
            self.data[(off & 0x7ff) as usize] = v;
        }
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.raw(&self.data);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        r.raw_into(&mut self.data)
    }
}

/// Tape playback, `$0A0000`-`$0A0FFF`: the firmware feeds pulse lengths
/// into a queue and the FPGA plays them to the C64's cassette port.
///
/// `$0A0000`-`$0A07FF` W control: bit 0 on, bit 1 clear error, bit 2 empty
/// the queue, bit 3 long pulses as three bytes, bit 4 the sense line,
/// bit 5 NTSC rate, bits 7:6 which line is driven. `$0A0800`-`$0A0FFF` W:
/// a byte into the queue (2048 deep). Every read in the window is the
/// status: bit 0 on, bit 1 ran empty, bit 2 full, bit 3 nearly full,
/// bit 6 playing, bit 7 queue empty. **`$80` here**, always.
///
/// To make it work later: the queue, the pulse generator clocked by the
/// C64 while its cassette motor runs, and the sense and read lines of the
/// C64's cassette port.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TapePlayback {
    pub control: u8,
}

/// The status of an empty queue.
pub const TAPE_PLAY_IDLE: u8 = 0x80;

impl TapePlayback {
    pub fn reset(&mut self) {
        self.control = 0;
    }

    pub fn read(&self, _off: u32) -> u8 {
        TAPE_PLAY_IDLE
    }

    pub fn write(&mut self, off: u32, v: u8) {
        if off & 0x800 == 0 {
            self.control = v;
        }
    }
}

/// Tape recording, `$0C0000`-`$0C0FFF`: the FPGA times the edges on the
/// cassette port and queues them for the firmware.
///
/// `$0C0000`-`$0C07FF` W control: bit 0 on, bit 1 clear error, bit 2 empty
/// the queue, bits 5:4 which edge, bit 6 which line, bit 7 interrupt
/// enable; R status: bit 0 on, bit 1 overflow, bit 3 a block of 512 bytes
/// waits, bit 7 a byte waits. `$0C0800`-`$0C0FFF` R: a byte from the
/// queue. Status and data read 0 here.
///
/// Interrupt: the ITU's low source 3, a level while the interrupt is
/// enabled and 512 bytes wait. Never raised here.
///
/// To make it work later: the edge timer on the C64's cassette write (or
/// read) line, the queue and the level on source 3.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TapeRecord {
    pub control: u8,
}

impl TapeRecord {
    pub fn reset(&mut self) {
        self.control = 0;
    }

    pub fn read(&self, _off: u32) -> u8 {
        0
    }

    pub fn write(&mut self, off: u32, v: u8) {
        if off & 0x800 == 0 {
            self.control = v;
        }
    }
}
