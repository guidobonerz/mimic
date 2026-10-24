"""Build the mimic user guide as a PDF.

    python tools/user_guide.py [docs/mimic-user-guide.pdf]

Needs reportlab (pip install reportlab). The text lives in this file; keep
it in step with `docs/usage.md` and the frontends' --help output.
"""

import os
import re
import sys
from datetime import date
from xml.sax.saxutils import escape

from reportlab.lib import colors
from reportlab.lib.enums import TA_CENTER
from reportlab.lib.pagesizes import A4
from reportlab.lib.styles import ParagraphStyle
from reportlab.lib.units import cm
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.platypus import (
    BaseDocTemplate,
    CondPageBreak,
    Frame,
    KeepTogether,
    PageBreak,
    PageTemplate,
    Paragraph,
    Preformatted,
    Spacer,
    Table,
    TableStyle,
)
from reportlab.platypus.tableofcontents import TableOfContents

# ---------------------------------------------------------------- fonts --

FONT_DIRS = [
    r"C:\Windows\Fonts",
    "/usr/share/fonts/truetype/dejavu",
    "/Library/Fonts",
]


def register_fonts():
    """Segoe UI + Consolas on Windows, DejaVu elsewhere (both have the
    arrows and the pound sign the key tables use)."""
    candidates = [
        ("segoeui.ttf", "segoeuib.ttf", "segoeuii.ttf", "consola.ttf", "consolab.ttf"),
        ("DejaVuSans.ttf", "DejaVuSans-Bold.ttf", "DejaVuSans-Oblique.ttf",
         "DejaVuSansMono.ttf", "DejaVuSansMono-Bold.ttf"),
    ]
    for d in FONT_DIRS:
        for names in candidates:
            paths = [os.path.join(d, n) for n in names]
            if all(os.path.exists(p) for p in paths):
                for name, path in zip(["Sans", "Sans-Bold", "Sans-Italic", "Mono", "Mono-Bold"], paths):
                    pdfmetrics.registerFont(TTFont(name, path))
                pdfmetrics.registerFontFamily(
                    "Sans", normal="Sans", bold="Sans-Bold", italic="Sans-Italic",
                    boldItalic="Sans-Bold")
                pdfmetrics.registerFontFamily(
                    "Mono", normal="Mono", bold="Mono-Bold", italic="Mono",
                    boldItalic="Mono-Bold")
                return
    sys.exit("no suitable TrueType fonts found (Segoe UI/Consolas or DejaVu)")


register_fonts()

# --------------------------------------------------------------- styles --

INK = colors.HexColor("#1d2330")
ACCENT = colors.HexColor("#2f5d9e")
MUTED = colors.HexColor("#5b6475")
RULE = colors.HexColor("#c9d1de")
CODE_BG = colors.HexColor("#f2f4f8")
HEAD_BG = colors.HexColor("#e4eaf3")

BASE = ParagraphStyle("base", fontName="Sans", fontSize=9.6, leading=13.4,
                      textColor=INK, spaceAfter=5)
STYLES = {
    "title": ParagraphStyle("title", parent=BASE, fontName="Sans-Bold", fontSize=30,
                            leading=36, textColor=ACCENT, alignment=TA_CENTER),
    "subtitle": ParagraphStyle("subtitle", parent=BASE, fontSize=13, leading=18,
                               textColor=MUTED, alignment=TA_CENTER),
    "h1": ParagraphStyle("h1", parent=BASE, fontName="Sans-Bold", fontSize=17,
                         leading=22, textColor=ACCENT, spaceBefore=4, spaceAfter=8),
    "h2": ParagraphStyle("h2", parent=BASE, fontName="Sans-Bold", fontSize=12.5,
                         leading=16, textColor=INK, spaceBefore=10, spaceAfter=5),
    "h3": ParagraphStyle("h3", parent=BASE, fontName="Sans-Bold", fontSize=10.5,
                         leading=14, textColor=INK, spaceBefore=7, spaceAfter=3),
    "body": BASE,
    "bullet": ParagraphStyle("bullet", parent=BASE, leftIndent=14, bulletIndent=4,
                             spaceAfter=2.5),
    "cell": ParagraphStyle("cell", parent=BASE, fontSize=8.6, leading=11.2, spaceAfter=0),
    "cellhead": ParagraphStyle("cellhead", parent=BASE, fontName="Sans-Bold", fontSize=8.6,
                               leading=11.2, spaceAfter=0),
    "code": ParagraphStyle("code", fontName="Mono", fontSize=8.2, leading=10.6,
                           textColor=INK),
    "note": ParagraphStyle("note", parent=BASE, fontSize=9, leading=12.6,
                           textColor=INK, leftIndent=8, rightIndent=8),
    "toc1": ParagraphStyle("toc1", parent=BASE, fontName="Sans-Bold", fontSize=10.5,
                           leading=15, leftIndent=0),
    "toc2": ParagraphStyle("toc2", parent=BASE, fontSize=9.4, leading=12.5,
                           leftIndent=16, textColor=MUTED),
}

