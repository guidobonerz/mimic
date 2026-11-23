use super::*;
use crate::io::NullC64;

/// A C64 that records what the port does to it. Its raster register
/// advances with every read, its CIA 1 has no key pressed, and a stop
/// can be made to take hold late.
struct Mock {
    ram: Vec<u8>,
    io: Vec<u8>,
    roms: [Vec<u8>; 3],
    raster: u8,
    stop_request: bool,
    /// Stops are granted only when this is set.
    grant: bool,
    reset_calls: Vec<bool>,
    ultimax: bool,
    nmi: bool,
    ntsc: Option<bool>,
    stop_condition: u8,
    matrix: [u8; 8],
    log: Vec<(char, u16)>,
    /// The last lines the port drove (/GAME, /EXROM low).
    cart_lines: (bool, bool),
}

impl Mock {
    fn new() -> Self {
        Self {
            ram: vec![0; 0x1_0000],
            io: vec![0; 0x1000],
            roms: [vec![0; 0x2000], vec![0; 0x2000], vec![0; 0x1000]],
            raster: 0,
            stop_request: false,
            grant: true,
            reset_calls: Vec::new(),
            ultimax: false,
            nmi: false,
            ntsc: None,
            stop_condition: 0xff,
            matrix: [0xff; 8],
            log: Vec::new(),
            cart_lines: (false, false),
        }
    }
}

impl C64Side for Mock {
    fn dma_read(&mut self, addr: u16) -> u8 {
        self.log.push(('r', addr));
        match addr {
            0xd012 => {
                self.raster = self.raster.wrapping_add(1);
                self.raster
            }
            0xdc01 => 0xff,
            0xd000..=0xdfff => self.io[usize::from(addr & 0xfff)],
            _ => self.ram[usize::from(addr)],
        }
    }

    fn dma_peek(&self, addr: u16) -> u8 {
        match addr {
            0xd012 => self.raster,
            0xdc01 => 0xff,
            0xd000..=0xdfff => self.io[usize::from(addr & 0xfff)],
            _ => self.ram[usize::from(addr)],
        }
    }

    fn dma_write(&mut self, addr: u16, v: u8) {
        self.log.push(('w', addr));
        match addr {
            0xd000..=0xdfff => self.io[usize::from(addr & 0xfff)] = v,
            _ => self.ram[usize::from(addr)] = v,
        }
    }

    fn set_stopped(&mut self, on: bool) {
        self.stop_request = on;
    }

    fn stopped(&self) -> bool {
        self.stop_request && self.grant
    }

    fn set_reset(&mut self, on: bool) {
        self.reset_calls.push(on);
    }

    fn rom_write(&mut self, rom: C64Rom, offset: u16, v: u8) {
        self.roms[rom as usize][usize::from(offset)] = v;
    }

    fn rom_read(&self, rom: C64Rom, offset: u16) -> u8 {
        self.roms[rom as usize][usize::from(offset)]
    }

    fn set_ntsc(&mut self, ntsc: bool) {
        self.ntsc = Some(ntsc);
    }

    fn set_matrix(&mut self, column: u8, rows: u8) {
        self.matrix[usize::from(column & 7)] = rows;
    }

    fn set_ultimax(&mut self, on: bool) {
        self.ultimax = on;
    }

    fn set_nmi(&mut self, on: bool) {
        self.nmi = on;
    }

    fn set_stop_condition(&mut self, mode: u8) {
        self.stop_condition = mode;
    }

    fn dma_read_ram(&mut self, addr: u16) -> u8 {
        self.log.push(('R', addr));
        self.ram[usize::from(addr)]
    }

    fn dma_peek_ram(&self, addr: u16) -> u8 {
        self.ram[usize::from(addr)]
    }

    fn dma_write_ram(&mut self, addr: u16, v: u8) {
        self.log.push(('W', addr));
        self.ram[usize::from(addr)] = v;
    }

    fn set_cart_lines(&mut self, game: bool, exrom: bool) {
        self.cart_lines = (game, exrom);
    }

    fn cart_window(&self, addr: u16) -> Option<CartWindow> {
        match addr {
            0x8000..=0x9fff => Some(CartWindow::RomL),
            0xde00..=0xdfff => Some(CartWindow::Io),
            _ => None,
        }
    }
}

/// The port with a C64 and the rest of an [`IoCtx`].
struct Rig<C: C64Side> {
    port: C64Port,
    c64: C,
    irq: Irq,
    ram: Vec<u8>,
    console: Vec<u8>,
    now: u64,
}

impl<C: C64Side> Rig<C> {
    fn with(c64: C) -> Self {
        Self {
            port: C64Port::new(),
            c64,
            irq: Irq {
                global: true,
                mask: 0xff,
                mask_high: 0xff,
                ..Irq::default()
            },
            ram: vec![0; 0x100],
            console: Vec::new(),
            now: 0,
        }
    }

    fn rd(&mut self, addr: u32) -> u8 {
        self.now += 4;
        let mut ctx = IoCtx {
            now: self.now,
            ram: &mut self.ram,
            irq: &mut self.irq,
            console: &mut self.console,
            c64: &mut self.c64,
            storage: None,
            net: None,
        };
        self.port.read(addr, &mut ctx)
    }

    fn wr(&mut self, addr: u32, v: u8) {
        self.now += 4;
        let mut ctx = IoCtx {
            now: self.now,
            ram: &mut self.ram,
            irq: &mut self.irq,
            console: &mut self.console,
            c64: &mut self.c64,
            storage: None,
            net: None,
        };
        self.port.write(addr, v, &mut ctx);
    }

    /// A 32-bit access as the bus bridge splits it.
    fn rd32(&mut self, addr: u32) -> u32 {
        (0..4).fold(0, |acc, i| acc | (u32::from(self.rd(addr + i)) << (8 * i)))
    }

    fn wr32(&mut self, addr: u32, v: u32) {
        for i in 0..4 {
            self.wr(addr + i, (v >> (8 * i)) as u8);
        }
    }

    fn peek(&self, addr: u32) -> u8 {
        self.port.peek_with(addr, Some(&self.c64))
    }
}

fn rig() -> Rig<Mock> {
    Rig::with(Mock::new())
}

const C64_MODE: u32 = 0x04_0000;
const C64_STOP: u32 = 0x04_0001;
const C64_STOP_MODE: u32 = 0x04_0002;
const C64_CLOCK_DETECT: u32 = 0x04_0003;

