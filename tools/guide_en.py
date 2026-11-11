"""English text of the mimic user guide. Built by tools/user_guide.py;
keep it in step with guide_de.py, the frontends' --help texts and the web
page. Part I explains how to use the emulator, Part II is the reference."""

from guide_lib import bullets, code, h1, h2, h3, note, p, part, table  # noqa: F401

part("Part I — Using mimic", "Installation, ROMs, the desktop emulator, the machines, media, "
     "monitors, live preview and debugging")

# ---- 1 introduction ----------------------------------------------------------

h1("Introduction")
p("""mimic is a cycle-accurate emulator for classic home computers and game
consoles, built on one shared core. Every machine gets the same remote
debugger and the same live preview stream automatically: you can watch a
machine in the browser, halt it at a breakpoint, inspect its chips and step
through its code, whichever machine it is.""")
p("""This guide describes how to run the emulator: building it, supplying
ROMs, starting machines in the desktop window or on the command line,
inserting disks, tapes and cartridges, using keyboards, joysticks, gamepads
and mice, choosing a monitor look, and connecting the browser preview and
debugging tools.""")

h2("Supported machines")
table(["Machine id", "Machine", "Models (`--model`)", "Video (`--videoformat`)"], [
    ["`c64`", "Commodore 64", "–", "`pal` (default), `ntsc`"],
    ["`vic20`", "Commodore VIC-20", "–", "`pal` (default), `ntsc`"],
    ["`pet`", "Commodore PET", "`4032` (default), `2001`, `3032`, `8032`", "–"],
    ["`c16`, `plus4`", "Commodore 16, Plus/4", "–", "`pal` (default), `ntsc`"],
    ["`c128`", "Commodore 128 (40 and 80 columns, Z80)", "–", "`pal` (default), `ntsc`"],
    ["`atari8`", "Atari 400/800/800XL/65XE/130XE",
     "`800xl` (default), `65xe`, `130xe`, `800`, `400`", "`pal` (default), `ntsc`"],
    ["`atari2600`", "Atari 2600 (VCS)", "–", "`auto` (default: NTSC or PAL from the "
     "program's field length), `ntsc`, `pal`"],
    ["`cpc`", "Amstrad CPC", "`6128` (default), `664`, `464`", "–"],
    ["`amiga`", "Commodore Amiga 500/1000/2000 (OCS), 500+ (ECS), 1200 (AGA)",
     "`500` (default), `1000`, `2000`, `500plus`, `1200`", "`pal` (default), `ntsc`"],
    ["`zx81`", "Sinclair ZX81 / Timex Sinclair 1000", "`zx81` (default, PAL), `ts1000` (NTSC)", "–"],
    ["`spectrum`", "Sinclair ZX Spectrum", "`48k` (default), `128k`, `plus2`", "–"],
    ["`coleco`", "ColecoVision", "–", "`ntsc` (default), `pal`"],
    ["`nes`", "Nintendo Entertainment System / Famicom", "–", "`ntsc` (default), `pal`"],
    ["`snes`", "Super Nintendo / Super Famicom", "–", "`ntsc` (default), `pal`"],
    ["`vectrex`", "Vectrex (vector display)", "`vectrex`", "–"],
    ["`imsai`", "IMSAI 8080 (front panel; terminal on F12)", "`8080`", "–"],
    ["`logophor16`", "Logophor 16, mimic's own steampunk computer (built-in ROM; profile `logophor16` boots the desktop Panorama)", "`16`", "–"],
    ["`archimedes`", "Acorn Archimedes", "`a310` (default), `a305`, `a440`, `a3000`", "–"],
    ["`apple2`", "Apple II, II+, IIe", "`iie` (enhanced IIe, default), `iie-unenhanced`, "
     "`iiplus`, `ii`", "–"],
    ["`mac`", "Apple Macintosh 128K, 512K, Plus, SE", "`plus` (default), `128k`, `512k`, `se`", "–"],
    ["`sim6502`", "6502 reference machine for tests", "`default`", "–"],
], [0.14, 0.32, 0.32, 0.22])
p("`mimic-headless --list` prints the same list from the program itself.")

h2("The two programs")
table(["Program", "Purpose"], [
    ["`mimic-desktop`", "The emulator with a window, sound, keyboard, gamepads and mouse. "
     "It can also serve the debugger, the VICE monitor and the browser preview."],
    ["`mimic-headless`", "No window and no sound: run a number of frames and save a "
     "picture, record sound to a WAV file, write savestates, or serve the debugger "
     "and the preview. For scripts, tests and remote use."],
], [0.22, 0.78])

# ---- 2 installation ----------------------------------------------------------

h1("Installation and quick start")
h2("Requirements")
bullets(
    "**Rust** (stable toolchain) with `cargo` — install it from rustup.rs.",
    "**Linux only:** the packages `libasound2-dev` and `libudev-dev` for the "
    "desktop program (sound and gamepads).",
    "**Optional:** Python 3 for the debug client `tools/polydbg.py` (standard "
    "library only).",
)
h2("Building")
p("Always build the optimised (release) version; a debug build is far "
  "slower than real time.")
code("""cargo build --release -p emu-desktop     # the desktop emulator
cargo build --release -p emu-headless    # the command-line frontend""")
p("""The programs end up in `target/release/` — on Windows
`mimic-desktop.exe` and `mimic-headless.exe`. They need no other files and
can be copied anywhere; ROMs live in `.mimic/roms` in your user folder or
are found through `--rom-dir` or the environment variable `EMU_ROM_DIR`.""")
note("""Older builds left files called `emu-desktop.exe` and `emu-headless.exe`
in `target/release/`. They are outdated (they do not know the newer
machines); use `mimic-desktop.exe` and `mimic-headless.exe`, or start
through `cargo run` as shown below, which always builds the current
version first.""", "Watch out")
h2("Starting a machine")
p("Two equivalent ways to start the desktop emulator:")
code("""cargo run --release -p emu-desktop -- --machine c64
.\\target\\release\\mimic-desktop.exe --machine c64""")
p("Everything after `--` goes to the emulator. Some first steps:")
code("""# C64 with a disk, loaded and started automatically
mimic-desktop --machine c64 --disk game.d64 --autostart

# Amiga 500 booting Workbench 1.3 from an ADF
mimic-desktop --machine amiga --disk workbench13.adf

# NES and SNES need no system ROM
mimic-desktop --machine nes --cart game.nes
mimic-desktop --machine snes --cart game.sfc

# Vectrex with a cartridge
mimic-desktop --machine vectrex --cart "Scramble (1982).vec\"""")
p("`mimic-desktop --help` lists every option with the key assignments.")
note("""On Windows `mimic-desktop` is a window program: a double-click or a
shortcut opens no console window next to the emulator. Started from a
terminal, its messages (gamepads, servers, errors, `--help`) go to that
terminal — after the prompt, as the shell does not wait for a window
program. Started without a terminal, a failed start and `--help` show a
dialog instead. `mimic-headless` is a console program as before.""")

# ---- 3 roms --------------------------------------------------------------

h1("ROMs")
p("""Most machines need their original system ROMs (KERNAL, BASIC,
Kickstart, operating system, BIOS). They are copyrighted and therefore
**not** part of mimic: you supply them from your own machines. Put them in a
folder: `.mimic/roms` in your user folder (on Windows
`%USERPROFILE%\\.mimic\\roms`), or another one passed with `--rom-dir DIR` or
set once in `EMU_ROM_DIR`.""")
h2("How ROMs are found")
bullets(
    "mimic looks in the machine's subfolder first (for example `~/.mimic/roms/c64/`), "
    "then in `~/.mimic/roms/drives/`, then in the folder itself.",
    "A file is recognised by the **start of its name** (case does not matter; "
    "`.` and `_` count as `-`) and by its **exact size**.",
    "If a ROM is missing, the error message names the role, the size and the "
    "accepted names.",
    "With `mimic-headless` a single ROM can also be given directly: "
    "`--rom kernal=path/kernal.bin`.",
)
h2("ROM table")
table(["Folder", "Machine", "ROMs (example names and sizes)"], [
    ["`c64/`", "C64", "`kernal` (8 KB), `basic` (8 KB), `chargen` or `characters` (4 KB)"],
    ["`vic20/`", "VIC-20", "KERNAL, BASIC, character ROM"],
    ["`pet/`", "PET", "BASIC, editor, KERNAL and character ROM for each model"],
    ["`plus4/`", "C16, Plus/4", "KERNAL, BASIC; optional 3-plus-1"],
    ["`c128/`", "C128", "KERNAL, BASIC, character ROM; optional function ROM"],
    ["`atari/`", "Atari 8-bit", "XL OS `atarixl.rom` (16 KB), BASIC `ataribas.rom` (8 KB); "
     "for the 400/800 OS-B `atariosb.rom` (10 KB)"],
    ["`cpc/`", "Amstrad CPC", "`os464`/`basic1-0`, `os664`/`basic664`, `os6128`/`basic1-1` "
     "(16 KB each) or 32 KB combined files; `amsdos`, optional `parados`"],
    ["`amiga/`", "Amiga", "Kickstart images of 256 or 512 KB, e.g. `Kickstart 1.3.rom`, "
     "`kick13.rom`; one per version (1.2, 1.3, 2.0, 3.0, 3.1), chosen with "
     "`--option kickstart=`; optional A1000 bootstrap ROM (8 KB)"],
    ["`zx81/`", "ZX81", "`zx81.rom` (8 KB)"],
    ["`spectrum/`", "ZX Spectrum", "`Spec48.rom` (16 KB), `spec128.rom`, `specp2.rom` (32 KB each); "
     "optional `mf128.rom` (Multiface 128), `specif1.rom` (Interface 1)"],
    ["`coleco/`", "ColecoVision", "`coleco.rom` (8 KB BIOS)"],
    ["`snes/`", "SNES", "optional: the 64-byte sound boot ROM `spc700.rom`"],
    ["`vectrex/`", "Vectrex", "the 8 KB system ROM (executive and Mine Storm); a 4 KB "
     "image of the executive alone also works, see the Vectrex section"],
    ["`apple2/` (or `apple/`)", "Apple II", "files named by Apple part number: II "
     "341-0001…0004, II+ 341-0011…0015 and 341-0020, IIe 342-0135 (CD) and 342-0134 "
     "(EF), enhanced IIe 342-0304 and 342-0303; character ROMs 341-0036, 342-0133, "
     "342-0265; Disk II boot PROM 341-0027 and sequencer PROM 341-0028"],
    ["`mac/`", "Macintosh", "`mac128k.rom` (64 KB), `mac512k.rom` (64 KB), `macplus.rom` "
     "(128 KB), `macse.rom` (256 KB), or each ROM's two chips (Apple part numbers "
     "342-0220/0221, 342-0341/0342, 342-0701/0702)"],
    ["`drives/`", "Disk drives", "`dos1541`, `dos1571`, `dos1581`, `dos1551` (16 KB, 318008-01)"],
], [0.14, 0.16, 0.70])
p("The Atari 2600 and the NES have no system ROM: the program is on the "
  "cartridge.")
note("""Many machines accept `--option roms=stub`: a tiny built-in program
instead of the real ROMs. It is meant for tests (it shows a test screen or
starts simple cartridges); real software needs the original ROMs.""")

# ---- 4 desktop -------------------------------------------------------------

h1("The desktop emulator")
h2("Command-line options")
p("""Every flag is listed in Part II (chapters 12 to 14). The ones you need
first: `--machine ID` picks the machine, `--rom-dir DIR` the ROM folder,
`--disk`, `--tape`, `--cart` and `--prg` the media, `--autostart` runs the
program after boot, `--option KEY=VALUE` configures the machine (chapter 15)
and `--profile NAME` starts a saved setup (chapter 16).""")
h2("Hotkeys")
p("""The Ctrl hotkeys take the left Ctrl key. The right Ctrl and AltGr
reach the machine, so a Ctrl or SYMBOL SHIFT on the emulated keyboard
works with any letter.""")
table(["Key", "Action"], [
    ["Shift+Alt+Left", "open or close the console panel at the left edge"],
    ["Shift+Alt+Right", "open or close the media panel at the right edge: mount, eject and save disks, tapes and cartridges; the tape keys"],
    ["Ctrl+Space", "show or hide the status footer"],
    ["Ctrl+D", "put the next disk of the `--disk` list into the first drive"],
    ["Ctrl+I", "Amiga: switch the deinterlacing between weave and bob"],
    ["Ctrl+R (held)", "run backwards at twice real speed through the last frames; "
     "on release the machine runs on from there"],
    ["F12", "switch between the machine's screens (C128: 40-column VIC and 80-column VDC)"],
    ["Middle mouse button", "release a captured mouse"],
], [0.28, 0.72])
p("Dropping an image file onto the window inserts it into the first drive "
  "(disks) or the cartridge slot (`.vec`, `.crt`, `.car`, `.a26`).")

h2("The console panel")
p("""At the left of the picture, a panel shows the buttons and switches of
the machine's case one under the other (Shift+Alt+Left or a click on its
handle opens and closes it): POWER and RESET, the Atari 2600 console switches, START/SELECT/OPTION
of the 800XL, RESTORE of the C64 and VIC-20, 40/80 and CAPS LOCK of the
C128, and so on. A button is pressed for as long as you hold the mouse
button on it; clicking a switch position moves the switch.""")
bullets(
    "**PAUSE** (at the top) halts the whole machine and lets it run again. It is "
    "lit while the machine stands — also when a debugger halted it — and the "
    "title bar shows where.",
    "**JOY KEYS** chooses the keys that play the joystick: CURSOR (cursor keys, "
    "Space fires), NUMPAD (8/2/4/6, fire on 0 or right Ctrl) or OFF (gamepad "
    "only).",
    "**JOY PORT** (machines with several joystick ports) moves the keyboard "
    "joystick and the gamepad to another port.",
    "**PAD FIRE** chooses the gamepad button that fires (SOUTH is A on an Xbox "
    "pad); **SAVE MAPPING** keeps JOY KEYS, JOY PORT and PAD FIRE for the "
    "machine, so the next start uses them (`--input FILE.toml` loads another).",
    "**Cheats** (from `--cheat`, the cheat database or the debugger) appear as "
    "OFF/ON switches.",
)