# ---------------------------------------------------------- inline text --

INLINE = re.compile(r"`([^`]+)`|\*\*([^*]+)\*\*|\*([^*]+)\*")


def rich(text):
    """`code`, **bold** and *italic* into reportlab markup; the rest escaped."""
    out, pos = [], 0
    for m in INLINE.finditer(text):
        out.append(escape(text[pos:m.start()]))
        code, bold, ital = m.groups()
        if code is not None:
            out.append('<font face="Mono" color="#24406b">%s</font>' % escape(code))
        elif bold is not None:
            out.append("<b>%s</b>" % escape(bold))
        else:
            out.append("<i>%s</i>" % escape(ital))
        pos = m.end()
    out.append(escape(text[pos:]))
    return "".join(out)


# --------------------------------------------------------------- blocks --

story = []
_counters = [0, 0]


def h1(text):
    _counters[0] += 1
    _counters[1] = 0
    story.append(PageBreak())
    p = Paragraph("%d&nbsp;&nbsp;%s" % (_counters[0], escape(text)), STYLES["h1"])
    p.toc_level = 0
    story.append(p)


def h2(text):
    _counters[1] += 1
    story.append(CondPageBreak(3 * cm))
    p = Paragraph("%d.%d&nbsp;&nbsp;%s" % (_counters[0], _counters[1], escape(text)),
                  STYLES["h2"])
    p.toc_level = 1
    story.append(p)


def h3(text):
    story.append(CondPageBreak(2.2 * cm))
    story.append(Paragraph(rich(text), STYLES["h3"]))


def p(text):
    story.append(Paragraph(rich(" ".join(text.split())), STYLES["body"]))


def bullets(*items):
    for it in items:
        story.append(Paragraph(rich(" ".join(it.split())), STYLES["bullet"], bulletText="•"))
    story.append(Spacer(1, 3))


def code(text):
    text = text.strip("\n")
    pre = Preformatted(text, STYLES["code"])
    t = Table([[pre]], colWidths=[None])
    t.setStyle(TableStyle([
        ("BACKGROUND", (0, 0), (-1, -1), CODE_BG),
        ("BOX", (0, 0), (-1, -1), 0.5, RULE),
        ("LEFTPADDING", (0, 0), (-1, -1), 7),
        ("RIGHTPADDING", (0, 0), (-1, -1), 7),
        ("TOPPADDING", (0, 0), (-1, -1), 5),
        ("BOTTOMPADDING", (0, 0), (-1, -1), 5),
    ]))
    story.append(t)
    story.append(Spacer(1, 6))


def note(text, label="Note"):
    para = Paragraph("<b>%s.</b> %s" % (label, rich(" ".join(text.split()))), STYLES["note"])
    t = Table([[para]], colWidths=[None])
    t.setStyle(TableStyle([
        ("BACKGROUND", (0, 0), (-1, -1), colors.HexColor("#eef3fb")),
        ("LINEBEFORE", (0, 0), (0, -1), 2.5, ACCENT),
        ("LEFTPADDING", (0, 0), (-1, -1), 6),
        ("TOPPADDING", (0, 0), (-1, -1), 5),
        ("BOTTOMPADDING", (0, 0), (-1, -1), 5),
    ]))
    story.append(t)
    story.append(Spacer(1, 7))


