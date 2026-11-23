# Phase 40: Nintendo Game Boy

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 40 | Nintendo Game Boy (`machine-gameboy`, added 2026-11-20 on the user's request, begun 2026-11-21: CPU, cartridges, picture and sound unit, timer and the machine built for the DMG and the Pocket; blargg's CPU and timing tests, dmg-acid2 and the mooneye suite pass; the Color and real games open): Game Boy (DMG), Game Boy Pocket and Game Boy Color as models of one machine; the Sharp SM83 CPU (new `cpu-sm83`), PPU (background, window, 40 sprites, the mode timing with its STAT interrupts; the Color's palettes, VRAM bank and HDMA), APU (two pulse channels, wave, noise) and timer in `chips-nintendo`, cartridge mappers MBC1, MBC2, MBC3 with its clock and MBC5 with battery RAM (`.sav`), joypad, serial port with a link cable between two instances as a later step; boot ROM optional (user-supplied), without it the machine starts in the state the boot ROM leaves; whether the Super Game Boy and the Game Boy Advance follow is decided later | 6 wk | the CPU and timing test ROMs (blargg's `cpu_instrs` and `instr_timing`, the mooneye acceptance tests for timer and PPU) pass, a PPU test picture (`dmg-acid2`, `cgb-acid2`) matches its reference, and commercial games of the user play with sound |

## Status

2026-11-20: added to the list on the user's request ("nehme noch den gameboy auf in die phasen liste der maschinen"). The scope in the table (DMG, Pocket and Color as models; the mappers; the test ROMs of the gate) is Claude's proposal, not discussed with the user.