h2("Start profiles")
p("""A **start profile** stores a whole machine setup under a name: the
machine and model, its options (memory, drives, slots, port devices), the
ROMs, the disks and other media, the monitor, window and joystick settings
and the debugger and preview servers. All profiles live in one YAML file,
`profiles.yaml`, looked for in the working folder and then in the user's
configuration folder (`%APPDATA%\\mimic` on Windows, `~/.config/mimic`
elsewhere); `--profiles FILE` names another. Its `settings` part holds
what applies to every profile: the ROM folder (`roms`), where snapshots go
(`snapshots`) and the folder relative disk paths count from (`media`).
`docs/profiles.example.yaml` shows every key.""")
table(["Command", "Effect"], [
    ["`mimic-desktop --profile mac-se-755`", "start a profile; flags given as well override it "
     "(`--profile c64 --videoformat ntsc`)"],
    ["`mimic-desktop`", "no machine and no profile given: the window lists the profiles, "
     "a click starts one"],
    ["`mimic-desktop --list-profiles`", "list the profiles"],
    ["`... --save-profile NAME`", "store the setup of this command line (with a profile, "
     "its values too) as a profile; a new name is added at the end of the file, "
     "replacing an existing one rewrites the file"],
], [0.42, 0.58])
p("""`mimic-headless` takes the same profile flags (without the window
settings); the browser frontend reads a profile file the page loads.""")

h2("The status footer")
p("""Ctrl+Space shows a status line below the picture. On the left is a
**speed control**: click a step or drag the knob for 10 %, 25 %, 50 %,
100 %, 150 %, 200 %, 400 % or 800 % of real time (sound is muted away from
100 %). At 100 % the emulator runs as fast as it can, muted, while a disk
drive works (LED or motor, e.g. during LOAD) and the machine has been silent
with a still picture for a second, and only until the loaded program first
plays sound or an animation (again after a disk change), so a demo that
loads its next part while it runs keeps its pace; the footer
then shows "warp: loading". `--warp-load off` turns that off. Next to it the **snapshot** button saves the picture as a PNG file
named after the machine (`mac-001.png`, `mac-002.png` …) in the `snapshots`
folder of the profile file's settings, else in the folder the emulator was
started from; the footer shows the file name for a few seconds. It then shows the emulated time since power-on (m:ss.t) with the
frame number, the mouse position in picture pixels, the joystick directions
and the drives' activity and errors.""")

h2("Window and picture")
bullets(
    "The window opens at the largest scale up to `--scale` that fits on the "
    "screen and can be resized freely.",
    "The display runs in step with the monitor when its refresh rate matches the "
    "machine's (50 or 60 Hz); otherwise the emulator follows the wall clock. The "
    "console prints which mode is used.",
    "The window title shows the machine, the inserted disk and, while paused, "
    "where the machine stopped.",
)

h2("Joysticks and gamepads")
p("""Every machine with joystick ports gets a joystick played by the
keyboard and by the first gamepad. The default port is the machine's game
port: port 2 on the C64 and C128, port 1 on the VIC-20, C16/Plus/4, the
Ataris, the CPC and the consoles, game port 1 on the Amiga (port 0 holds
the mouse).""")
h3("Keyboard joystick")
table(["Setting", "Keys", "Default on"], [
    ["CURSOR", "cursor keys, Space fires",
     "Atari 2600, ColecoVision, NES, SNES, Vectrex (machines without a keyboard)"],
    ["NUMPAD", "numeric keypad 8/2/4/6, fire on 0 or right Ctrl", "the computers"],
    ["OFF", "gamepad only", "Amstrad CPC (its keypad is the function keys)"],
], [0.14, 0.40, 0.46])
h3("Gamepads (Xbox 360 and others)")
p("""Gamepads are read through the operating system's game controller
interface: on Windows Windows.Gaming.Input (Xbox 360, Xbox One and
compatible pads), on Linux evdev, on macOS IOKit.
The desktop prints the pads it finds when it starts and whenever one is
plugged in or out:""")
code("""gamepad 0 connected: Xbox 360 Controller for Windows
gamepad 1 connected: HID-compliant device""")
p("""**Gamepad 0** plays the joystick port. Buttons are named by position
(South = A on an Xbox pad, East = B, West = X, North = Y):""")
table(["Gamepad", "Joystick and controllers"], [
    ["D-pad, left stick", "directions (the stick is digital; on the Vectrex it is analog, "
     "see below)"],
    ["South (A)", "fire / button 1"],
    ["East (B)", "second fire button (NES B, SNES A, Vectrex button 2 …)"],
    ["West (X), North (Y)", "further buttons (SNES Y and X, Vectrex buttons 3 and 4)"],
    ["Shoulder buttons", "SNES L and R"],
    ["Back / Start", "Select / Start (NES, SNES)"],
], [0.30, 0.70])
note("""Hot-plugging is fine: a pad can be connected while the emulator runs.
If your pad shows up with a number other than 0, unplug other game
devices or start with the pad already connected.""")

h2("Mouse, paddles and light guns")
p("""When a port holds a mouse, paddles, a steering wheel or a light gun, a
click into the window captures the host mouse for the machine; the middle
mouse button releases it again. Examples:""")
bullets(
    "C64: `--option port1=mouse1351` (1351 mouse) or `port1=paddles`.",
    "Amiga: the mouse sits in game port 0 by default.",
    "CPC: `--option joy1=amx` (AMX mouse).",
    "Atari 2600: `--option port1=paddles` or `port1=driving` — the mouse turns paddle 0 "
    "or the wheel.",
    "NES: `--option port2=zapper` (aim with the mouse, left button fires) or "
    "`port2=arkanoid` (the mouse turns the knob).",
    "SNES: `--option port2=superscope` (left button = trigger, right = cursor) or "
    "`port1=mouse`.",
    "ColecoVision: `superaction` (Speed Roller), `driving` (wheel, left button = gas), "
    "`roller` (trackball).",
)

h2("Changing disks")
p("""`--disk` can be given several times. The disks go into the drives in
order (Amiga: DF0:, DF1: … with `--option drives=N`); the rest form a swap
list for the first drive. **Ctrl+D** inserts the next disk of that list,
and an image dropped onto the window goes into the first drive as well. As
when swapping by hand, the drive stays empty for about two seconds before
the new disk goes in: programs that only look at the drive now and then
notice the change only if they find it empty once. The title bar shows the
disk in the drive.""")
code("""# a demo on two disks with one drive: press Ctrl+D when it asks for disk B
mimic-desktop --machine amiga --disk demo-a.adf --disk demo-b.adf""")

# ---- 5 machines ----------------------------------------------------------------

h1("The machines")
p("""Keyboards are mapped by **position** on a PC keyboard (US layout), not
by the printed labels: the key where the original machine has a key plays
that key. Options are set with `--option KEY=VALUE` and can be repeated.""")

h2("Commodore 64")
table(["Option", "Values"], [
    ["`sid`", "`6581` or `8580`"],
    ["`cia`", "`6526` or `6526a`"],
    ["`drive8`", "`1541` (true drive emulation), `1571`, `1581`, `virtual` (fast, "
     "KERNAL load/save only), `none`"],
    ["`port1`, `port2`", "`joystick`, `paddles`, `mouse1351`, `none`"],
    ["`autostart`", "`on`: load and run the disk's first program"],
    ["`fastload`", "`on` (with `drive8=virtual`): the KERNAL's serial routines are served "
     "by the virtual drive without bus time; LOAD, SAVE and OPEN/GET# take no time on the bus"],
    ["`uci`", "`on`: the Ultimate 64 command interface at $DF1C-$DF1F (see below)"],
    ["`reu`", "a RAM Expansion Unit: `128k` (1700), `256k` (1764), `512k` (1750), "
     "`1m`, `2m`, `4m`, `8m`, `16m`; `off` (default)"],
    ["`userport`", "`wic64`: a WiC64 WLAN adapter in the user port; `none` (default)"],
    ["`net`", "`off`: no network for the UCI and the WiC64, even if the frontend has one"],
    ["`sd2iec`", "`on`: an SD2IEC on the serial bus (see below); `sd2iec_device` = 4–30 "
     "(default 9); `drive8=sd2iec` puts it in place of drive 8"],
], [0.22, 0.78])
p("""Keys: Esc = RUN/STOP, Tab = CTRL, left Ctrl = C=, Insert = £, Delete =
↑, Page Up = RESTORE, Home = CLR/HOME, F2/F4/F6/F8 = shifted F1/F3/F5/F7.
The joystick is in port 2.""")
p("""A **folder** can be used as a disk with the virtual drive:
`--option drive8=virtual --disk ./myprograms`.""")
h3("Network: Ultimate command interface and WiC64")
p("""**Ultimate command interface** (`--option uci=on`): the four registers
of the Ultimate 64 / 1541 Ultimate II+ command interface at $DF1C-$DF1F.
The DOS targets 1 and 2 work on a folder of your computer (`--uci-dir`,
default `.mimic/ultimate` in your user folder): open, read, write, seek, file info, delete, rename,
copy, change, create and list directories, the time, REU data to and from
the open file, and mounting disk images from the folder in drive 8
(mount, unmount, swap to the next disk of a set like game-1.d64,
game-2.d64). Paths can never leave the folder. The real Ultimate shows
its SD card and USB sticks as `/SD`, `/Usb0` … in the root; `--uci-mount
Usb0=DIR` (repeatable) hangs a folder of your computer in as such a
directory. With `uci=on` the folders are created at start. The network target 3 opens
TCP and UDP sockets (and a TCP listener on 127.0.0.1). The control target 4
reboots the C64, loads and saves REU images, saves the RAM, erases
EasyFlash sectors, reports the SID and drive 8, switches drive 8 off and on
and reads or changes the palette (freezing is not offered). The Software
IEC target 5 gives KERNAL replacements direct file access: LOAD and SAVE by
DMA, OPEN/CHKIN/CHKOUT/CLOSE, partitions on sub-folders, `LOAD"$"` as a
directory listing. The HTTP target 6 sends requests with any verb and
header fields and JSON, URL-encoded or binary bodies, and lets programs
query the JSON answer by path (`http:` only). A cartridge can stay plugged
in; the interface takes only its four registers.""")
p("""**RAM Expansion Unit** (`--option reu=512k`): a Commodore REU with
the REC 8726 at $DF00 (all of I/O2 but the command interface's four
registers). Its DMA stops the CPU, one byte a cycle; GEOS and REU-aware
programs use it. Its memory is part of the savestate (16 MB make large
savestates).""")
p("""**WiC64** (`--option userport=wic64`): the WLAN adapter on the user port
with firmware 2.x commands in the legacy and the protocol-2 format: HTTP
GET (plain and URL-encoded), HTTP POST, TCP open/read/write/close, IP, MAC,
SSID, signal strength, time and time zone, server, version, status
message, time-outs and echo. `https:` addresses are reported as
unsupported.""")
p("""Both use your computer's network by default; `--net off` switches it
off and the devices report "no network". The clock is UTC unless the
environment variable `EMU_UTC_OFFSET` (e.g. `+01:00`) gives the offset.
The web version has no network (browsers allow no sockets); there the
command interface sees the uploaded files as its folder. A debugger
recording (`record_start`/`record_stop`) also records every answer from
the network and the folder, so `replay_input` repeats a run exactly
without the network.""")

h3("SD2IEC")
p("""**SD2IEC** (`--option sd2iec=on`, device 9; `--option drive8=sd2iec` in
place of drive 8): a mass storage device on the serial bus whose "SD card"
is a folder of your computer (`--sd2iec-dir`, default `.mimic/sd2iec` in
your user folder). It loads at KERNAL speed (no fast loaders). Directories
as on CMD drives (`CD:NAME`, `CD//`, `CD:←`, `MD`, `RD`, paths like
`LOAD"//GAMES/:ELITE",9`); D64/D71/D81 images are mounted with
`CD:GAME.D64` and left with `CD:←`; `N:NEW.D64,ID` makes a new one. Names
of either letter case show as plain letters, names longer than 16
characters as an 8.3 short name. Direct access inside a mounted image
(`#` buffers, `U1`/`U2`, `B-P`, `B-A`/`B-F`), REL files with `P`, swap
lists (`XS:LIST.LST`, then the panel buttons SD2IEC NEXT and PREV; both
together go to the first entry, and without a list they write an
`AUTOSWAP.GEN` with the folder's images), `XI`, `XD`, `XR` and the usual
`M-R` drive detection. A disk image in the "sd2iec" media slot stays
mounted until ejected.""")