// Hazard C12 (doc 10 H1, H2): the firmware's hard stop sets "at once",
// requests the stop and polls the acknowledge without a timeout.
#[test]
fn forced_stop_is_acknowledged_at_once() {
    let mut r = rig();
    r.c64.grant = false;
    assert_eq!(r.rd(C64_STOP), 0);
    r.wr(C64_STOP_MODE, 2);
    r.wr(C64_STOP, 1);
    assert_eq!(r.rd(C64_STOP), 3);
    assert!(r.c64.stop_request);
    assert_eq!(r.c64.stop_condition, 2);
    r.wr(C64_STOP, 0);
    assert_eq!(r.rd(C64_STOP), 0);
    assert!(!r.c64.stop_request);
    // Only bit 0 is a request.
    r.wr(C64_STOP, 0xfe);
    assert_eq!(r.rd(C64_STOP), 0);
}

// Doc 10 H3 / C17: with the conditions "bad line" and "write then read"
// the acknowledge is the C64's; the firmware gives up after a time and
// forces the stop.
#[test]
fn conditional_stop_follows_the_c64() {
    let mut r = rig();
    r.c64.grant = false;
    r.wr(C64_STOP_MODE, 1);
    r.wr(C64_STOP, 1);
    assert_eq!(r.rd(C64_STOP), 1);
    assert_eq!(r.rd(C64_STOP_MODE), 1);
    assert_eq!(r.c64.stop_condition, 1);
    // The fallback.
    r.wr(C64_STOP_MODE, 2);
    assert_eq!(r.rd(C64_STOP), 3);
    assert_eq!(r.c64.stop_condition, 2);
    // A C64 that grants the stop.
    r.wr(C64_STOP_MODE, 0);
    r.c64.grant = true;
    assert_eq!(r.rd(C64_STOP), 3);
    // Without the C64 at hand the debugger's view takes it as granted.
    assert_eq!(r.port.peek(C64_STOP), 3);
}

// Hazards C14 (doc 10 H4) and H8: after C64_MODE = release the firmware
// polls the reset sense bit until it is 0; PHI2 must show always.
#[test]
fn clock_detect_shows_phi2_and_the_reset_line() {
    let mut r = rig();
    // Power-on: held in reset.
    assert_eq!(r.rd(C64_CLOCK_DETECT), 0x11);
    r.wr(C64_MODE, 0x08);
    assert_eq!(r.rd(C64_CLOCK_DETECT), 0x01);
    r.wr(C64_MODE, 0x04);
    assert_eq!(r.rd(C64_CLOCK_DETECT), 0x11);
    r.wr(C64_MODE, 0x08);
    assert_eq!(r.rd(C64_CLOCK_DETECT) & 0x10, 0);
    assert_eq!(r.rd(C64_CLOCK_DETECT) & 0x01, 1);
}

#[test]
fn reset_sequence_reaches_the_c64_and_the_itu() {
    let mut r = rig();
    // Release from the power-on reset: no event.
    r.wr(C64_MODE, 0x08);
    assert_eq!(r.c64.reset_calls, [false]);
    assert_eq!(r.irq.flags, 0);
    // The firmware's reset: mode 0, pull, wait, release.
    r.wr(C64_MODE, 0x00);
    r.wr(C64_MODE, 0x04);
    assert_eq!(r.rd(C64_MODE), 0x04);
    assert_eq!(r.c64.reset_calls, [false, true]);
    assert_eq!(r.irq.flags, 0x80);
    assert!(r.irq.line());
    r.irq.clear(0x80);
    // Pulling again while held is no new edge.
    r.wr(C64_MODE, 0x04);
    assert_eq!(r.irq.flags, 0);
    assert_eq!(r.c64.reset_calls, [false, true]);
    r.wr(C64_MODE, 0x08);
    assert_eq!(r.rd(C64_MODE), 0x00);
    assert_eq!(r.c64.reset_calls, [false, true, false]);
    assert_eq!(r.irq.flags, 0);
    // Releasing what is not held does nothing.
    r.wr(C64_MODE, 0x08);
    assert_eq!(r.c64.reset_calls.len(), 3);
    // Bit 2 wins over bit 3.
    r.wr(C64_MODE, 0x0c);
    assert_eq!(r.rd(C64_MODE), 0x04);
    // A reset the C64 makes itself is the same event.
    r.irq.clear(0xff);
    r.port.c64_reset_event(&mut r.irq);
    assert_eq!(r.irq.flags, 0x80);
}

// Doc 10 H16: C64_MODE reads back by the register's bit rules.
#[test]
fn mode_bits_follow_the_register_rules() {
    let mut r = rig();
    r.wr(C64_MODE, 0x08);
    r.wr(C64_MODE, 0x02);
    assert_eq!(r.rd(C64_MODE), 0x02);
    assert!(r.c64.ultimax);
    r.wr(C64_MODE, 0x10);
    assert_eq!(r.rd(C64_MODE), 0x10);
    assert!(!r.c64.ultimax);
    assert!(r.c64.nmi);
    // A write with the reset bit leaves ULTIMAX and NMI alone.
    r.wr(C64_MODE, 0x12);
    r.wr(C64_MODE, 0x04);
    assert_eq!(r.rd(C64_MODE), 0x16);
    assert!(r.c64.ultimax && r.c64.nmi);
    r.wr(C64_MODE, 0x08);
    assert_eq!(r.rd(C64_MODE), 0x12);
    r.wr(C64_MODE, 0x00);
    assert_eq!(r.rd(C64_MODE), 0x00);
    assert!(!r.c64.ultimax && !r.c64.nmi);
    // The registers repeat every 16 bytes.
    r.wr(C64_MODE + 0x10, 0x02);
    assert_eq!(r.rd(C64_MODE), 0x02);
}

// Hazard C16 (doc 10 H5): the SID detection waits for raster line $FF by
// reading $D012 through the DMA window, interrupts off, no timeout.
#[test]
fn d012_poll_ends_when_the_raster_advances() {
    let mut r = rig();
    r.wr(C64_STOP_MODE, 2);
    r.wr(C64_STOP, 1);
    let mut reads = 0;
    while r.rd(0x05_d012) != 0xff {
        reads += 1;
        assert!(reads < 1000, "the raster never reached $FF");
    }
    assert_eq!(reads, 254);
}