2026-11-21: **begun** ("starte mit der gameboy implementierung"). Own choices, not asked: the DMG and the Pocket first, the Color as a later step; the order CPU, cartridges, picture unit and timer, machine, sound unit; the test programs fetched one by one (blargg's set and dmg-acid2, not the whole fetch script).

Built the same day:

- **`cpu-sm83`** (new crate): the Sharp SM83. [`Sm83::step`] runs an instruction and reports every machine cycle (four clocks) to the machine's `MBus` as a read, a write or an idle cycle, so the machine advances its chips inside these calls and every access lands on its cycle. Interrupt dispatch in five cycles with the vector chosen after the first push, EI's delay, HALT and the HALT bug, STOP (waits for the joypad, or a speed switch if the machine says so), the eleven opcodes without an instruction lock the processor up. 12 unit tests (every opcode's cycle count, flags, DAA, interrupts, HALT, savestate, disassembly).
- **`devices-cartridge::gameboy`**: the header, no controller, MBC1 (both modes), MBC2, MBC3 with its clock (on emulated time), MBC5; cartridge RAM with the battery. 6 unit tests.
- **`chips-nintendo`**: `gbppu` (the DMG's picture unit: modes and their timing, STAT's interrupt line with its blocking, background, window with its own line counter, ten objects a line with the DMG's priority, drawn a pixel a dot from the registers as they are; 6 tests), `gbtimer` (the divider and TIMA with the falling-edge detector, the late reload, the writes that count once more; 5 tests), `gbapu` (two pulse channels with the sweep, the wave channel, noise; frame sequencer, lengths with the extra clock, envelopes, the registers' read masks, power off; stereo with the output's high pass; 6 tests).
- **`machine-gameboy`** (`--machine gameboy`, models `dmg` and `pocket`): the memory map, the buttons (a digital pad on port 0 or the keyboard's machine keys 0–3), the serial port without a partner, OAM DMA, the optional boot ROM (role `gameboy-boot`; without it the state the boot ROM leaves), the battery RAM as a second media slot (`.srm` beside the cartridge, as for the other consoles), five memory spaces (`cpu`, `vram`, `oam`, `rom`, `cartram`), savestates. A test cartridge built in code (`demo`) for tests without ROM files. 10 unit tests.
- Frontends: registered in `mimic-machines`; `.gb` is a cartridge for the desktop's file dialog; the input mapper's default profile (pad on port 0, cursor keys as the cross, X/Z/right Shift/Enter as A/B/Select/Start); the ROM folder `gameboy` (`gb`, `dmg`) for the boot ROM.

**Test programs** (`tests/tests/gameboy_tests.rs`, `testdata/gb`, not committed):

| Suite | Result |
| --- | --- |
| blargg `cpu_instrs` (11 single tests) | 11/11 |
| blargg `instr_timing`, `mem_timing` (3), `mem_timing-2` (3), `halt_bug` | 8/8 |
| blargg `dmg_sound` (12 single tests) | 9/12 |
| blargg `oam_bug` (8 single tests) | 3/8 |
| dmg-acid2 against its reference picture | 0 pixels differ |
| mooneye `acceptance` and `emulator-only`, DMG | 94/94 (first run 81/94) |
| mooneye `acceptance` and `emulator-only`, Pocket | 94/94 |

The three sound tests that fail (09, 10, 12) test what the DMG's wave RAM shows and takes while its channel plays (only in the two clocks the channel reads it) and what a trigger while playing does to it; not modelled.

2026-11-21, later: **the mooneye test suite** ("mache mit mooneye weiter"; with the two orders Claude offered, the user chose this one before the Color). Fetched as its author's prebuilt archive; the runner takes every test of `acceptance` and `emulator-only` whose name holds for the model and reads the verdict from the registers at the final `LD B,B` (the tests write their bytes to SB without starting a transfer, so the serial port shows nothing). First run: DMG 81/94. What the thirteen failures showed, and what was changed:

- **The picture unit as the processor sees it** (`ppu/intr_2_mode0_timing`, `intr_2_mode3_timing`, `intr_2_oam_ok_timing`, `lcdon_timing`, `lcdon_write_timing`, `stat_lyc_onoff`, `vblank_stat_intr`): a mode's interrupt comes at its first dot, but a read there still gets the old mode. `gbppu` now keeps the mode one dot ago for STAT and for the locks' release; reading closes with the unit's own mode, writing follows the mode as read (the object table takes a write at dot 80). LY = LYC is a latch: false in a line's first four dots, kept while the display is off, compared at once when it is switched on. The first line after switching on starts four dots in. Mode 2's interrupt source also fires when line 144 begins.
- **Mode 3 with objects** (`ppu/intr_2_mode0_timing_sprites`): six dots an object and up to five more for the first object on a background tile, by its place in the tile (`GbPpu::object_dots`), instead of a flat six.
- **The state after the boot ROM** (`boot_div`, `boot_hwio`, `boot_regs-mgb`, `serial/boot_sclk_align`): the divider at `$ABC8` (was `$ABCC`: the first read is one machine cycle later than assumed), both joypad groups selected (P1 reads `$CF`), the Pocket's A = `$FF`. The serial port's clock is bit 8 of the timer's counter, not a count from the write of SC.
- **TAC** (`timer/rapid_toggle`): a write takes effect before the cycle's last clock, so switching the timer on in the cycle its bit falls counts.
- **MBC1 multicarts** (`mbc1/multicart_rom_8Mb`): told by the logo repeated at bank `$10` of a 1 MB image.

As a side effect blargg's `oam_bug/1-lcd_sync` passes ("starts too early" before); the suite's other tests that need no corruption bug pass too and are in the runner now. Nothing that passed before fails.

Own choice, not asked: mooneye's newest build (2026-07-14) rather than an older, more widely used one.

## Open

- **The gate:** commercial games of the user (none at hand yet: the folder `applications\mimic` holds no `.gb` file).
- Mooneye's `madness/mgb_oam_dma_halt_sprites` and `manual-only/sprite_priority` (pictures to judge by eye) are not run; `misc` is for the Color and the Advance.
- The beam's place after the boot ROM is line 0, dot 0 by assumption: `boot_hwio` passes with it, but pins it only to a range. A boot ROM of the user would settle it.
- The Game Boy Color (model `color`: palettes, the second VRAM bank, HDMA, the speed switch, 32 KB of work RAM).
- Picture unit: the pixel pipeline's own delays (a write inside mode 3 lands some dots early; the objects' and the window's stops are in mode 3's length but not in where the pixels fall), the window's part in mode 3's length, the object table's corruption bug (five of blargg's `oam_bug`).
- Sound: the wave RAM while its channel plays; the Pocket's and Color's differences.
- The serial port between two machines (link cable); the Super Game Boy; MBC6/7, HuC1/3, the camera; MBC3's clock is not kept between sessions (the battery file holds the RAM only).
- Debugger: no graphics views (`gfx_views`) for tiles, maps and objects yet; `check_interrupt` is not called (interrupt breakpoints do not hit).
- Not tried by hand: the desktop and web frontends (only `mimic-headless`); the README's picture is dmg-acid2.
- The user guide's PDFs are not rebuilt.