h2("Commodore VIC-20, C16/Plus/4, PET and C128")
h3("VIC-20")
table(["Option", "Values"], [
    ["`ram`", "memory expansion: `none` (default), `3k`, `8k`, `16k`, `24k`, `all`"],
    ["`port1`", "`joystick` (default), `paddles`, `mouse1351`, `none`; also the mice "
     "and keypads of the Commodore ports (Part II)"],
    ["`drive8`", "`1541`, `1571`, `1581`, `virtual`, `none`"],
    ["`autostart`", "`on`: load and run the disk's first program"],
    ["`roms`", "`stub`: a test ROM without the Commodore ROMs"],
], [0.22, 0.78])
h3("C16 and Plus/4")
table(["Option", "Values"], [
    ["`ram`", "`16k` (default on the C16) or `64k` (default on the Plus/4)"],
    ["`port1`", "`joystick` (default) or `none`; also the mice and keypads of the "
     "Commodore ports that need no paddle lines (Part II)"],
    ["`drive8`", "`1541`, `1571`, `1581`, `1551` (the parallel TCBM drive with its "
     "interface cartridge, needs `dos1551`), `virtual`, `none`"],
    ["`sidcard`", "`6581`, `8580`, `off` (default): a SID card at `$FD40`, for demos "
     "and games with SID music"],
    ["`autostart`", "`on`: load and run the disk's first program"],
    ["`roms`", "`stub`: a test ROM without the Commodore ROMs"],
], [0.22, 0.78])
h3("PET")
table(["Option", "Values"], [
    ["`ram`", "`8k` (default on the 2001), `16k`, `32k` (default otherwise)"],
    ["`keyboard`", "`graphics` (default) or `business` (default on the 8032)"],
    ["`hz`", "`50` (default) or `60`"],
    ["`drive8`", "`virtual` (default; an IEEE-488 drive at file level) or `none`"],
    ["`roms`", "`stub`: a test ROM without the Commodore ROMs"],
], [0.22, 0.78])
h3("C128")
table(["Option", "Values"], [
    ["`display`", "`40` (default) or `80`: the screen the machine starts in"],
    ["`vdc_ram`", "`16k` (default) or `64k`"],
    ["`sid`", "`8580` (default) or `6581`"],
    ["`drive8`", "`1571`, `1541`, `1581`, `virtual`, `none`"],
    ["`port1`, `port2`", "as on the C64 (defaults: port 1 `none`, port 2 `joystick`)"],
    ["`autostart`", "`on`: load and run the disk's first program"],
    ["`roms`", "`stub`: a test ROM without the Commodore ROMs"],
], [0.22, 0.78])
p("""The C128 has two screens: F12 switches between the 40-column VIC and the
80-column VDC; the window starts on the one the 40/80 key selects.""")

h2("Atari 400, 800, 800XL, 65XE and 130XE")
table(["Option", "Values"], [
    ["`port1`, `port2`", "`joystick`, `paddles`, `none` (400/800 also `port3`, `port4`)"],
    ["`basic`", "`on`/`off` — XL/XE: hold OPTION at boot; 400/800: BASIC cartridge in the left slot"],
    ["`ram`", "400/800 only: `8k`, `16k`, `32k`, `48k`"],
    ["`drives`", "`0` to `4` disk drives"],
    ["`drive`", "`1050`, `810` or `xf551`"],
], [0.22, 0.78])
p("""Keys: < > on - =, - = on [ ], + * on ' \\, right Alt = Atari key, cursor keys
= CONTROL + - = + *, F2 START, F3 SELECT, F4 OPTION, F5 RESET, F6 HELP,
F7/Pause BREAK.""")

h2("Atari 2600")
table(["Option", "Values"], [
    ["`port1`, `port2`", "`joystick`, `paddles`, `keypad`, `driving`, `none`"],
    ["`tv`", "`color` or `bw`"],
    ["`left`, `right`", "difficulty switches `b` or `a`"],
    ["`bank`", "force a bank-switching scheme: `2k`, `4k`, `f8`, `f6`, `f4`, `f8sc`, `f6sc`, "
     "`f4sc`, `fa`, `fe`, `e0`, `e7`, `3f`, `dpc` (Pitfall II; normally detected)"],
], [0.22, 0.78])
p("""Console: F1 SELECT, F2 RESET (held), F3 colour/black-and-white, F4/F5 left/right
difficulty. Keypads: left 1 2 3 / Q W E / A S D / Z X C, right 8 9 0 / I O P /
K L ; / , . /.""")

h2("Amstrad CPC 464, 664 and 6128")
table(["Option", "Values"], [
    ["`crtc`", "`0`, `1` or `2` (default: 0 on the 464, else 1)"],
    ["`dos`", "`amsdos`, `parados` or `none`"],
    ["`joy1`", "`joystick`, `amx` (AMX mouse) or `none`"],
    ["`joy2`", "`joystick` (a second stick daisy-chained) or `none`"],
], [0.22, 0.78])
p("""Disks (DSK/EDSK) go into drive A on the 664/6128: type `CAT` or
`RUN"DISC`. The 464 has a cassette only (CDT/TZX); on the 664/6128 type
`|TAPE` first to switch to tape. Keys: - ^ on - =, @ [ on [ ], : ; on ; ',
] on the backslash key, Backspace = DEL, Delete = CLR, left Alt = COPY, the
numeric keypad and F1–F10 = the function keys f0–f9 and f.""")

h2("Commodore Amiga")
table(["Option", "Values"], [
    ["`kickstart`", "`1.2`, `1.3`, `2.0`, `3.0`, `3.1` (default 1.3; A500+ 2.0; A1200 3.1)"],
    ["`chip`", "chip RAM `256k`, `512k`, `1m`, `2m`"],
    ["`slow`", "trapdoor RAM `512k` or `0`"],
    ["`fast`", "Zorro II fast RAM `0` … `8m` (found by Kickstart through autoconfig)"],
    ["`chipset`", "`ocs`, `ecs`, `aga`; or `agnus=ocs|ecs`, `denise=ocs|ecs` separately"],
    ["`drives`", "`1` to `4` (DF0: to DF3:)"],
    ["`drivetype`", "`dd` or `hd` (1.76 MB disks)"],
    ["`port0`, `port1`", "`mouse`, `joystick`, `none` (defaults: mouse in 0, joystick in 1)"],
    ["`serial`", "`none` or `loopback`"],
    ["`wcs`", "A1000: `rom` or `disk` (load Kickstart from disk with the bootstrap ROM)"],
], [0.22, 0.78])
p("""Without a disk, Kickstart shows its insert-disk screen; an ADF in DF0:
boots. Workbench 1.3 takes about three minutes of emulated time to reach
its screen from disk. Keys: left/right Windows key = left/right Amiga key,
both Ctrl = CTRL, Insert = HELP, Delete = DEL; **Ctrl + both Windows keys**
resets like the real keyboard. Sound is stereo with the A500's filters.""")

h2("Sinclair ZX81")
table(["Option", "Values"], [
    ["`ram`", "`16k` (default), `1k` or `2k`"],
    ["`traps`", "`on` (default) or `off`: LOAD and SAVE of the `.p` file through traps "
     "in the ROM"],
    ["`roms`", "`stub`: a test ROM without the Sinclair ROM"],
], [0.22, 0.78])
p("""Models `zx81` (PAL) and `ts1000` (NTSC, 60 Hz). Programs are
`.p` files given with `--tape`: `LOAD ""` loads the program from the tape
slot and `SAVE "NAME"` writes it, through traps in the ROM.""")

h2("Sinclair ZX Spectrum")
p("""Models `48k`, `128k` and `plus2`.""")
table(["Option", "Values"], [
    ["`traps`", "`on` (default: `LOAD \"\"` copies standard tape blocks at once) or "
     "`off` (the tape plays in real time)"],
    ["`autostart`", "`on`: with a tape, types `LOAD \"\"` or picks the 128K menu's "
     "Tape Loader"],
    ["`joystick`", "`kempston` (default), `sinclair1`, `sinclair2`, `cursor`, `none`"],
    ["`mouse`", "`kempston` or `none` (default)"],
    ["`issue`", "board issue `3` (default) or `2`: the EAR feedback in port $FE bit 6"],
    ["`multiface`", "`on`: a Multiface 128 (needs its ROM)"],
    ["`if1`", "`on`: an Interface 1 (needs its ROM) with `microdrives` = `1` to `8` "
     "Microdrives (default 2)"],
    ["`beta`", "`on`: a Beta Disk interface with TR-DOS (needs its ROM)"],
    ["`roms`", "`stub`: a test ROM without the Sinclair ROMs"],
], [0.22, 0.78])
p("""Tapes are TAP or TZX files (`--tape`); `LOAD ""` presses PLAY, and
the tape stops at TZX "stop" blocks. Snapshots (SNA, Z80) go in with
`--prg` and take over at once. Keys: Shift = CAPS SHIFT, right Ctrl or
AltGr = SYMBOL SHIFT (the left Ctrl too, but Ctrl+D, I, R and Space
are hotkeys with it; `"` is AltGr+P), Backspace = DELETE, the cursor keys = CAPS SHIFT + 5–8,
Escape = BREAK, `.` and `,` as on the keys. The Multiface 128's red
button is the panel's MULTIFACE; the Interface 1's Microdrives take MDR
cartridges (`--disk`, `FORMAT "m";1;"name"`, `SAVE *"m";1;"file"`,
`CAT 1`, `LOAD *"m";1;"file"`). The RS-232 port has no host device
yet.""")

h2("ColecoVision")
table(["Option", "Values"], [
    ["`port1`, `port2`", "`controller` (default: joystick, two fire buttons, keypad), "
     "`superaction` (Super Action Controller), `driving` (Expansion Module #2: wheel "
     "and pedal), `roller` (Roller Controller, a trackball; its second cable takes "
     "port 2 as well), `none`"],
    ["`roms`", "`stub`: a small BIOS of mimic's own that starts cartridges directly "
     "(games that call BIOS routines need the real BIOS)"],
], [0.22, 0.78])
p("""Keys: controller 1's keypad on the digit row (1–9, 0, - = *,
= = #, [ ] = the Super Action Controller's purple and blue buttons, X = right
fire), controller 2's keypad on the numeric keypad; joystick 1 on the cursor
keys, Space = left fire.""")

h2("TI-99/4A")
p("""ROMs: the console ROM (`994AROM.BIN`, 8 KB) and GROM 0–2
(`994AGROM.BIN`, 24 KB) in the `ti99` folder of the ROM folder; the disk
controller's DSR ROM (`DISK.BIN`, 8 KB) adds the disk card, the speech ROM
(`SPCHROM.BIN`, 32 KB) the Speech Synthesizer (Parsec and
other games talk). `--videoformat
pal` is the TMS9929A.""")
table(["Option", "Values"], [
    ["`ram`", "`32k` (default: the 32 KB memory expansion) or `none`"],
    ["`port1`, `port2`", "`joystick` (default) or `none`"],
    ["`disk`", "`on` or `off`: the disk controller (default: on when its DSR ROM is there)"],
    ["`speech`", "`on` or `off`: the Speech Synthesizer (default: on when its ROM is there)"],
    ["`roms`", "`stub`: a small ROM of mimic's own (a test picture only, no BASIC)"],
], [0.22, 0.78])
p("""Cartridges: `--cart GAMEC.BIN` — the `G` (GROM) and `D`
(second bank) dumps next to it are loaded with it. Cassette CS1:
`--tape tape.wav`; `SAVE CS1` and `OLD CS1` ask for the recorder keys,
which are the media slot's play/record/stop/rewind. Disks: `--disk game.dsk`
(TI sector dumps, 90 or 180 KB) in DSK1 of the disk controller. Keys: Alt =
FCTN, Caps Lock = ALPHA LOCK (on at start), the arrows = FCTN+E/S/D/X,
Backspace = FCTN+S, Delete = FCTN+1, Escape = FCTN+9 (BACK), F4 = FCTN+4
(CLEAR).""")

h2("Nintendo NES / Famicom")
table(["Option", "Values"], [
    ["`port1`, `port2`", "`controller` (default), `zapper` (light gun: the mouse aims, "
     "left button = trigger), `arkanoid` (the mouse turns the knob), `none`"],
    ["`fourscore`", "`on`: a Four Score with controllers in ports 3 and 4 (ports 1 "
     "and 2 must be controllers)"],
], [0.22, 0.78])
p("""Keys: the D-pad on the cursor keys, Space
and X = A, Z = B, right Shift = Select, Enter = Start. Cartridges are iNES
or NES 2.0 files with mappers 0–4 (NROM, MMC1, UxROM, CNROM, MMC3).""")

h2("Super Nintendo / Super Famicom")
table(["Option", "Values"], [
    ["`port1`, `port2`", "`pad` (default), `mouse`, `superscope` (port 2 only: the "
     "mouse aims, left button = trigger, right = cursor), `none`"],
    ["`multitap`", "`on`: a Multiplayer 5 in port 2 (pads in ports 2 to 5)"],
    ["`roms`", "`stub`: mimic's own sound boot loader instead of the IPL ROM"],
], [0.22, 0.78])
p("""Keys: the
D-pad on the cursor keys, Z = B (also Space), X = A, A = Y, S = X, Q = L,
W = R, right Shift = Select, Enter = Start. Cartridges are `.sfc`/`.smc`
(LoROM, HiROM, ExHiROM, with or without copier header); games with
coprocessor chips (DSP, Super FX, SA-1) are not supported yet.""")