// Hazard C18 (doc 10 H10, H11): the boot hotkey scan reads CIA 1 until
// two reads of $DC01 agree, and $DC00 must return what was written.
#[test]
fn cia_keyboard_scan_loop_ends() {
    let mut r = rig();
    r.wr(0x05_dc02, 0xff);
    r.wr(0x05_dc03, 0x00);
    r.wr(0x05_dc00, 0x00);
    assert_eq!(r.rd(0x05_dc01), 0xff);
    for column in 0..8 {
        let mut rounds = 0;
        loop {
            let row = r.rd(0x05_dc01);
            r.wr(0x05_dc00, !(1 << column));
            rounds += 1;
            if row == r.rd(0x05_dc01) {
                break;
            }
            assert!(rounds < 10);
        }
        assert_eq!(r.rd(0x05_dc00), !(1 << column));
    }
    r.wr(0x05_dc00, 0xff);
    assert_eq!(r.rd(0x05_dc00), 0xff);
}

// Doc 10 H17: one byte of the window is one access on the C64's bus;
// wider accesses are bytes at ascending addresses.
#[test]
fn dma_window_reaches_the_c64_bus() {
    let mut r = rig();
    r.wr(0x05_0400, 0x41);
    assert_eq!(r.c64.ram[0x400], 0x41);
    r.c64.ram[0xc000] = 0x99;
    assert_eq!(r.rd(0x05_c000), 0x99);
    r.wr(0x05_d020, 0x06);
    assert_eq!(r.c64.io[0x020], 0x06);
    assert_eq!(r.c64.ram[0xd020], 0);
    r.c64.log.clear();
    r.wr32(0x05_0800, 0x4433_2211);
    assert_eq!(r.rd32(0x05_0800), 0x4433_2211);
    assert_eq!(
        r.c64.log,
        [
            ('w', 0x800),
            ('w', 0x801),
            ('w', 0x802),
            ('w', 0x803),
            ('r', 0x800),
            ('r', 0x801),
            ('r', 0x802),
            ('r', 0x803)
        ]
    );
    assert_eq!(r.c64.ram[0x800..0x804], [0x11, 0x22, 0x33, 0x44]);
    // The top of the window.
    r.wr(0x05_ffff, 0x5a);
    assert_eq!(r.c64.ram[0xffff], 0x5a);
}

#[test]
fn dma_memory_only_bypasses_io() {
    let mut r = rig();
    r.wr(0x05_d020, 0x01);
    r.wr(0x18_0003, 1);
    assert_eq!(r.rd(0x18_0003), 1);
    r.c64.log.clear();
    r.wr(0x05_d020, 0x77);
    assert_eq!(r.rd(0x05_d020), 0x77);
    assert_eq!(r.c64.log, [('W', 0xd020), ('R', 0xd020)]);
    assert_eq!(r.c64.ram[0xd020], 0x77);
    assert_eq!(r.c64.io[0x020], 0x01);
    assert_eq!(r.peek(0x05_d020), 0x77);
    r.wr(0x18_0003, 0);
    assert_eq!(r.rd(0x05_d020), 0x01);
    assert_eq!(r.peek(0x05_d020), 0x01);
}

// Golden rule 2: looking does not change anything.
#[test]
fn peek_has_no_side_effects() {
    let mut r = rig();
    r.c64.raster = 0x40;
    for _ in 0..3 {
        assert_eq!(r.peek(0x05_d012), 0x40);
    }
    assert!(r.c64.log.is_empty());
    assert_eq!(r.rd(0x05_d012), 0x41);
    // Without the C64 the window reads 0.
    assert_eq!(r.port.peek(0x05_d012), 0);
    let before = r.port.clone();
    for &(lo, hi) in &CLAIMS {
        for addr in (lo..=hi).step_by(7) {
            let _ = r.peek(addr);
            let _ = r.port.peek(addr);
        }
    }
    assert_eq!(r.port, before);
    assert!(r.c64.log.len() == 1);
}

// Memory map M8 and hazard C30: the firmware writes the ROMs into the
// windows (also as 32-bit stores) and reads them back.
#[test]
fn rom_windows_reach_the_roms_and_read_back() {
    let mut r = rig();
    r.wr32(0x18_8000, 0xa0e3_7b94);
    r.wr(0x18_9fff, 0x11);
    r.wr(0x18_a000, 0x85);
    r.wr(0x18_bfff, 0xe5);
    r.wr(0x18_c000, 0x3c);
    r.wr(0x18_cfff, 0x0f);
    assert_eq!(r.c64.roms[0][..4], [0x94, 0x7b, 0xe3, 0xa0]);
    assert_eq!(r.c64.roms[0][0x1fff], 0x11);
    assert_eq!(r.c64.roms[1][0], 0x85);
    assert_eq!(r.c64.roms[1][0x1fff], 0xe5);
    assert_eq!(r.c64.roms[2][0], 0x3c);
    assert_eq!(r.c64.roms[2][0xfff], 0x0f);
    assert_eq!(r.rd32(0x18_8000), 0xa0e3_7b94);
    assert_eq!(r.rd(0x18_bfff), 0xe5);
    assert_eq!(r.rd(0x18_cfff), 0x0f);
    assert_eq!(r.peek(0x18_a000), 0x85);
    // Behind the character ROM there is nothing.
    r.wr(0x18_d000, 0xff);
    assert_eq!(r.rd(0x18_d000), 0);
    // The same against the plain stand-in.
    let mut n = Rig::with(NullC64::default());
    n.wr(0x18_a123, 0x4c);
    assert_eq!(n.rd(0x18_a123), 0x4c);
    assert_eq!(n.c64.roms[1][0x123], 0x4c);
}

// Hazard C30 (doc 10 H16): the firmware reads the latches back.
#[test]
fn cartridge_latches_read_back() {
    let mut r = rig();
    // Out of reset, so the type is not latched (see `carts`).
    r.wr(0x04_0000, 0x08);
    r.wr(0x04_0005, 0x41);
    assert_eq!(r.rd(0x04_0005), 0x41);
    assert_eq!(r.port.cart.cartridge_type, 0x41);
    r.wr(0x04_0008, 1);
    r.wr(0x04_0009, 5);
    assert_eq!(r.rd(0x04_0008), 1);
    assert_eq!(r.rd(0x04_0009), 5);
    assert_eq!(r.port.cart.reu_bytes(), 4 << 20);
    r.wr(0x04_0009, 0xff);
    assert_eq!(r.rd(0x04_0009), 7);
    assert_eq!(r.port.cart.reu_bytes(), 16 << 20);
    r.wr(0x04_000e, 1);
    assert_eq!(r.rd(0x04_000e), 1);
    r.wr(0x04_000d, 0xff);
    assert_eq!(r.rd(0x04_000d), 1);
    r.wr(0x04_0007, 1);
    assert_eq!(r.rd(0x04_0007), 1);
    r.wr(0x04_0007, 0);
    assert_eq!(r.rd(0x04_0007), 0);
    // Kill: the force strobe twice, as the firmware writes it, latches
    // the type with the cartridge off; the register reads "cartridge
    // mapped".
    r.wr(0x04_0006, 2);
    r.wr(0x04_0006, 2);
    assert_eq!(r.port.cart.force_strobes, 2);
    assert_eq!(r.rd(0x04_0006), 0);
    r.port.carts.latch(0x41, true);
    assert_eq!(r.rd(0x04_0006), 1);
    r.port.carts.kill();
    // The firmware's $FF to the button swap and the write-only timing
    // registers leave nothing to read.
    r.wr(0x04_000a, 0xff);
    r.wr(0x04_000b, 0xbb);
    assert_eq!(r.rd(0x04_000a), 0);
    assert_eq!(r.rd(0x04_000b), 0);
    assert!(r.port.cart.swap_buttons);
    r.wr(0x04_000c, 0x0f);
    assert_eq!(r.rd(0x04_000c), 0x05);
    assert_eq!(r.rd(0x04_0004), 0);
    assert_eq!(r.rd(0x04_000f), 0);
}