def table(head, rows, widths):
    """`widths` in fractions of the text width."""
    total = A4[0] - 4.2 * cm
    data = [[Paragraph(rich(h), STYLES["cellhead"]) for h in head]]
    for r in rows:
        data.append([Paragraph(rich(" ".join(str(c).split())), STYLES["cell"]) for c in r])
    t = Table(data, colWidths=[w * total for w in widths], repeatRows=1)
    t.setStyle(TableStyle([
        ("BACKGROUND", (0, 0), (-1, 0), HEAD_BG),
        ("LINEBELOW", (0, 0), (-1, 0), 0.8, ACCENT),
        ("LINEBELOW", (0, 1), (-1, -1), 0.3, RULE),
        ("VALIGN", (0, 0), (-1, -1), "TOP"),
        ("LEFTPADDING", (0, 0), (-1, -1), 4),
        ("RIGHTPADDING", (0, 0), (-1, -1), 4),
        ("TOPPADDING", (0, 0), (-1, -1), 3),
        ("BOTTOMPADDING", (0, 0), (-1, -1), 3),
    ]))
    story.append(t)
    story.append(Spacer(1, 8))


# ============================================================== content ==

# ---- title page and contents -------------------------------------------

story.append(Spacer(1, 6 * cm))
story.append(Paragraph("mimic", STYLES["title"]))
story.append(Spacer(1, 0.4 * cm))
story.append(Paragraph("Imitates Machines, Inspects Code", STYLES["subtitle"]))
story.append(Spacer(1, 1.2 * cm))
story.append(Paragraph("User Guide", ParagraphStyle(
    "ug", parent=STYLES["subtitle"], fontName="Sans-Bold", fontSize=18, leading=24,
    textColor=INK)))
story.append(Spacer(1, 0.3 * cm))
story.append(Paragraph(
    "Running the emulator: desktop, command line, media, controllers, "
    "live preview and debugger", STYLES["subtitle"]))
story.append(Spacer(1, 5 * cm))
story.append(Paragraph(date.today().strftime("%d %B %Y"), STYLES["subtitle"]))
story.append(PageBreak())

toc = TableOfContents()
toc.levelStyles = [STYLES["toc1"], STYLES["toc2"]]
story.append(Paragraph("Contents", STYLES["h1"]))
story.append(toc)

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
table(["Machine id", "Machine", "Models (--model)"], [
    ["`c64`", "Commodore 64", "`pal` (default), `ntsc`"],
    ["`vic20`", "Commodore VIC-20", "`pal`, `ntsc`"],
    ["`pet`", "Commodore PET", "`4032` (default), `2001`, `3032`, `8032`"],
    ["`c16`, `plus4`", "Commodore 16, Plus/4", "`pal`, `ntsc`"],
    ["`c128`", "Commodore 128 (40 and 80 columns, Z80)", "`pal`, `ntsc`"],
    ["`atari8`", "Atari 400/800/800XL/65XE/130XE",
     "`800xl` (default), `65xe`, `130xe`, `800`, `400`; each also `-ntsc`"],
    ["`atari2600`", "Atari 2600 (VCS)", "`ntsc` (default), `pal`"],
    ["`cpc`", "Amstrad CPC", "`6128` (default), `664`, `464`"],
    ["`amiga`", "Commodore Amiga 500/1000/2000 (OCS), 500+ (ECS), 1200 (AGA)",
     "`500` (default), `1000`, `2000`, `500plus`, `1200`; each also `-ntsc`"],
    ["`zx81`", "Sinclair ZX81 / Timex Sinclair 1000", "`zx81` (default, PAL), `ts1000` (NTSC)"],
    ["`coleco`", "ColecoVision", "`ntsc` (default), `pal`"],
    ["`nes`", "Nintendo Entertainment System / Famicom", "`ntsc` (default), `pal`"],
    ["`snes`", "Super Nintendo / Super Famicom", "`ntsc` (default), `pal`"],
    ["`vectrex`", "Vectrex (vector display)", "`vectrex`"],
    ["`apple2`", "Apple II, II+, IIe", "`iie` (enhanced IIe, default), `iie-unenhanced`, "
     "`iiplus`, `ii`"],
    ["`mac`", "Apple Macintosh 128K, 512K, Plus, SE", "`plus` (default), `128k`, `512k`, `se`"],
    ["`sim6502`", "6502 reference machine for tests", "`default`"],
], [0.16, 0.44, 0.40])
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
can be copied anywhere; ROMs are found through `--rom-dir` or the
environment variable `EMU_ROM_DIR`.""")
note("""Older builds left files called `emu-desktop.exe` and `emu-headless.exe`
in `target/release/`. They are outdated (they do not know the newer
machines); use `mimic-desktop.exe` and `mimic-headless.exe`, or start
through `cargo run` as shown below, which always builds the current
version first.""", "Watch out")
h2("Starting a machine")
p("Two equivalent ways to start the desktop emulator:")
code("""cargo run --release -p emu-desktop -- --machine c64 --rom-dir roms
.\\target\\release\\mimic-desktop.exe --machine c64 --rom-dir roms""")
p("Everything after `--` goes to the emulator. Some first steps:")
code("""# C64 with a disk, loaded and started automatically
mimic-desktop --machine c64 --rom-dir roms --disk game.d64 --autostart