h2("Vectrex")
p("""The Vectrex draws with a beam steered by analog hardware, not with
pixels. mimic follows the beam and shows a rendered picture of it in the
window (portrait, 800 × 1000 pixels).""")
table(["Option", "Values"], [
    ["`port1`", "`controller` (default) or `none`"],
    ["`port2`", "`controller` (default), `lightpen`, `imager` (3D Imager) or `none`"],
    ["`lines`", "`fine` (default: thin lines, bright points where the beam rests) or "
     "`soft` (even, brighter lines with a soft edge)"],
    ["`eye`", "with the 3D Imager: `left` (default), `right` or `both`"],
    ["`roms`", "`stub`: a test picture without the system ROM"],
], [0.22, 0.78])
p("""Controls: the analog stick on the gamepad's left stick (analog) or the
cursor keys and D-pad (full deflection); buttons 1–4 on Z X C V or A B X Y
of the gamepad; Space = button 1.""")
bullets(
    "**System ROM.** The real ROM is 8 KB: the executive and the built-in game "
    "Mine Storm. A 4 KB image of the executive alone starts cartridges, but "
    "without a cartridge there is no Mine Storm, and cartridges that call Mine "
    "Storm's routines (such as 3-D Mine Storm) hang.",
    "**Start-up.** After power-on the executive shows its title for about ten "
    "seconds before a cartridge starts.",
    "**3D Imager.** The spinning colour wheel is emulated (motor, index pulse, "
    "colour filters); the picture shows the eye chosen with `eye`.",
    "**Afterglow.** The default monitor `vectrex` draws every line with its glow "
    "from the moment the beam draws it: moving objects leave a faint trail, a still "
    "picture stays as bright as without. `--crt raw` shows the machine's picture "
    "without afterglow.",
    "**Overlays.** The games' coloured sheets are your own pictures (PNG) in the "
    "folder `~/.mimic/overlays/vectrex/`, named like the cartridge (`scramble.png` "
    "for `Scramble.vec`, `minestorm.png` for the built-in game); mimic ships none. "
    "Transparent parts let the picture through, coloured parts tint it; the overlay "
    "follows the cartridge, `--overlay FILE` fixes one, `--overlay none` shows none.",
)
code("""mimic-desktop --machine vectrex --cart "Scramble (1982).vec"
mimic-desktop --machine vectrex --option lines=soft --cart game.vec
mimic-desktop --machine vectrex --option port2=imager --cart 3d-game.vec""")

h2("Apple II")
p("""The Apple II family as four models: the original II (Integer BASIC),
the II+ (Applesoft), the IIe and the enhanced IIe (65C02, MouseText). Every
model has a language card, a Disk II card with two drives in slot 6 and a
Mockingboard sound card in slot 4. Colour comes from the monitor: the
Apple sends a black-and-white dot pattern and a colour TV decodes colours
from it, so the default monitor is `tv`; text screens show in black and
white (the Apple turns the colour burst off), and `--crt raw` shows the dot
pattern itself.""")
table(["Option", "Values"], [
    ["`ram`", "II/II+: `4k` … `48k` (default `48k`)"],
    ["`lc`", "II/II+: language card `on` (default) or `off`"],
    ["`aux`", "IIe: auxiliary memory `64k` (default, Extended 80-Column Card), `1k` or `none`"],
    ["`slot4`", "`mockingboard` (default) or `none`"],
    ["`slot6`", "`disk2` (default) or `none` (without it the machine starts BASIC "
     "instead of waiting for a disk)"],
    ["`port`", "game I/O: `joystick` (default), `paddles` or `none`"],
    ["`roms`", "`stub`: a test ROM without the Apple ROMs"],
], [0.22, 0.78])
p("""Keyboard: the PC keys type what their legends show (letters in upper
case on the II and II+); Backspace and ← are the Apple's ← (backspace),
the Alt keys are Open Apple and Solid Apple, **F5 is RESET** (on the IIe
with Ctrl, as on the real machine). The joystick is analog: the gamepad's
left stick, or the numeric keypad at full deflection.""")
bullets(
    "**Disks.** DSK/DO (DOS 3.3 order), PO (ProDOS order), NIB and WOZ 1/2 "
    "images go into slot 6, drive 1 (`--disk`; a second `--disk` fills drive 2). "
    "With the boot PROM the machine boots the disk at power-on; without a disk "
    "it waits with the drive running, like the real one.",
    "**Cassette.** WAV recordings (`--tape`). The tape moves while a program "
    "listens to it, so type `LOAD` (or the monitor's `R`) at any time. To save, "
    "insert an empty file, press Rec in the media panel, `SAVE`, then Stop: "
    "the recording is written as a WAV file.",
    "**Sequencer PROM.** With the P6 PROM (341-0028) the Disk II reads and "
    "writes the way the real card does; without it a model of the card stands in.",
)
code("""mimic-desktop --machine apple2 --disk "Zork I.dsk"
mimic-desktop --machine apple2 --model iiplus --disk game.woz
mimic-desktop --machine apple2 --model ii --option slot6=none""")

h2("Macintosh")
p("""The compact Macintosh as four models: the 128K and 512K (64 KB ROM,
400K single-sided drives, the M0110 keyboard without keypad), the Plus
(128 KB ROM, 800K drives, SCSI, the M0110A keyboard with keypad and arrows)
and the SE (256 KB ROM, ADB keyboard and mouse, two internal drives, SCSI).
Without a disk the Mac shows the disk icon with a question mark, as the real
one does. The screen is the built-in black-and-white monitor (profile
`mac`).""")
table(["Option", "Values"], [
    ["`ram`", "Plus/SE: `512k`, `1m` (default), `2m` or `4m`"],
    ["`serial`", "`loopback`: both serial ports wired back to themselves"],
    ["`roms`", "`stub`: a test ROM without the Mac ROMs"],
], [0.22, 0.78])
p("""Keyboard: the PC keys sit where the Mac's are on a US layout; **Alt is
Command** (next to the space bar, as on the Mac), the Windows keys are
Option, NumLock is the keypad's Clear. The mouse is captured with a click
into the window. The panel has POWER, RESET and the programmer's INTERRUPT
switch.""")
bullets(
    "**Floppies.** 400K and 800K images, raw (`.dsk`, `.img`) or DiskCopy 4.2 "
    "(`.image`), with tag bytes; written back in their own format. The first "
    "`--disk` goes into the internal drive, the next into the external one "
    "(the SE: the second internal drive first).",
    "**Hard disks.** A hard disk image (whole 512-byte blocks, with an Apple "
    "partition map and driver, as Apple's HD SC Setup makes it) given with "
    "`--disk` goes to the first SCSI ID that is free: a floppy drive refuses "
    "it and passes it on (Plus and SE).",
    "**System software.** System 6.0.8 suits the Plus (1 MB or more) and the "
    "SE; the 128K needs System 1.x/2.x on 400K MFS disks, the 512K up to 3.x.",
)
code("""mimic-desktop --machine mac --disk "System Tools.image"
mimic-desktop --machine mac --model se --disk system.dsk --disk hd20.img
mimic-desktop --machine mac --model 128k --disk mfs-system.dsk""")

h2("Tandy TRS-80")
p("""Models `1` (Model I, default) and `3` (Model III with its FD1793 disk
controller, 48 KB and lower case built in).""")
table(["Option", "Values"], [
    ["`level`", "Model I: BASIC ROM `2` (Level II, default) or `1` (Level I)"],
    ["`ram`", "`4k`, `16k` (default on the Model I), `48k` (default on the Model III)"],
    ["`lowercase`", "Model I: `on` or `off` (default): the lower-case modification"],
    ["`ei`", "Model I: `on` or `off` (default): the Expansion Interface with its "
     "FD1771 and drives 0–3"],
    ["`roms`", "`stub`: a test ROM without the Tandy ROMs"],
], [0.22, 0.78])
p("""Disks are DMK, JV1 or JV3 images (`--disk`); the machine boots drive 0,
and BREAK held at power-on skips the disk. On the Model I a cassette is a
CAS file (`--tape`): `CLOAD` plays it, `CSAVE` appends to it. Keys: Home or
Delete = CLEAR, Escape or End = BREAK, Backspace = ←.""")

h2("BBC Micro")
p("""Models `b` (default: the Model B), `bplus` (B+, 64 KB with shadow
RAM), `bplus128` (B+ 128, sideways RAM in slots 0, 1, 12 and 13) and
`master` (Master 128). ROMs in the `bbc` folder of the ROM folder: the
Model B needs `os12.rom` and `basic2.rom`, `dfs-1.20.rom` adds the 8271
disk controller; the B+ takes `bplus-mos.rom` and `dfs-2.26.rom` (the
1770); the Master its MOS 3.20 in sixteen-kilobyte pieces in
`bbc/master320`. `--disk` takes SSD and DSD images, on the Master also ADFS
images (`.adf`, `.adl`); Shift+BREAK boots a disk. Tapes are UEF or WAV
files (`--tape`; type `*TAPE` first when a disk filing system is in).""")
table(["Option", "Values"], [
    ["`rtc`", "Master: the clock's start, `YYYY-MM-DD[THH:MM[:SS]]` (default "
     "2000-01-01; MOS 3.20 shows the year without its century)"],
    ["`cmos`", "Master: `configured` (default: BASIC, DFS, MODE 7 as a new Master "
     "came) or `blank` (empty CMOS RAM)"],
    ["`roms`", "`stub`: a test ROM without the Acorn ROMs"],
], [0.22, 0.78])
p("""Keys: F1–F9 = f1–f9, F10 = f0, Backspace = DELETE,
End = COPY, left Alt = SHIFT LOCK, Pause = BREAK. The numeric keypad is the
Master's keypad; on the Model B and B+ its digits, `-`, `/`, `.` and Enter
type the main keys. The CMOS RAM is kept in savestates, not between
sessions. The debugger's graphics views: `screen` (the picture from
memory), `palette`, `charset` (the SAA5050's characters).""")

h2("MEGA65")
p("""The MEGA65 starts its ROM directly (no hypervisor yet): the 128 KB
`mega65.rom` in the `mega65` folder of the ROM folder, the MEGA65 Open ROMs
or the official 920xxx ROM (so far only the Open ROMs of 2021 are tried;
their BASIC answers numbers with "?NOT IMPLEMENTED ERROR"). `--disk` takes
a D64 for drive 8 on the serial bus (a true 1541 with the 1541 DOS ROM in
`drives`), a D81 for the internal drive, a raw SD card image for the SD
card.""")
table(["Option", "Values"], [
    ["`board`", "`r1` to `r15`: the model ID the ROM reads (default `r3`)"],
    ["`port1`, `port2`", "`joystick` (port 2 by default), `paddles`, `mouse1351`, "
     "`amigamouse`, `stmouse`, `cx22`, the keypads, `none`"],
    ["`drive8`", "`1541`, `1571`, `1581` (true drive, needs its DOS ROM), `virtual`, `none`"],
    ["`roms`", "`stub`: a small test ROM of mimic's own"],
], [0.22, 0.78])
p("""Keys: the arrows are the four cursor keys, Escape = ESC, Tab = TAB,
left Ctrl = CTRL, left Windows key = MEGA, left Alt = ALT, End = RUN/STOP,
Page Up = RESTORE, Page Down = HELP, Scroll Lock = NO SCROLL, Caps Lock =
CAPS LOCK, F1–F11 the odd function keys (Shift for the even ones), Print
Screen = F13. The debugger's graphics views: `screen`, `charset`, `sprites`,
`palette`.""")

h2("Acorn Archimedes")
p("""Models `a310` (default), `a305`, `a440` and `a3000`.""")
table(["Option", "Values"], [
    ["`os`", "RISC OS `3.11`, `3.10`, `3.00`, `2.00` or Arthur `1.20` (default: the "
     "newest one in the ROM folder)"],
    ["`ram`", "`512k`, `1m`, `2m`, `4m` (default: A305 512k, A310 and A3000 1m, A440 4m)"],
    ["`roms`", "`stub`: a test ROM without the Acorn ROMs"],
], [0.22, 0.78])

h2("IMSAI 8080")
p("""The picture is the front panel: the paddles are clicked with the mouse,
the lamps show the bus. The serial terminal is the second screen (F12), and
the keyboard types on it. The machine needs no ROM.""")
table(["Option", "Values"], [
    ["`ram`", "`1k` to `64k` (default `64k`)"],
    ["`baud`", "the serial lines' speed, `50` to `115200` (default `9600`)"],
    ["`mpu`", "`a` or `b`: the MPU-A or MPU-B monitor ROM (2 KB); `none` (default)"],
    ["`load`", "the address (hex) a binary program is loaded to (default `0`)"],
    ["`autostart`", "`on`: run a loaded program, the ROM or the disk at power-on"],
    ["`reader`", "`card` (default: the tape reader on its own serial card) or "
     "`console` (the tape plays into the terminal's line, as on a Teletype)"],
    ["`cardreader`", "`text` (default: a card deck plays into the reader line) or "
     "`raw` (its columns at ports $20–$22)"],
], [0.22, 0.78])
p("""Disks are raw 8-inch images in drives A: to D: of the FIF floppy
interface (`--disk`); with a disk in drive A, RESET and RUN boot it
(CP/M 2.2).""")

h2("IBM 029 card punch")
p("""A machine for typing card decks (`.deck`, `.crd`, `.chx`), which the
IMSAI's card reader reads back. The picture is the card at the punch
station.""")
table(["Option", "Values"], [
    ["`autofeed`", "`on` (default) or `off`: the AUTO FEED switch"],
    ["`print`", "`on` (default) or `off`: the PRINT switch (characters along the "
     "card's top edge)"],
    ["`program`", "the card format for Tab (SKIP) and the field lines: `free` "
     "(default), `fortran`, `cobol`, `assembler`"],
    ["`seq`", "`on` or `off` (default): punch sequence numbers into released cards"],
], [0.22, 0.78])
p("""Keys: Enter = FEED without a card, REL with one; Tab = SKIP; Backspace
moves the card back a column; Space or → moves on without punching.""")