#[test]
fn core_config_latches_and_constants() {
    let mut r = rig();
    assert_eq!(r.rd(0x18_0010), coreconfig::CORE_VERSION);
    r.wr(0x18_0010, 0);
    assert_eq!(r.rd(0x18_0010), coreconfig::CORE_VERSION);
    // The injected joystick lines power on released.
    assert_eq!(r.rd(0x18_0030), 0xff);
    assert_eq!(r.port.core.joy_swout(1), 0xff);
    // Read-modify-write registers.
    r.wr(0x18_002b, 0x0d);
    let v = r.rd(0x18_002b);
    r.wr(0x18_002b, v | 2);
    assert_eq!(r.rd(0x18_002b), 0x0f);
    r.wr(0x18_002a, 5);
    r.wr(0x18_002c, 0);
    assert_eq!(r.port.core.bus_sharing(), (5, 0x0f, 0));
    // The SID decode of the firmware's detection.
    r.wr(0x18_0008, 0x40);
    r.wr(0x18_000c, 0xf0);
    r.wr(0x18_000a, 0x60);
    r.wr(0x18_000e, 0xfe);
    r.wr(0x18_0011, 1);
    assert_eq!(r.port.core.sid_decode(0), (0x40, 0xf0));
    assert_eq!(r.port.core.sid_decode(2), (0x60, 0xfe));
    assert!(r.port.core.sid_socket_enabled(0));
    assert!(!r.port.core.sid_socket_enabled(1));
    // Joystick, paddles, mouse.
    r.wr(0x18_0031, 0xef);
    r.wr(0x18_0034, 0x80);
    r.wr(0x18_0037, 1);
    r.wr(0x18_0013, 1);
    assert_eq!(r.port.core.joy_swout(1), 0xef);
    assert_eq!(r.port.core.paddle(2), 0x80);
    assert!(r.port.core.mouse_enabled(1));
    assert!(r.port.core.paddle_enabled());
    // The envelope levels read 0 and take no writes.
    r.wr(0x18_0081, 0x55);
    assert_eq!(r.rd(0x18_0081), 0);
    assert_eq!(r.rd(0x18_0086), 0);
}

#[test]
fn video_format_tells_the_c64() {
    let mut r = rig();
    assert_eq!(r.c64.ntsc, None);
    // PAL: 63 cycles.
    r.wr(0x18_0001, 0x00);
    assert_eq!(r.c64.ntsc, None);
    // NTSC: 65 cycles, NTSC clock and colour, 60 Hz.
    r.wr(0x18_0001, 0x2b);
    assert_eq!(r.rd(0x18_0001), 0x2b);
    assert_eq!(r.c64.ntsc, Some(true));
    assert!(r.port.core.ntsc() && r.port.core.ntsc_colour());
    // PAL colours on NTSC timing stay an NTSC machine.
    r.c64.ntsc = None;
    r.wr(0x18_0001, 0x2a);
    assert_eq!(r.c64.ntsc, None);
    assert!(!r.port.core.ntsc_colour());
    // NTSC colours on PAL timing are a PAL machine.
    r.wr(0x18_0001, 0x01);
    assert_eq!(r.c64.ntsc, Some(false));
}

#[test]
fn turbo_setting_is_taken_on_the_strobe() {
    let mut r = rig();
    assert_eq!(r.port.core.cpu_speed_mhz(), 1);
    assert!(r.port.core.badline_timing());
    assert_eq!(r.port.core.turbo_control(), 0);
    // "Manual", step 9, no bad lines.
    r.wr(0x18_0002, 0x00);
    r.wr(0x18_002d, 0x09);
    assert_eq!(r.port.core.cpu_speed(), 0);
    r.wr(0x18_002e, 1);
    assert_eq!(r.port.core.cpu_speed(), 9);
    assert_eq!(r.port.core.cpu_speed_mhz(), 16);
    assert!(!r.port.core.badline_timing());
    assert_eq!(r.port.core.speed_updates(), 1);
    // Turbo registers with SuperCPU detection, fastest, bad lines kept.
    r.wr(0x18_0002, 0x03);
    r.wr(0x18_002d, 0x8f);
    r.wr(0x18_002e, 1);
    assert_eq!(r.port.core.turbo_control(), 3);
    assert_eq!(r.port.core.cpu_speed_mhz(), 64);
    assert!(r.port.core.badline_timing());
    // Off.
    r.wr(0x18_0002, 0);
    r.wr(0x18_002d, 0x80);
    r.wr(0x18_002e, 1);
    assert_eq!(r.port.core.cpu_speed_mhz(), 1);
    assert_eq!(r.port.core.speed_updates(), 3);
}

