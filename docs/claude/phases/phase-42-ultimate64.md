# Phase 42: Ultimate 64 Elite II / Commodore 64 Ultimate with the real firmware

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 42 | Ultimate 64 Elite II and Commodore 64 Ultimate (`ultimate64` in `machine-c64`, models `elite2` and `c64u`): the board's RISC-V soft CPU (`cpu-rv32`) runs the user's unmodified firmware; the FPGA's blocks are modelled anew (`chips-ultimate`); the C64 inside is mimic's own | open | the user's update file flashes the emulated board, the firmware then starts from that flash and its menu works on the C64's keyboard |

## History and decisions

- 2026-11-23, morning: a first attempt (rebuilt menu and updater, turbo CPU, models u64/c64u) was built and removed again on the user's word; it is in the commits a998f96 and 6fb60ad. The user's aim — trying firmware before flashing a real board — needs the real firmware, which that attempt did not run.
- 2026-11-23, the user: "dann baue das für den elite ii und c64u, also die risc-v variante". The Nios II board (Ultimate 64 mk1) is not part of this phase.
- Golden rule 7 in its wording of the same day: the 1541 Ultimate sources and the UE2-C64U-Emulator's documentation (GPL-3) were read for facts; no code of either was taken, and no `.rs` file of UE2 was opened. Every file read is in `docs/sources.md`.
- Decided by Claude: the board lives in `machine-c64` (module `u64board`), the blocks in a crate of their own; the board's processor runs four clocks of its 100 MHz clock per instruction (25 MIPS); the board's files are `/Board/elite2-flash.bin`, `/Board/c64u-flash.bin` (+ `.reg` for the flash chip's non-volatile registers) and an optional `/Board/chars.bin` in the frontend's file area (`--uci-dir`); the C64 keeps its own ROMs unless `c64roms=firmware`; the update file runs once per machine start (`--option update=FILE`); capability word $34000222 (Elite II) / $24000226 (C64U), so the firmware uses only blocks that work.

## Status (2026-11-23)

Built and checked with the user's update 3.15a (`update_v3.15a.ue2`):
- The updater starts, shows its screen on the C64 (C64 keyboard answers its questions), checks its images, writes the FPGA image, the application, the flash disk's files and the configuration into the emulated S25FL128L, protects the flash and switches the machine off.
- After power-on the boot ROM's step (application record from the flash) starts the firmware: FreeRTOS, all init functions, its tasks; the C64 shows the firmware's welcome page when the ROMs come from the firmware (`c64roms=firmware`); the MENU button opens the firmware's menu (devices SD, Flash, Temp, FTP, WiFi), cursor keys and RETURN work.
- Tests: `cpu-rv32` 27, `chips-ultimate` 155, `machine-c64` 6 for the machine plus the `#[ignore]` test with the user's file (`MIMIC_U64_UPDATE`), all green.

2026-11-23, storage (the user: "mach weiter" after the proposal "USB-Speicher mit einem Ordner auf dem PC dahinter"):
- `chips-ultimate::sdcard`: the SD card's SPI port and an SDHC card in SPI mode (init, single/multi block read and write, CSD/CID, write protect, card change).
- `chips-ultimate::usb`: the nano USB CPU at the level of the protocol the firmware speaks with its blob (HLE), the USB2513 hub as root with 3 ports, a Bulk-Only mass-storage stick per port, hot-plug; capability bit 23 is now set.
- `chips_ultimate::Storage` (in `IoCtx`): the media behind them; `machine-c64::u64board::storage`: the machine's media slots "sd card", "usb 1" … "usb 3", flushed by the machine 2 s after the firmware's last write.
- `media-backends::fat_dir`: a host folder as a FAT32 volume (built at open, file data read lazily, guest changes written back on commit; deletions go to `.mimic-trash`, host-changed files are never overwritten — the guest's version becomes `name (mimic).ext`); `media-backends::block`: `--sd PATH` / `--usb PATH` (image or folder) for both frontends.
- Checked with the firmware 3.15a: a folder as `--usb` appears as "USB0 mimic USB Stick Ready", its subfolder and a D64 inside are browsed; as `--sd` the SD card is "Ready". Not yet checked with the firmware: its writes to a stick or card (covered by the blocks' and the backend's tests).
- Tests: `chips-ultimate` 197, `media-backends` 34, `machine-c64` 7 for the machine.

2026-11-23, drives (the user: mounting/starting D64 files from the SD card did not work):
- Without firmware in the flash the C64 now runs on its own and says on its screen how to install one (before, the board held it in reset: a black screen).
- Drive 8 of `ultimate64` is the board's drive A (`u64board::drives`): power, reset, device number and ROM from the firmware (ROM from DDR), the disk surface straight from the GCR tracks the firmware keeps in DDR (track table in the param RAM; `drives-cbm` 1541 got an external `DiskSurface` mode), written tracks back into DDR with the DIRTY bits, so the firmware writes the image back. The c64u's drive B is `drive9`. `drive8` is refused on this machine.
- Checked with the firmware 3.15a and a D64 on the SD card (folder): "Mount Disk" works, `LOAD"$",8` lists the disk, programs load and run, `SAVE` writes into the D64 in the PC folder.
- "Run Disk" did not work then: the firmware starts it through its boot cartridge (a cartridge image it copies into DDR), and the board's cartridge logic was not built.

2026-11-23, boot cartridge (the user: "mach weiter mit dem boot-modul"):
- `chips-ultimate::c64port::carts` (`CartLogic`): the FPGA's cartridge logic for the types "none" and "normal" (8K/16K, the boot cartridge is `$41`): the type latches while the C64's reset is held, kill and force strobes, the `$DFFF` write that switches the cartridge off; ROM read live from DDR `$03C00000`, RAM at `$00EF0000`. Other type numbers are accepted and stay off.
- `u64board::cart`: the board's cartridge on the C64's expansion port as an `ExpansionCart` (the PLA stays mimic's). `ultimate64` has no cartridge slot of its own any more.
- The board's lines (forced ULTIMAX, the cartridge's GAME/EXROM) now reach the PLA at once on a register write (`C64Side::set_cart_lines`). Before, leaving ULTIMAX took effect a cycle late: the 6510 could run one cycle on the ULTIMAX map, and DMA right after it still saw that map. This is the probable cause of the jam at `$E5D3` seen once (not reproduced, so not proven).
- Checked with the firmware 3.15a from the SD card: "Run Disk" on a D64 with a BASIC program loads through drive A and runs it; "Run" on a PRG (the DMA load path) loads and runs it; opening and leaving the menu three times with debugger pause/resume leaves the C64 intact.
- Tests: `chips-ultimate` 206, `machine-c64` 85 (+ the ignored `installed_firmware_menu` with `MIMIC_U64_FLASH`).

2026-11-23, sound and network:
- The user: with the menu open the last note sounded on. While the board holds the 6510 (C64_STOP) the machine now puts out silence (`Io::mute`); the SID runs on. Not checked against a real board; done on the user's word.
- The user: "use host lan in u64 emulator". Decided by Claude: user-mode NAT through `mimic_core::host` (no packet driver, no administrator rights; the board has no address of its own in the LAN — bridging would need a capture driver and was not asked for in so many words).
- `chips-ultimate::rmii`: the RMII MAC (free/used buffer queues, DMA, filter, level interrupt) and the PHY behind the bit-banged MDIO pins; capability bit 24 set. `chips_ultimate::EtherLink` (in `IoCtx`) is the cable.
- `devices-net::vrouter`: a virtual router between Ethernet frames and `NetBackend`: ARP, DHCP server (board 10.0.2.15, router 10.0.2.2 = the PC itself, DNS 10.0.2.3), DNS with synthetic addresses (198.18.x.y) so connections are opened by name, UDP flows, TCP both ways with its own TCP endpoint, host ports forwarded to the board's servers.
- `u64board::net`: the cable with the router; default forwards 6480→80 (web interface, REST), 6421→21 (FTP), 6423→23 (Telnet), 6464→64 (socket DMA); `--option netfwd=HOST:BOARD,…`; `--net off` unplugs the cable.
- Checked with the firmware 3.15a: it gets its address by DHCP; from the PC `http://127.0.0.1:6480/` gives the web interface, `/v1/info` the REST answer (product, firmware 3.15a, hostname, MAC), port 6421 the FTP greeting ("IP = 10.0.2.15"), port 6423 the Telnet menu. The firmware's outgoing connections: confirmed working by the user the same day ("lan nach aussen funktioniert"). Not checked: FTP data connections through the forward.
- Tests: `chips-ultimate` 245, `devices-net` 45, `machine-c64` 88.

2026-11-23, "Run Update" from the firmware's menu, the WiFi module, SID tunes:
- "Run Update" (the user: "mach weiter mit run update"): with `update_v3.15a.cfw` on the SD card the firmware's context menu offers "Run Update", loads the file and starts the updater; it checks its images, asks "Reformat Flash Disk?", "About to update. Continue?", "Reset Configuration?", writes the flash disk's files, the FPGA image and the application, protects the flash and switches the board off. This is the gate's path.
- The WiFi module: the updater flashes the ESP32 when its software differs from the one it carries. `chips-ultimate::wifi` now has the ESP32's ROM serial loader as far as the updater uses it (greeting, sync, attach, parameters, flash begin/data, read register); after the flash the module reports the version found in what was flashed. The machine keeps that version in `/Board/<model>-esp32.ver`. An update file started with `--option update=` no longer sets the module's version beforehand, so that path flashes the module too ("Flashing ESP32 Success!", checked with the user's 3.15a in the ignored test).
- SID tunes (the user: tunes picked at random through the firmware's Assembly 64 search sometimes crashed): the board's stop held the 6510 through `stall_off_bus`, which is for a read stopped by RDY and carries on interrupt polls with the flags saved by that read; with the stop landing in mid instruction and the IRQ line low, an IRQ was taken with I set. The firmware's SID player waits for its DMA load inside an IRQ handler with `$01` = `$34`, so the IRQ went through the RAM vector to `$0000` and the CPU jammed. Now the 6510 stands and polls nothing while the board holds it (NMI edges are latched). Before: 2 of 9 plays through Assembly 64 crashed; after: 40 of 40 ran (two HVSC queries of 20, PSID and RSID). "Ran" = no jam, the CPU in the player's or the tune's code; sound was not recorded.
- Tests: `chips-ultimate` 256, `machine-c64` 89 (+3 ignored).

Open (each a later step; the blocks are T0 in `chips-ultimate` and say what they need):
- The overlay menu (HDMI picture): built and drawn over a stretched C64 picture, not yet seen with the firmware set to "overlay" mode; the firmware's freeze menu on the C64 screen works.
- Storage, further: USB HID keyboard/mouse; loading other cores from a stick.
- SID playing, further: sub-tunes, 2SID/3SID routing, MUS files; the stop conditions (bad line, write-then-read) are not modelled, the stop is at once.
- The ESP32 loader: stub upload, MD5 and compressed flashing are refused (the updater uses none).
- The other cartridge types (CRT files through the firmware, freezers, EasyFlash, Ocean, REU/GeoRAM, the KERNAL replacement), the `$DFFE`/`$DFFF` trigger and sync, SERVE_CONTROL; the command interface, sampler, SIDs, turbo, tape, ACIA.
- Drives: 1571/1581 (WD177x), RAMMAP, drive sounds, bus IDs above 11; drive B untested with the C64U's firmware.
- Headless: `polydbg.py panel MENU press`/`release` opened the menu in the first sessions and not in a later one; a timed press (one `input` request: control 5 on, off 200 000 cycles later) works. Not looked into.
- Network, further: FTP data connections (passive mode ports are not forwarded), the firmware's UDP identify (broadcast), bridged mode, WiFi beyond the identity stub, the ESP32 serial loader.
- The C64U's real update file (not at hand), speed (idle skip; a full C64 frame with the board costs about 12 ms in release builds).