h2("Logophor 16")
p("""mimic's own steampunk computer; its ROM is built in. The profile
`logophor16` boots the desktop Panorama.""")
table(["Option", "Values"], [
    ["`card`", "`system`: the built-in system card (Boiler and Panorama; writes are "
     "lost at the end, `--disk` takes its place); `none` (default)"],
    ["`port0`, `port1`", "`mouse`, `joystick`, `none` (defaults: mouse in port 0, "
     "joystick in port 1)"],
    ["`net`", "`host`: the gateway in the machine (DHCP, DNS and the Internet through "
     "the host's network); `HOST:PORT`: join the network of an `ada-asm hub`"],
    ["`mac`", "the network address, six hex bytes (default `02:4c:31:36:00:01`)"],
    ["`rtc`", "the clock's start in seconds since 2000-01-01 (default 2026-01-01)"],
], [0.22, 0.78])
p("""The SD card is a FAT32 image given with `--disk`; Foundry starts
`BOOT/BOILER.BIN` from it.""")

# ---- 6 media ------------------------------------------------------------------

h1("Disks, tapes, cartridges and programs")
table(["Kind", "Formats", "Option"], [
    ["Disk", "D64, D71, D81, G64, D80/D82 (PET), ATR (Atari), DSK/EDSK (CPC), ADF "
     "(Amiga DD and HD), extended ADF, DMS (read only), DSK/DO/PO/NIB/WOZ (Apple II), "
     "400K/800K DSK and DiskCopy 4.2, SCSI hard disk images (Macintosh); "
     "a folder as a virtual disk",
     "`--disk`"],
    ["Tape", "TAP (Commodore), CAS (Atari), CDT/TZX (CPC), P/81 (ZX81), TAP/TZX (ZX Spectrum), WAV (Apple II); a PRG is "
     "recorded in the machine's tape format", "`--tape`"],
    ["Cartridge", "CRT (C64/C128), raw VIC-20 and C16/Plus/4 dumps, CAR or 8/16 KB ROM "
     "(Atari), .a26/.bin (Atari 2600), .col/.rom (ColecoVision, MegaCart), .nes (NES), "
     ".sfc/.smc (SNES), .vec/.bin (Vectrex, up to 64 KB)", "`--cart`"],
    ["Program", "PRG, T64, XEX", "`--prg`"],
], [0.12, 0.74, 0.14])
p("""Images are opened through a **copy-on-write overlay**: the file on your
disk stays unchanged while the emulated machine writes to its copy. Changes
are written back only on request (`flush_media` in the debugger).""")
h2("Commodore drives")
p("""Each Commodore drive slot has two modes: **true drive emulation** (the
drive's own CPU and the disk's GCR data, cycle-exact — needed for fast
loaders and copy protection) and the **virtual drive** (serves files on the
protocol level — fast, but only for normal LOAD and SAVE). The virtual
drive still answers at the pace of the KERNAL's serial routines (about 400
bytes a second, as a 1541); with `--option fastload=on` the C64 hands every
byte of the KERNAL's TALK/LISTEN/SECOND/TKSA/CIOUT/ACPTR/UNTLK/UNLSN
straight to it (stock KERNAL only; fast loaders are not affected).""")

# ---- 7 monitors ----------------------------------------------------------------

h1("Monitor profiles")
p("""Machines always produce the raw picture; monitor and signal effects
(composite colour bleeding, PAL colour mixing, NTSC artefact colours,
scanlines, phosphor) are added afterwards. `--crt PROFILE` chooses the
look; without it each machine uses the monitor it shipped with.""")
table(["Profile", "Use"], [
    ["`raw`", "sharp pixels, no effects (default for the NES, ColecoVision and Vectrex)"],
    ["`1702`, `1084s`", "Commodore monitors over S-Video (C64, VIC-20, C16/Plus/4, C128)"],
    ["`1084s-rgb`", "the 1084S on its RGB input (Amiga, SNES, C128 VDC)"],
    ["`tv`, `pal-tv`, `ntsc-tv`", "a television over composite (Atari, ZX81, ZX Spectrum, Apple II)"],
    ["`pet-white`, `pet-green`", "the PET's built-in monitors"],
    ["`ctm644`", "Amstrad colour monitor (CPC)"],
    ["`gt65`", "Amstrad green monitor"],
    ["`mac`", "the compact Macintosh's built-in black-and-white monitor"],
    ["`logophor`, `logophor-scope`", "the Logophor 16's monitor: an RGB tube with clear raster lines, and its green phosphor tube with glow and afterglow; the program switches between them (Panorama's oscilloscope theme), the monitor follows"],
    ["`vectrex`", "the Vectrex's tube: glowing lines that fade from the moment the beam draws them (lines drawn early in a frame are brighter at its end than late ones, moving lines leave a trail); `phosphor_ms` sets the decay time (default 6 ms)"],
    ["`lite`", "aspect correction and light scanlines only, for slow computers and the web version"],
], [0.30, 0.70])
p("""Besides the signal (composite, S-Video, RGB, monochrome) a profile sets
scanlines, the phosphor mask (`shadow`, `aperture` or `slot` with strength
and pitch), bloom (bright areas glow), the curvature of the glass, a
vignette and the persistence of the phosphor (a trail behind moving
objects, as on the green PET and GT65 monitors). The same picture comes out
of the graphics card in the desktop program and of the processor in
snapshots, the preview and the web version.""")
p("""Own profiles are TOML files that start from a preset and change single
values (`base = "1084s"` …), loaded with `--crt myprofile.toml`. The desktop
program re-reads such a file when it changes, so a profile can be tuned
while the machine runs. The live preview has a **monitor editor** at
`http://127.0.0.1:6580/crt`: sliders for every value, the machine's picture
through the profile, and a button that saves the TOML file.""")

# ---- 9 preview ----------------------------------------------------------------

h1("Live preview in the browser")
p("""With `--preview` the emulator serves a viewer page: open
`http://127.0.0.1:6580/` in a browser to watch the running machine. The
viewer forwards keyboard, gamepad and mouse input, so the machine can be
played from the browser, and shows the console panel.""")
code("""mimic-headless --machine c64 --preview
mimic-desktop  --machine amiga --preview      # window and browser""")
table(["Address", "Content"], [
    ["`/`", "the viewer page"],
    ["`/snapshot.png`", "the current frame as PNG"],
    ["`/preview` (WebSocket)", "a picture stream as PNG or QOI, also through a monitor profile"],
    ["`/stream` (WebSocket)", "a raw stream: colour indices, changed lines only, optional sound"],
    ["`/controls`", "the console's buttons and switches"],
    ["`/gfx`, `/gfx/VIEW.png`", "the graphics the video chip works from: sprites, character sets, "
     "bitmaps, screen memory, tiles, palettes"],
    ["`/overlays`", "the debug overlays the machine offers"],
    ["`/crt`", "the monitor profile editor"],
], [0.30, 0.70])
p("""Parameters: `filter=raw|crt`, `profile=NAME`, `scale=1..4`, for the
streams `fps=1..50`, `format=png|qoi`, `delta=0|1`, `audio=0|1`. The server
listens on 127.0.0.1 only.""")
p("""**Overlays** (`overlay=beam,grid,…`, switches in the viewer's header)
draw debugging information over the raw picture: the beam position (also in
a picture stopped half way), a raster grid with line numbers, and what the
video chip did where — the C64's bad lines and where each sprite really
appeared, the Atari display list's mode lines, the Amiga's DMA slot map and
Copper writes, CPC CRTC changes, NES and SNES sprites, interrupts and HDMA.
The **Graphics data** panel under the picture shows the sprites, character
sets, bitmaps and other graphics in memory; its options browse other
addresses (`index=2 addr=$2000`).""")

# ---- 10 debugger --------------------------------------------------------------

h1("Debugging")
p("""Every machine can be debugged the same way. The debugger knows no
particular machine: it works through registers, memory spaces, chips and
devices that each machine describes, so second processors (the 1541's 6502,
the C128's Z80, the Amiga's Copper and Blitter, the SNES sound CPU) are
debug targets of their own. Looking at memory never changes it — reading an
interrupt register in the debugger does not acknowledge the interrupt.""")
h2("Starting the debug server")
code("""mimic-headless --machine c64 --debug          # paused, port 6510
mimic-desktop  --machine c64 --debug          # with window and sound
mimic-headless --machine c64 --vice --preview --run""")
h2("The Python client polydbg")
p("`tools/polydbg.py` needs only Python's standard library:")
code("""python tools/polydbg.py info                 # devices and memory spaces
python tools/polydbg.py regs
python tools/polydbg.py disasm
python tools/polydbg.py break '$0810'        # breakpoint
python tools/polydbg.py resume --wait        # run until it stops
python tools/polydbg.py peek '$d000' 64
python tools/polydbg.py chips vic2           # chip registers
python tools/polydbg.py freeze '$0400' 9     # freeze a memory value (cheat)
python tools/polydbg.py search start         # cheat search
python tools/polydbg.py save state.bin       # savestate
python tools/polydbg.py gfx sprites index=2 --scale 4 -o s.png   # graphics in memory
python tools/polydbg.py picture --overlays beam,badlines -o now.png
python tools/polydbg.py rewind --on 1        # keep a state every frame
python tools/polydbg.py rewind 50            # 50 frames back
python tools/polydbg.py back 3               # 3 instructions back
python tools/polydbg.py shell                # interactive prompt""")
p("""`--host`/`--port` choose the server, `--json` prints raw answers;
`python tools/polydbg.py --help` lists all commands.""")
h2("What the debugger can do")
bullets(
    "**Execution:** pause, resume, step an instruction, a cycle, a raster line "
    "or a frame, step over and out, run to an address.",
    "**Breakpoints:** on execution, memory reads and writes, raster positions, "
    "IRQ and NMI; with conditions such as `A == $FF && [$D012] > 100` and hit "
    "counts.",
    "**Inspection:** registers, memory, disassembly, chip registers, beam "
    "position; symbols from VICE label files and KickAssembler.",
    "**State:** savestates, a raster trace of instructions and memory accesses.",
    "**Backwards:** rewind by frames, and step back instruction by instruction "
    "(the emulator replays from the last saved moment, exactly).",
    "**Pictures:** the current picture, also half drawn, with the beam marked and "
    "overlays; the graphics data the video chip works from.",
    "**Cheats:** freeze values in memory, narrowing search for unknown "
    "addresses (lives, energy), cheat files with code patches, imports of POKE "
    "lists, VICE monitor commands and address/value codes, and a cheat "
    "database that applies by the image's CRC-32.",
    "**Devices and media:** plug devices into ports, insert and eject media, "
    "inject and record input.",
)
h2("VICE binary monitor (VS64 and others)")
p("""With `--vice` mimic also speaks the binary monitor protocol of the VICE
emulator. Tools such as VS64 (Visual Studio Code), C64 Studio or IceBro Lite
can then debug any Commodore 8-bit machine in mimic. In VS64's `launch.json`:""")
code("""{ "type": "vice", "request": "attach", "name": "mimic",
  "hostname": "127.0.0.1", "port": 6502 }""")
note("Use `127.0.0.1`, not `localhost`: the servers listen on IPv4 only.")

# ---- 12 troubleshooting ---------------------------------------------------------

h1("Troubleshooting")
table(["Problem", "Solution"], [
    ["`unknown machine 'vectrex'` (or another new machine)",
     "You started an outdated `emu-desktop.exe`. Use `mimic-desktop.exe` or `cargo run "
     "--release -p emu-desktop -- …`."],
    ["`missing ROM '…'` or `no … ROM`",
     "Check the folder given with `--rom-dir` and the file's exact size; the message "
     "names the accepted file names."],
    ["The emulator runs slowly or stutters",
     "Use the release build. `EMU_DESKTOP_DEBUG=1` prints frame and sound statistics "
     "every 5 seconds. Check the speed control in the footer (Ctrl+Space)."],
    ["The window is too large or too small",
     "The window fits itself to the screen; resize it, or set `--scale 1`/`--scale 3`."],
    ["The gamepad does nothing",
     "Look at the console output: the pad must appear as `gamepad 0`. Check JOY PORT in "
     "the panel and that the machine has a joystick in that port."],
    ["The keyboard types instead of steering",
     "Set JOY KEYS in the panel (or `--joykeys`) to CURSOR or NUMPAD."],
    ["A disk program waits for the next disk",
     "Give all disks with `--disk` and press Ctrl+D, or drop the image onto the window."],
    ["Vectrex: black screen after the cartridge's title",
     "The cartridge may need the full 8 KB system ROM (it calls Mine Storm's routines), "
     "or a 3D Imager (`--option port2=imager`)."],
    ["No sound on Linux", "Install `libasound2-dev` before building."],
], [0.36, 0.64])

h2("Getting help")
bullets(
    "`mimic-desktop --help` and `mimic-headless --help` list every option and key.",
    "`docs/usage.md` is the detailed reference (German) for building, ROMs, media, "
    "the debug protocol and the test suites.",
    "`README.md` lists all CPUs, machines, chips and devices with their state.",
)



part("Part II — Reference", "Every option of mimic-desktop and mimic-headless, the machine "
     "options, the start profiles and the web front end")

# ---- 1 overview ---------------------------------------------------------------

