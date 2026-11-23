//! The C64 side of the FPGA as the board's firmware sees it: everything on
//! the I/O bus that belongs to the C64, its drives and its ports, behind
//! one device, [`C64Port`].
//!
//! The C64 itself is not here. It is the machine's own C64, reached only
//! through [`C64Side`]: the port turns register writes into its reset,
//! stop, memory-map, NMI and keyboard lines, passes every byte of the DMA
//! window to its bus and the ROM windows to its ROMs.
//!
//! | Range | Block | Here |
//! |---|---|---|
//! | `$020000`-`$021FFF` | drive A with its WD177x | the 1541's lines, track table, DIRTY bits and head status work, the machine's 1541 behind them; the WD177x idles ([`drive`]) |
//! | `$024000`-`$025FFF` | drive B | the same |
//! | `$028000`-`$028FFF` | IEC processor | idle ([`iec`]) |
//! | `$040000`-`$041FFF` | cartridge / machine control | works ([`cartregs`]); the cartridge logic itself: normal 8K/16K/ULTIMAX ([`carts`]) |
//! | `$042000`-`$043FFF` | SID control of older boards | reads 0, ignores writes (the firmware has no user) |
//! | `$044000`-`$044FFF` | command interface (UCI) | idle ([`uci`]) |
//! | `$046000`-`$047FFF` | bus timing samples | reads 0 (a developer's tool) |
//! | `$048000`-`$049FFF` | sampler | idle ([`periph::Sampler`]) |
//! | `$04A000`-`$04ABFF` | ACIA 6551 | idle ([`periph::Acia`]) |
//! | `$04C000`-`$04CFFF` | cartridge EEPROM | memory ([`periph::Eeprom`]) |
//! | `$050000`-`$05FFFF` | DMA window: the C64's bus | works |
//! | `$0A0000`-`$0A0FFF` | tape playback | idle ([`periph::TapePlayback`]) |
//! | `$0C0000`-`$0C0FFF` | tape recording | idle ([`periph::TapeRecord`]) |
//! | `$100300`-`$1003FF` | keyboard matrix into the C64 | works |
//! | `$180000`-`$1800FF` | C64 core configuration | works ([`coreconfig`]) |
//! | `$180800`-`$18083F`, `$180C00`-`$180C3F` | C64 palette, RGB and YUV | kept |
//! | `$181000`-`$18101F` | PLD access | memory |
//! | `$181800` | debug register | memory |
//! | `$185000`-`$185FFF` | UltiSID filter curves | kept |
//! | `$188000`, `$18A000`, `$18C000` | BASIC, KERNAL, character ROM | works |
//! | `$190000`-`$1900FF` | UDP stream header templates | kept |
//!
//! ## The DMA window
//!
//! A byte access at `$050000 + a` is one access at address `a` on the
//! C64's bus, through its memory configuration, with the side effects a
//! read or write has there. With bit 0 of C64_DMA_MEMONLY (`$180003`) set
//! it reaches the RAM alone. The firmware stops the 6510 first when the
//! access must not disturb a running program, but not always: it pokes the
//! CIA while the C64 runs when it starts, so the window works in either
//! state.
//!
//! ## Reset, stop, NMI
//!
//! C64_MODE (`$040000`) pulls and releases the C64's reset line. Pulling
//! it is the "C64 reset" event of the ITU (low source 7, an edge): the
//! open top level wires the reset line to that input, and the firmware
//! runs its reset task on it (settings applied again, sampler voices
//! off). A reset the C64 makes itself is the same event; the machine
//! reports it with [`C64Port::c64_reset_event`].
//!
//! C64_STOP (`$040001`) holds the 6510 while VIC, CIAs and SID run on. The
//! firmware waits for the acknowledge bit in loops without a timeout, so
//! with the stop condition "at once" the port acknowledges at once;
//! otherwise the acknowledge is [`C64Side::stopped`].
//!
//! The NMI line is C64_MODE bit 4 or the RESTORE byte of the keyboard
//! matrix.
//!
//! ## The keyboard matrix
//!
//! `$100300`-`$100307`: per column a byte with a set bit for every pressed
//! key (index column * 8 + row as in the C64's matrix, column = the line
//! CIA 1 drives). `$100308` reset keys held, `$100309` RESTORE, `$10030A`
//! freeze button. `$10030B`-`$10030E`: a 32-bit latch, "W, A, S, D act as
//! joystick" - written as one word at this unaligned address. All of it
//! reads back.
//!
//! ## Interrupts
//!
//! Of the sources that belong here only two are ever raised: low 7 on a
//! C64 reset, and high 6 ("unlock") when the machine reports that the C64
//! ran the unlock sequence ([`C64Port::raise_unlock`]); the firmware's
//! handler answers that with a DMA write of 0 to `$D038`, which drops the
//! line again (the ITU has no acknowledge for high sources). Low 3 (tape),
//! low 4 (UCI), high 0 (ACIA) and high 1, 2 (the drives' WD177x; the 1541
//! needs no interrupt) belong to the idle blocks and stay low.
//!
//! Sources: UE2-C64U-Emulator `docs/hw/00-memory-map.md` (map, boot
//! hazards C3, C12, C14-C18, C22, C26-C31, interrupts),
//! `docs/hw/10-c64-machine.md`, `docs/hw/11-drives-iec-periph.md`,
//! `docs/hw/05-ui-overlay-input.md` (MATRIX_KEYB), `docs/hw/12-gaps.md`,
//! `docs/specs/S14-c64-trx64.md` and `S15-uci.md` (how the registers map
//! onto a real C64); the firmware's `software/io/c64/c64.h`,
//! `software/system/u64.h`, `software/u64/u64_config.cc`,
//! `software/u64/color_timings.cc`,
//! `fpga/cart_slot/vhdl_source/cart_slot_registers.vhd`,
//! `fpga/io/itu/vhdl_source/itu.vhd` for the details named in the
//! submodules.