// Hazard C22 (doc 11 H10-H12): the IEC server polls the queues; they
// must read "empty", and the stray write in front of the code memory
// must be taken.
#[test]
fn iec_processor_idles() {
    let mut r = rig();
    assert_eq!(r.rd(0x02_8000), 0x25);
    assert_eq!(r.rd(0x02_8001), 0x01);
    assert_eq!(r.rd(0x02_8002), 0x01);
    for reg in 3..16 {
        assert_eq!(r.rd(0x02_8000 + reg), 0, "register {reg}");
    }
    // The microcode goes in byte by byte and the addresses are patched.
    r.wr(0x02_8003, 0);
    for i in 0..0x768u32 {
        r.wr(0x02_8800 + i, i as u8);
    }
    assert_eq!(r.port.iec.code()[0x767], 0x67);
    for (listen, talk) in [(0x44, 0x28), (0x7c, 0x60), (0xb4, 0x98)] {
        r.wr(0x02_8800 + listen, 0x3f);
        r.wr(0x02_8800 + talk, 0x5f);
    }
    r.wr(0x02_87ff, 0x3f);
    r.wr(0x02_87ff, 0x5f);
    assert_eq!(r.rd(0x02_87ff), 0);
    assert_eq!(r.port.iec.device_numbers(), [None; 3]);
    r.wr(0x02_8844, 0x2b);
    r.wr(0x02_8828, 0x4b);
    assert_eq!(r.port.iec.device_numbers(), [Some(11), None, None]);
    assert_eq!(r.rd(0x02_8844), 0x2b);
    // Still idle after traffic towards it.
    r.wr(0x02_8003, 1);
    r.wr(0x02_8008, 0x41);
    r.wr(0x02_8009, 0x1d);
    r.wr(0x02_800d, 0);
    r.wr(0x02_800c, 1);
    assert!(r.port.iec.enabled && r.port.iec.irq_enable);
    assert_eq!(r.rd(0x02_8001), 0x01);
    assert_eq!(r.rd(0x02_8002), 0x01);
    assert_eq!(r.rd(0x02_8006), 0);
    assert_eq!(r.rd(0x02_800c), 0);
}

// Hazard C28 (doc 11 H2-H5, H9, H13): the drives' start-up.
#[test]
fn drives_idle() {
    let mut r = rig();
    for base in [0x02_0000u32, 0x02_4000] {
        // The power-on value of RESET.
        assert_eq!(r.rd(base + 1), 7);
        // The drive can change its type: the 2 written reads back.
        r.wr(base + 0xd, 2);
        assert_eq!(r.rd(base + 0xd) & 3, 2);
        // The track table: written, reads 0.
        for i in 0..256u32 {
            r.wr32(base + 0x1000 + i * 8, 0x0012_3400 + i);
            r.wr32(base + 0x1000 + i * 8 + 4, 0x028a_0100);
        }
        assert_eq!(r.rd32(base + 0x1000), 0);
        r.wr(base + 3, 1);
        r.wr(base + 4, 0);
        // The WD177x: the command queue is empty, however often asked
        // and whatever is written.
        r.wr(base + 0x1800, 3);
        let mut rounds = 0;
        while r.rd(base + 0x1806) & 0x80 != 0 {
            r.wr(base + 0x1806, 1);
            rounds += 1;
            assert!(rounds < 10);
        }
        assert_eq!(r.rd(base + 0x1806), 0);
        r.wr(base + 0x180e, 40);
        assert_eq!(r.rd(base + 0x180e), 0);
        assert_eq!(r.rd(base + 0x180f), 12);
        assert_eq!(r.rd(base + 0x1801), 1);
        // Its status register with set and clear.
        r.wr(base + 0x1805, 0x30);
        r.wr(base + 0x1804, 0x10);
        assert_eq!(r.rd(base + 0x1804), 0x20);
        assert_eq!(r.rd(base + 0x1805), 0x20);
        // Reset and power read back.
        r.wr(base + 1, 1);
        r.wr(base + 1, 6);
        assert_eq!(r.rd(base + 1), 6);
        assert_eq!(r.rd(base), 0);
        r.wr(base, 1);
        assert_eq!(r.rd(base), 1);
        // DIRTY is not memory: the $80 the firmware writes after power
        // on must not read back.
        r.wr(base + 0x800, 0x80);
        assert_eq!(r.rd(base + 0x800), 0);
        assert_eq!(r.rd(base + 0x87f), 0);
        // Track, side, status.
        r.wr(base + 8, 0xff);
        for reg in [6, 8, 9] {
            assert_eq!(r.rd(base + reg), 0);
        }
    }
    let d = &r.port.drive_a;
    assert!(d.powered() && !d.write_protected() && !d.inserted());
    assert_eq!(d.device(), 8);
    assert_eq!(d.drive_type(), 2);
    assert_eq!(d.reset_control(), 6);
    assert_eq!(d.track_entry(0, 3), (0x0012_3403, 0x100, 0x28a));
    // No interrupt from either WD177x.
    assert_eq!(r.irq.high, 0);
    // A drive with one type only.
    r.port.drive_b.multi_mode = false;
    assert_eq!(r.rd(0x02_400d), 0);
}

// Hazards A5 and C3 (doc 11 H6, H7, H14).
#[test]
fn uci_idles_with_its_constants() {
    let mut r = rig();
    // What the firmware's constructor does.
    r.wr(0x04_4000, 0x47);
    r.wr(0x04_4002, 0x87);
    assert_eq!(r.rd(0x04_4006), 0x70);
    assert_eq!(r.rd(0x04_4008), 0xe0);
    assert_eq!(r.rd(0x04_4004), 0x00);
    assert_eq!(r.rd(0x04_4005), 0x6f);
    assert_eq!(r.rd(0x04_4007), 0xdf);
    assert_eq!(r.rd(0x04_4009), 0xff);
    // The handshake register is not memory.
    assert_eq!(r.rd(0x04_4002), 0);
    assert_eq!(r.rd(0x04_4003), 0);
    // Read positions at rest, command length.
    assert_eq!(r.rd(0x04_400a), 0x00);
    assert_eq!(r.rd(0x04_400c), 0x80);
    assert_eq!(r.rd(0x04_400d), 0x03);
    assert_eq!(r.rd(0x04_400e), 0);
    assert_eq!(r.rd(0x04_400f), 0);
    // The interrupt mask: set and clear registers.
    assert_eq!(r.rd(0x04_400b), 7);
    r.wr(0x04_4005, 7);
    assert_eq!(r.rd(0x04_400b), 0);
    r.wr(0x04_4004, 0xfd);
    assert_eq!(r.rd(0x04_400b), 5);
    // Enable and bus number share a register.
    r.wr(0x04_4001, 0x8b);
    assert_eq!(r.rd(0x04_4001), 0);
    r.wr(0x04_4001, 1);
    assert_eq!(r.rd(0x04_4001), 1);
    assert_eq!(r.port.uci.bus_id, 11);
    assert_eq!(r.rd(0x04_4000), 0x47);
    assert_eq!(r.port.uci.c64_address(), 0xdf1c);
    // The shared memory.
    r.wr(0x04_4800, 0x12);
    r.wr(0x04_4b80, 0x34);
    r.wr(0x04_4fff, 0x56);
    assert_eq!(r.rd(0x04_4800), 0x12);
    assert_eq!(r.rd(0x04_4b80), 0x34);
    assert_eq!(r.rd(0x04_4fff), 0x56);
    r.wr(0x04_400c, 0x10);
    r.wr(0x04_400d, 0x02);
    assert_eq!(r.port.uci.response_length, 0x210);
    // With the mask open and the source enabled the line stays low.
    assert_eq!(r.irq.level, 0);
    assert!(!r.irq.line());
}