h1("Reference overview")
p("""mimic is one emulator core with three front ends. They take the same
machines, the same ROMs and the same media, and they share one vocabulary
for configuring a machine: a machine id, a model, a video format, ROM
roles, and machine options given as `KEY=VALUE` pairs.""")
table(["Front end", "Program", "What it adds"], [
    ["Desktop", "`mimic-desktop`",
     "A window with sound, keyboard, gamepads and mouse; the console and media side "
     "panels; rewind; the CRT shaders on the GPU; optionally the debug, VICE-monitor "
     "and preview servers."],
    ["Headless", "`mimic-headless`",
     "No window, no sound: runs a number of frames and writes pictures, sound, "
     "savestates; serves the debugger and the live preview; imports and exports "
     "real disks through a Greaseweazle or ZoomFloppy."],
    ["Web", "`index.html` in `target/web`",
     "The same core as WebAssembly in the browser, driven by a page: files are "
     "uploaded (and kept in the browser's storage on request), the emulation runs "
     "in a Web Worker, the picture is drawn with WebGL2."],
], [0.14, 0.24, 0.62])
h2("Where settings come from")
p("""A setting can come from four places; the later ones win:""")
bullets(
    "**Defaults** built into the machine (model, video format, devices in the ports).",
    "**Start profiles** in `profiles.yaml` (`--profile NAME`), including a "
    "`settings.roms` folder.",
    "**Command-line flags** (`--machine`, `--model`, `--option KEY=VALUE` …); in the "
    "web page the form fields and the URL parameters play this part.",
    "**Environment variables**: `EMU_ROM_DIR` (ROM folder), `EMU_CHEATS_DIR` (cheat "
    "database), `EMU_DESKTOP_DEBUG=1` (statistics every 5 s).",
)
h2("Reading this reference")
bullets(
    "Flags are written as on the command line: `--disk FILE`. A flag marked "
    "*(repeatable)* may be given several times.",
    "Machine options are given as `--option KEY=VALUE`; several are listed as "
    "`KEY=a|b|c` with the default first unless stated otherwise.",
    "The same options are typed into the web page's options field as "
    "`KEY=VALUE` words separated by spaces, or passed in the URL separated by commas.",
    "`mimic-desktop --help`, `mimic-headless --help` and `mimic-headless --list` "
    "print the lists from the programs themselves.",
)

# ---- 2 common options -----------------------------------------------------------

h1("Options of both command-line programs")
p("""These flags mean the same in `mimic-desktop` and `mimic-headless`.""")
h2("Choosing the machine")
table(["Flag", "Meaning"], [
    ["`--machine ID`", "The machine to run (desktop default `c64`, headless default "
     "`sim6502`). See the machine table below and `mimic-headless --list`."],
    ["`--model NAME`", "Model variant of the machine, e.g. Atari `130xe`, Amiga `1200`, "
     "PET `8032`, Spectrum `128k`, Mac `se`. Models are configuration, not separate "
     "machines."],
    ["`--videoformat FORMAT`", "`pal`, `ntsc` or `auto`. The default is the machine's "
     "own; `auto` on the Atari 2600 picks NTSC or PAL from the program's field length "
     "after about 20 frames. Machines with one format refuse the other."],
    ["`--profile NAME`", "Start profile from `profiles.yaml` (in the working folder or "
     "the user's config folder; `--profiles FILE` names another file). Flags given on "
     "the same command line override the profile. Without `--machine` and "
     "`--profile` the desktop lists the profiles to choose from."],
    ["`--profiles FILE`", "The profiles file to use instead of `profiles.yaml`."],
    ["`--list-profiles`", "List the profiles and exit."],
    ["`--save-profile NAME`", "Store this command line's setup as a profile."],
    ["`--version`, `--help`", "Print the version or the option list and exit."],
], [0.26, 0.74])
h2("Machines and models")
table(["Machine id", "Machine", "Models (`--model`)", "Video formats (`--videoformat`)"], [
    ["`c64`", "Commodore 64", "–", "`pal` (default), `ntsc`"],
    ["`vic20`", "Commodore VIC-20", "–", "`pal` (default), `ntsc`"],
    ["`pet`", "Commodore PET", "`4032` (default), `2001`, `3032`, `8032`", "–"],
    ["`c16`, `plus4`", "Commodore 16, Plus/4", "–", "`pal` (default), `ntsc`"],
    ["`c128`", "Commodore 128", "–", "`pal` (default), `ntsc`"],
    ["`atari8`", "Atari 400/800/800XL/65XE/130XE",
     "`800xl` (default), `65xe`, `130xe`, `800`, `400`", "`pal` (default), `ntsc`"],
    ["`atari2600`", "Atari 2600 (VCS)", "–", "`auto` (default: from the program), "
     "`ntsc`, `pal`"],
    ["`cpc`", "Amstrad CPC", "`6128` (default), `664`, `464`", "–"],
    ["`amiga`", "Commodore Amiga", "`500` (default), `1000`, `2000`, `500plus` (ECS), "
     "`1200` (AGA)", "`pal` (default), `ntsc`"],
    ["`zx81`", "Sinclair ZX81 / TS1000", "`zx81` (default), `ts1000`", "–"],
    ["`spectrum`", "Sinclair ZX Spectrum", "`48k` (default), `128k`, `plus2`", "–"],
    ["`coleco`", "ColecoVision", "–", "`ntsc` (default), `pal`"],
    ["`nes`", "Nintendo NES / Famicom", "–", "`ntsc` (default), `pal`"],
    ["`snes`", "Super Nintendo / Super Famicom", "–", "`ntsc` (default), `pal`"],
    ["`vectrex`", "Vectrex", "`vectrex`", "–"],
    ["`apple2`", "Apple II, II+, IIe", "`iie` (default), `iie-unenhanced`, `iiplus`, "
     "`ii`", "–"],
    ["`mac`", "Apple Macintosh", "`plus` (default), `128k`, `512k`, `se`", "–"],
    ["`archimedes`", "Acorn Archimedes", "`a310` (default), `a305`, `a440`, `a3000`", "–"],
    ["`imsai`", "IMSAI 8080", "`8080`", "–"],
    ["`logophor16`", "Logophor 16", "`16`", "–"],
    ["`sim6502`", "6502 reference machine", "`default`", "–"],
], [0.14, 0.28, 0.34, 0.24])
note("""A machine with a dash under video formats has one standard built in
(the ZX81's `ts1000` model is the 60 Hz machine); a dash under models means
`--model` is refused. `--model pal` or `ntsc` is never accepted.""")
h2("ROMs")
table(["Flag", "Meaning"], [
    ["`--rom-dir DIR`", "The ROM folder: `DIR/<machine>/` holds the machine's images, "
     "`DIR/drives/` the drive DOS ROMs. Images are found by name prefix and size, so "
     "the usual dump names work. Default: `settings.roms` of the profile, then "
     "`$EMU_ROM_DIR`."],
    ["`--rom ROLE=FILE`", "One image for one role, e.g. `kernal=kernal.bin`, "
     "`dos1541=dos1541.bin`, `kickstart=kick13.rom` *(repeatable)*."],
    ["`--option roms=stub`", "Run with the built-in test ROMs instead (every machine "
     "has a small stub that shows a picture; no software runs on it)."],
], [0.26, 0.74])
h2("Media")
table(["Flag", "Meaning"], [
    ["`--prg FILE`", "A program: PRG or T64 (Commodore), XEX (Atari), Z80/SNA snapshot "
     "(Spectrum), .P (ZX81), BIN/HEX (IMSAI). It goes into the machine's program slot "
     "and runs after boot (`--autostart` where the machine needs it)."],
    ["`--disk FILE`", "A disk image into the first drive: D64/D71/D81/G64 (drive 8), "
     "ATR (Atari D1:), ADF/HD ADF/extended ADF/DMS (Amiga DF0:), DSK/EDSK (CPC), "
     "DSK/DO/PO/NIB/WOZ (Apple II slot 6), 400K/800K DSK or DiskCopy 4.2 and hard-disk "
     "images (Mac, SCSI), ADF (Archimedes), TRD (Spectrum with `beta=on`), MDR "
     "(Microdrive). *(repeatable)*: the next image goes to the next drive; a drive "
     "that refuses an image passes it on. Extra disks swap into the first drive with "
     "Ctrl+D in the desktop."],
    ["`--tape FILE`", "TAP/T64 (datasette), CAS (Atari), CDT/TZX (CPC, Spectrum), TAP "
     "(Spectrum), WAV (Apple II), .P (ZX81), PTP (IMSAI paper tape). A PRG is recorded "
     "as a tape first."],
    ["`--cart FILE`", "A cartridge: CRT (C64/C128), raw VIC-20 (load address + ROM) or "
     "C16/Plus/4 (16/32 KB) dumps, Atari CAR or raw 8/16 KB ROM, 2600 .a26/.bin, "
     "ColecoVision ROM, NES iNES/NES 2.0, SNES .sfc/.smc, Vectrex .vec/.bin."],
    ["`--autostart`", "Load and run the disk (C64: the first program of drive 8) or "
     "tape (Spectrum: LOAD \"\" and PLAY) after boot."],
], [0.2, 0.8])
note("""Writes go to a copy-on-write overlay. The image file on disk changes only
when the media is flushed: the desktop's media panel has a Save button per
slot, the debugger the `flush_media` command. The headless program never
writes back.""")
h2("Cheats, input, network, printing")
table(["Flag", "Meaning"], [
    ["`--cheats DIR`", "Cheat database: `DIR/<machine>/<crc32>.toml` applies to the "
     "images inserted, also after a disk swap. Default `$EMU_CHEATS_DIR`, else "
     "`./cheats` if it exists."],
    ["`--cheat FILE`", "A cheat file: `.toml` (freezes and patches), a POKE list "
     "(`.pokes`, `.bas`), VICE monitor commands (`.mon`, `.vice`) or address/value "
     "codes (`.txt`, `.codes`) *(repeatable)*."],
    ["`--input FILE`", "Desktop only: a joystick mapping profile (TOML, as saved by "
     "SAVE MAPPING in `<user folder>/input/<machine>.toml`)."],
    ["`--net on|off`", "The host network for devices that use it: the C64's WiC64 "
     "(`userport=wic64`) and Ultimate command interface (`uci=on`). Default `on`."],
    ["`--uci-dir DIR`", "The folder the Ultimate command interface's DOS works in "
     "(default `.mimic/ultimate` in your user folder, created on the first write; "
     "paths cannot leave it)."],
    ["`--sd2iec-dir DIR`", "The C64 SD2IEC's card (`--option sd2iec=on` or `drive8=sd2iec`; "
     "default `.mimic/sd2iec` in your user folder, created at start)."],
    ["`--uci-mount NAME=DIR`", "A host folder as a UCI drive `/NAME`, like the "
     "Ultimate's SD, Usb0, Usb1, Flash, Temp *(repeatable)*."],
    ["`--printer-dir DIR`", "Where `--option printer=on` writes its jobs (default "
     "`./printer`): `print-001.prn` and `print-001-1.png` per page."],
    ["`--host-printer NAME`", "Also send each finished page to this printer of the "
     "host; `default` means the host's default printer."],
], [0.26, 0.74])
h2("Servers")
table(["Flag", "Meaning"], [
    ["`--debug [ADDR]`", "Serve the native debug protocol (JSON lines over TCP; "
     "default `127.0.0.1:6510`, loopback only). Headless: serve instead of running "
     "frames, paused until `resume` or `--run`."],
    ["`--vice [ADDR]`", "Also serve the VICE binary monitor protocol for VS64, "
     "C64 Studio and other tools (default `127.0.0.1:6502`); implies `--debug`."],
    ["`--preview [ADDR]`", "Serve the live preview: the viewer page, `/snapshot.png`, "
     "the `/preview` WebSocket stream, the raw `/stream` with audio, the graphics "
     "views under `/gfx` and the CRT profile editor under `/crt` (default "
     "`127.0.0.1:6580`); implies `--debug`."],
], [0.24, 0.76])
note("""The servers listen on the loopback interface; they are meant for tools
on the same computer.""")

# ---- 3 desktop ----------------------------------------------------------------------