pub mod cartregs;
pub mod carts;
pub mod coreconfig;
pub mod drive;
pub mod iec;
pub mod periph;
pub mod uci;

pub use cartregs::CartRegs;
pub use carts::{CartLogic, CartWindow};
pub use coreconfig::CoreConfig;
pub use drive::Drive;
pub use iec::IecProcessor;
pub use periph::{Acia, Eeprom, Sampler, TapePlayback, TapeRecord};
pub use uci::Uci;

use crate::io::{C64Rom, C64Side, IoCtx, IoDevice};
use crate::irq::Irq;
use mimic_core::{StateError, StateReader, StateWriter};

/// The ranges of the I/O bus the port answers, first and last address.
pub const CLAIMS: [(u32, u32); 9] = [
    (0x02_0000, 0x02_1fff),
    (0x02_4000, 0x02_5fff),
    (0x02_8000, 0x02_8fff),
    (0x04_0000, 0x04_dfff),
    (0x05_0000, 0x05_ffff),
    (0x0a_0000, 0x0a_0fff),
    (0x0c_0000, 0x0c_0fff),
    (0x10_0300, 0x10_03ff),
    (0x18_0000, 0x19_00ff),
];

const DRIVE_A: u32 = 0x02_0000;
const DRIVE_B: u32 = 0x02_4000;
const IEC: u32 = 0x02_8000;
const CART: u32 = 0x04_0000;
const UCI: u32 = 0x04_4000;
const SAMPLER: u32 = 0x04_8000;
const ACIA: u32 = 0x04_a000;
const EEPROM: u32 = 0x04_c000;
const DMA: u32 = 0x05_0000;
const TAPE_PLAY: u32 = 0x0a_0000;
const TAPE_REC: u32 = 0x0c_0000;
const MATRIX: u32 = 0x10_0300;
const CORE: u32 = 0x18_0000;
const PALETTE_RGB: u32 = 0x18_0800;
const PALETTE_YUV: u32 = 0x18_0c00;
const PLD: u32 = 0x18_1000;
const DEBUG: u32 = 0x18_1800;
const SID_FILTER: u32 = 0x18_5000;
const ROM_BASIC: u32 = 0x18_8000;
const ROM_KERNAL: u32 = 0x18_a000;
const ROM_CHAR: u32 = 0x18_c000;
const UDP: u32 = 0x19_0000;

/// Matrix bytes: reset keys, RESTORE, freeze button, the WASD latch.
const MATRIX_RESET: usize = 8;
const MATRIX_RESTORE: usize = 9;
const MATRIX_FREEZE: usize = 10;
const MATRIX_WASD: usize = 11;

/// The ITU's low source of the C64 reset event.
pub const IRQ_C64_RESET: u8 = 7;
/// The ITU's high source of the unlock event.
pub const IRQ_HIGH_UNLOCK: u8 = 6;