# Amiga 500 booting Workbench 1.3 from an ADF
mimic-desktop --machine amiga --rom-dir roms --disk workbench13.adf

# NES and SNES need no system ROM
mimic-desktop --machine nes --cart game.nes
mimic-desktop --machine snes --cart game.sfc

# Vectrex with a cartridge
mimic-desktop --machine vectrex --rom-dir roms --cart "Scramble (1982).vec\"""")
p("`mimic-desktop --help` lists every option with the key assignments.")

# ---- 3 roms --------------------------------------------------------------

h1("ROMs")
p("""Most machines need their original system ROMs (KERNAL, BASIC,
Kickstart, operating system, BIOS). They are copyrighted and therefore
**not** part of mimic: you supply them from your own machines. Put them in a
folder and pass it with `--rom-dir DIR`, or set `EMU_ROM_DIR` once.""")
h2("How ROMs are found")
bullets(
    "mimic looks in the machine's subfolder first (for example `roms/c64/`), "
    "then in `roms/drives/`, then in the folder itself.",
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
    ["`drives/`", "Disk drives", "`dos1541`, `dos1571`, `dos1581`"],
], [0.14, 0.16, 0.70])
p("The Atari 2600 and the NES have no system ROM: the program is on the "
  "cartridge.")
note("""Many machines accept `--option roms=stub`: a tiny built-in program
instead of the real ROMs. It is meant for tests (it shows a test screen or
starts simple cartridges); real software needs the original ROMs.""")

# ---- 4 desktop -------------------------------------------------------------

h1("The desktop emulator")
h2("Command-line options")
table(["Option", "Meaning"], [
    ["`--machine ID`", "the machine (see the table in section 1; default `c64`)"],
    ["`--model NAME`", "model variant, e.g. `ntsc`, `130xe`, `1200`"],
    ["`--rom-dir DIR`", "ROM folder (default: `EMU_ROM_DIR`)"],
    ["`--option KEY=VALUE`", "machine option, repeatable (section 5)"],
    ["`--prg FILE`", "a program (PRG, T64, Atari XEX) to load and run"],
    ["`--disk FILE`", "a disk image; repeat for further drives, extra disks form a swap list"],
    ["`--tape FILE`", "a tape image (TAP, CAS, CDT/TZX, ZX81 P)"],
    ["`--cart FILE`", "a cartridge or console ROM"],
    ["`--autostart`", "load and run the first program on the disk after boot"],
    ["`--crt PROFILE`", "monitor look (section 7; default: the machine's own monitor)"],
    ["`--scale N`", "initial window size in screen rows per picture line (default 2; "
     "reduced automatically so the window fits the screen)"],
    ["`--deinterlace weave|bob`", "Amiga interlace: both fields woven (default) or the "
     "current field's lines doubled"],
    ["`--joykeys cursor|numpad|off`", "which keys play the joystick"],
    ["`--joyport N`", "which joystick port the keys and the gamepad play (1-based)"],
    ["`--footer`", "start with the status footer shown"],
    ["`--no-panel`", "start with the console panel hidden"],
    ["`--debug [ADDR]`", "serve the debug protocol (default 127.0.0.1:6510)"],
    ["`--preview [ADDR]`", "serve the live preview (default 127.0.0.1:6580)"],
    ["`--vice [ADDR]`", "serve the VICE binary monitor (default 127.0.0.1:6502)"],
], [0.30, 0.70])