h1("mimic-desktop")
code("""mimic-desktop --machine c64 --disk game.d64 --autostart
mimic-desktop --profile mac-se-755
mimic-desktop --machine amiga --disk demo-a.adf --disk demo-b.adf --crt 1084s
mimic-desktop --machine c64 --option port1=stmouse --option drive8=1541""")
h2("Desktop-only flags")
table(["Flag", "Meaning"], [
    ["`--scale N`", "Initial window size in rows per emulated line (default 2, less if "
     "the window would not fit the screen)."],
    ["`--crt PROFILE`", "The monitor: `raw`, `lite`, `1702`, `1084s`, `1084s-rgb`, "
     "`tv`, `pal-tv`, `ntsc-tv`, `pet-white`, `pet-green`, `ctm644`, `gt65`, `mac`, "
     "`logophor`, `logophor-scope`, `vectrex` or a `.toml` file, which is re-read when it changes. Default: the machine's own "
     "monitor."],
    ["`--deinterlace MODE`", "Interlaced pictures (Amiga LACE): `weave` (default, "
     "both fields interleaved) or `bob` (the current field's lines doubled); Ctrl+I "
     "switches at run time."],
    ["`--speed PERCENT`", "Start at 10, 25, 50, 100, 150, 200, 400 or 800 % speed."],
    ["`--warp-load on|off`", "At 100 %: run as fast as possible, muted, while a disk "
     "drive works and the machine has been silent with a still picture for a second, "
     "until the loaded program first shows sound or an animation (default `on`)."],
    ["`--rewind-mb N`", "Memory for rewinding with Ctrl+R (default 128; 0 = off)."],
    ["`--joykeys KEYS`", "Keys that play the joystick: `cursor` (cursor keys, Space "
     "fires; default on the consoles), `numpad` (8/2/4/6, 0 or right Ctrl; default on "
     "the computers) or `off`. Also a switch on the console panel."],
    ["`--joyport N`", "The port the keyboard joystick and the gamepad play, 1-based "
     "(default: the machine's game port, port 2 on the C64/C128)."],
    ["`--no-panel`", "Start with the console panel closed (Shift+Alt+Left opens it)."],
    ["`--media-panel`", "Start with the media side panel open (Shift+Alt+Right)."],
    ["`--footer`", "Start with the status footer shown (Ctrl+Space)."],
], [0.24, 0.76])
h2("Keys of the window")
table(["Key", "Effect"], [
    ["Click on the picture", "Keyboard input goes to the machine; with a mouse, paddles "
     "or a driving controller in a port the click captures the host mouse (the "
     "middle button releases it). The left Ctrl key carries the hotkeys below; the "
     "right Ctrl and AltGr reach the machine."],
    ["Shift+Alt+Left / Right", "Open or close the console panel (left edge) or the "
     "media panel (right edge); the handles on the edges do the same."],
    ["Ctrl+D", "Swap the next `--disk` image (or a dropped one) into the first drive."],
    ["Ctrl+R", "Rewind: hold to run backwards through the ring of savestates."],
    ["Ctrl+I", "Weave or bob deinterlacing (Amiga)."],
    ["PAUSE on the console panel", "Stop the whole machine and let it run again "
     "(lit while stopped, also by a debugger)."],
    ["Ctrl+Space", "Show or hide the status footer: speed slider (10–800 %), snapshot "
     "button (`<machine>-NNN.png`), emulated time, pointer position, joystick "
     "directions, drive activity."],
    ["F12", "Switch between a machine's screens: C128 VIC/VDC, IMSAI front panel "
     "and terminal."],
    ["F1–F7", "Console keys of the Atari 8-bit (F1 START, F2 OPTION … F5 RESET, F6 HELP, "
     "F7 BREAK) and the Atari 2600 (F1 SELECT, F2 RESET, F3 colour/B&W, F4/F5 "
     "difficulty); see `--help`."],
    ["Drop a file on the window", "Disks go into the first drive (swap list), "
     "cartridges into the cartridge slot (the machine powers up again)."],
], [0.3, 0.7])
h2("The console panel")
p("""The panel at the left edge shows PAUSE and the machine's own buttons and
switches: POWER and RESET, the Atari 2600's console switches, the 800XL's
START/SELECT/OPTION, RESTORE on the C64 and VIC-20, 40/80 and CAPS LOCK on
the C128, the Spectrum's MULTIFACE. Below them come the input switches
**JOY KEYS**, **JOY PORT** and **PAD FIRE**, **SAVE MAPPING** (writes the
mapping to `<user folder>/input/<machine>.toml`) and one OFF/ON switch per
loaded cheat. The IMSAI's switches are on its front panel picture instead
and are worked with the mouse.""")
h2("The media panel")
p("""The panel at the right edge lists every drive, datasette and cartridge
slot of the machine with one button that mounts (a native file dialog) or
ejects, and **Save**, which writes the copy-on-write overlay back to the
image file. A datasette or cassette has a second line with its keys
**Play**, **Rec**, **Stop** and **Rew** (there are no hotkeys for them, so
F9–F11 reach the machine). On the C64 the drive's line names its mode: `drive 8 (1541)`
for the true drive with its DOS ROM, `drive 8 (virtual)` without one.""")

# ---- 4 headless ------------------------------------------------------------------

h1("mimic-headless")
code("""mimic-headless --machine c64 --prg demo.prg --frames 300 --snapshot out.png
mimic-headless --machine c128 --option display=80 --frames 600 --screen vdc --snapshot c128.png
mimic-headless --machine amiga --disk wb13.adf --frames 9000 --crt 1084s:2 --wav sound.wav
mimic-headless --machine c64 --vice --preview --run
mimic-headless --list""")
h2("Headless-only flags")
table(["Flag", "Meaning"], [
    ["`--frames N`", "Frames to run (default 1)."],
    ["`--snapshot FILE.png`", "Write the last frame as PNG (raw pixels unless `--crt`)."],
    ["`--snapshot-every N`", "With `--snapshot`: also write `FILE-<frame>.png` every "
     "N frames."],
    ["`--screen NAME`", "Snapshot this screen: `vic` or `vdc` on the C128, the "
     "terminal on the IMSAI (default: the main screen)."],
    ["`--crt PROFILE[:N]`", "Snapshot through a monitor profile (see the desktop's "
     "list) at N rows per line (default 2), computed on the CPU."],
    ["`--wav FILE.wav`", "Write the run's audio as 16-bit PCM at 48 kHz."],
    ["`--load-state FILE`", "Restore a savestate before running."],
    ["`--save-state FILE`", "Write a savestate after running."],
    ["`--run`", "With `--debug`: start running instead of paused."],
    ["`--no-realtime`", "With `--debug`: run as fast as possible instead of at the "
     "machine's speed."],
    ["`--list`", "List the machines with their models and exit."],
], [0.26, 0.74])
h2("Real hardware (feature `real-hardware`)")
p("""Built with `cargo build --release -p emu-headless --features real-hardware`
(or `greaseweazle` / `opencbm` alone) the program also talks to a
Greaseweazle flux reader and, through OpenCBM (`opencbm.dll` in the PATH or
`MIMIC_OPENCBM`), to a ZoomFloppy with a real Commodore drive.""")
table(["Flag", "Meaning"], [
    ["`--import-flux FILE`", "Read a disk through the Greaseweazle into an image: "
     "D64, G64, D71, D81, ADF or IMG by extension."],
    ["`--gw-port PORT`", "The Greaseweazle's serial port (e.g. `COM5`)."],
    ["`--gw-format F`", "`1541`, `1571`, `1581`, `amiga`, `amiga-hd`, `ibm` …"],
    ["`--gw-drive a|b`, `--gw-tracks N`, `--gw-step auto|1|2`, `--gw-revs N`",
     "Drive select, track count, step size and revolutions per track."],
    ["`--import-drive FILE.d64`", "Copy a disk from a real drive on the ZoomFloppy "
     "into an image; `--export-drive FILE.d64` writes one back."],
    ["`--cbm-device N`, `--cbm-type 1541|1571|1581`", "The real drive's device "
     "number and type."],
    ["`--real-drive N`", "Use the real drive as the emulated drive's disk "
     "(`--real-drive-write` allows writes)."],
], [0.36, 0.64])

# ---- 5 machine options ----------------------------------------------------------------

h1("Machine options (--option KEY=VALUE)")
p("""Options configure a machine beyond its model: memory, devices in its
ports, drives and expansions. They are the same in every front end. Unknown
keys and values are refused with a message listing the choices.""")
h2("Options of several machines")
table(["Key", "Machines", "Values"], [
    ["`roms`", "all", "`stub`: the built-in test ROMs"],
    ["`port1`, `port2`", "C64, C128, VIC-20 (`port1` only), C16/Plus/4",
     "`joystick`, `paddles`, `mouse1351`, `lightpen` (port 1 of the C64/C128), the "
     "proportional mice `amigamouse`, `stmouse`, `cx22`, the keypads `rushware`, "
     "`coplin`, `cx85`, `cardkey`, `cx21`, or `none`. Defaults: port 2 joystick, port 1 "
     "none (VIC-20 and Plus/4: port 1 joystick)."],
    ["`port1` … `port4`", "Atari 8-bit", "`joystick`, `paddles`, `lightpen`, `none` "
     "(ports 3 and 4 on the 400/800 only)"],
    ["`port1`, `port2`", "Atari 2600", "`joystick`, `paddles`, `driving`, `keypad`, "
     "`none`"],
    ["`port1`, `port2`", "ColecoVision", "`controller`, `superaction`, `driving`, "
     "`roller`, `none`"],
    ["`port1`, `port2`", "NES", "`controller`, `zapper`, `arkanoid`, `none`; "
     "`fourscore=on` adds ports 3 and 4"],
    ["`port1`, `port2`", "SNES", "`pad`, `mouse`, `superscope`, `none`; `multitap=on`"],
    ["`port0`, `port1`", "Amiga", "`mouse`, `joystick`, `lightpen`, `none` (default "
     "mouse in port 0, joystick in port 1)"],
    ["`port`", "Apple II", "`joystick`, `paddles`, `none`"],
    ["`port1`, `port2`", "Vectrex", "`controller`, `lightpen`, `imager`, `none`"],
    ["`joy1`, `joy2`", "CPC", "`joystick`, `amx` (mouse on joy1), `none`; `joy2` "
     "daisy-chains a second stick"],
    ["`expansion`", "CPC", "`lightpen`"],
    ["`printer`", "C64, C128, VIC-20, C16/Plus/4, PET, CPC, Amiga, Atari 8-bit, Mac",
     "`on`, or the device number `4`/`5` on the Commodore serial bus; "
     "`printermodel=fx80|p6|imagewriter`"],
], [0.14, 0.26, 0.6])
h2("Commodore 8-bit")
table(["Key", "Machine", "Values"], [
    ["`drive8`", "C64, C128, VIC-20, Plus/4", "`1541` (true drive with its DOS ROM; "
     "the C128 `1571`/`1581`; the C16/Plus/4 also `1551` on the TCBM bus), `virtual` "
     "(KERNAL-level file access, the default without a DOS ROM), `none`"],
    ["`drive8_ppm`", "C64", "the drive crystal's deviation in ppm"],
    ["`fastload`", "C64", "`on`: the virtual drive serves the KERNAL's serial routines "
     "without bus time"],
    ["`debugcart`", "C64", "`on`: the test-bench debug cartridge at `$D7FF`"],
    ["`cia`", "C64", "`6526` (old), `6526a` (new)"],
    ["`sid`", "C64", "`6581`, `8580`"],
    ["`sidcard`", "C16/Plus/4", "`6581` (also `on`), `8580`: a SID card at `$FD40` "
     "(Solder's card, clocked at the Plus/4's single clock); default `off`"],
    ["`vicii`", "C64", "`6569`, `8565` (PAL), `6567`, `8562` (NTSC)"],
    ["`reu`", "C64", "`128k`, `256k`, `512k`, `1m` … `16m`: a REU (REC 8726)"],
    ["`uci`", "C64", "`on`: the Ultimate 64 command interface at `$DF1C`–`$DF1F` "
     "(DOS, network, control, software IEC, HTTP); see `--uci-dir`, `--uci-mount`"],
    ["`userport`", "C64", "`wic64`: the WiC64 (HTTP, TCP, time) on the user port"],
    ["`ram`", "VIC-20", "`none`, `3k`, `8k`, `16k`, `24k`, `32k`, `35k`"],
    ["`hz`", "PET", "`50`, `60`"],
    ["`display`", "C128", "`40`, `80`: the screen the machine starts in"],
], [0.14, 0.22, 0.64])
h2("Atari")
table(["Key", "Machine", "Values"], [
    ["`basic`", "Atari 8-bit", "`off`: XL/XE self test, 400/800 without the BASIC cartridge"],
    ["`ram`", "Atari 8-bit", "400/800: `8k`, `16k`, `32k`, `48k`; XL/XE: `64k`, `128k` "
     "(130XE banks), `320k` (RAMBO), `320k-compy` (Compy Shop)"],
    ["`drives`", "Atari 8-bit", "`0` … `4` SIO drives; `drive=810|1050|xf551`"],
    ["`bank`", "Atari 2600", "the cartridge's bank switching when detection fails: "
     "`2k`, `4k`, `f8`, `f6`, `f4`, `fe`, `e0`, `e7`, `3f`, `fa`, `dpc` …"],
], [0.14, 0.22, 0.64])
h2("Amiga")
table(["Key", "Values"], [
    ["`kickstart`", "`1.2`, `1.3`, `2.0`, `3.0`, `3.1` (the A1200 takes its own 3.1 "
     "image when present)"],
    ["`chip`", "`256k`, `512k`, `1m`, `2m`"],
    ["`slow`", "`512k`, `0`"],
    ["`fast`", "`0`, `512k` … `8m` (Zorro II autoconfig)"],
    ["`chipset`, `agnus`, `denise`", "`ocs`, `ecs`, `aga` (chipset); `ocs`, `ecs` "
     "for the chips alone"],
    ["`drives`, `drivetype`", "`1` … `4` floppies; `dd`, `hd`"],
    ["`serial`", "`none`, `loopback`"],
    ["`wcs`", "A1000: `rom`, `disk` (Kickstart from a bootstrap disk)"],
], [0.26, 0.74])
h2("Other machines")
table(["Key", "Machine", "Values"], [
    ["`crtc`", "CPC", "`0`, `1`, `2`: the CRTC type"],
    ["`dos`", "CPC", "`amsdos`, `parados`, `none`"],
    ["`ram`, `lc`, `aux`", "Apple II", "`4k` … `48k`; language card `on|off` (II/II+); "
     "auxiliary memory `64k|1k|none` (IIe)"],
    ["`slot4`, `slot6`", "Apple II", "`mockingboard|none`; `disk2|none`"],
    ["`ram`", "Mac", "`512k`, `1m`, `2m`, `4m` (Plus and SE)"],
    ["`serial`", "Mac", "`loopback`"],
    ["`os`, `ram`", "Archimedes", "`3.11`, `3.10`, `3.00`, `2.00`, `1.20`; `512k`, "
     "`1m`, `2m`, `4m`"],
    ["`traps`", "Spectrum, ZX81", "`off`: no tape traps (real-time loading only)"],
    ["`joystick`, `mouse`", "Spectrum", "`kempston`, `sinclair1`, `sinclair2`, "
     "`cursor`, `none`; `mouse=kempston`"],
    ["`issue`, `multiface`, `if1`, `microdrives`, `beta`", "Spectrum",
     "`issue=2`; `multiface=on`; `if1=on` with `microdrives=1..8`; `beta=on` (TR-DOS)"],
    ["`ram`", "ZX81", "`1k`, `2k`, `16k`"],
    ["`ram`, `baud`, `reader`, `uppercase`, `mpu`, `load`", "IMSAI",
     "`1k` … `64k`; `9600`; `card|console`; `off`; `a|b` (monitor ROM); `HEX`"],
    ["`port1`, `port2`", "Vectrex", "`controller`, `lightpen`, `imager`; `eye`, "
     "`lines=fine|soft`"],
    ["`card`, `rtc`, `port0`, `port1`, `net`, `mac`", "Logophor 16", "`card=system` puts in the "
     "built-in system card (Boiler and the desktop Panorama; the profile `logophor16` sets it, "
     "writes are lost at the end; `--disk` takes its place), `none` (default); `rtc`: seconds since 2000-01-01 "
     "(the clock's start; default 2026-01-01); `mouse|joystick|none` (defaults: mouse in "
     "port 0, joystick in port 1); `net=host` puts the hub's gateway into the machine (10.16.0.1: DHCP, DNS and the Internet through the host's network, no hub needed; the profile `logophor16` sets it), `net=HOST:PORT` joins the network of an `ada-asm hub` (also the net's gateway; `--no-gateway` relays only) "
     "(default port 6516); `mac=02:4c:31:36:00:01`. The SD card is a FAT32 image given with `--disk` "
     "(`ada-asm sd card.img --boot prog.bin`); Foundry starts `BOOT/BOILER.BIN` "
     "from it, a key held while the start picture shows opens its monitor"],
], [0.26, 0.18, 0.56])
h2("Keypads and proportional mice on the Commodore ports")
p("""Besides the joystick, paddles and the 1351 mouse, the Commodore control
ports take devices that software with its own driver reads as joystick
lines:""")
bullets(
    "`amigamouse`, `stmouse`, `cx22`: the Amiga mouse, the Atari ST mouse (also the "
    "later CX80 trak-ball) and the Atari CX22 trak-ball. The captured host mouse "
    "drives them; a click on the picture captures it.",
    "`rushware`, `coplin`, `cx85`, `cardkey`: numeric keypads. The host's numeric "
    "keypad works them (digits, `.`, Enter, `+`, `-`, `*`, `/`), plus Escape, N, Y, "
    "Delete, P and R for the keys some of them have. While a keypad is plugged in the "
    "keyboard joystick moves to the cursor keys.",
    "`cx21`: the Atari keypad (1 2 3 / 4 5 6 / 7 8 9 / * 0 #) on the host's numeric "
    "keypad, `*` on multiply and `#` on divide; it needs a port with POT lines.",
)