/// The C64 address whose DMA write acknowledges the unlock interrupt.
const UNLOCK_ACK: u16 = 0xd038;

/// The ROM a window address lies in, and the offset.
fn rom_at(addr: u32) -> Option<(C64Rom, u16)> {
    match addr {
        ROM_BASIC..=0x18_9fff => Some((C64Rom::Basic, (addr - ROM_BASIC) as u16)),
        ROM_KERNAL..=0x18_bfff => Some((C64Rom::Kernal, (addr - ROM_KERNAL) as u16)),
        ROM_CHAR..=0x18_cfff => Some((C64Rom::Char, (addr - ROM_CHAR) as u16)),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct C64Port {
    pub cart: CartRegs,
    /// The cartridge logic behind the registers.
    pub carts: CartLogic,
    pub core: CoreConfig,
    pub drive_a: Drive,
    pub drive_b: Drive,
    pub iec: IecProcessor,
    pub uci: Uci,
    pub sampler: Sampler,
    pub acia: Acia,
    pub eeprom: Eeprom,
    pub tape_play: TapePlayback,
    pub tape_rec: TapeRecord,
    matrix: [u8; 0x100],
    palette_rgb: [u8; 0x40],
    palette_yuv: [u8; 0x40],
    pld: [u8; 0x20],
    debug: u8,
    sid_filter: Vec<u8>,
    udp: [u8; 0x100],
    /// The unlock interrupt is up and waits for its acknowledge.
    unlock: bool,
}

impl Default for C64Port {
    fn default() -> Self {
        Self::new()
    }
}

impl C64Port {
    /// The power-on state. The C64 is held in reset; tell it with
    /// [`C64Port::apply_lines`].
    pub fn new() -> Self {
        Self {
            cart: CartRegs::new(),
            carts: CartLogic::new(),
            core: CoreConfig::new(),
            drive_a: Drive::new(true),
            drive_b: Drive::new(true),
            iec: IecProcessor::new(),
            uci: Uci::new(),
            sampler: Sampler::default(),
            acia: Acia::default(),
            eeprom: Eeprom::default(),
            tape_play: TapePlayback::default(),
            tape_rec: TapeRecord::default(),
            matrix: [0; 0x100],
            palette_rgb: [0; 0x40],
            palette_yuv: [0; 0x40],
            pld: [0; 0x20],
            debug: 0,
            sid_filter: vec![0; 0x1000],
            udp: [0; 0x100],
            unlock: false,
        }
    }

    /// The port answers this address of the I/O bus.
    pub fn claims(addr: u32) -> bool {
        let addr = addr & 0xff_ffff;
        CLAIMS.iter().any(|&(lo, hi)| (lo..=hi).contains(&addr))
    }

    /// Bring the C64's lines in step with the registers: after power-on,
    /// [`IoDevice::reset`] and [`IoDevice::load`], which have no C64 at
    /// hand.
    pub fn apply_lines(&self, c64: &mut dyn C64Side) {
        c64.set_ntsc(self.core.ntsc());
        c64.set_ultimax(self.cart.ultimax);
        self.push_cart_lines(c64);
        c64.set_nmi(self.nmi_line());
        for (column, rows) in self.matrix[..8].iter().enumerate() {
            c64.set_matrix(column as u8, !rows);
        }
        c64.set_stop_condition(self.cart.stop_mode);
        c64.set_stopped(self.cart.stop);
        c64.set_reset(self.cart.reset);
    }

    /// The C64 was reset by something other than C64_MODE (its reset
    /// button, the machine): the ITU's reset event.
    pub fn c64_reset_event(&mut self, irq: &mut Irq) {
        irq.pulse(IRQ_C64_RESET);
        self.acia.c64_reset();
        // The reset line latches the cartridge and switches it on.
        self.carts.latch(self.cart.cartridge_type, true);
    }

    /// The lines the board drives into the expansion port, `true` =
    /// pulled low: (/GAME, /EXROM). ULTIMAX of C64_MODE wins over the
    /// cartridge.
    pub fn cart_lines(&self) -> (bool, bool) {
        if self.cart.ultimax {
            (true, false)
        } else {
            self.carts.lines()
        }
    }

    fn push_cart_lines(&self, c64: &mut dyn C64Side) {
        let (game, exrom) = self.cart_lines();
        c64.set_cart_lines(game, exrom);
    }

    /// The C64 ran the unlock sequence of the command interface: the
    /// ITU's high source 6 goes up until the firmware's handler has
    /// written `$D038` through the DMA window.
    pub fn raise_unlock(&mut self, irq: &mut Irq) {
        self.unlock = true;
        irq.set_high(IRQ_HIGH_UNLOCK, true);
    }

    /// The NMI line the board drives.
    pub fn nmi_line(&self) -> bool {
        self.cart.nmi || self.matrix[MATRIX_RESTORE] != 0
    }

    /// The matrix as the firmware set it: per column the pressed rows as
    /// set bits.
    pub fn matrix(&self) -> &[u8] {
        &self.matrix[..8]
    }

    /// The board's keyboard path holds the reset key combination.
    pub fn matrix_reset(&self) -> bool {
        self.matrix[MATRIX_RESET] != 0
    }

    /// The freeze button of the board's keyboard path.
    pub fn matrix_freeze(&self) -> bool {
        self.matrix[MATRIX_FREEZE] != 0
    }

    /// The "W, A, S, D as joystick" latch.
    pub fn wasd_to_joy(&self) -> u32 {
        let m = &self.matrix;
        u32::from_le_bytes([
            m[MATRIX_WASD],
            m[MATRIX_WASD + 1],
            m[MATRIX_WASD + 2],
            m[MATRIX_WASD + 3],
        ])
    }

    /// The C64's sixteen colours as the firmware set them: four bytes
    /// each, red, green, blue, unused.
    pub fn palette_rgb(&self) -> &[u8; 0x40] {
        &self.palette_rgb
    }

    /// The same as Y, U, V (U and V signed), unused.
    pub fn palette_yuv(&self) -> &[u8; 0x40] {
        &self.palette_yuv
    }

    /// The filter curve of UltiSID 1 or 2 (index 0 or 1): 1024 values of
    /// 16 bits, little-endian.
    pub fn sid_filter_curve(&self, sid: usize) -> &[u8] {
        let o = (sid & 1) * 0x800;
        &self.sid_filter[o..o + 0x800]
    }

    /// The header template of UDP stream 0-3 (VIC, audio, debug, IEC):
    /// Ethernet, IPv4 and UDP header in the first 42 bytes.
    pub fn udp_header(&self, stream: usize) -> &[u8] {
        let o = (stream & 3) * 0x40;
        &self.udp[o..o + 0x40]
    }

    /// The acknowledge bit of C64_STOP.
    fn has_stopped(&self, c64: Option<&dyn C64Side>) -> bool {
        self.cart.stop
            && (self.cart.stop_mode == cartregs::STOP_FORCE
                || match c64 {
                    Some(c) => c.stopped(),
                    None => true,
                })
    }

    /// What a read returns, without side effects. With the C64 at hand
    /// the DMA window, the ROM windows and the lines' state are the real
    /// ones; without it they read 0 and the stop is taken as granted.
    pub fn peek_with(&self, addr: u32, c64: Option<&dyn C64Side>) -> u8 {
        let addr = addr & 0xff_ffff;
        match addr {
            DRIVE_A..=0x02_1fff => self.drive_a.read(addr - DRIVE_A),
            DRIVE_B..=0x02_5fff => self.drive_b.read(addr - DRIVE_B),
            IEC..=0x02_8fff => self.iec.read(addr - IEC),
            CART..=0x04_1fff => self
                .cart
                .read(addr, self.has_stopped(c64), self.carts.active()),
            UCI..=0x04_4fff => self.uci.read(addr - UCI),
            SAMPLER..=0x04_9fff => self.sampler.read(addr - SAMPLER),
            ACIA..=0x04_afff => self.acia.read(addr - ACIA),
            EEPROM..=0x04_cfff => self.eeprom.read(addr - EEPROM),
            DMA..=0x05_ffff => {
                let a = addr as u16;
                match c64 {
                    Some(c) if self.core.dma_memonly() => c.dma_peek_ram(a),
                    Some(c) => c.dma_peek(a),
                    None => 0,
                }
            }
            TAPE_PLAY..=0x0a_0fff => self.tape_play.read(addr - TAPE_PLAY),
            TAPE_REC..=0x0c_0fff => self.tape_rec.read(addr - TAPE_REC),
            MATRIX..=0x10_03ff => self.matrix[(addr - MATRIX) as usize],
            CORE..=0x18_00ff => self.core.read(addr - CORE),
            PLD..=0x18_101f => self.pld[(addr - PLD) as usize],
            DEBUG => self.debug,
            ROM_BASIC..=0x18_cfff => match (rom_at(addr), c64) {
                (Some((rom, off)), Some(c)) => c.rom_read(rom, off),
                _ => 0,
            },
            // The SID control of older boards, the timing samples, the
            // palettes, filter curves and UDP templates (write only) and
            // everything between the blocks.
            _ => 0,
        }
    }

    fn write_mode(&mut self, v: u8, ctx: &mut IoCtx<'_>) {
        if v & cartregs::MODE_RESET != 0 {
            if !self.cart.reset {
                self.cart.reset = true;
                ctx.c64.set_reset(true);
                self.c64_reset_event(ctx.irq);
                self.push_cart_lines(ctx.c64);
            }
        } else if v & cartregs::MODE_UNRESET != 0 {
            if self.cart.reset {
                self.cart.reset = false;
                ctx.c64.set_reset(false);
            }
        } else {
            let ultimax = v & cartregs::MODE_ULTIMAX != 0;
            if ultimax != self.cart.ultimax {
                self.cart.ultimax = ultimax;
                ctx.c64.set_ultimax(ultimax);
                // At once: the DMA right after it and the 6510's next
                // cycle see the new map (leaving ULTIMAX late let the 6510
                // fetch from the open bus at $E000-$FFFF).
                self.push_cart_lines(ctx.c64);
            }
            self.cart.nmi = v & cartregs::MODE_NMI != 0;
            ctx.c64.set_nmi(self.nmi_line());
        }
    }

    fn write_cart(&mut self, addr: u32, v: u8, ctx: &mut IoCtx<'_>) {
        match addr & 0xf {
            cartregs::MODE => self.write_mode(v, ctx),
            cartregs::STOP => {
                self.cart.stop = v & 1 != 0;
                ctx.c64.set_stop_condition(self.cart.stop_mode);
                ctx.c64.set_stopped(self.cart.stop);
            }
            reg => {
                self.cart.write(reg, v);
                match reg {
                    cartregs::STOP_MODE => ctx.c64.set_stop_condition(self.cart.stop_mode),
                    // While the reset is held the logic follows the type.
                    cartregs::CARTRIDGE_TYPE if self.cart.reset => {
                        self.carts.latch(v, true);
                        self.push_cart_lines(ctx.c64);
                    }
                    cartregs::CARTRIDGE_KILL => {
                        if v & 2 != 0 {
                            self.carts.latch(self.cart.cartridge_type, self.cart.reset);
                        }
                        if v & 1 != 0 {
                            self.carts.kill();
                        }
                        self.push_cart_lines(ctx.c64);
                    }
                    _ => {}
                }
            }
        }
    }

    fn write_matrix(&mut self, off: usize, v: u8, ctx: &mut IoCtx<'_>) {
        self.matrix[off] = v;
        match off {
            0..=7 => ctx.c64.set_matrix(off as u8, !v),
            MATRIX_RESTORE => ctx.c64.set_nmi(self.nmi_line()),
            _ => {}
        }
    }

    fn write_dma(&mut self, a: u16, v: u8, ctx: &mut IoCtx<'_>) {
        if self.core.dma_memonly() {
            ctx.c64.dma_write_ram(a, v);
        } else {
            if let Some(w) = ctx.c64.cart_window(a) {
                if self.carts.write(w, a, v, ctx.ram) {
                    self.push_cart_lines(ctx.c64);
                }
            }
            ctx.c64.dma_write(a, v);
        }
        if a == UNLOCK_ACK && self.unlock {
            self.unlock = false;
            ctx.irq.set_high(IRQ_HIGH_UNLOCK, false);
        }
    }
}

impl IoDevice for C64Port {
    fn read(&mut self, addr: u32, ctx: &mut IoCtx<'_>) -> u8 {
        let addr = addr & 0xff_ffff;
        match addr {
            DMA..=0x05_ffff => {
                let a = addr as u16;
                if self.core.dma_memonly() {
                    return ctx.c64.dma_read_ram(a);
                }
                if let Some(w) = ctx.c64.cart_window(a) {
                    if let Some(v) = self.carts.read(w, a, ctx.ram) {
                        return v;
                    }
                }
                ctx.c64.dma_read(a)
            }
            // No other read has a side effect.
            _ => self.peek_with(addr, Some(&*ctx.c64)),
        }
    }

    fn peek(&self, addr: u32) -> u8 {
        self.peek_with(addr, None)
    }

    fn write(&mut self, addr: u32, v: u8, ctx: &mut IoCtx<'_>) {
        let addr = addr & 0xff_ffff;
        match addr {
            DRIVE_A..=0x02_1fff => self.drive_a.write(addr - DRIVE_A, v),
            DRIVE_B..=0x02_5fff => self.drive_b.write(addr - DRIVE_B, v),
            IEC..=0x02_8fff => self.iec.write(addr - IEC, v),
            CART..=0x04_1fff => self.write_cart(addr, v, ctx),
            UCI..=0x04_4fff => self.uci.write(addr - UCI, v),
            SAMPLER..=0x04_9fff => self.sampler.write(addr - SAMPLER, v),
            ACIA..=0x04_afff => self.acia.write(addr - ACIA, v),
            EEPROM..=0x04_cfff => self.eeprom.write(addr - EEPROM, v),
            DMA..=0x05_ffff => self.write_dma(addr as u16, v, ctx),
            TAPE_PLAY..=0x0a_0fff => self.tape_play.write(addr - TAPE_PLAY, v),
            TAPE_REC..=0x0c_0fff => self.tape_rec.write(addr - TAPE_REC, v),
            MATRIX..=0x10_03ff => self.write_matrix((addr - MATRIX) as usize, v, ctx),
            CORE..=0x18_00ff => {
                let ntsc = self.core.ntsc();
                self.core.write(addr - CORE, v);
                if self.core.ntsc() != ntsc {
                    ctx.c64.set_ntsc(!ntsc);
                }
            }
            PALETTE_RGB..=0x18_083f => self.palette_rgb[(addr - PALETTE_RGB) as usize] = v,
            PALETTE_YUV..=0x18_0c3f => self.palette_yuv[(addr - PALETTE_YUV) as usize] = v,
            PLD..=0x18_101f => self.pld[(addr - PLD) as usize] = v,
            DEBUG => self.debug = v,
            SID_FILTER..=0x18_5fff => self.sid_filter[(addr - SID_FILTER) as usize] = v,
            ROM_BASIC..=0x18_cfff => {
                if let Some((rom, off)) = rom_at(addr) {
                    ctx.c64.rom_write(rom, off, v);
                }
            }
            UDP..=0x19_00ff => self.udp[(addr - UDP) as usize] = v,
            _ => {}
        }
    }

    fn reset(&mut self) {
        let multi = (self.drive_a.multi_mode, self.drive_b.multi_mode);
        *self = Self::new();
        (self.drive_a.multi_mode, self.drive_b.multi_mode) = multi;
    }

    fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"U6CP", 2, |w| {
            self.cart.save(w);
            self.core.save(w);
            self.drive_a.save(w);
            self.drive_b.save(w);
            self.iec.save(w);
            self.uci.save(w);
            self.sampler.save(w);
            self.acia.save(w);
            self.eeprom.save(w);
            w.u8(self.tape_play.control);
            w.u8(self.tape_rec.control);
            w.raw(&self.matrix);
            w.raw(&self.palette_rgb);
            w.raw(&self.palette_yuv);
            w.raw(&self.pld);
            w.u8(self.debug);
            w.raw(&self.sid_filter);
            w.raw(&self.udp);
            w.bool(self.unlock);
            // Version 2: the cartridge logic.
            self.carts.save(w);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut r) = r.chunk(*b"U6CP", 2)?;
        let r = &mut r;
        let mut n = Self::new();
        n.cart.load(r)?;
        n.core.load(r)?;
        n.drive_a.load(r)?;
        n.drive_b.load(r)?;
        n.iec.load(r)?;
        n.uci.load(r)?;
        n.sampler.load(r)?;
        n.acia.load(r)?;
        n.eeprom.load(r)?;
        n.tape_play.control = r.u8()?;
        n.tape_rec.control = r.u8()?;
        r.raw_into(&mut n.matrix)?;
        r.raw_into(&mut n.palette_rgb)?;
        r.raw_into(&mut n.palette_yuv)?;
        r.raw_into(&mut n.pld)?;
        n.debug = r.u8()?;
        r.raw_into(&mut n.sid_filter)?;
        r.raw_into(&mut n.udp)?;
        n.unlock = r.bool()?;
        if version >= 2 {
            n.carts.load(r)?;
        }
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