// Hazards C26, C27 (doc 11 H8, H15, H16) and the other idle blocks.
#[test]
fn small_blocks_idle() {
    let mut r = rig();
    // Tape playback: "queue empty" everywhere in the window.
    r.wr(0x0a_0000, 0x06);
    r.wr(0x0a_0000, 0x00);
    r.wr(0x0a_0800, 0x95);
    for addr in [0x0a_0000, 0x0a_0001, 0x0a_07ff, 0x0a_0800, 0x0a_0fff] {
        assert_eq!(r.rd(addr), 0x80);
    }
    // Tape recording: nothing waits.
    r.wr(0x0c_0000, 0x00);
    r.wr(0x0c_0000, 0x06);
    r.wr(0x0c_0000, 0x91);
    assert_eq!(r.port.tape_rec.control, 0x91);
    assert_eq!(r.rd(0x0c_0000), 0);
    assert_eq!(r.rd32(0x0c_0800), 0);
    // The sampler: the firmware switches the voices off; even addresses
    // read the interrupt flags, odd ones the version, all over the 8 KB.
    for voice in 0..8 {
        r.wr(0x04_8000 + voice * 0x20, 0);
    }
    r.wr(0x04_9f21, 0x3f);
    assert_eq!(r.port.sampler.regs()[0x21], 0x3f);
    assert_eq!(r.rd(0x04_8000), 0);
    assert_eq!(r.rd(0x04_8001), 0x10);
    assert_eq!(r.rd(0x04_9fe0), 0);
    assert_eq!(r.rd(0x04_9fff), 0x10);
    // The ACIA: what the firmware's constructor and modem do.
    r.wr(0x04_a00a, 0x00);
    r.wr(0x04_a007, 0x00);
    assert_eq!(r.rd(0x04_a009), 0);
    assert_eq!(r.rd(0x04_a005), 0x02);
    assert_eq!(r.rd(0x04_a006), 0x70);
    assert_eq!(r.rd(0x04_a008), 0x40);
    r.wr(0x04_a008, 0xff);
    assert_eq!(r.rd(0x04_a008), 0x75);
    assert_eq!(r.rd(0x04_a006), 0x10);
    r.wr(0x04_a007, 0xff);
    assert_eq!(r.rd(0x04_a007), 0x1f);
    r.wr(0x04_aa05, 0x41);
    assert_eq!(r.rd(0x04_ab05), 0x41);
    r.wr(0x04_a000, 6);
    r.wr(0x04_a003, 9);
    assert_eq!(r.rd(0x04_a000), 6);
    assert_eq!(r.rd(0x04_a003), 9);
    r.wr(0x04_a805, 0x99);
    assert_eq!(r.rd(0x04_a805), 0);
    assert_eq!(r.rd(0x04_a001), 0);
    assert_eq!(r.rd(0x04_a002), 0);
    // A C64 reset starts the rings over.
    r.wr(C64_MODE, 0x08);
    r.wr(C64_MODE, 0x04);
    assert_eq!(r.rd(0x04_a000), 0);
    assert_eq!(r.rd(0x04_a003), 0);
    // The EEPROM: never "changed", the bytes are kept.
    r.wr(0x04_c000, 1);
    assert_eq!(r.rd(0x04_c000), 0);
    r.wr(0x04_c800, 0xa5);
    r.wr(0x04_cfff, 0x5a);
    assert_eq!(r.rd(0x04_c800), 0xa5);
    assert_eq!(r.rd(0x04_cfff), 0x5a);
    assert_eq!(r.port.eeprom.data()[0x7ff], 0x5a);
    // The SID control of older boards and the timing samples.
    for addr in [0x04_2000, 0x04_200b, 0x04_2800, 0x04_6000, 0x04_67ff] {
        r.wr(addr, 0xff);
        assert_eq!(r.rd(addr), 0);
    }
    // Low 3 (tape), low 4 (UCI), high 0 (ACIA) stay low.
    assert_eq!(r.irq.level, 0);
    assert_eq!(r.irq.high, 0);
}

#[test]
fn keyboard_matrix_reaches_the_c64() {
    let mut r = rig();
    // The firmware clears the eight columns, then applies its state.
    for i in 0..11 {
        r.wr(0x10_0300 + i, 0);
    }
    assert_eq!(r.c64.matrix, [0xff; 8]);
    // "A": column 1, row 2.
    r.wr(0x10_0301, 0x04);
    assert_eq!(r.c64.matrix[1], 0xfb);
    assert_eq!(r.rd(0x10_0301), 0x04);
    assert_eq!(r.port.matrix()[1], 0x04);
    r.wr(0x10_0301, 0);
    assert_eq!(r.c64.matrix[1], 0xff);
    // RESTORE is the NMI line, together with C64_MODE bit 4.
    r.wr(0x10_0309, 1);
    assert!(r.c64.nmi);
    r.wr(C64_MODE, 0x00);
    assert!(r.c64.nmi);
    r.wr(0x10_0309, 0);
    assert!(!r.c64.nmi);
    r.wr(C64_MODE, 0x10);
    assert!(r.c64.nmi);
    r.wr(C64_MODE, 0x00);
    assert!(!r.c64.nmi);
    // Reset keys and freeze button are kept for the machine.
    r.wr(0x10_0308, 1);
    r.wr(0x10_030a, 1);
    assert!(r.port.matrix_reset() && r.port.matrix_freeze());
    // Hazard C19: the 32-bit store at the unaligned $10030B.
    r.wr32(0x10_030b, 0x0000_0001);
    assert_eq!(r.port.wasd_to_joy(), 1);
    assert_eq!(r.rd32(0x10_030b), 1);
    r.wr32(0x10_030b, 0xdead_beef);
    assert_eq!(r.port.wasd_to_joy(), 0xdead_beef);
    assert_eq!(r.rd(0x10_03ff), 0);
}