# ---- 6 profiles -----------------------------------------------------------------------

h1("Start profiles (profiles.yaml)")
p("""A start profile is a named machine setup: the machine, its model and
video format, the ROMs, the machine options, the media to insert, the
monitor and window settings, the input settings and the servers. Instead of
typing a long command line each time, you start the profile by name, pick
it from a list in the desktop window, or choose it on the web page.""")
h2("Where the file lives")
bullets(
    "`profiles.yaml` in the working folder (the folder you start mimic from), or",
    "the user's config folder: `%APPDATA%/mimic/profiles.yaml` on Windows, "
    "`~/.config/mimic/profiles.yaml` on Linux and macOS, or",
    "any file named with `--profiles FILE`.",
    "`docs/profiles.example.yaml` in the repository is a commented starting point; "
    "copy it to one of the places above.",
)
h2("Starting a profile")
table(["Where", "How"], [
    ["Command line", "`mimic-desktop --profile NAME` or `mimic-headless --profile NAME`. "
     "Any flag given on the same command line overrides the profile's value; "
     "`--option` and `--rom` values merge by key, a `--disk` list replaces the "
     "profile's disks."],
    ["Desktop window", "Start `mimic-desktop` without `--machine` and `--profile`: when a "
     "profiles file is found, the window shows its name and lists the profiles with "
     "their descriptions. Click one to start it. If a profile cannot start (a ROM "
     "missing, a wrong option), the reason is shown under the list and you can pick "
     "another. Without a profiles file the C64 starts as before."],
    ["Web page", "**Profiles…** reads a `profiles.yaml` from your disk; the profiles "
     "then appear in the **start profile…** list next to it, and choosing one starts "
     "it. Media and ROM paths in the profile are matched by file name against the "
     "files uploaded to the page, so upload the images (and the ROM folder) first."],
    ["Listing", "`--list-profiles` prints every profile with its description and exits."],
    ["Saving", "`--save-profile NAME` stores the current command line's setup as a "
     "profile in the file in use (created if it does not exist), then starts."],
], [0.18, 0.82])
h2("The file")
p("""Two top-level keys: `settings`, shared by every profile, and `profiles`,
one entry per name. Relative paths count from the profile file's folder;
media paths from `settings.media` when it is set.""")
code("""settings:
  roms: roms                 # ROM folder when a profile names none (then $EMU_ROM_DIR)
  snapshots: snapshots       # where the desktop's snapshot button saves
  media: disks               # base folder for relative media paths

profiles:
  mac-se-755:
    description: Mac SE 4 MB with System 7.5.5 on a SCSI disk
    machine: mac
    model: se
    options: { ram: 4m }
    media:
      disks: [system755.hda]   # each goes to the next drive that takes it
    display: { crt: mac, scale: 2, footer: true, panel: false }

  c64-game:
    description: C64 NTSC with a 1351 mouse and a disk that starts itself
    machine: c64
    videoformat: ntsc
    roms: c64roms            # this profile's own ROM folder
    rom: { kernal: kernal.901227-03.bin }   # or single files by role
    options: { port1: mouse1351, drive8: "1541" }
    media: { disks: [game.d64], autostart: true }   # also tape:, cart:, prg:
    input: { joykeys: cursor, joyport: 2, speed: 100 }

  amiga-dev:
    description: Amiga 500 with 1 MB chip RAM and the debugger
    machine: amiga
    options: { chip: 1m, drives: 2 }
    media: { disks: [workbench.adf, extras.adf] }
    display: { deinterlace: weave }
    debug: { server: true, preview: "127.0.0.1:6580", vice: false }""")
table(["Key", "Meaning", "Command-line equivalent"], [
    ["`description`", "One line shown in the picker and by `--list-profiles`.", "–"],
    ["`machine`, `model`, `videoformat`", "What to run.", "`--machine`, `--model`, "
     "`--videoformat`"],
    ["`roms`", "The ROM folder for this profile.", "`--rom-dir`"],
    ["`rom`", "Single ROM files by role (`kernal`, `basic`, `kick13` …).", "`--rom ROLE=FILE`"],
    ["`options`", "Machine options, one key per line or in braces.", "`--option KEY=VALUE`"],
    ["`media.disks`", "Disk images in order; each goes to the next drive that takes it.",
     "`--disk` (repeated)"],
    ["`media.tape`, `media.cart`, `media.prg`", "One tape, cartridge or program.",
     "`--tape`, `--cart`, `--prg`"],
    ["`media.autostart`", "Load and run after boot.", "`--autostart`"],
    ["`display.crt`", "Monitor preset or `.toml` file.", "`--crt`"],
    ["`display.scale`", "Window rows per emulated line.", "`--scale`"],
    ["`display.footer`, `display.panel`", "Status footer shown; console panel open.",
     "`--footer`, `--no-panel`"],
    ["`display.deinterlace`", "`weave` or `bob`.", "`--deinterlace`"],
    ["`input.joykeys`", "`cursor`, `numpad` or `off`.", "`--joykeys`"],
    ["`input.joyport`", "Port the keyboard joystick and gamepad play.", "`--joyport`"],
    ["`input.speed`", "Percent of real time.", "`--speed`"],
    ["`debug.server`", "`true` (127.0.0.1:6510) or an address.", "`--debug [ADDR]`"],
    ["`debug.preview`", "`true` (127.0.0.1:6580) or an address.", "`--preview [ADDR]`"],
    ["`debug.vice`", "`true` (127.0.0.1:6502) or an address.", "`--vice [ADDR]`"],
    ["`settings.roms`", "ROM folder for profiles without `roms`.", "`$EMU_ROM_DIR`"],
    ["`settings.snapshots`", "Folder for the footer's snapshot button.", "–"],
    ["`settings.media`", "Base folder for relative media paths.", "–"],
], [0.3, 0.44, 0.26])
h2("Precedence")
p("""The ROM folder comes from the command line, then the profile's `roms`,
then `settings.roms`, then `EMU_ROM_DIR`, then `~/.mimic/roms`. Everything else: a profile is
turned into a command line and the flags you type are laid over it, so a
typed value replaces the profile's; options and ROM roles merge by key;
a typed disk list replaces the profile's disks. The headless program takes
the same file and ignores the `display` and `input` keys it has no use for.""")

# ---- 7 web -------------------------------------------------------------------------------

h1("The web front end")
p("""The web front end is the emulator compiled to WebAssembly with a page
around it. Nothing is bundled: ROMs, disks, tapes and cartridges are
uploaded in the browser and can be kept there (IndexedDB), only in that
browser. Every machine runs; the desktop's printer output, host network
and real-hardware backends are not available on the web.""")
h2("Building and serving")
code("""bash tools/build-web.sh        # -> target/web: mimic.wasm, index.html, mimic.js, mimic-worker.js
python -m http.server -b 127.0.0.1 -d target/web 8080
# then open http://127.0.0.1:8080/
node tools/web-smoke.mjs       # every machine starts in the wasm module (no browser)""")
note("""The page must come from an http server, not from a `file:` URL: the
browser refuses to load the WebAssembly module and the worker from local
files. Any static server will do; nothing on the server side runs.""")
h2("The page")
table(["Area", "What it does"], [
    ["Machine", "Machine, model and video format; an options field for `KEY=VALUE` "
     "words; the **built-in test ROMs** box (`roms=stub`); **Start** and **Stop**. "
     "Below it the ROMs the machine needs and the optional ones (a drive DOS ROM "
     "makes the drive a true 1541 instead of a virtual one)."],
    ["Files", "**Add files…** uploads single files, **Add ROM folder…** a whole folder "
     "(like `--rom-dir`: subfolders per machine, names and sizes as in the ROM "
     "search). Files dropped on the page are added; files dropped on the picture go "
     "into the matching drive. **keep files in this browser** stores them in "
     "IndexedDB, so they are there on the next visit."],
    ["Drives and slots", "One line per drive, datasette and cartridge slot with the "
     "image in it and a **Mount** / **Eject** button (Mount opens a file dialog)."],
    ["Cheats", "The loaded freezes as check boxes; **Import** (POKE list, address/value "
     "codes, VICE monitor commands, TOML), export, and the memory search with a "
     "**Freeze** button per result. `cheats/<machine>/<crc32>.toml` in an uploaded "
     "folder applies to the matching image."],
    ["State", "**Save state** downloads a `.mimicstate` file, **Load state…** restores "
     "one; **Profiles…** reads a `profiles.yaml` and lists its profiles."],
    ["Header", "**Pause**, **Reset** (Shift+click: power cycle), the **Sound** box, "
     "the **Monitor** choice (raw pixels, the machine's monitor, lite) and the "
     "**Scale** (1–4)."],
    ["Console panel", "Below the picture: the machine's buttons and switches, as in "
     "the desktop."],
    ["Status line", "What runs and at how many frames per second, or the last error."],
], [0.2, 0.8])
h2("Input")
bullets(
    "Click on the picture: keys go to the machine by position, as in the desktop. "
    "Keys the browser would use itself (scrolling, find) stay with the machine while "
    "the picture has the focus.",
    "Gamepads with the standard mapping play the joystick (d-pad or left stick, "
    "buttons by the machine's mapping).",
    "A mouse or paddles in a port capture the pointer on a click (Esc releases it); "
    "a light pen, Zapper or Super Scope aims with the pointer over the picture.",
    "Keypads in a Commodore port take the numeric keypad; the joystick keys move to "
    "the cursor keys.",
)
h2("URL parameters")
p("""The page starts a machine at once when the URL names one:""")
code("""http://127.0.0.1:8080/?machine=c64&options=roms=stub&start=1
http://127.0.0.1:8080/?machine=atari8&model=130xe&videoformat=ntsc&options=ram=320k,basic=off&crt=default&start=1""")
table(["Parameter", "Meaning"], [
    ["`machine=ID`", "The machine id (required for an automatic start)."],
    ["`model=NAME`", "The model."],
    ["`videoformat=pal|ntsc`", "The video format."],
    ["`options=KEY=VALUE,...`", "Machine options separated by commas."],
    ["`crt=raw|default|lite`", "The monitor: sharp pixels, the machine's own monitor "
     "profile, or the lite preset."],
    ["`start=1`", "Start without pressing the button (the ROMs must already be in "
     "the browser's storage, or `options=roms=stub`)."],
], [0.3, 0.7])
h2("How the page runs")
bullets(
    "The emulation runs in a **Web Worker** (`mimic-worker.js`), which owns the "
    "WebAssembly module, paces the frames at the machine's rate and answers the "
    "page's commands; the page stays responsive and only draws the pictures it is "
    "sent.",
    "Sound goes from the worker straight to an **AudioWorklet** through a port the "
    "page hands over; the Sound box mutes it. Browsers start audio only after a "
    "click on the page.",
    "The picture is drawn with **WebGL2**: raw pixels through a pass-through shader, "
    "a monitor through the CRT shader (the same model as the desktop's and the "
    "preview's). The composite signal stage (PAL colour blending, NTSC artefacts) runs "
    "in the WebAssembly module. Without WebGL2 the module renders on the CPU and the "
    "page shows it on a 2D canvas.",
    "A savestate is the machine's complete state; it loads only into the same "
    "machine with the same ROMs and media.",
)
h2("The live preview (a different thing)")
p("""The preview server of `mimic-desktop --preview` and `mimic-headless
--preview` is not the web front end: there the emulator runs natively and
the browser only shows its picture, forwards keyboard, gamepad and mouse
input, works the console panel, browses the graphics views and edits CRT
profiles. Open `http://127.0.0.1:6580/` for its viewer; the user guide
describes its endpoints (chapter 8).""")