h2("Hotkeys")
table(["Key", "Action"], [
    ["Ctrl+P", "show or hide the console panel"],
    ["Ctrl+Space", "show or hide the status footer"],
    ["Ctrl+D", "put the next disk of the `--disk` list into the first drive"],
    ["Ctrl+I", "Amiga: switch the deinterlacing between weave and bob"],
    ["F12", "switch between the machine's screens (C128: 40-column VIC and 80-column VDC)"],
    ["F9 / Shift+F9", "datasette or cassette: PLAY / RECORD"],
    ["F10 / F11", "datasette or cassette: STOP / REWIND"],
    ["Middle mouse button", "release a captured mouse"],
], [0.28, 0.72])
p("Dropping an image file onto the window inserts it into the first drive "
  "(disks) or the cartridge slot (`.vec`, `.crt`, `.car`, `.a26`).")

h2("The console panel")
p("""Below the picture, a bar shows the buttons and switches of the machine's
case: POWER and RESET, the Atari 2600 console switches, START/SELECT/OPTION
of the 800XL, RESTORE of the C64 and VIC-20, 40/80 and CAPS LOCK of the
C128, and so on. A button is pressed for as long as you hold the mouse
button on it; clicking a switch position moves the switch.""")
bullets(
    "**PAUSE** (far left) halts the whole machine and lets it run again. It is "
    "lit while the machine stands — also when a debugger halted it — and the "
    "title bar shows where.",
    "**JOY KEYS** chooses the keys that play the joystick: CURSOR (cursor keys, "
    "Space fires), NUMPAD (8/2/4/6, fire on 0 or right Ctrl) or OFF (gamepad "
    "only).",
    "**JOY PORT** (machines with several joystick ports) moves the keyboard "
    "joystick and the gamepad to another port.",
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
     "(`--profile c64 --model ntsc`)"],
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
100 %). Next to it the **snapshot** button saves the picture as a PNG file
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
mimic-desktop --machine amiga --rom-dir roms --disk demo-a.adf --disk demo-b.adf""")

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
], [0.22, 0.78])
p("""Keys: Esc = RUN/STOP, Tab = CTRL, left Ctrl = C=, Insert = £, Delete =
↑, Page Up = RESTORE, Home = CLR/HOME, F2/F4/F6/F8 = shifted F1/F3/F5/F7.
The joystick is in port 2.""")
p("""A **folder** can be used as a disk with the virtual drive:
`--option drive8=virtual --disk ./myprograms`.""")

h2("Commodore VIC-20, C16/Plus/4, PET and C128")
table(["Machine", "Options"], [
    ["VIC-20", "`ram=none|3k|8k|16k|24k|all`, `port1=joystick|none`, `drive8=…`, `autostart=on`"],
    ["C16/Plus/4", "`ram=16k|64k`, `drive8=…`, `port1=joystick|none`, `autostart=on`"],
    ["PET", "`ram=8k|16k|32k`, `keyboard=graphics|business`, `hz=50|60`, `drive8=virtual|none`"],
    ["C128", "`display=40|80`, `vdc_ram=16k|64k`, `sid=…`, "
     "`drive8=1571|1541|1581|virtual|none`, `port2=…`, `autostart=on`"],
], [0.18, 0.82])
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
     "`f4sc`, `fa`, `fe`, `e0`, `e7`, `3f` (normally detected)"],
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
p("""Options: `ram=16k|1k|2k` (default 16k), `traps=on|off`. Programs are
`.p` files given with `--tape`: `LOAD ""` loads the program from the tape
slot and `SAVE "NAME"` writes it, through traps in the ROM.""")

h2("ColecoVision")
p("""Options: `port1`, `port2` = `controller` (default: joystick, two fire
buttons, keypad), `superaction`, `driving`, `roller`, `none`;
`roms=stub`. Keys: controller 1's keypad on the digit row (1–9, 0, - = *,
= = #, [ ] = the Super Action Controller's purple and blue buttons, X = right
fire), controller 2's keypad on the numeric keypad; joystick 1 on the cursor
keys, Space = left fire.""")

h2("Nintendo NES / Famicom")
p("""Options: `port1`, `port2` = `controller`, `zapper`, `arkanoid`, `none`;
`fourscore=on` (ports 3 and 4). Keys: the D-pad on the cursor keys, Space
and X = A, Z = B, right Shift = Select, Enter = Start. Cartridges are iNES
or NES 2.0 files with mappers 0–4 (NROM, MMC1, UxROM, CNROM, MMC3).""")

h2("Super Nintendo / Super Famicom")
p("""Options: `port1`, `port2` = `pad`, `mouse`, `superscope` (port 2 only),
`none`; `multitap=on` (Multiplayer 5 in port 2); `roms=stub`. Keys: the
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
)
code("""mimic-desktop --machine vectrex --rom-dir roms --cart "Scramble (1982).vec"
mimic-desktop --machine vectrex --rom-dir roms --option lines=soft --cart game.vec
mimic-desktop --machine vectrex --rom-dir roms --option port2=imager --cart 3d-game.vec""")

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
    "insert an empty file, press Shift+F9 (RECORD), `SAVE`, then F10 (STOP): "
    "the recording is written as a WAV file.",
    "**Sequencer PROM.** With the P6 PROM (341-0028) the Disk II reads and "
    "writes the way the real card does; without it a model of the card stands in.",
)
code("""mimic-desktop --machine apple2 --rom-dir roms --disk "Zork I.dsk"
mimic-desktop --machine apple2 --model iiplus --rom-dir roms --disk game.woz
mimic-desktop --machine apple2 --model ii --option slot6=none --rom-dir roms""")

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
code("""mimic-desktop --machine mac --rom-dir roms --disk "System Tools.image"
mimic-desktop --machine mac --model se --rom-dir roms --disk system.dsk --disk hd20.img
mimic-desktop --machine mac --model 128k --rom-dir roms --disk mfs-system.dsk""")