#[test]
fn stores_for_the_machine() {
    let mut r = rig();
    for i in 0..0x40u32 {
        r.wr(0x18_0800 + i, i as u8);
        r.wr(0x18_0c00 + i, !(i as u8));
    }
    assert_eq!(r.port.palette_rgb()[5], 5);
    assert_eq!(r.port.palette_yuv()[5], !5);
    // Write only.
    assert_eq!(r.rd(0x18_0805), 0);
    // The PLD window and the debug register are memory.
    r.wr(0x18_101e, 0x80);
    assert_eq!(r.rd(0x18_101e), 0x80);
    r.wr(0x18_1010, 0x12);
    assert_eq!(r.rd(0x18_1010), 0x12);
    assert_eq!(r.rd(0x18_1000), 0);
    r.wr(0x18_1800, 0x3c);
    assert_eq!(r.rd(0x18_1800), 0x3c);
    // Filter curves.
    r.wr(0x18_5000, 0x34);
    r.wr(0x18_5001, 0x12);
    r.wr(0x18_5ffe, 0x78);
    assert_eq!(r.port.sid_filter_curve(0)[..2], [0x34, 0x12]);
    assert_eq!(r.port.sid_filter_curve(1)[0x7fe], 0x78);
    assert_eq!(r.rd(0x18_5000), 0);
    // UDP header templates.
    r.wr(0x19_0040, 0xff);
    r.wr(0x19_00ff, 0x01);
    assert_eq!(r.port.udp_header(1)[0], 0xff);
    assert_eq!(r.port.udp_header(3)[0x3f], 0x01);
    assert_eq!(r.rd(0x19_0040), 0);
}

// Doc S15: the unlock interrupt is a level without an acknowledge
// register; the handler's write to $D038 takes it away.
#[test]
fn unlock_interrupt_is_dropped_by_the_handlers_write() {
    let mut r = rig();
    r.wr(0x05_d038, 0);
    assert_eq!(r.irq.high, 0);
    r.port.raise_unlock(&mut r.irq);
    assert_eq!(r.irq.high_active(), 0x40);
    assert!(r.irq.line());
    // Other accesses leave it.
    r.wr(0x05_d036, 0);
    let _ = r.rd(0x05_d038);
    assert_eq!(r.irq.high, 0x40);
    r.wr(0x05_d038, 0);
    assert_eq!(r.irq.high, 0);
    assert!(!r.irq.line());
}

// No interrupt of these blocks comes up by itself: everything is
// written with every value, with all sources enabled.
#[test]
fn nothing_raises_an_interrupt_wrongly() {
    let mut r = rig();
    r.wr(C64_MODE, 0x08);
    for &(lo, hi) in &CLAIMS {
        // The cartridge registers have their own tests (C64_MODE pulls
        // the reset, which is an interrupt).
        if lo == 0x04_0000 {
            continue;
        }
        for addr in (lo..=hi).step_by(3) {
            r.wr(addr, 0xff);
            let _ = r.rd(addr);
            r.wr(addr, 0x00);
        }
    }
    for addr in 0x04_2000..0x04_e000u32 {
        r.wr(addr, 0xff);
        let _ = r.rd(addr);
    }
    for reg in 1..16 {
        r.wr(0x04_0000 + reg, 0xff);
    }
    assert_eq!(r.irq.flags, 0);
    assert_eq!(r.irq.level, 0);
    assert_eq!(r.irq.high, 0);
    assert!(!r.irq.line());
    assert_eq!(r.port.next_event(), u64::MAX);
}

#[test]
fn claimed_ranges() {
    for addr in [
        0x02_0000, 0x02_17ff, 0x02_180f, 0x02_4000, 0x02_580f, 0x02_8000, 0x02_8fff, 0x04_0000,
        0x04_2000, 0x04_4fff, 0x04_6000, 0x04_9fff, 0x04_abff, 0x04_cfff, 0x05_0000, 0x05_ffff,
        0x0a_0000, 0x0c_0800, 0x10_0300, 0x10_03ff, 0x18_0000, 0x18_0c00, 0x18_1800, 0x18_5000,
        0x18_cfff, 0x19_0000, 0x19_00ff,
    ] {
        assert!(C64Port::claims(addr), "{addr:06x}");
        // The bus hands the address with or without its upper byte.
        assert!(C64Port::claims(0x1000_0000 | addr));
    }
    for addr in [
        0x00_0000, 0x01_ffff, 0x02_2000, 0x02_9000, 0x03_ffff, 0x04_e000, 0x06_0000, 0x08_0000,
        0x0a_1000, 0x10_0000, 0x10_02ff, 0x10_0400, 0x14_0000, 0x17_ffff, 0x19_0100, 0x20_0000,
    ] {
        assert!(!C64Port::claims(addr), "{addr:06x}");
    }
}

#[test]
fn power_on_and_apply_lines() {
    let mut r = rig();
    r.port.apply_lines(&mut r.c64);
    assert_eq!(r.c64.reset_calls, [true]);
    assert!(!r.c64.stop_request);
    assert_eq!(r.c64.ntsc, Some(false));
    assert_eq!(r.c64.matrix, [0xff; 8]);
    // A changed state, a reset of the FPGA, and the lines again.
    r.wr(C64_MODE, 0x08);
    r.wr(C64_MODE, 0x12);
    r.wr(C64_STOP_MODE, 2);
    r.wr(C64_STOP, 1);
    r.wr(0x18_0001, 0x2b);
    r.wr(0x10_0302, 0x10);
    let mut other = Mock::new();
    r.port.apply_lines(&mut other);
    assert_eq!(other.reset_calls, [false]);
    assert!(other.stop_request && other.ultimax && other.nmi);
    assert_eq!(other.stop_condition, 2);
    assert_eq!(other.ntsc, Some(true));
    assert_eq!(other.matrix[2], 0xef);
    r.port.drive_b.multi_mode = false;
    r.port.reset();
    let mut fresh = C64Port::new();
    fresh.drive_b.multi_mode = false;
    assert_eq!(r.port, fresh);
    assert_eq!(r.port.peek(C64_CLOCK_DETECT), 0x11);
}

/// A port with something in every block.
fn busy_port() -> Rig<Mock> {
    let mut r = rig();
    r.wr(C64_MODE, 0x08);
    r.wr(C64_MODE, 0x12);
    r.wr(C64_STOP_MODE, 2);
    r.wr(C64_STOP, 1);
    for reg in 5..15 {
        r.wr(0x04_0000 + reg, 0x43 + reg as u8);
    }
    for off in 0..0x40 {
        r.wr(0x18_0000 + off, 0x80 | off as u8);
    }
    r.wr(0x18_002e, 1);
    for base in [0x02_0000u32, 0x02_4000] {
        for reg in 0..16 {
            r.wr(base + reg, 0xff);
            r.wr(base + 0x1800 + reg, 0x5a + reg as u8);
        }
        r.wr32(base + 0x1010, 0x00ab_cdef);
    }
    r.wr(0x02_8003, 1);
    r.wr(0x02_800c, 1);
    r.wr(0x02_8844, 0x28);
    r.wr(0x04_4000, 0x47);
    r.wr(0x04_4001, 1);
    r.wr(0x04_4001, 0x8a);
    r.wr(0x04_4005, 2);
    r.wr(0x04_400a, 9);
    r.wr(0x04_400c, 0x34);
    r.wr(0x04_400d, 0x01);
    r.wr(0x04_4900, 0x77);
    r.wr(0x04_8020, 0x03);
    for reg in 0..11 {
        r.wr(0x04_a000 + reg, 0x21 + reg as u8);
    }
    r.wr(0x04_aa10, 0x66);
    r.wr(0x04_c801, 0x99);
    r.wr(0x0a_0000, 0x19);
    r.wr(0x0c_0000, 0x91);
    for i in 0..15 {
        r.wr(0x10_0300 + i, 1 << (i & 7));
    }
    r.wr(0x18_0803, 0x44);
    r.wr(0x18_0c03, 0x55);
    r.wr(0x18_101e, 0x80);
    r.wr(0x18_1800, 0x0c);
    r.wr(0x18_5123, 0x45);
    r.wr(0x19_0029, 0x11);
    r.port.raise_unlock(&mut r.irq);
    r
}

#[test]
fn savestate_round_trip() {
    let r = busy_port();
    let mut w = StateWriter::new();
    r.port.save(&mut w);
    let bytes = w.into_bytes();
    let mut d = C64Port::new();
    d.load(&mut StateReader::new(&bytes)).unwrap();
    assert_eq!(d, r.port);
    // And the copy behaves the same.
    assert_eq!(d.peek(C64_STOP), 3);
    assert_eq!(d.core.cpu_speed(), 0x0d);
    let mut w2 = StateWriter::new();
    d.save(&mut w2);
    assert_eq!(w2.into_bytes(), bytes);
}

#[test]
fn savestate_rejects_bad_data() {
    let r = busy_port();
    let mut w = StateWriter::new();
    r.port.save(&mut w);
    let bytes = w.into_bytes();
    let fresh = C64Port::new();
    // Cut anywhere: an error, never a panic, and the port keeps its
    // state.
    for cut in (0..bytes.len())
        .step_by(61)
        .chain(bytes.len() - 3..bytes.len())
    {
        let mut d = C64Port::new();
        assert!(d.load(&mut StateReader::new(&bytes[..cut])).is_err());
        assert_eq!(d, fresh);
    }
    // A flag that is neither 0 nor 1 (the first byte of the body).
    let mut bad = bytes.clone();
    bad[10] = 7;
    let mut d = C64Port::new();
    assert!(d.load(&mut StateReader::new(&bad)).is_err());
    assert_eq!(d, fresh);
    // Values beyond their width are cut to it, not trusted.
    let mut wide = bytes.clone();
    let stop_mode = 10 + 9;
    assert_eq!(wide[stop_mode], 2);
    wide[stop_mode] = 0xfe;
    d.load(&mut StateReader::new(&wide)).unwrap();
    assert_eq!(d.cart.stop_mode, 2);
    // Another block's chunk is not ours.
    let mut other = StateWriter::new();
    other.chunk(*b"XXXX", 1, |w| w.u8(0));
    let other = other.into_bytes();
    assert!(d.load(&mut StateReader::new(&other)).is_err());
}

// Nothing the firmware can do panics: every claimed address, read,
// written and looked at.
#[test]
fn every_address_is_safe() {
    let mut r = Rig::with(NullC64::default());
    for &(lo, hi) in &CLAIMS {
        for addr in lo..=hi {
            let _ = r.rd(addr);
            r.wr(addr, addr as u8);
            let _ = r.port.peek(addr);
        }
    }
    // And outside of them, should the bus ever ask.
    for addr in [
        0u32,
        0x02_2000,
        0x04_e000,
        0x10_0400,
        0x19_0100,
        0xff_ffff,
        u32::MAX,
    ] {
        r.wr(addr, 0xff);
        assert_eq!(r.rd(addr), 0);
        assert_eq!(r.port.peek(addr), 0);
    }
}

/// The firmware's `start_cartridge` with its boot cartridge, then the
/// cartridge's own switch-off and the firmware's `restoreCart`.
#[test]
fn boot_cartridge_sequence() {
    let mut r = Rig::with(Mock::new());
    // Stop, ULTIMAX for the VIC registers, back to normal.
    r.wr(0x04_0002, 2);
    r.wr(0x04_0001, 1);
    r.wr(0x04_0000, 0x02);
    assert_eq!(r.c64.cart_lines, (true, false));
    r.wr(0x04_0000, 0x00);
    assert_eq!(r.c64.cart_lines, (false, false), "ULTIMAX leaves at once");
    // Reset, the type while it is held, release.
    r.wr(0x04_0000, 0x04);
    r.wr(0x04_0005, 0x00);
    r.wr(0x04_0001, 0);
    r.wr(0x04_0005, 0x41);
    assert_eq!(r.c64.cart_lines, (false, true), "8 KB: /EXROM low");
    r.wr(0x04_0000, 0x08);
    assert_eq!(r.rd(0x04_0006), 1);
    assert_eq!(r.port.cart_lines(), (false, true));
    // The menu's ULTIMAX wins while it is set.
    r.wr(0x04_0000, 0x02);
    assert_eq!(r.port.cart_lines(), (true, false));
    r.wr(0x04_0000, 0x00);
    // $40 to $DFFF switches the cartridge off (here by DMA; the 6510's
    // write takes the machine's path to the same logic).
    r.wr(0x05_dfff, 0x40);
    assert_eq!(r.rd(0x04_0006), 0);
    assert_eq!(r.c64.cart_lines, (false, false));
    // restoreCart: no cartridge configured, the force strobe twice.
    r.wr(0x04_0005, 0x00);
    r.wr(0x04_0006, 2);
    r.wr(0x04_0006, 2);
    assert_eq!(r.rd(0x04_0006), 0);
    // A reset of the C64 latches the type again: none stays off.
    r.port.c64_reset_event(&mut r.irq);
    assert!(!r.port.carts.active());
    // The force strobe never switches a cartridge on; the kill does off.
    r.wr(0x04_0005, 0x41);
    r.wr(0x04_0006, 2);
    assert!(!r.port.carts.active());
    r.port.c64_reset_event(&mut r.irq);
    assert!(r.port.carts.active());
    r.wr(0x04_0006, 1);
    assert!(!r.port.carts.active());
}