# ---- 6 media ------------------------------------------------------------------

h1("Disks, tapes, cartridges and programs")
table(["Kind", "Formats", "Option"], [
    ["Disk", "D64, D71, D81, G64, D80/D82 (PET), ATR (Atari), DSK/EDSK (CPC), ADF "
     "(Amiga DD and HD), extended ADF, DMS (read only), DSK/DO/PO/NIB/WOZ (Apple II), "
     "400K/800K DSK and DiskCopy 4.2, SCSI hard disk images (Macintosh); "
     "a folder as a virtual disk",
     "`--disk`"],
    ["Tape", "TAP (Commodore), CAS (Atari), CDT/TZX (CPC), P/81 (ZX81), WAV (Apple II); a PRG is "
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
protocol level — fast, but only for normal LOAD and SAVE).""")

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
    ["`tv`, `pal-tv`, `ntsc-tv`", "a television over composite (Atari, ZX81, Apple II)"],
    ["`pet-white`, `pet-green`", "the PET's built-in monitors"],
    ["`ctm644`", "Amstrad colour monitor (CPC)"],
    ["`gt65`", "Amstrad green monitor"],
    ["`mac`", "the compact Macintosh's built-in black-and-white monitor"],
], [0.30, 0.70])
p("""Own profiles are TOML files that start from a preset and change single
values (`base = "1084s"` …), loaded with `--crt myprofile.toml`.""")

# ---- 8 headless -----------------------------------------------------------------

h1("The command-line program")
p("""`mimic-headless` runs a machine without a window or sound — for
screenshots, scripts, automated tests and as a debug server.""")
code("""# run 300 frames and save the last one as PNG
mimic-headless --machine c64 --rom-dir roms --prg demo.prg --frames 300 --snapshot out.png

# C128 in 80 columns, picture of the VDC
mimic-headless --machine c128 --option display=80 --rom-dir roms --frames 600 --screen vdc --snapshot c128.png

# through a monitor profile, sound to WAV, savestate afterwards
mimic-headless --machine cpc --rom-dir roms --frames 150 --crt ctm644 --snapshot cpc.png --wav cpc.wav --save-state cpc.state""")
table(["Option", "Meaning"], [
    ["`--frames N`", "frames to run (default 1)"],
    ["`--snapshot FILE.png`", "save the last frame"],
    ["`--snapshot-every N`", "with `--snapshot`: also save every N-th frame"],
    ["`--screen NAME`", "which screen (C128: `vic` or `vdc`)"],
    ["`--crt PROFILE[:N]`", "the snapshot through a monitor profile, N rows per line"],
    ["`--wav FILE.wav`", "the sound of the run as 16-bit PCM"],
    ["`--load-state F`, `--save-state F`", "restore a savestate before, write one after the run"],
    ["`--rom ROLE=FILE`", "a single ROM image"],
    ["`--debug [ADDR]`", "serve the debugger instead of running frames"],
    ["`--preview [ADDR]`", "serve the live preview (implies `--debug` and `--run`)"],
    ["`--vice [ADDR]`", "also serve the VICE binary monitor"],
    ["`--run`", "with `--debug`: start running instead of paused"],
    ["`--no-realtime`", "with `--debug`: run as fast as possible"],
    ["`--list`", "list the machines"],
], [0.33, 0.67])
p("The other options (`--machine`, `--model`, `--option`, `--disk`, `--tape`, "
  "`--cart`, `--prg`, `--autostart`, `--rom-dir`) work as in the desktop program.")

# ---- 9 preview ----------------------------------------------------------------

h1("Live preview in the browser")
p("""With `--preview` the emulator serves a viewer page: open
`http://127.0.0.1:6580/` in a browser to watch the running machine. The
viewer forwards keyboard, gamepad and mouse input, so the machine can be
played from the browser, and shows the console panel.""")
code("""mimic-headless --machine c64 --rom-dir roms --preview
mimic-desktop  --machine amiga --rom-dir roms --preview      # window and browser""")
table(["Address", "Content"], [
    ["`/`", "the viewer page"],
    ["`/snapshot.png`", "the current frame as PNG"],
    ["`/preview` (WebSocket)", "a picture stream as PNG or QOI, also through a monitor profile"],
    ["`/stream` (WebSocket)", "a raw stream: colour indices, changed lines only, optional sound"],
    ["`/controls`", "the console's buttons and switches"],
], [0.30, 0.70])
p("""Parameters: `filter=raw|crt`, `profile=NAME`, `scale=1..4`, for the
streams `fps=1..50`, `format=png|qoi`, `delta=0|1`, `audio=0|1`. The server
listens on 127.0.0.1 only.""")

# ---- 10 debugger --------------------------------------------------------------

h1("Debugging")
p("""Every machine can be debugged the same way. The debugger knows no
particular machine: it works through registers, memory spaces, chips and
devices that each machine describes, so second processors (the 1541's 6502,
the C128's Z80, the Amiga's Copper and Blitter, the SNES sound CPU) are
debug targets of their own. Looking at memory never changes it — reading an
interrupt register in the debugger does not acknowledge the interrupt.""")
h2("Starting the debug server")
code("""mimic-headless --machine c64 --rom-dir roms --debug          # paused, port 6510
mimic-desktop  --machine c64 --rom-dir roms --debug          # with window and sound
mimic-headless --machine c64 --rom-dir roms --vice --preview --run""")
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
    "**Cheats:** freeze values in memory, narrowing search for unknown "
    "addresses (lives, energy), cheat files.",
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

# ---- 11 troubleshooting ---------------------------------------------------------

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


# ================================================================ layout ==

class Doc(BaseDocTemplate):
    def __init__(self, path):
        super().__init__(path, pagesize=A4, leftMargin=2.1 * cm, rightMargin=2.1 * cm,
                         topMargin=2.0 * cm, bottomMargin=2.0 * cm,
                         title="mimic User Guide", author="mimic",
                         subject="Running the mimic emulator")
        frame = Frame(self.leftMargin, self.bottomMargin, self.width, self.height, id="f")
        self.addPageTemplates([PageTemplate(id="page", frames=[frame], onPage=self.decorate)])

    def decorate(self, canvas, doc):
        if doc.page == 1:
            return
        canvas.saveState()
        canvas.setStrokeColor(RULE)
        canvas.setLineWidth(0.5)
        y = A4[1] - 1.4 * cm
        canvas.line(self.leftMargin, y, A4[0] - self.rightMargin, y)
        canvas.setFont("Sans", 8)
        canvas.setFillColor(MUTED)
        canvas.drawString(self.leftMargin, y + 4, "mimic — User Guide")
        canvas.drawRightString(A4[0] - self.rightMargin, 1.2 * cm, str(doc.page))
        canvas.restoreState()

    def afterFlowable(self, flowable):
        level = getattr(flowable, "toc_level", None)
        if level is not None:
            self.notify("TOCEntry", (level, flowable.getPlainText(), self.page))


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(root, "docs", "mimic-user-guide.pdf")
    Doc(out).multiBuild(story)
    print("wrote", out)


if __name__ == "__main__":
    main()
