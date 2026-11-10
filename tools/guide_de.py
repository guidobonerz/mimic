"""Deutscher Text des mimic-Benutzerhandbuchs. Gebaut von tools/user_guide.py;
in Schritt halten mit guide_en.py, den --help-Texten der Frontends und der
Web-Seite. Teil I erklärt die Bedienung, Teil II ist die Referenz."""

from guide_lib import bullets, code, h1, h2, h3, note, p, part, table  # noqa: F401

part("Teil I — mimic benutzen", "Installation, ROMs, der Desktop-Emulator, die Maschinen, "
     "Medien, Monitore, Live-Vorschau und Debugging")

# ---- 1 Einführung ----------------------------------------------------------

h1("Einführung")
p("""mimic ist ein zyklusgenauer Emulator für klassische Heimcomputer und
Spielkonsolen auf einem gemeinsamen Kern. Jede Maschine bekommt automatisch
denselben Fern-Debugger und denselben Live-Vorschau-Strom: Sie können eine
Maschine im Browser beobachten, an einem Haltepunkt anhalten, ihre Chips
ansehen und durch ihren Code schreiten, welche Maschine es auch ist.""")
p("""Dieses Handbuch beschreibt, wie der Emulator benutzt wird: Bauen,
ROMs bereitstellen, Maschinen im Desktop-Fenster oder auf der Kommandozeile
starten, Disketten, Kassetten und Module einlegen, Tastatur, Joysticks,
Gamepads und Mäuse verwenden, ein Monitorbild wählen und die
Browser-Vorschau und die Debug-Werkzeuge anschließen.""")

h2("Unterstützte Maschinen")
table(["Maschinen-ID", "Maschine", "Modelle (`--model`)", "Video (`--videoformat`)"], [
    ["`c64`", "Commodore 64", "–", "`pal` (Standard), `ntsc`"],
    ["`vic20`", "Commodore VIC-20", "–", "`pal` (Standard), `ntsc`"],
    ["`pet`", "Commodore PET", "`4032` (Standard), `2001`, `3032`, `8032`", "–"],
    ["`c16`, `plus4`", "Commodore 16, Plus/4", "–", "`pal` (Standard), `ntsc`"],
    ["`c128`", "Commodore 128 (40 und 80 Zeichen, Z80)", "–", "`pal` (Standard), `ntsc`"],
    ["`atari8`", "Atari 400/800/800XL/65XE/130XE",
     "`800xl` (Standard), `65xe`, `130xe`, `800`, `400`", "`pal` (Standard), `ntsc`"],
    ["`atari2600`", "Atari 2600 (VCS)", "–", "`auto` (Standard: NTSC oder PAL nach der "
     "Bildlänge des Programms), `ntsc`, `pal`"],
    ["`cpc`", "Amstrad CPC", "`6128` (Standard), `664`, `464`", "–"],
    ["`amiga`", "Commodore Amiga 500/1000/2000 (OCS), 500+ (ECS), 1200 (AGA)",
     "`500` (Standard), `1000`, `2000`, `500plus`, `1200`", "`pal` (Standard), `ntsc`"],
    ["`zx81`", "Sinclair ZX81 / Timex Sinclair 1000", "`zx81` (Standard, PAL), `ts1000` (NTSC)", "–"],
    ["`spectrum`", "Sinclair ZX Spectrum", "`48k` (Standard), `128k`, `plus2`", "–"],
    ["`coleco`", "ColecoVision", "–", "`ntsc` (Standard), `pal`"],
    ["`nes`", "Nintendo Entertainment System / Famicom", "–", "`ntsc` (Standard), `pal`"],
    ["`snes`", "Super Nintendo / Super Famicom", "–", "`ntsc` (Standard), `pal`"],
    ["`vectrex`", "Vectrex (Vektorbildschirm)", "`vectrex`", "–"],
    ["`imsai`", "IMSAI 8080 (Frontplatte; Terminal auf F12)", "`8080`", "–"],
    ["`logophor16`", "Logophor 16, mimics eigener Steampunk-Rechner (ROM eingebaut; Profil `logophor16` startet den Desktop Panorama)", "`16`", "–"],
    ["`archimedes`", "Acorn Archimedes", "`a310` (Standard), `a305`, `a440`, `a3000`", "–"],
    ["`apple2`", "Apple II, II+, IIe", "`iie` (enhanced IIe, Standard), `iie-unenhanced`, "
     "`iiplus`, `ii`", "–"],
    ["`mac`", "Apple Macintosh 128K, 512K, Plus, SE", "`plus` (Standard), `128k`, `512k`, `se`", "–"],
    ["`sim6502`", "6502-Referenzmaschine für Tests", "`default`", "–"],
], [0.14, 0.32, 0.32, 0.22])
p("`mimic-headless --list` gibt dieselbe Liste aus dem Programm selbst aus.")

h2("Die zwei Programme")
table(["Programm", "Zweck"], [
    ["`mimic-desktop`", "Der Emulator mit Fenster, Ton, Tastatur, Gamepads und Maus. Er "
     "kann zusätzlich den Debugger, den VICE-Monitor und die Browser-Vorschau anbieten."],
    ["`mimic-headless`", "Ohne Fenster und Ton: eine Anzahl Bilder rechnen und ein Bild "
     "speichern, Ton als WAV aufnehmen, Savestates schreiben oder Debugger und "
     "Vorschau anbieten. Für Skripte, Tests und Fernbedienung."],
], [0.22, 0.78])

# ---- 2 Installation ----------------------------------------------------------

h1("Installation und Schnellstart")
h2("Voraussetzungen")
bullets(
    "**Rust** (stabile Toolchain) mit `cargo` — Installation über rustup.rs.",
    "**Nur Linux:** die Pakete `libasound2-dev` und `libudev-dev` für das "
    "Desktop-Programm (Ton und Gamepads).",
    "**Optional:** Python 3 für den Debug-Client `tools/polydbg.py` (nur "
    "Standardbibliothek).",
)
h2("Bauen")
p("Immer die optimierte Fassung (release) bauen; ein Debug-Build ist weit "
  "langsamer als Echtzeit.")
code("""cargo build --release -p emu-desktop     # der Desktop-Emulator
cargo build --release -p emu-headless    # das Kommandozeilen-Frontend""")
p("""Die Programme landen in `target/release/` — unter Windows
`mimic-desktop.exe` und `mimic-headless.exe`. Sie brauchen keine weiteren
Dateien und können beliebig kopiert werden; ROMs liegen im Ordner
`.mimic/roms` Ihres Benutzerordners oder werden über `--rom-dir` bzw. die
Umgebungsvariable `EMU_ROM_DIR` gefunden.""")
note("""Ältere Builds hinterließen Dateien namens `emu-desktop.exe` und
`emu-headless.exe` in `target/release/`. Sie sind veraltet (sie kennen die
neueren Maschinen nicht); benutzen Sie `mimic-desktop.exe` und
`mimic-headless.exe` oder starten Sie wie unten über `cargo run`, das immer
zuerst die aktuelle Fassung baut.""", "Achtung")
h2("Eine Maschine starten")
p("Zwei gleichwertige Wege, den Desktop-Emulator zu starten:")
code("""cargo run --release -p emu-desktop -- --machine c64
.\\target\\release\\mimic-desktop.exe --machine c64""")
p("Alles nach `--` geht an den Emulator. Einige erste Schritte:")
code("""# C64 mit einer Diskette, automatisch geladen und gestartet
mimic-desktop --machine c64 --disk game.d64 --autostart

# Amiga 500 bootet Workbench 1.3 von einem ADF
mimic-desktop --machine amiga --disk workbench13.adf

# NES und SNES brauchen kein System-ROM
mimic-desktop --machine nes --cart game.nes
mimic-desktop --machine snes --cart game.sfc

# Vectrex mit einem Modul
mimic-desktop --machine vectrex --cart "Scramble (1982).vec\"""")
p("`mimic-desktop --help` listet jede Option mit den Tastenbelegungen.")
note("""Unter Windows ist `mimic-desktop` ein Fensterprogramm: ein Doppelklick
oder eine Verknüpfung öffnet kein Konsolenfenster neben dem Emulator. Aus
einem Terminal gestartet gehen seine Meldungen (Gamepads, Server, Fehler,
`--help`) in dieses Terminal — hinter die Eingabeaufforderung, weil die
Shell auf ein Fensterprogramm nicht wartet. Ohne Terminal zeigen ein
fehlgeschlagener Start und `--help` stattdessen einen Dialog.
`mimic-headless` bleibt ein Konsolenprogramm.""", "Hinweis")

# ---- 3 ROMs --------------------------------------------------------------

h1("ROMs")
p("""Die meisten Maschinen brauchen ihre originalen System-ROMs (KERNAL,
BASIC, Kickstart, Betriebssystem, BIOS). Sie sind urheberrechtlich geschützt
und darum **nicht** Teil von mimic: Sie stellen sie von Ihren eigenen
Maschinen bereit. Legen Sie sie in den Ordner `.mimic/roms` Ihres
Benutzerordners (unter Windows `%USERPROFILE%\\.mimic\\roms`); einen anderen
Ordner übergeben Sie mit `--rom-dir DIR` oder setzen einmal `EMU_ROM_DIR`.""")
h2("Wie ROMs gefunden werden")
bullets(
    "mimic sucht zuerst im Unterordner der Maschine (etwa `~/.mimic/roms/c64/`), dann in "
    "`~/.mimic/roms/drives/`, dann im Ordner selbst.",
    "Eine Datei wird am **Anfang ihres Namens** erkannt (Groß- und Kleinschreibung "
    "egal; `.` und `_` zählen wie `-`) und an ihrer **genauen Größe**.",
    "Fehlt ein ROM, nennt die Fehlermeldung die Rolle, die Größe und die "
    "akzeptierten Namen.",
    "Bei `mimic-headless` lässt sich ein einzelnes ROM auch direkt angeben: "
    "`--rom kernal=pfad/kernal.bin`.",
)
h2("ROM-Tabelle")
table(["Ordner", "Maschine", "ROMs (Beispielnamen und Größen)"], [
    ["`c64/`", "C64", "`kernal` (8 KB), `basic` (8 KB), `chargen` oder `characters` (4 KB)"],
    ["`vic20/`", "VIC-20", "KERNAL, BASIC, Zeichen-ROM"],
    ["`pet/`", "PET", "BASIC, Editor, KERNAL und Zeichen-ROM je Modell"],
    ["`plus4/`", "C16, Plus/4", "KERNAL, BASIC; optional 3-plus-1"],
    ["`c128/`", "C128", "KERNAL, BASIC, Zeichen-ROM; optional Funktions-ROM"],
    ["`atari/`", "Atari 8-Bit", "XL-OS `atarixl.rom` (16 KB), BASIC `ataribas.rom` (8 KB); "
     "für 400/800 OS-B `atariosb.rom` (10 KB)"],
    ["`cpc/`", "Amstrad CPC", "`os464`/`basic1-0`, `os664`/`basic664`, `os6128`/`basic1-1` "
     "(je 16 KB) oder 32-KB-Kombidateien; `amsdos`, optional `parados`"],
    ["`amiga/`", "Amiga", "Kickstart-Abbilder von 256 oder 512 KB, z. B. `Kickstart 1.3.rom`, "
     "`kick13.rom`; eines je Version (1.2, 1.3, 2.0, 3.0, 3.1), gewählt mit "
     "`--option kickstart=`; optional das A1000-Bootstrap-ROM (8 KB)"],
    ["`zx81/`", "ZX81", "`zx81.rom` (8 KB)"],
    ["`spectrum/`", "ZX Spectrum", "`Spec48.rom` (16 KB), `spec128.rom`, `specp2.rom` (je 32 KB); "
     "optional `mf128.rom` (Multiface 128), `specif1.rom` (Interface 1)"],
    ["`coleco/`", "ColecoVision", "`coleco.rom` (8 KB BIOS)"],
    ["`snes/`", "SNES", "optional: das 64-Byte-Sound-Boot-ROM `spc700.rom`"],
    ["`vectrex/`", "Vectrex", "das 8-KB-System-ROM (Executive und Mine Storm); ein 4-KB-Abbild "
     "nur des Executive geht auch, siehe den Vectrex-Abschnitt"],
    ["`apple2/` (oder `apple/`)", "Apple II", "Dateien nach Apple-Teilenummer: II "
     "341-0001…0004, II+ 341-0011…0015 und 341-0020, IIe 342-0135 (CD) und 342-0134 "
     "(EF), enhanced IIe 342-0304 und 342-0303; Zeichen-ROMs 341-0036, 342-0133, "
     "342-0265; Disk-II-Boot-PROM 341-0027 und Sequencer-PROM 341-0028"],
    ["`mac/`", "Macintosh", "`mac128k.rom` (64 KB), `mac512k.rom` (64 KB), `macplus.rom` "
     "(128 KB), `macse.rom` (256 KB), oder die zwei Chips jedes ROMs (Apple-Teilenummern "
     "342-0220/0221, 342-0341/0342, 342-0701/0702)"],
    ["`drives/`", "Diskettenlaufwerke", "`dos1541`, `dos1571`, `dos1581`, `dos1551` (16 KB, 318008-01)"],
], [0.14, 0.16, 0.70])
p("Atari 2600 und NES haben kein System-ROM: das Programm steckt im Modul.")
note("""Viele Maschinen nehmen `--option roms=stub`: ein winziges eingebautes
Programm statt der echten ROMs. Es ist für Tests gedacht (es zeigt ein
Testbild oder startet einfache Module); echte Software braucht die
Original-ROMs.""", "Hinweis")

# ---- 4 Desktop -------------------------------------------------------------

h1("Der Desktop-Emulator")
h2("Kommandozeilenoptionen")
p("""Jede Option steht in Teil II (Kapitel 12 bis 14). Die wichtigsten für den
Anfang: `--machine ID` wählt die Maschine, `--rom-dir DIR` den ROM-Ordner,
`--disk`, `--tape`, `--cart` und `--prg` die Medien, `--autostart` startet
das Programm nach dem Booten, `--option KEY=VALUE` konfiguriert die Maschine
(Kapitel 15) und `--profile NAME` startet eine gespeicherte Zusammenstellung
(Kapitel 16).""")
h2("Tastenkürzel")
p("""Die Strg-Kürzel nehmen die linke Strg-Taste. Die rechte Strg-Taste und
AltGr erreichen die Maschine, so dass ein Ctrl oder SYMBOL SHIFT der
emulierten Tastatur mit jedem Buchstaben geht.""")
table(["Taste", "Wirkung"], [
    ["Umschalt+Alt+Links", "Konsolenpanel am linken Rand öffnen oder schließen"],
    ["Umschalt+Alt+Rechts", "Medienpanel am rechten Rand öffnen oder schließen: Disketten, Kassetten und Module einlegen, auswerfen und speichern"],
    ["Strg+Leertaste", "Statusfußzeile zeigen oder verbergen"],
    ["Strg+D", "die nächste Diskette der `--disk`-Liste ins erste Laufwerk legen"],
    ["Strg+I", "Amiga: Deinterlacing zwischen weave und bob umschalten"],
    ["Strg+R (gehalten)", "mit doppelter Geschwindigkeit rückwärts durch die letzten Bilder; "
     "beim Loslassen läuft die Maschine von dort weiter"],
    ["F12", "zwischen den Bildschirmen der Maschine wechseln (C128: 40-Zeichen-VIC und 80-Zeichen-VDC)"],
    ["F9 / Umschalt+F9", "Datasette oder Kassette: PLAY / RECORD"],
    ["F10 / F11", "Datasette oder Kassette: STOP / REWIND"],
    ["Mittlere Maustaste", "eine eingefangene Maus freigeben"],
], [0.28, 0.72])
p("Eine auf das Fenster gezogene Abbilddatei kommt ins erste Laufwerk "
  "(Disketten) oder in den Modulschacht (`.vec`, `.crt`, `.car`, `.a26`).")

h2("Das Konsolenpanel")
p("""Links vom Bild zeigt ein Panel die Knöpfe und Schalter des Gehäuses
untereinander (Umschalt+Alt+Links oder ein Klick auf seinen Griff öffnet und
schließt es): POWER und RESET, die Konsolenschalter des Atari 2600,
START/SELECT/OPTION des 800XL, RESTORE des C64 und VIC-20, 40/80 und CAPS
LOCK des C128 und so weiter. Ein Knopf bleibt gedrückt, solange Sie die
Maustaste darauf halten; ein Klick auf eine Schalterstellung legt den
Schalter um.""")
bullets(
    "**PAUSE** (oben) hält die ganze Maschine an und lässt sie weiterlaufen. Es "
    "leuchtet, solange die Maschine steht — auch wenn ein Debugger sie angehalten "
    "hat — und die Titelzeile zeigt, wo.",
    "**JOY KEYS** wählt die Tasten, die den Joystick spielen: CURSOR (Cursortasten, "
    "Leertaste feuert), NUMPAD (8/2/4/6, Feuer auf 0 oder rechter Strg) oder OFF "
    "(nur Gamepad).",
    "**JOY PORT** (Maschinen mit mehreren Joystick-Anschlüssen) legt den "
    "Tastatur-Joystick und das Gamepad auf einen anderen Anschluss.",
    "**PAD FIRE** wählt den Gamepad-Knopf, der feuert (SOUTH ist A auf einem "
    "Xbox-Pad); **SAVE MAPPING** merkt sich JOY KEYS, JOY PORT und PAD FIRE für die "
    "Maschine, so dass der nächste Start sie nimmt (`--input DATEI.toml` lädt eine "
    "andere).",
    "**Cheats** (aus `--cheat`, der Cheat-Datenbank oder dem Debugger) erscheinen "
    "als OFF/ON-Schalter.",
)

h2("Startprofile")
p("""Ein **Startprofil** speichert eine ganze Maschinenzusammenstellung unter
einem Namen: Maschine und Modell, ihre Optionen (Speicher, Laufwerke,
Schächte, Geräte an den Anschlüssen), die ROMs, Disketten und andere Medien,
Monitor-, Fenster- und Joystick-Einstellungen sowie Debugger- und
Vorschau-Server. Alle Profile stehen in einer YAML-Datei, `profiles.yaml`,
die im Arbeitsordner und dann im Konfigurationsordner des Benutzers gesucht
wird (`%APPDATA%\\mimic` unter Windows, sonst `~/.config/mimic`);
`--profiles DATEI` nennt eine andere. Ihr Teil `settings` enthält, was für
jedes Profil gilt: den ROM-Ordner (`roms`), wohin Schnappschüsse gehen
(`snapshots`) und den Ordner, von dem relative Diskettenpfade zählen
(`media`). `docs/profiles.example.yaml` zeigt jeden Schlüssel; Kapitel 16
erklärt sie.""")
table(["Befehl", "Wirkung"], [
    ["`mimic-desktop --profile mac-se-755`", "ein Profil starten; zusätzlich angegebene Optionen "
     "überschreiben es (`--profile c64 --videoformat ntsc`)"],
    ["`mimic-desktop`", "weder Maschine noch Profil angegeben: das Fenster listet die Profile, "
     "ein Klick startet eines"],
    ["`mimic-desktop --list-profiles`", "die Profile auflisten"],
    ["`... --save-profile NAME`", "die Zusammenstellung dieser Kommandozeile (mit Profil: auch "
     "dessen Werte) als Profil speichern; ein neuer Name wird ans Ende der Datei "
     "gehängt, ein vorhandener ersetzt und die Datei neu geschrieben"],
], [0.42, 0.58])
p("""`mimic-headless` nimmt dieselben Profil-Optionen (ohne die
Fenstereinstellungen); die Browser-Oberfläche liest eine Profildatei, die
die Seite lädt.""")

h2("Die Statusfußzeile")
p("""Strg+Leertaste zeigt eine Statuszeile unter dem Bild. Links sitzt der
**Geschwindigkeitsregler**: Klick auf eine Stufe oder Ziehen des Knopfs für
10 %, 25 %, 50 %, 100 %, 150 %, 200 %, 400 % oder 800 % der Echtzeit (abseits
von 100 % ist der Ton stumm). Bei 100 % läuft der Emulator stumm so schnell
er kann, solange ein Diskettenlaufwerk arbeitet (LED oder Motor, etwa beim
LOAD) und die Maschine eine Sekunde lang ohne Ton und mit stehendem Bild war,
und nur, bis das geladene Programm zum ersten Mal Ton oder eine Animation
zeigt (nach einem Diskwechsel wieder), so dass ein Demo, das beim Laufen
seinen nächsten Teil nachlädt, sein Tempo behält; die Fußzeile zeigt
dann "warp: loading". `--warp-load off` schaltet das ab. Daneben speichert
der **Schnappschuss**-Knopf das Bild als PNG-Datei mit dem Namen der
Maschine (`mac-001.png`, `mac-002.png` …) im `snapshots`-Ordner der
Profildatei-Einstellungen, sonst im Ordner, aus dem der Emulator gestartet
wurde; die Fußzeile zeigt den Dateinamen einige Sekunden lang. Dann folgen
die emulierte Zeit seit dem Einschalten (m:ss.t) mit der Bildnummer, die
Mausposition in Bildpixeln, die Joystick-Richtungen und die Aktivität und
Fehler der Laufwerke.""")

h2("Fenster und Bild")
bullets(
    "Das Fenster öffnet in der größten Skalierung bis `--scale`, die auf den "
    "Bildschirm passt, und lässt sich frei vergrößern und verkleinern.",
    "Die Anzeige läuft im Takt des Monitors, wenn dessen Bildrate zur Maschine "
    "passt (50 oder 60 Hz); sonst folgt der Emulator der Uhrzeit. Die Konsole gibt "
    "aus, welcher Modus gilt.",
    "Die Titelzeile zeigt die Maschine, die eingelegte Diskette und im Pausezustand, "
    "wo die Maschine steht.",
)

h2("Joysticks und Gamepads")
p("""Jede Maschine mit Joystick-Anschlüssen bekommt einen Joystick, den die
Tastatur und das erste Gamepad spielen. Der Standardanschluss ist der
Spielanschluss der Maschine: Port 2 beim C64 und C128, Port 1 beim VIC-20,
C16/Plus/4, den Ataris, dem CPC und den Konsolen, Gameport 1 beim Amiga
(Port 0 hält die Maus).""")
h3("Tastatur-Joystick")
table(["Einstellung", "Tasten", "Standard bei"], [
    ["CURSOR", "Cursortasten, Leertaste feuert",
     "Atari 2600, ColecoVision, NES, SNES, Vectrex (Maschinen ohne Tastatur)"],
    ["NUMPAD", "Ziffernblock 8/2/4/6, Feuer auf 0 oder rechter Strg", "den Computern"],
    ["OFF", "nur Gamepad", "Amstrad CPC (sein Ziffernblock sind die Funktionstasten)"],
], [0.14, 0.40, 0.46])
h3("Gamepads (Xbox 360 und andere)")
p("""Gamepads werden über die Spielcontroller-Schnittstelle des
Betriebssystems gelesen: unter Windows Windows.Gaming.Input (Xbox 360, Xbox
One und kompatible Pads), unter Linux evdev, unter macOS IOKit. Der Desktop
gibt beim Start und bei jedem An- und Abstecken aus, welche Pads er
findet:""")
code("""gamepad 0 connected: Xbox 360 Controller for Windows
gamepad 1 connected: HID-compliant device""")
p("""**Gamepad 0** spielt den Joystick-Anschluss. Knöpfe heißen nach ihrer
Lage (South = A auf einem Xbox-Pad, East = B, West = X, North = Y):""")
table(["Gamepad", "Joystick und Controller"], [
    ["Steuerkreuz, linker Stick", "Richtungen (der Stick ist digital; beim Vectrex analog, "
     "siehe unten)"],
    ["South (A)", "Feuer / Knopf 1"],
    ["East (B)", "zweiter Feuerknopf (NES B, SNES A, Vectrex Knopf 2 …)"],
    ["West (X), North (Y)", "weitere Knöpfe (SNES Y und X, Vectrex Knöpfe 3 und 4)"],
    ["Schultertasten", "SNES L und R"],
    ["Back / Start", "Select / Start (NES, SNES)"],
], [0.30, 0.70])
note("""Anstecken im Betrieb ist in Ordnung: ein Pad kann angeschlossen werden,
während der Emulator läuft. Erscheint Ihr Pad mit einer anderen Nummer als
0, stecken Sie andere Spielgeräte ab oder starten Sie mit bereits
angeschlossenem Pad.""", "Hinweis")

h2("Maus, Paddles und Lichtpistolen")
p("""Hält ein Anschluss eine Maus, Paddles, ein Lenkrad oder eine
Lichtpistole, fängt ein Klick ins Fenster die Maus des Rechners für die
Maschine ein; die mittlere Maustaste gibt sie wieder frei. Beispiele:""")
bullets(
    "C64: `--option port1=mouse1351` (Maus 1351) oder `port1=paddles`.",
    "Amiga: die Maus sitzt standardmäßig in Gameport 0.",
    "CPC: `--option joy1=amx` (AMX-Maus).",
    "Atari 2600: `--option port1=paddles` oder `port1=driving` — die Maus dreht Paddle 0 "
    "oder das Rad.",
    "NES: `--option port2=zapper` (mit der Maus zielen, linke Taste feuert) oder "
    "`port2=arkanoid` (die Maus dreht den Knopf).",
    "SNES: `--option port2=superscope` (linke Taste = Abzug, rechte = Cursor) oder "
    "`port1=mouse`.",
    "ColecoVision: `superaction` (Speed Roller), `driving` (Lenkrad, linke Taste = Gas), "
    "`roller` (Trackball).",
)

h2("Disketten wechseln")
p("""`--disk` kann mehrfach angegeben werden. Die Disketten kommen der Reihe
nach in die Laufwerke (Amiga: DF0:, DF1: … mit `--option drives=N`); der
Rest bildet eine Wechselliste für das erste Laufwerk. **Strg+D** legt die
nächste Diskette dieser Liste ein, und ein auf das Fenster gezogenes Abbild
kommt ebenfalls ins erste Laufwerk. Wie beim Wechsel von Hand bleibt das
Laufwerk etwa zwei Sekunden leer, bevor die neue Diskette hineinkommt:
Programme, die nur ab und zu nach dem Laufwerk sehen, bemerken den Wechsel
nur, wenn sie es einmal leer vorfinden. Die Titelzeile zeigt die Diskette im
Laufwerk.""")
code("""# ein Demo auf zwei Disketten mit einem Laufwerk: Strg+D drücken, wenn es Diskette B verlangt
mimic-desktop --machine amiga --disk demo-a.adf --disk demo-b.adf""")

# ---- 5 Maschinen ----------------------------------------------------------------

h1("Die Maschinen")
p("""Tastaturen werden nach **Lage** auf einer PC-Tastatur (US-Belegung)
abgebildet, nicht nach den aufgedruckten Beschriftungen: die Taste, wo die
Originalmaschine eine Taste hat, spielt diese Taste. Optionen werden mit
`--option KEY=VALUE` gesetzt und können wiederholt werden.""")

h2("Commodore 64")
table(["Option", "Werte"], [
    ["`sid`", "`6581` oder `8580`"],
    ["`cia`", "`6526` oder `6526a`"],
    ["`drive8`", "`1541` (echte Laufwerksemulation), `1571`, `1581`, `virtual` (schnell, "
     "nur KERNAL-Laden/Speichern), `none`"],
    ["`port1`, `port2`", "`joystick`, `paddles`, `mouse1351`, `none`"],
    ["`autostart`", "`on`: das erste Programm der Diskette laden und starten"],
    ["`fastload`", "`on` (mit `drive8=virtual`): das virtuelle Laufwerk bedient die seriellen "
     "KERNAL-Routinen ohne Buszeit; LOAD, SAVE und OPEN/GET# kosten keine Zeit auf dem Bus"],
    ["`uci`", "`on`: das Ultimate-64-Command-Interface bei $DF1C-$DF1F (siehe unten)"],
    ["`reu`", "eine RAM Expansion Unit: `128k` (1700), `256k` (1764), `512k` (1750), "
     "`1m`, `2m`, `4m`, `8m`, `16m`; `off` (Standard)"],
    ["`userport`", "`wic64`: ein WiC64-WLAN-Adapter am User-Port; `none` (Standard)"],
    ["`net`", "`off`: kein Netz für UCI und WiC64, auch wenn das Frontend eines hat"],
    ["`sd2iec`", "`on`: ein SD2IEC am seriellen Bus (siehe unten); `sd2iec_device` = 4–30 "
     "(Standard 9); `drive8=sd2iec` setzt es an die Stelle von Laufwerk 8"],
], [0.22, 0.78])
p("""Tasten: Esc = RUN/STOP, Tab = CTRL, linke Strg = C=, Einfg = £, Entf =
↑, Bild auf = RESTORE, Pos1 = CLR/HOME, F2/F4/F6/F8 = F1/F3/F5/F7 mit
Umschalt. Der Joystick steckt in Port 2.""")
p("""Ein **Ordner** lässt sich mit dem virtuellen Laufwerk als Diskette
benutzen: `--option drive8=virtual --disk ./meineprogramme`.""")
h3("Netz: Ultimate Command Interface und WiC64")
p("""**Ultimate Command Interface** (`--option uci=on`): die vier Register
des Command Interface von Ultimate 64 / 1541 Ultimate II+ bei $DF1C-$DF1F.
Die DOS-Ziele 1 und 2 arbeiten auf einem Ordner Ihres Rechners
(`--uci-dir`, Standard `.mimic/ultimate` im Benutzerordner): öffnen, lesen, schreiben, positionieren,
Dateiinfo, löschen, umbenennen, kopieren, Verzeichnisse wechseln, anlegen
und auflisten, die Uhrzeit, REU-Daten in die und aus der offenen Datei, und
das Einlegen von Disketten-Abbildern aus dem Ordner in Laufwerk 8 (mount,
unmount, Wechsel zur nächsten Diskette eines Satzes wie game-1.d64,
game-2.d64). Pfade können den Ordner nie verlassen. Der echte Ultimate
zeigt SD-Karte und USB-Sticks als `/SD`, `/Usb0` … in der Wurzel;
`--uci-mount Usb0=DIR` (wiederholbar) hängt einen Ordner Ihres Rechners als
solches Verzeichnis ein. Mit `uci=on` werden die Ordner beim Start angelegt.
Das Netzziel 3 öffnet TCP- und UDP-Sockets (und einen TCP-Listener auf
127.0.0.1). Das Steuerziel 4 startet den C64 neu, lädt und speichert
REU-Abbilder, sichert das RAM, löscht EasyFlash-Sektoren, meldet SID und
Laufwerk 8, schaltet Laufwerk 8 aus und ein und liest oder ändert die
Palette (Freeze wird nicht angeboten). Das Software-IEC-Ziel 5 gibt
KERNAL-Ersatz direkten Dateizugriff: LOAD und SAVE per DMA,
OPEN/CHKIN/CHKOUT/CLOSE, Partitionen auf Unterordnern, `LOAD"$"` als
Verzeichnisliste. Das HTTP-Ziel 6 schickt Anfragen mit beliebigem Verb,
Kopffeldern und JSON-, URL-kodierten oder binären Rümpfen und lässt
Programme die JSON-Antwort per Pfad abfragen (nur `http:`). Ein Modul kann
gesteckt bleiben; die Schnittstelle belegt nur ihre vier Register.""")
p("""**RAM Expansion Unit** (`--option reu=512k`): eine Commodore-REU mit
dem REC 8726 bei $DF00 (ganz I/O2 außer den vier Registern des Command
Interface). Ihr DMA hält die CPU an, ein Byte je Zyklus; GEOS und
REU-Programme nutzen sie. Ihr Speicher gehört zum Savestate (16 MB machen
große Savestates).""")
p("""**WiC64** (`--option userport=wic64`): der WLAN-Adapter am User-Port mit
den Befehlen der Firmware 2.x im Legacy- und im Protokoll-2-Format: HTTP
GET (roh und URL-kodiert), HTTP POST, TCP öffnen/lesen/schreiben/schließen,
IP, MAC, SSID, Signalstärke, Uhrzeit und Zeitzone, Server, Version,
Statusmeldung, Zeitüberschreitungen und Echo. `https:`-Adressen werden als
nicht unterstützt gemeldet.""")
p("""Beide benutzen standardmäßig das Netz Ihres Rechners; `--net off`
schaltet es ab, und die Geräte melden "kein Netz". Die Uhr läuft in UTC,
sofern nicht die Umgebungsvariable `EMU_UTC_OFFSET` (z. B. `+01:00`) den
Versatz angibt. Die Web-Fassung hat kein Netz (Browser erlauben keine
Sockets); dort sieht das Command Interface die hochgeladenen Dateien als
seinen Ordner. Eine Debugger-Aufzeichnung (`record_start`/`record_stop`)
zeichnet auch jede Antwort aus dem Netz und dem Ordner auf, so dass
`replay_input` einen Lauf ohne Netz exakt wiederholt.""")

h3("SD2IEC")
p("""**SD2IEC** (`--option sd2iec=on`, Gerät 9; `--option drive8=sd2iec`
statt Laufwerk 8): ein Massenspeicher am seriellen Bus, dessen „SD-Karte“
ein Ordner Ihres Rechners ist (`--sd2iec-dir`, Standard `.mimic/sd2iec` im
Benutzerordner). Geladen wird im KERNAL-Tempo (keine Schnelllader).
Verzeichnisse wie bei CMD-Laufwerken (`CD:NAME`, `CD//`, `CD:←`, `MD`,
`RD`, Pfade wie `LOAD"//SPIELE/:ELITE",9`); D64/D71/D81-Abbilder hängt
`CD:SPIEL.D64` ein, `CD:←` verlässt sie; `N:NEU.D64,ID` legt ein neues an.
Namen in beiden Schreibweisen erscheinen als normale Buchstaben, Namen über
16 Zeichen als 8.3-Kurzname. Direktzugriff im eingehängten Abbild
(`#`-Puffer, `U1`/`U2`, `B-P`, `B-A`/`B-F`), REL-Dateien mit `P`,
Swap-Listen (`XS:LISTE.LST`, dann die Bedienfeld-Tasten SD2IEC NEXT und
PREV; beide zusammen springen zum ersten Eintrag, ohne Liste legen sie
`AUTOSWAP.GEN` mit den Abbildern des Ordners an), `XI`, `XD`, `XR` und die
übliche Laufwerkserkennung per `M-R`. Ein Abbild im Medienschacht „sd2iec“
bleibt eingehängt, bis es ausgeworfen wird.""")

h2("Commodore VIC-20, C16/Plus/4, PET und C128")
table(["Maschine", "Optionen"], [
    ["VIC-20", "`ram=none|3k|8k|16k|24k|all`, `port1=joystick|none`, `drive8=…`, `autostart=on`"],
    ["C16/Plus/4", "`ram=16k|64k`, `drive8=1541|1571|1581|1551|virtual|none` (`1551`: das "
     "parallele TCBM-Laufwerk mit seinem Interface-Modul, braucht `dos1551`), "
     "`sidcard=6581|8580|off` (eine SID-Karte bei `$FD40`, für Demos und Spiele mit "
     "SID-Musik), `port1=joystick|none`, `autostart=on`"],
    ["PET", "`ram=8k|16k|32k`, `keyboard=graphics|business`, `hz=50|60`, `drive8=virtual|none`"],
    ["C128", "`display=40|80`, `vdc_ram=16k|64k`, `sid=…`, "
     "`drive8=1571|1541|1581|virtual|none`, `port2=…`, `autostart=on`"],
], [0.18, 0.82])
p("""Der C128 hat zwei Bildschirme: F12 wechselt zwischen dem 40-Zeichen-VIC
und dem 80-Zeichen-VDC; das Fenster beginnt auf dem, den die 40/80-Taste
wählt.""")

h2("Atari 400, 800, 800XL, 65XE und 130XE")
table(["Option", "Werte"], [
    ["`port1`, `port2`", "`joystick`, `paddles`, `none` (400/800 auch `port3`, `port4`)"],
    ["`basic`", "`on`/`off` — XL/XE: OPTION beim Booten halten; 400/800: BASIC-Modul im linken Schacht"],
    ["`ram`", "nur 400/800: `8k`, `16k`, `32k`, `48k`"],
    ["`drives`", "`0` bis `4` Diskettenlaufwerke"],
    ["`drive`", "`1050`, `810` oder `xf551`"],
], [0.22, 0.78])
p("""Tasten: < > auf - =, - = auf [ ], + * auf ' \\, rechte Alt = Atari-Taste,
Cursortasten = CONTROL + - = + *, F2 START, F3 SELECT, F4 OPTION, F5 RESET,
F6 HELP, F7/Pause BREAK.""")

h2("Atari 2600")
table(["Option", "Werte"], [
    ["`port1`, `port2`", "`joystick`, `paddles`, `keypad`, `driving`, `none`"],
    ["`tv`", "`color` oder `bw`"],
    ["`left`, `right`", "Schwierigkeitsschalter `b` oder `a`"],
    ["`bank`", "ein Bankumschaltverfahren erzwingen: `2k`, `4k`, `f8`, `f6`, `f4`, `f8sc`, `f6sc`, "
     "`f4sc`, `fa`, `fe`, `e0`, `e7`, `3f`, `dpc` (Pitfall II; normalerweise erkannt)"],
], [0.22, 0.78])
p("""Konsole: F1 SELECT, F2 RESET (gehalten), F3 Farbe/Schwarzweiß, F4/F5
Schwierigkeit links/rechts. Keypads: links 1 2 3 / Q W E / A S D / Z X C,
rechts 8 9 0 / I O P / K L ; / , . /.""")

h2("Amstrad CPC 464, 664 und 6128")
table(["Option", "Werte"], [
    ["`crtc`", "`0`, `1` oder `2` (Standard: 0 beim 464, sonst 1)"],
    ["`dos`", "`amsdos`, `parados` oder `none`"],
    ["`joy1`", "`joystick`, `amx` (AMX-Maus) oder `none`"],
    ["`joy2`", "`joystick` (ein zweiter Stick in Reihe) oder `none`"],
], [0.22, 0.78])
p("""Disketten (DSK/EDSK) kommen beim 664/6128 in Laufwerk A: `CAT` oder
`RUN"DISC` eingeben. Der 464 hat nur eine Kassette (CDT/TZX); beim 664/6128
zuerst `|TAPE` eingeben, um auf Band umzuschalten. Tasten: - ^ auf - =, @ [
auf [ ], : ; auf ; ', ] auf der Backslash-Taste, Rücktaste = DEL, Entf =
CLR, linke Alt = COPY, der Ziffernblock und F1–F10 = die Funktionstasten
f0–f9 und f.""")

h2("Commodore Amiga")
table(["Option", "Werte"], [
    ["`kickstart`", "`1.2`, `1.3`, `2.0`, `3.0`, `3.1` (Standard 1.3; A500+ 2.0; A1200 3.1)"],
    ["`chip`", "Chip-RAM `256k`, `512k`, `1m`, `2m`"],
    ["`slow`", "Trapdoor-RAM `512k` oder `0`"],
    ["`fast`", "Zorro-II-Fast-RAM `0` … `8m` (von Kickstart per Autoconfig gefunden)"],
    ["`chipset`", "`ocs`, `ecs`, `aga`; oder getrennt `agnus=ocs|ecs`, `denise=ocs|ecs`"],
    ["`drives`", "`1` bis `4` (DF0: bis DF3:)"],
    ["`drivetype`", "`dd` oder `hd` (1,76-MB-Disketten)"],
    ["`port0`, `port1`", "`mouse`, `joystick`, `none` (Standard: Maus in 0, Joystick in 1)"],
    ["`serial`", "`none` oder `loopback`"],
    ["`wcs`", "A1000: `rom` oder `disk` (Kickstart mit dem Bootstrap-ROM von Diskette laden)"],
], [0.22, 0.78])
p("""Ohne Diskette zeigt Kickstart seinen Diskette-einlegen-Bildschirm; ein ADF
in DF0: bootet. Workbench 1.3 braucht etwa drei Minuten emulierter Zeit bis
zu ihrem Bildschirm. Tasten: linke/rechte Windows-Taste = linke/rechte
Amiga-Taste, beide Strg = CTRL, Einfg = HELP, Entf = DEL; **Strg + beide
Windows-Tasten** löst einen Reset aus wie die echte Tastatur. Der Ton ist
stereo mit den Filtern des A500.""")

h2("Sinclair ZX81")
p("""Optionen: `ram=16k|1k|2k` (Standard 16k), `traps=on|off`. Programme
sind `.p`-Dateien, angegeben mit `--tape`: `LOAD ""` lädt das Programm aus
dem Bandschacht und `SAVE "NAME"` schreibt es, über Traps im ROM.""")

h2("Sinclair ZX Spectrum")
p("""Modelle `48k`, `128k` und `plus2`. Optionen: `traps=on|off` (Standard
on: `LOAD ""` kopiert Standard-Bandblöcke auf einmal; off spielt das Band in
Echtzeit), `autostart=on` (mit einem Band: tippt `LOAD ""` oder wählt den
Tape Loader des 128K-Menüs), `joystick=kempston|sinclair1|sinclair2|cursor|none`,
`mouse=kempston`, `issue=3|2`, `multiface=on`, `if1=on` mit
`microdrives=1..8`, `roms=stub`.""")
p("""Bänder sind TAP- oder TZX-Dateien (`--tape`); `LOAD ""` drückt PLAY,
und das Band hält an TZX-"Stop"-Blöcken an. Schnappschüsse (SNA, Z80) gehen
mit `--prg` hinein und übernehmen sofort. Tasten: Umschalt = CAPS SHIFT,
rechte Strg oder AltGr = SYMBOL SHIFT (die linke Strg auch, aber Strg+D, I, R
und Leertaste sind damit Kürzel; `"` ist AltGr+P), Rücktaste = DELETE, die
Cursortasten = CAPS SHIFT + 5–8, Escape = BREAK, `.` und `,` wie auf den
Tasten. Der rote Knopf des Multiface 128 ist MULTIFACE im Panel; die
Microdrives des Interface 1 nehmen MDR-Kassetten (`--disk`,
`FORMAT "m";1;"name"`, `SAVE *"m";1;"file"`, `CAT 1`, `LOAD *"m";1;"file"`).
Der RS-232-Anschluss hat noch kein Gerät auf der Rechnerseite.""")

h2("ColecoVision")
p("""Optionen: `port1`, `port2` = `controller` (Standard: Joystick, zwei
Feuerknöpfe, Tastenfeld), `superaction`, `driving`, `roller`, `none`;
`roms=stub`. Tasten: das Tastenfeld von Controller 1 auf der Ziffernreihe
(1–9, 0, - = *, = = #, [ ] = die lila und blauen Knöpfe des Super Action
Controllers, X = rechter Feuerknopf), das von Controller 2 auf dem
Ziffernblock; Joystick 1 auf den Cursortasten, Leertaste = linker
Feuerknopf.""")

h2("TI-99/4A")
p("""ROMs: das Konsolen-ROM (`994AROM.BIN`, 8 KB) und GROM 0–2
(`994AGROM.BIN`, 24 KB) im Unterordner `ti99` des ROM-Ordners; das DSR-ROM
des Diskettencontrollers (`DISK.BIN`, 8 KB) fügt die Diskettenkarte hinzu,
das Sprach-ROM (`SPCHROM.BIN`, 32 KB) den Speech Synthesizer (`speech=on|off`;
Parsec und andere Spiele sprechen).
`--videoformat pal` ist der TMS9929A. Optionen: `ram=32k|none` (die
32-KB-Speichererweiterung, Standard `32k`), `port1`, `port2` = `joystick`
oder `none`, `disk=on|off`, `roms=stub`. Module: `--cart SPIELC.BIN` — die
Dateien `G` (GROM) und `D` (zweite Bank) daneben werden mitgeladen.
Kassette CS1: `--tape band.wav`; `SAVE CS1` und `OLD CS1` fragen nach den
Rekordertasten, das sind play/record/stop/rewind des Medienschachts.
Disketten: `--disk spiel.dsk` (TI-Sektorabbilder, 90 oder 180 KB) in DSK1
des Diskettencontrollers. Tasten: Alt = FCTN, Feststelltaste = ALPHA LOCK
(beim Start an), Pfeiltasten = FCTN+E/S/D/X, Rücktaste = FCTN+S, Entf =
FCTN+1, Esc = FCTN+9 (BACK), F4 = FCTN+4 (CLEAR).""")

h2("Nintendo NES / Famicom")
p("""Optionen: `port1`, `port2` = `controller`, `zapper`, `arkanoid`, `none`;
`fourscore=on` (Anschlüsse 3 und 4). Tasten: das Steuerkreuz auf den
Cursortasten, Leertaste und X = A, Z = B, rechte Umschalt = Select, Enter =
Start. Module sind iNES- oder NES-2.0-Dateien mit den Mappern 0–4 (NROM,
MMC1, UxROM, CNROM, MMC3).""")

h2("Super Nintendo / Super Famicom")
p("""Optionen: `port1`, `port2` = `pad`, `mouse`, `superscope` (nur Port 2),
`none`; `multitap=on` (Multiplayer 5 in Port 2); `roms=stub`. Tasten: das
Steuerkreuz auf den Cursortasten, Z = B (auch Leertaste), X = A, A = Y, S =
X, Q = L, W = R, rechte Umschalt = Select, Enter = Start. Module sind
`.sfc`/`.smc` (LoROM, HiROM, ExHiROM, mit oder ohne Copier-Header); Spiele
mit Koprozessor-Chips (DSP, Super FX, SA-1) werden noch nicht
unterstützt.""")

h2("Vectrex")
p("""Der Vectrex zeichnet mit einem analog gesteuerten Strahl, nicht mit
Pixeln. mimic folgt dem Strahl und zeigt im Fenster ein gerendertes Bild
davon (Hochformat, 800 × 1000 Pixel).""")
table(["Option", "Werte"], [
    ["`port1`", "`controller` (Standard) oder `none`"],
    ["`port2`", "`controller` (Standard), `lightpen`, `imager` (3D Imager) oder `none`"],
    ["`lines`", "`fine` (Standard: dünne Linien, helle Punkte, wo der Strahl ruht) oder "
     "`soft` (gleichmäßige, hellere Linien mit weichem Rand)"],
    ["`eye`", "mit dem 3D Imager: `left` (Standard), `right` oder `both`"],
    ["`roms`", "`stub`: ein Testbild ohne das System-ROM"],
], [0.22, 0.78])
p("""Steuerung: der Analogstick auf dem linken Stick des Gamepads (analog)
oder Cursortasten und Steuerkreuz (Vollausschlag); Knöpfe 1–4 auf Z X C V
oder A B X Y des Gamepads; Leertaste = Knopf 1.""")
bullets(
    "**System-ROM.** Das echte ROM hat 8 KB: das Executive und das eingebaute Spiel "
    "Mine Storm. Ein 4-KB-Abbild nur des Executive startet Module, aber ohne Modul "
    "gibt es kein Mine Storm, und Module, die Mine Storms Routinen aufrufen (etwa "
    "3-D Mine Storm), bleiben hängen.",
    "**Start.** Nach dem Einschalten zeigt das Executive etwa zehn Sekunden seinen "
    "Titel, bevor ein Modul startet.",
    "**3D Imager.** Die rotierende Farbscheibe wird emuliert (Motor, Indeximpuls, "
    "Farbfilter); das Bild zeigt das mit `eye` gewählte Auge.",
)
code("""mimic-desktop --machine vectrex --cart "Scramble (1982).vec"
mimic-desktop --machine vectrex --option lines=soft --cart game.vec
mimic-desktop --machine vectrex --option port2=imager --cart 3d-game.vec""")

h2("Apple II")
p("""Die Apple-II-Familie als vier Modelle: der ursprüngliche II (Integer
BASIC), der II+ (Applesoft), der IIe und der enhanced IIe (65C02, MouseText).
Jedes Modell hat eine Language Card, eine Disk-II-Karte mit zwei Laufwerken
in Schacht 6 und eine Mockingboard-Soundkarte in Schacht 4. Farbe kommt vom
Monitor: der Apple sendet ein Schwarzweiß-Punktmuster, und ein Farbfernseher
dekodiert Farben daraus, darum ist der Standardmonitor `tv`; Textbildschirme
erscheinen schwarzweiß (der Apple schaltet den Farbburst ab), und `--crt raw`
zeigt das Punktmuster selbst.""")
table(["Option", "Werte"], [
    ["`ram`", "II/II+: `4k` … `48k` (Standard `48k`)"],
    ["`lc`", "II/II+: Language Card `on` (Standard) oder `off`"],
    ["`aux`", "IIe: Zusatzspeicher `64k` (Standard, Extended 80-Column Card), `1k` oder `none`"],
    ["`slot4`", "`mockingboard` (Standard) oder `none`"],
    ["`slot6`", "`disk2` (Standard) oder `none` (ohne startet die Maschine BASIC, "
     "statt auf eine Diskette zu warten)"],
    ["`port`", "Game I/O: `joystick` (Standard), `paddles` oder `none`"],
    ["`roms`", "`stub`: ein Test-ROM ohne die Apple-ROMs"],
], [0.22, 0.78])
p("""Tastatur: die PC-Tasten tippen, was ihre Beschriftung zeigt (Buchstaben
beim II und II+ in Großschrift); Rücktaste und ← sind der ← des Apple
(Rückschritt), die Alt-Tasten sind Open Apple und Solid Apple, **F5 ist
RESET** (beim IIe mit Strg, wie an der echten Maschine). Der Joystick ist
analog: der linke Stick des Gamepads oder der Ziffernblock mit
Vollausschlag.""")
bullets(
    "**Disketten.** DSK/DO (DOS-3.3-Reihenfolge), PO (ProDOS-Reihenfolge), NIB und "
    "WOZ 1/2 kommen in Schacht 6, Laufwerk 1 (`--disk`; ein zweites `--disk` füllt "
    "Laufwerk 2). Mit dem Boot-PROM bootet die Maschine die Diskette beim Einschalten; "
    "ohne Diskette wartet sie mit laufendem Laufwerk, wie die echte.",
    "**Kassette.** WAV-Aufnahmen (`--tape`). Das Band läuft, während ein Programm "
    "darauf hört, also jederzeit `LOAD` (oder das `R` des Monitors) tippen. Zum "
    "Speichern eine leere Datei einlegen, Umschalt+F9 (RECORD), `SAVE`, dann F10 "
    "(STOP): die Aufnahme wird als WAV-Datei geschrieben.",
    "**Sequencer-PROM.** Mit dem P6-PROM (341-0028) liest und schreibt die Disk II "
    "wie die echte Karte; ohne es steht ein Modell der Karte ein.",
)
code("""mimic-desktop --machine apple2 --disk "Zork I.dsk"
mimic-desktop --machine apple2 --model iiplus --disk game.woz
mimic-desktop --machine apple2 --model ii --option slot6=none""")

h2("Macintosh")
p("""Der kompakte Macintosh als vier Modelle: 128K und 512K (64-KB-ROM,
400K-Laufwerke einseitig, Tastatur M0110 ohne Ziffernblock), der Plus
(128-KB-ROM, 800K-Laufwerke, SCSI, Tastatur M0110A mit Ziffernblock und
Pfeilen) und der SE (256-KB-ROM, ADB-Tastatur und -Maus, zwei interne
Laufwerke, SCSI). Ohne Diskette zeigt der Mac das Diskettensymbol mit
Fragezeichen, wie der echte. Der Bildschirm ist der eingebaute
Schwarzweißmonitor (Profil `mac`).""")
table(["Option", "Werte"], [
    ["`ram`", "Plus/SE: `512k`, `1m` (Standard), `2m` oder `4m`"],
    ["`serial`", "`loopback`: beide seriellen Anschlüsse auf sich selbst zurückgeführt"],
    ["`roms`", "`stub`: ein Test-ROM ohne die Mac-ROMs"],
], [0.22, 0.78])
p("""Tastatur: die PC-Tasten sitzen, wo die des Mac in US-Belegung sind;
**Alt ist Command** (neben der Leertaste, wie beim Mac), die Windows-Tasten
sind Option, Num ist das Clear des Ziffernblocks. Die Maus wird mit einem
Klick ins Fenster eingefangen. Das Panel hat POWER, RESET und den
INTERRUPT-Schalter des Programmierers.""")
bullets(
    "**Disketten.** 400K- und 800K-Abbilder, roh (`.dsk`, `.img`) oder DiskCopy 4.2 "
    "(`.image`), mit Tag-Bytes; zurückgeschrieben im eigenen Format. Das erste "
    "`--disk` kommt ins interne Laufwerk, das nächste ins externe (beim SE zuerst "
    "ins zweite interne).",
    "**Festplatten.** Ein Festplattenabbild (ganze 512-Byte-Blöcke mit Apple-"
    "Partitionstabelle und Treiber, wie Apples HD SC Setup es anlegt), angegeben mit "
    "`--disk`, geht an die erste freie SCSI-ID: ein Diskettenlaufwerk lehnt es ab "
    "und reicht es weiter (Plus und SE).",
    "**Systemsoftware.** System 6.0.8 passt zum Plus (ab 1 MB) und zum SE; der 128K "
    "braucht System 1.x/2.x auf 400K-MFS-Disketten, der 512K bis 3.x.",
)
code("""mimic-desktop --machine mac --disk "System Tools.image"
mimic-desktop --machine mac --model se --disk system.dsk --disk hd20.img
mimic-desktop --machine mac --model 128k --disk mfs-system.dsk""")

# ---- 6 Medien ------------------------------------------------------------------

h1("Disketten, Kassetten, Module und Programme")
table(["Art", "Formate", "Option"], [
    ["Diskette", "D64, D71, D81, G64, D80/D82 (PET), ATR (Atari), DSK/EDSK (CPC), ADF "
     "(Amiga DD und HD), extended ADF, DMS (nur lesen), DSK/DO/PO/NIB/WOZ (Apple II), "
     "400K/800K-DSK und DiskCopy 4.2, SCSI-Festplattenabbilder (Macintosh); "
     "ein Ordner als virtuelle Diskette",
     "`--disk`"],
    ["Kassette", "TAP (Commodore), CAS (Atari), CDT/TZX (CPC), P/81 (ZX81), TAP/TZX (ZX Spectrum), WAV (Apple II); ein PRG wird "
     "im Bandformat der Maschine aufgenommen", "`--tape`"],
    ["Modul", "CRT (C64/C128), rohe VIC-20- und C16/Plus/4-Dumps, CAR oder 8/16-KB-ROM "
     "(Atari), .a26/.bin (Atari 2600), .col/.rom (ColecoVision, MegaCart), .nes (NES), "
     ".sfc/.smc (SNES), .vec/.bin (Vectrex, bis 64 KB)", "`--cart`"],
    ["Programm", "PRG, T64, XEX", "`--prg`"],
], [0.12, 0.74, 0.14])
p("""Abbilder werden über eine **Copy-on-Write-Schicht** geöffnet: die Datei
auf Ihrer Platte bleibt unverändert, während die emulierte Maschine in ihre
Kopie schreibt. Änderungen werden nur auf Wunsch zurückgeschrieben
(`flush_media` im Debugger, Save im Medienpanel).""")
h2("Commodore-Laufwerke")
p("""Jeder Commodore-Laufwerksschacht hat zwei Betriebsarten: **echte
Laufwerksemulation** (die eigene CPU des Laufwerks und die GCR-Daten der
Diskette, zyklusgenau — nötig für Schnelllader und Kopierschutz) und das
**virtuelle Laufwerk** (bedient Dateien auf Protokollebene — schnell, aber
nur für normales LOAD und SAVE). Das virtuelle Laufwerk antwortet trotzdem im
Tempo der seriellen KERNAL-Routinen (etwa 400 Bytes je Sekunde, wie eine
1541); mit `--option fastload=on` reicht der C64 jedes Byte von
TALK/LISTEN/SECOND/TKSA/CIOUT/ACPTR/UNTLK/UNLSN des KERNAL direkt an es
weiter (nur Original-KERNAL; Schnelllader sind nicht betroffen).""")

# ---- 7 Monitore ----------------------------------------------------------------

h1("Monitorprofile")
p("""Maschinen erzeugen immer das rohe Bild; Monitor- und Signaleffekte
(Farbsäume des Composite-Signals, PAL-Farbmischung, NTSC-Artefaktfarben,
Zeilen, Phosphor) kommen danach dazu. `--crt PROFIL` wählt das Aussehen;
ohne Angabe benutzt jede Maschine den Monitor, mit dem sie ausgeliefert
wurde.""")
table(["Profil", "Verwendung"], [
    ["`raw`", "scharfe Pixel, keine Effekte (Standard bei NES, ColecoVision und Vectrex)"],
    ["`1702`, `1084s`", "Commodore-Monitore über S-Video (C64, VIC-20, C16/Plus/4, C128)"],
    ["`1084s-rgb`", "der 1084S an seinem RGB-Eingang (Amiga, SNES, C128 VDC)"],
    ["`tv`, `pal-tv`, `ntsc-tv`", "ein Fernseher über Composite (Atari, ZX81, ZX Spectrum, Apple II)"],
    ["`pet-white`, `pet-green`", "die eingebauten Monitore des PET"],
    ["`ctm644`", "Amstrad-Farbmonitor (CPC)"],
    ["`gt65`", "Amstrad-Grünmonitor"],
    ["`mac`", "der eingebaute Schwarzweißmonitor des kompakten Macintosh"],
    ["`logophor`, `logophor-scope`", "der Monitor des Logophor 16: eine RGB-Röhre mit deutlichen Rasterlinien und seine grüne Leuchtröhre mit Glühen und Nachleuchten; das Programm schaltet um (Panoramas Oszilloskop-Theme), der Monitor folgt"],
    ["`lite`", "nur Seitenverhältnis und leichte Zeilen, für langsame Rechner und die Web-Fassung"],
], [0.30, 0.70])
p("""Neben dem Signal (Composite, S-Video, RGB, monochrom) legt ein Profil
Zeilen, die Phosphormaske (`shadow`, `aperture` oder `slot` mit Stärke und
Teilung), Bloom (helle Flächen leuchten), die Wölbung des Glases, eine
Vignette und das Nachleuchten des Phosphors fest (eine Spur hinter bewegten
Objekten, wie auf dem grünen PET- und GT65-Monitor). Dasselbe Bild kommt aus
der Grafikkarte im Desktop-Programm und aus dem Prozessor bei
Schnappschüssen, der Vorschau und der Web-Fassung.""")
p("""Eigene Profile sind TOML-Dateien, die von einer Voreinstellung ausgehen
und einzelne Werte ändern (`base = "1084s"` …), geladen mit `--crt
meinprofil.toml`. Das Desktop-Programm liest eine solche Datei neu, wenn sie
sich ändert, so dass ein Profil bei laufender Maschine abgestimmt werden
kann. Die Live-Vorschau hat einen **Monitor-Editor** unter
`http://127.0.0.1:6580/crt`: Schieberegler für jeden Wert, das Bild der
Maschine durch das Profil und einen Knopf, der die TOML-Datei speichert.""")

# ---- 8 Vorschau ----------------------------------------------------------------

h1("Live-Vorschau im Browser")
p("""Mit `--preview` liefert der Emulator eine Betrachterseite: öffnen Sie
`http://127.0.0.1:6580/` in einem Browser, um die laufende Maschine zu
sehen. Der Betrachter leitet Tastatur, Gamepad und Maus weiter, so dass die
Maschine aus dem Browser gespielt werden kann, und zeigt das
Konsolenpanel.""")
code("""mimic-headless --machine c64 --preview
mimic-desktop  --machine amiga --preview      # Fenster und Browser""")
table(["Adresse", "Inhalt"], [
    ["`/`", "die Betrachterseite"],
    ["`/snapshot.png`", "das aktuelle Bild als PNG"],
    ["`/preview` (WebSocket)", "ein Bildstrom als PNG oder QOI, auch durch ein Monitorprofil"],
    ["`/stream` (WebSocket)", "ein Rohstrom: Farbindizes, nur geänderte Zeilen, optional Ton"],
    ["`/controls`", "die Knöpfe und Schalter der Konsole"],
    ["`/gfx`, `/gfx/VIEW.png`", "die Grafiken, mit denen der Videochip arbeitet: Sprites, Zeichensätze, "
     "Bitmaps, Bildschirmspeicher, Kacheln, Paletten"],
    ["`/overlays`", "die Debug-Overlays, die die Maschine anbietet"],
    ["`/crt`", "der Monitorprofil-Editor"],
], [0.30, 0.70])
p("""Parameter: `filter=raw|crt`, `profile=NAME`, `scale=1..4`, für die
Ströme `fps=1..50`, `format=png|qoi`, `delta=0|1`, `audio=0|1`. Der Server
hört nur auf 127.0.0.1.""")
p("""**Overlays** (`overlay=beam,grid,…`, Schalter im Kopf des Betrachters)
zeichnen Debug-Informationen über das rohe Bild: die Strahlposition (auch in
einem halb gezeichneten Bild), ein Rastergitter mit Zeilennummern und was
der Videochip wo getan hat — die Bad Lines des C64 und wo jedes Sprite
wirklich erschien, die Moduszeilen der Atari-Display-List, die
DMA-Slot-Karte und Copper-Schreibzugriffe des Amiga, CRTC-Änderungen des
CPC, Sprites, Interrupts und HDMA bei NES und SNES. Das Panel
**Graphics data** unter dem Bild zeigt Sprites, Zeichensätze, Bitmaps und
andere Grafiken im Speicher; seine Optionen blättern zu anderen Adressen
(`index=2 addr=$2000`).""")

# ---- 9 Debugger --------------------------------------------------------------

h1("Debugging")
p("""Jede Maschine lässt sich auf dieselbe Weise debuggen. Der Debugger
kennt keine bestimmte Maschine: er arbeitet über Register, Speicherräume,
Chips und Geräte, die jede Maschine beschreibt, so dass zweite Prozessoren
(der 6502 der 1541, der Z80 des C128, Copper und Blitter des Amiga, die
Sound-CPU des SNES) eigene Debug-Ziele sind. Speicher ansehen verändert ihn
nie — ein Interrupt-Register im Debugger zu lesen quittiert den Interrupt
nicht.""")
h2("Den Debug-Server starten")
code("""mimic-headless --machine c64 --debug          # angehalten, Port 6510
mimic-desktop  --machine c64 --debug          # mit Fenster und Ton
mimic-headless --machine c64 --vice --preview --run""")
h2("Der Python-Client polydbg")
p("`tools/polydbg.py` braucht nur Pythons Standardbibliothek:")
code("""python tools/polydbg.py info                 # Geräte und Speicherräume
python tools/polydbg.py regs
python tools/polydbg.py disasm
python tools/polydbg.py break '$0810'        # Haltepunkt
python tools/polydbg.py resume --wait        # laufen lassen, bis es hält
python tools/polydbg.py peek '$d000' 64
python tools/polydbg.py chips vic2           # Chip-Register
python tools/polydbg.py freeze '$0400' 9     # einen Speicherwert einfrieren (Cheat)
python tools/polydbg.py search start         # Cheat-Suche
python tools/polydbg.py save state.bin       # Savestate
python tools/polydbg.py gfx sprites index=2 --scale 4 -o s.png   # Grafiken im Speicher
python tools/polydbg.py picture --overlays beam,badlines -o now.png
python tools/polydbg.py rewind --on 1        # jedes Bild einen Zustand behalten
python tools/polydbg.py rewind 50            # 50 Bilder zurück
python tools/polydbg.py back 3               # 3 Befehle zurück
python tools/polydbg.py shell                # interaktive Eingabe""")
p("""`--host`/`--port` wählen den Server, `--json` gibt rohe Antworten aus;
`python tools/polydbg.py --help` listet alle Befehle.""")
h2("Was der Debugger kann")
bullets(
    "**Ausführung:** anhalten, weiterlaufen, einen Befehl, einen Zyklus, eine "
    "Rasterzeile oder ein Bild schreiten, step over und step out, bis zu einer "
    "Adresse laufen.",
    "**Haltepunkte:** auf Ausführung, Lese- und Schreibzugriffe, Rasterpositionen, "
    "IRQ und NMI; mit Bedingungen wie `A == $FF && [$D012] > 100` und Trefferzählern.",
    "**Einsicht:** Register, Speicher, Disassembly, Chip-Register, Strahlposition; "
    "Symbole aus VICE-Label-Dateien und KickAssembler.",
    "**Zustand:** Savestates, eine Raster-Spur der Befehle und Speicherzugriffe.",
    "**Rückwärts:** bildweise zurückspulen und Befehl für Befehl zurückschreiten "
    "(der Emulator spielt vom letzten gesicherten Moment exakt nach).",
    "**Bilder:** das aktuelle Bild, auch halb gezeichnet, mit markiertem Strahl und "
    "Overlays; die Grafikdaten, mit denen der Videochip arbeitet.",
    "**Cheats:** Werte im Speicher einfrieren, eingrenzende Suche nach unbekannten "
    "Adressen (Leben, Energie), Cheat-Dateien mit Code-Patches, Import von "
    "POKE-Listen, VICE-Monitor-Befehlen und Adresse/Wert-Codes, und eine "
    "Cheat-Datenbank, die nach der CRC-32 des Abbilds greift.",
    "**Geräte und Medien:** Geräte in Anschlüsse stecken, Medien einlegen und "
    "auswerfen, Eingaben einspeisen und aufzeichnen.",
)
h2("VICE-Binärmonitor (VS64 und andere)")
p("""Mit `--vice` spricht mimic auch das Binärmonitor-Protokoll des
VICE-Emulators. Werkzeuge wie VS64 (Visual Studio Code), C64 Studio oder
IceBro Lite können dann jede Commodore-8-Bit-Maschine in mimic debuggen. In
der `launch.json` von VS64:""")
code("""{ "type": "vice", "request": "attach", "name": "mimic",
  "hostname": "127.0.0.1", "port": 6502 }""")
note("`127.0.0.1` benutzen, nicht `localhost`: die Server hören nur auf IPv4.", "Hinweis")

# ---- 10 Fehlersuche ---------------------------------------------------------

h1("Fehlersuche")
table(["Problem", "Lösung"], [
    ["`unknown machine 'vectrex'` (oder eine andere neue Maschine)",
     "Sie haben eine veraltete `emu-desktop.exe` gestartet. Benutzen Sie `mimic-desktop.exe` "
     "oder `cargo run --release -p emu-desktop -- …`."],
    ["`missing ROM '…'` oder `no … ROM`",
     "Prüfen Sie den mit `--rom-dir` angegebenen Ordner und die genaue Dateigröße; die "
     "Meldung nennt die akzeptierten Dateinamen."],
    ["Der Emulator läuft langsam oder ruckelt",
     "Den Release-Build benutzen. `EMU_DESKTOP_DEBUG=1` gibt alle 5 Sekunden Bild- und "
     "Tonstatistiken aus. Den Geschwindigkeitsregler in der Fußzeile prüfen (Strg+Leertaste)."],
    ["Das Fenster ist zu groß oder zu klein",
     "Das Fenster passt sich dem Bildschirm an; ziehen Sie es, oder setzen Sie `--scale 1`/`--scale 3`."],
    ["Das Gamepad tut nichts",
     "In die Konsolenausgabe sehen: das Pad muss als `gamepad 0` erscheinen. JOY PORT im "
     "Panel prüfen und ob die Maschine in diesem Port einen Joystick hat."],
    ["Die Tastatur tippt, statt zu steuern",
     "JOY KEYS im Panel (oder `--joykeys`) auf CURSOR oder NUMPAD stellen."],
    ["Ein Diskettenprogramm wartet auf die nächste Diskette",
     "Alle Disketten mit `--disk` angeben und Strg+D drücken, oder das Abbild auf das Fenster ziehen."],
    ["Vectrex: schwarzer Bildschirm nach dem Titel des Moduls",
     "Das Modul braucht vielleicht das volle 8-KB-System-ROM (es ruft Mine Storms Routinen) "
     "oder einen 3D Imager (`--option port2=imager`)."],
    ["Kein Ton unter Linux", "Vor dem Bauen `libasound2-dev` installieren."],
], [0.36, 0.64])

h2("Hilfe bekommen")
bullets(
    "`mimic-desktop --help` und `mimic-headless --help` listen jede Option und Taste.",
    "`docs/usage.md` ist die ausführliche Referenz zu Bauen, ROMs, Medien, dem "
    "Debug-Protokoll und den Testsuiten.",
    "`README.md` listet alle CPUs, Maschinen, Chips und Geräte mit ihrem Stand.",
)


part("Teil II — Referenz", "Jede Option von mimic-desktop und mimic-headless, die "
     "Maschinenoptionen, die Startprofile und die Web-Oberfläche")

# ---- 11 Überblick ---------------------------------------------------------------

h1("Überblick über die Referenz")
p("""mimic ist ein Emulatorkern mit drei Oberflächen. Sie nehmen dieselben
Maschinen, dieselben ROMs und dieselben Medien und teilen ein Vokabular, um
eine Maschine zu konfigurieren: eine Maschinen-ID, ein Modell, ein
Videoformat, ROM-Rollen und Maschinenoptionen als `KEY=VALUE`-Paare.""")
table(["Oberfläche", "Programm", "Was sie hinzufügt"], [
    ["Desktop", "`mimic-desktop`",
     "Ein Fenster mit Ton, Tastatur, Gamepads und Maus; Konsolen- und Medienpanel; "
     "Zurückspulen; die CRT-Shader auf der GPU; optional Debug-, VICE-Monitor- und "
     "Vorschau-Server."],
    ["Headless", "`mimic-headless`",
     "Kein Fenster, kein Ton: rechnet eine Anzahl Bilder und schreibt Bilder, Ton, "
     "Savestates; bietet Debugger und Live-Vorschau an; liest und schreibt echte "
     "Disketten über Greaseweazle oder ZoomFloppy."],
    ["Web", "`index.html` in `target/web`",
     "Derselbe Kern als WebAssembly im Browser, bedient von einer Seite: Dateien werden "
     "hochgeladen (und auf Wunsch im Browser behalten), die Emulation läuft in einem "
     "Web Worker, das Bild wird mit WebGL2 gezeichnet."],
], [0.14, 0.24, 0.62])
h2("Woher Einstellungen kommen")
p("""Eine Einstellung kann aus vier Quellen kommen; die späteren gewinnen:""")
bullets(
    "**Standardwerte** der Maschine (Modell, Videoformat, Geräte an den Anschlüssen).",
    "**Startprofile** in `profiles.yaml` (`--profile NAME`), einschließlich eines "
    "`settings.roms`-Ordners.",
    "**Kommandozeilenoptionen** (`--machine`, `--model`, `--option KEY=VALUE` …); auf "
    "der Web-Seite übernehmen die Formularfelder und die URL-Parameter diese Rolle.",
    "**Umgebungsvariablen**: `EMU_ROM_DIR` (ROM-Ordner), `EMU_CHEATS_DIR` "
    "(Cheat-Datenbank), `EMU_DESKTOP_DEBUG=1` (Statistik alle 5 s).",
)
h2("Wie diese Referenz zu lesen ist")
bullets(
    "Optionen stehen wie auf der Kommandozeile: `--disk DATEI`. Eine mit "
    "*(wiederholbar)* markierte Option darf mehrfach angegeben werden.",
    "Maschinenoptionen werden als `--option KEY=VALUE` angegeben; mehrere Werte stehen "
    "als `KEY=a|b|c`, der Standard zuerst, sofern nicht anders gesagt.",
    "Dieselben Optionen werden auf der Web-Seite ins Optionsfeld als `KEY=VALUE`-Wörter "
    "mit Leerzeichen getippt oder in der URL durch Kommas getrennt übergeben.",
    "`mimic-desktop --help`, `mimic-headless --help` und `mimic-headless --list` geben "
    "die Listen aus den Programmen selbst aus.",
)

# ---- 12 gemeinsame Optionen -------------------------------------------------------

h1("Optionen beider Kommandozeilenprogramme")
p("""Diese Optionen bedeuten in `mimic-desktop` und `mimic-headless` dasselbe.""")
h2("Die Maschine wählen")
table(["Option", "Bedeutung"], [
    ["`--machine ID`", "Die Maschine (Standard im Desktop `c64`, im Headless `sim6502`). "
     "Siehe die Maschinentabelle unten und `mimic-headless --list`."],
    ["`--model NAME`", "Modellvariante der Maschine, z. B. Atari `130xe`, Amiga `1200`, "
     "PET `8032`, Spectrum `128k`, Mac `se`. Modelle sind Konfiguration, keine "
     "getrennten Maschinen."],
    ["`--videoformat FORMAT`", "`pal`, `ntsc` oder `auto`. Standard ist das eigene Format "
     "der Maschine; `auto` wählt beim Atari 2600 nach etwa 20 Bildern NTSC oder PAL "
     "nach der Bildlänge des Programms. Maschinen mit einem Format lehnen das andere ab."],
    ["`--profile NAME`", "Startprofil aus `profiles.yaml` (im Arbeitsordner oder im "
     "Konfigurationsordner des Benutzers; `--profiles DATEI` nennt eine andere Datei). "
     "Optionen auf derselben Kommandozeile überschreiben das Profil. Ohne `--machine` und "
     "`--profile` listet der Desktop die Profile zur Auswahl."],
    ["`--profiles DATEI`", "Die Profildatei statt `profiles.yaml`."],
    ["`--list-profiles`", "Die Profile auflisten und beenden."],
    ["`--save-profile NAME`", "Die Zusammenstellung dieser Kommandozeile als Profil speichern."],
    ["`--version`, `--help`", "Version oder Optionsliste ausgeben und beenden."],
], [0.26, 0.74])
h2("Maschinen und Modelle")
table(["Maschinen-ID", "Maschine", "Modelle (`--model`)", "Videoformate (`--videoformat`)"], [
    ["`c64`", "Commodore 64", "–", "`pal` (Standard), `ntsc`"],
    ["`vic20`", "Commodore VIC-20", "–", "`pal` (Standard), `ntsc`"],
    ["`pet`", "Commodore PET", "`4032` (Standard), `2001`, `3032`, `8032`", "–"],
    ["`c16`, `plus4`", "Commodore 16, Plus/4", "–", "`pal` (Standard), `ntsc`"],
    ["`c128`", "Commodore 128", "–", "`pal` (Standard), `ntsc`"],
    ["`atari8`", "Atari 400/800/800XL/65XE/130XE",
     "`800xl` (Standard), `65xe`, `130xe`, `800`, `400`", "`pal` (Standard), `ntsc`"],
    ["`atari2600`", "Atari 2600 (VCS)", "–", "`auto` (Standard: aus dem Programm), "
     "`ntsc`, `pal`"],
    ["`cpc`", "Amstrad CPC", "`6128` (Standard), `664`, `464`", "–"],
    ["`amiga`", "Commodore Amiga", "`500` (Standard), `1000`, `2000`, `500plus` (ECS), "
     "`1200` (AGA)", "`pal` (Standard), `ntsc`"],
    ["`zx81`", "Sinclair ZX81 / TS1000", "`zx81` (Standard), `ts1000`", "–"],
    ["`spectrum`", "Sinclair ZX Spectrum", "`48k` (Standard), `128k`, `plus2`", "–"],
    ["`coleco`", "ColecoVision", "–", "`ntsc` (Standard), `pal`"],
    ["`nes`", "Nintendo NES / Famicom", "–", "`ntsc` (Standard), `pal`"],
    ["`snes`", "Super Nintendo / Super Famicom", "–", "`ntsc` (Standard), `pal`"],
    ["`vectrex`", "Vectrex", "`vectrex`", "–"],
    ["`apple2`", "Apple II, II+, IIe", "`iie` (Standard), `iie-unenhanced`, `iiplus`, "
     "`ii`", "–"],
    ["`mac`", "Apple Macintosh", "`plus` (Standard), `128k`, `512k`, `se`", "–"],
    ["`archimedes`", "Acorn Archimedes", "`a310` (Standard), `a305`, `a440`, `a3000`", "–"],
    ["`imsai`", "IMSAI 8080", "`8080`", "–"],
    ["`logophor16`", "Logophor 16", "`16`", "–"],
    ["`sim6502`", "6502-Referenzmaschine", "`default`", "–"],
], [0.14, 0.28, 0.34, 0.24])
note("""Eine Maschine mit Strich unter Videoformate hat eine feste Norm (das
Modell `ts1000` des ZX81 ist die 60-Hz-Maschine); ein Strich unter Modelle
heißt, `--model` wird abgelehnt. `--model pal` oder `ntsc` wird nie
angenommen.""", "Hinweis")
h2("ROMs")
table(["Option", "Bedeutung"], [
    ["`--rom-dir DIR`", "Der ROM-Ordner: `DIR/<maschine>/` enthält die Abbilder der Maschine, "
     "`DIR/drives/` die DOS-ROMs der Laufwerke. Abbilder werden an Namensanfang und Größe "
     "erkannt, so dass die üblichen Dump-Namen gehen. Standard: `settings.roms` des "
     "Profils, dann `$EMU_ROM_DIR`."],
    ["`--rom ROLLE=DATEI`", "Ein Abbild für eine Rolle, z. B. `kernal=kernal.bin`, "
     "`dos1541=dos1541.bin`, `kickstart=kick13.rom` *(wiederholbar)*."],
    ["`--option roms=stub`", "Stattdessen mit den eingebauten Test-ROMs laufen (jede Maschine "
     "hat einen kleinen Stub, der ein Bild zeigt; Software läuft darauf nicht)."],
], [0.26, 0.74])
h2("Medien")
table(["Option", "Bedeutung"], [
    ["`--prg DATEI`", "Ein Programm: PRG oder T64 (Commodore), XEX (Atari), Z80/SNA-Schnappschuss "
     "(Spectrum), .P (ZX81), BIN/HEX (IMSAI). Es kommt in den Programmschacht der Maschine "
     "und läuft nach dem Booten (`--autostart`, wo die Maschine es braucht)."],
    ["`--disk DATEI`", "Ein Diskettenabbild ins erste Laufwerk: D64/D71/D81/G64 (Laufwerk 8), "
     "ATR (Atari D1:), ADF/HD-ADF/extended ADF/DMS (Amiga DF0:), DSK/EDSK (CPC), "
     "DSK/DO/PO/NIB/WOZ (Apple II Schacht 6), 400K/800K-DSK oder DiskCopy 4.2 und "
     "Festplattenabbilder (Mac, SCSI), ADF (Archimedes), TRD (Spectrum mit `beta=on`), MDR "
     "(Microdrive). *(wiederholbar)*: das nächste Abbild geht ins nächste Laufwerk; ein "
     "Laufwerk, das ein Abbild ablehnt, reicht es weiter. Weitere Disketten wechseln im "
     "Desktop mit Strg+D ins erste Laufwerk."],
    ["`--tape DATEI`", "TAP/T64 (Datasette), CAS (Atari), CDT/TZX (CPC, Spectrum), TAP "
     "(Spectrum), WAV (Apple II), .P (ZX81), PTP (IMSAI-Lochstreifen). Ein PRG wird zuerst "
     "als Band aufgenommen."],
    ["`--cart DATEI`", "Ein Modul: CRT (C64/C128), rohe VIC-20- (Ladeadresse + ROM) oder "
     "C16/Plus/4-Dumps (16/32 KB), Atari CAR oder rohes 8/16-KB-ROM, 2600 .a26/.bin, "
     "ColecoVision-ROM, NES iNES/NES 2.0, SNES .sfc/.smc, Vectrex .vec/.bin."],
    ["`--autostart`", "Nach dem Booten die Diskette (C64: das erste Programm von Laufwerk 8) "
     "oder das Band (Spectrum: LOAD \"\" und PLAY) laden und starten."],
], [0.2, 0.8])
note("""Schreibzugriffe gehen in eine Copy-on-Write-Schicht. Die Abbilddatei auf
der Platte ändert sich nur, wenn das Medium zurückgeschrieben wird: das
Medienpanel des Desktops hat je Schacht einen Save-Knopf, der Debugger den
Befehl `flush_media`. Das Headless-Programm schreibt nie zurück.""", "Hinweis")
h2("Cheats, Eingabe, Netz, Drucken")
table(["Option", "Bedeutung"], [
    ["`--cheats DIR`", "Cheat-Datenbank: `DIR/<maschine>/<crc32>.toml` gilt für die eingelegten "
     "Abbilder, auch nach einem Diskettenwechsel. Standard `$EMU_CHEATS_DIR`, sonst "
     "`./cheats`, falls vorhanden."],
    ["`--cheat DATEI`", "Eine Cheat-Datei: `.toml` (Freezes und Patches), eine POKE-Liste "
     "(`.pokes`, `.bas`), VICE-Monitor-Befehle (`.mon`, `.vice`) oder Adresse/Wert-Codes "
     "(`.txt`, `.codes`) *(wiederholbar)*."],
    ["`--input DATEI`", "Nur Desktop: ein Joystick-Zuordnungsprofil (TOML, wie SAVE MAPPING es "
     "in `<Benutzerordner>/input/<maschine>.toml` speichert)."],
    ["`--net on|off`", "Das Netz des Rechners für Geräte, die es nutzen: WiC64 "
     "(`userport=wic64`) und Ultimate Command Interface (`uci=on`) des C64. Standard `on`."],
    ["`--uci-dir DIR`", "Der Ordner, in dem das DOS des Ultimate Command Interface arbeitet "
     "(Standard `.mimic/ultimate` im Benutzerordner, beim ersten Schreiben angelegt; "
     "Pfade können ihn nicht verlassen)."],
    ["`--sd2iec-dir DIR`", "Die Karte des C64-SD2IEC (`--option sd2iec=on` oder "
     "`drive8=sd2iec`; Standard `.mimic/sd2iec` im Benutzerordner, beim Start angelegt)."],
    ["`--uci-mount NAME=DIR`", "Ein Ordner des Rechners als UCI-Laufwerk `/NAME`, wie SD, "
     "Usb0, Usb1, Flash, Temp des Ultimate *(wiederholbar)*."],
    ["`--printer-dir DIR`", "Wohin `--option printer=on` seine Aufträge schreibt (Standard "
     "`./printer`): `print-001.prn` und `print-001-1.png` je Seite."],
    ["`--host-printer NAME`", "Jede fertige Seite zusätzlich an diesen Drucker des Rechners "
     "schicken; `default` ist der Standarddrucker."],
], [0.26, 0.74])
h2("Server")
table(["Option", "Bedeutung"], [
    ["`--debug [ADR]`", "Das native Debug-Protokoll anbieten (JSON-Zeilen über TCP; Standard "
     "`127.0.0.1:6510`, nur Loopback). Headless: anbieten statt Bilder zu rechnen, "
     "angehalten bis `resume` oder `--run`."],
    ["`--vice [ADR]`", "Zusätzlich das VICE-Binärmonitor-Protokoll für VS64, C64 Studio und "
     "andere Werkzeuge anbieten (Standard `127.0.0.1:6502`); schließt `--debug` ein."],
    ["`--preview [ADR]`", "Die Live-Vorschau anbieten: Betrachterseite, `/snapshot.png`, der "
     "WebSocket-Strom `/preview`, der Rohstrom `/stream` mit Ton, die Grafikansichten "
     "unter `/gfx` und der CRT-Profil-Editor unter `/crt` (Standard `127.0.0.1:6580`); "
     "schließt `--debug` ein."],
], [0.24, 0.76])
note("""Die Server hören auf der Loopback-Schnittstelle; sie sind für Werkzeuge
auf demselben Rechner gedacht.""", "Hinweis")

# ---- 13 Desktop ----------------------------------------------------------------------

h1("mimic-desktop")
code("""mimic-desktop --machine c64 --disk game.d64 --autostart
mimic-desktop --profile mac-se-755
mimic-desktop --machine amiga --disk demo-a.adf --disk demo-b.adf --crt 1084s
mimic-desktop --machine c64 --option port1=stmouse --option drive8=1541""")
h2("Nur im Desktop")
table(["Option", "Bedeutung"], [
    ["`--scale N`", "Anfängliche Fenstergröße in Zeilen je emulierter Zeile (Standard 2, "
     "weniger, wenn das Fenster nicht auf den Bildschirm passt)."],
    ["`--crt PROFIL`", "Der Monitor: `raw`, `lite`, `1702`, `1084s`, `1084s-rgb`, "
     "`tv`, `pal-tv`, `ntsc-tv`, `pet-white`, `pet-green`, `ctm644`, `gt65`, `mac`, "
     "`logophor`, `logophor-scope` oder eine `.toml`-Datei, die bei Änderung neu gelesen wird. Standard: der eigene "
     "Monitor der Maschine."],
    ["`--deinterlace MODUS`", "Zeilensprungbilder (Amiga LACE): `weave` (Standard, beide "
     "Halbbilder verschränkt) oder `bob` (die Zeilen des aktuellen Halbbilds verdoppelt); "
     "Strg+I schaltet im Betrieb um."],
    ["`--speed PROZENT`", "Mit 10, 25, 50, 100, 150, 200, 400 oder 800 % Geschwindigkeit starten."],
    ["`--warp-load on|off`", "Bei 100 %: stumm so schnell wie möglich laufen, solange ein "
     "Diskettenlaufwerk arbeitet und die Maschine eine Sekunde ohne Ton mit stehendem Bild war, "
     "bis das geladene Programm zum ersten Mal Ton oder eine Animation zeigt (Standard `on`)."],
    ["`--rewind-mb N`", "Speicher fürs Zurückspulen mit Strg+R (Standard 128; 0 = aus)."],
    ["`--joykeys TASTEN`", "Tasten, die den Joystick spielen: `cursor` (Cursortasten, "
     "Leertaste feuert; Standard bei den Konsolen), `numpad` (8/2/4/6, 0 oder rechte "
     "Strg; Standard bei den Computern) oder `off`. Auch ein Schalter im Konsolenpanel."],
    ["`--joyport N`", "Der Anschluss, den Tastatur-Joystick und Gamepad spielen, ab 1 "
     "(Standard: der Spielanschluss der Maschine, Port 2 beim C64/C128)."],
    ["`--no-panel`", "Mit geschlossenem Konsolenpanel starten (Umschalt+Alt+Links öffnet es)."],
    ["`--media-panel`", "Mit offenem Medienpanel starten (Umschalt+Alt+Rechts)."],
    ["`--footer`", "Mit sichtbarer Statusfußzeile starten (Strg+Leertaste)."],
], [0.24, 0.76])
h2("Tasten des Fensters")
table(["Taste", "Wirkung"], [
    ["Klick auf das Bild", "Tastatureingaben gehen an die Maschine; mit Maus, Paddles oder "
     "Lenkrad in einem Anschluss fängt der Klick die Maus des Rechners ein (die mittlere "
     "Taste gibt sie frei). Die linke Strg-Taste trägt die Kürzel unten; rechte Strg und "
     "AltGr erreichen die Maschine."],
    ["Umschalt+Alt+Links / Rechts", "Konsolenpanel (linker Rand) oder Medienpanel (rechter "
     "Rand) öffnen oder schließen; die Griffe an den Rändern tun dasselbe."],
    ["Strg+D", "Das nächste `--disk`-Abbild (oder ein hingezogenes) ins erste Laufwerk wechseln."],
    ["Strg+R", "Zurückspulen: gehalten läuft die Maschine rückwärts durch den Ring der Savestates."],
    ["Strg+I", "Weave- oder Bob-Deinterlacing (Amiga)."],
    ["PAUSE im Konsolenpanel", "Die ganze Maschine anhalten und weiterlaufen lassen "
     "(leuchtet im Stillstand, auch durch einen Debugger)."],
    ["Strg+Leertaste", "Statusfußzeile zeigen oder verbergen: Geschwindigkeitsregler "
     "(10–800 %), Schnappschuss-Knopf (`<maschine>-NNN.png`), emulierte Zeit, "
     "Zeigerposition, Joystick-Richtungen, Laufwerksaktivität."],
    ["F9 / Umschalt+F9 / F10 / F11", "Datasette: Play, Record, Stop, Rewind."],
    ["F12", "Zwischen den Bildschirmen einer Maschine wechseln: C128 VIC/VDC, IMSAI "
     "Frontplatte und Terminal."],
    ["F1–F7", "Konsolentasten der Atari 8-Bit (F1 START, F2 OPTION … F5 RESET, F6 HELP, "
     "F7 BREAK) und des Atari 2600 (F1 SELECT, F2 RESET, F3 Farbe/SW, F4/F5 "
     "Schwierigkeit); siehe `--help`."],
    ["Datei auf das Fenster ziehen", "Disketten kommen ins erste Laufwerk (Wechselliste), "
     "Module in den Modulschacht (die Maschine schaltet sich neu ein)."],
], [0.3, 0.7])
h2("Das Konsolenpanel")
p("""Das Panel am linken Rand zeigt PAUSE und die eigenen Knöpfe und
Schalter der Maschine: POWER und RESET, die Konsolenschalter des Atari 2600,
START/SELECT/OPTION des 800XL, RESTORE bei C64 und VIC-20, 40/80 und CAPS
LOCK beim C128, MULTIFACE beim Spectrum. Darunter folgen die Eingabeschalter
**JOY KEYS**, **JOY PORT** und **PAD FIRE**, **SAVE MAPPING** (schreibt die
Zuordnung nach `<Benutzerordner>/input/<maschine>.toml`) und je geladenem
Cheat ein OFF/ON-Schalter. Die Schalter des IMSAI sitzen stattdessen auf
seinem Frontplattenbild und werden mit der Maus bedient.""")
h2("Das Medienpanel")
p("""Das Panel am rechten Rand listet jedes Laufwerk, jede Datasette und
jeden Modulschacht der Maschine mit einem Knopf, der einlegt (nativer
Dateidialog) oder auswirft, und **Save**, das die Copy-on-Write-Schicht in
die Abbilddatei zurückschreibt. Beim C64 nennt die Laufwerkszeile die
Betriebsart: `drive 8 (1541)` für das echte Laufwerk mit DOS-ROM, `drive 8
(virtual)` ohne.""")

# ---- 14 Headless ------------------------------------------------------------------

h1("mimic-headless")
code("""mimic-headless --machine c64 --prg demo.prg --frames 300 --snapshot out.png
mimic-headless --machine c128 --option display=80 --frames 600 --screen vdc --snapshot c128.png
mimic-headless --machine amiga --disk wb13.adf --frames 9000 --crt 1084s:2 --wav sound.wav
mimic-headless --machine c64 --vice --preview --run
mimic-headless --list""")
h2("Nur im Headless")
table(["Option", "Bedeutung"], [
    ["`--frames N`", "Zu rechnende Bilder (Standard 1)."],
    ["`--snapshot DATEI.png`", "Das letzte Bild als PNG schreiben (rohe Pixel, außer mit `--crt`)."],
    ["`--snapshot-every N`", "Mit `--snapshot`: zusätzlich alle N Bilder `DATEI-<bild>.png` schreiben."],
    ["`--screen NAME`", "Diesen Bildschirm aufnehmen: `vic` oder `vdc` beim C128, das Terminal "
     "beim IMSAI (Standard: der Hauptbildschirm)."],
    ["`--crt PROFIL[:N]`", "Schnappschuss durch ein Monitorprofil (siehe die Liste des Desktops) "
     "mit N Zeilen je Bildzeile (Standard 2), auf der CPU gerechnet."],
    ["`--wav DATEI.wav`", "Den Ton des Laufs als 16-Bit-PCM mit 48 kHz schreiben."],
    ["`--load-state DATEI`", "Vor dem Lauf einen Savestate laden."],
    ["`--save-state DATEI`", "Nach dem Lauf einen Savestate schreiben."],
    ["`--run`", "Mit `--debug`: laufend statt angehalten starten."],
    ["`--no-realtime`", "Mit `--debug`: so schnell wie möglich statt mit der Geschwindigkeit "
     "der Maschine laufen."],
    ["`--list`", "Die Maschinen mit ihren Modellen auflisten und beenden."],
], [0.26, 0.74])
h2("Echte Hardware (Feature `real-hardware`)")
p("""Mit `cargo build --release -p emu-headless --features real-hardware`
(oder `greaseweazle` / `opencbm` allein) gebaut, spricht das Programm auch
mit einem Greaseweazle-Flussleser und über OpenCBM (`opencbm.dll` im PATH
oder `MIMIC_OPENCBM`) mit einem ZoomFloppy samt echtem
Commodore-Laufwerk.""")
table(["Option", "Bedeutung"], [
    ["`--import-flux DATEI`", "Eine Diskette über den Greaseweazle in ein Abbild lesen: D64, "
     "G64, D71, D81, ADF oder IMG nach Endung."],
    ["`--gw-port PORT`", "Der serielle Anschluss des Greaseweazle (z. B. `COM5`)."],
    ["`--gw-format F`", "`1541`, `1571`, `1581`, `amiga`, `amiga-hd`, `ibm` …"],
    ["`--gw-drive a|b`, `--gw-tracks N`, `--gw-step auto|1|2`, `--gw-revs N`",
     "Laufwerkswahl, Spurzahl, Schrittweite und Umdrehungen je Spur."],
    ["`--import-drive DATEI.d64`", "Eine Diskette aus einem echten Laufwerk am ZoomFloppy in "
     "ein Abbild kopieren; `--export-drive DATEI.d64` schreibt eines zurück."],
    ["`--cbm-device N`, `--cbm-type 1541|1571|1581`", "Gerätenummer und Typ des echten "
     "Laufwerks."],
    ["`--real-drive N`", "Das echte Laufwerk als Diskette des emulierten Laufwerks benutzen "
     "(`--real-drive-write` erlaubt Schreibzugriffe)."],
], [0.36, 0.64])

# ---- 15 Maschinenoptionen ----------------------------------------------------------

h1("Maschinenoptionen (--option KEY=VALUE)")
p("""Optionen konfigurieren eine Maschine über ihr Modell hinaus: Speicher,
Geräte an ihren Anschlüssen, Laufwerke und Erweiterungen. Sie sind in jeder
Oberfläche gleich. Unbekannte Schlüssel und Werte werden mit einer Meldung
abgelehnt, die die Auswahl nennt.""")
h2("Optionen mehrerer Maschinen")
table(["Schlüssel", "Maschinen", "Werte"], [
    ["`roms`", "alle", "`stub`: die eingebauten Test-ROMs"],
    ["`port1`, `port2`", "C64, C128, VIC-20 (nur `port1`), C16/Plus/4",
     "`joystick`, `paddles`, `mouse1351`, `lightpen` (Port 1 von C64/C128), die "
     "Proportionalmäuse `amigamouse`, `stmouse`, `cx22`, die Tastenfelder `rushware`, "
     "`coplin`, `cx85`, `cardkey`, `cx21`, oder `none`. Standard: Port 2 Joystick, Port 1 "
     "none (VIC-20 und Plus/4: Port 1 Joystick)."],
    ["`port1` … `port4`", "Atari 8-Bit", "`joystick`, `paddles`, `lightpen`, `none` "
     "(Ports 3 und 4 nur beim 400/800)"],
    ["`port1`, `port2`", "Atari 2600", "`joystick`, `paddles`, `driving`, `keypad`, `none`"],
    ["`port1`, `port2`", "ColecoVision", "`controller`, `superaction`, `driving`, "
     "`roller`, `none`"],
    ["`port1`, `port2`", "NES", "`controller`, `zapper`, `arkanoid`, `none`; "
     "`fourscore=on` ergänzt Ports 3 und 4"],
    ["`port1`, `port2`", "SNES", "`pad`, `mouse`, `superscope`, `none`; `multitap=on`"],
    ["`port0`, `port1`", "Amiga", "`mouse`, `joystick`, `lightpen`, `none` (Standard "
     "Maus in Port 0, Joystick in Port 1)"],
    ["`port`", "Apple II", "`joystick`, `paddles`, `none`"],
    ["`port1`, `port2`", "Vectrex", "`controller`, `lightpen`, `imager`, `none`"],
    ["`joy1`, `joy2`", "CPC", "`joystick`, `amx` (Maus an joy1), `none`; `joy2` hängt "
     "einen zweiten Stick in Reihe"],
    ["`expansion`", "CPC", "`lightpen`"],
    ["`printer`", "C64, C128, VIC-20, C16/Plus/4, PET, CPC, Amiga, Atari 8-Bit, Mac",
     "`on`, oder die Gerätenummer `4`/`5` am seriellen Commodore-Bus; "
     "`printermodel=fx80|p6|imagewriter`"],
], [0.14, 0.26, 0.6])
h2("Commodore 8-Bit")
table(["Schlüssel", "Maschine", "Werte"], [
    ["`drive8`", "C64, C128, VIC-20, Plus/4", "`1541` (echtes Laufwerk mit DOS-ROM; beim "
     "C128 `1571`/`1581`; beim C16/Plus/4 auch `1551` am TCBM-Bus), `virtual` "
     "(Dateizugriff auf KERNAL-Ebene, Standard ohne DOS-ROM), `none`"],
    ["`drive8_ppm`", "C64", "Abweichung des Laufwerksquarzes in ppm"],
    ["`fastload`", "C64", "`on`: das virtuelle Laufwerk bedient die seriellen KERNAL-Routinen "
     "ohne Buszeit"],
    ["`debugcart`", "C64", "`on`: das Debug-Modul der Testumgebung bei `$D7FF`"],
    ["`cia`", "C64", "`6526` (alt), `6526a` (neu)"],
    ["`sid`", "C64", "`6581`, `8580`"],
    ["`sidcard`", "C16/Plus/4", "`6581` (auch `on`), `8580`: eine SID-Karte bei `$FD40` "
     "(Solders Karte, mit dem Einzeltakt des Plus/4 getaktet); Standard `off`"],
    ["`vicii`", "C64", "`6569`, `8565` (PAL), `6567`, `8562` (NTSC)"],
    ["`reu`", "C64", "`128k`, `256k`, `512k`, `1m` … `16m`: eine REU (REC 8726)"],
    ["`uci`", "C64", "`on`: das Ultimate-64-Command-Interface bei `$DF1C`–`$DF1F` "
     "(DOS, Netz, Steuerung, Software-IEC, HTTP); siehe `--uci-dir`, `--uci-mount`"],
    ["`userport`", "C64", "`wic64`: der WiC64 (HTTP, TCP, Uhrzeit) am User-Port"],
    ["`ram`", "VIC-20", "`none`, `3k`, `8k`, `16k`, `24k`, `32k`, `35k`"],
    ["`hz`", "PET", "`50`, `60`"],
    ["`display`", "C128", "`40`, `80`: der Bildschirm, in dem die Maschine startet"],
], [0.14, 0.22, 0.64])
h2("Atari")
table(["Schlüssel", "Maschine", "Werte"], [
    ["`basic`", "Atari 8-Bit", "`off`: XL/XE-Selbsttest, 400/800 ohne BASIC-Modul"],
    ["`ram`", "Atari 8-Bit", "400/800: `8k`, `16k`, `32k`, `48k`; XL/XE: `64k`, `128k` "
     "(130XE-Bänke), `320k` (RAMBO), `320k-compy` (Compy Shop)"],
    ["`drives`", "Atari 8-Bit", "`0` … `4` SIO-Laufwerke; `drive=810|1050|xf551`"],
    ["`bank`", "Atari 2600", "die Bankumschaltung des Moduls, wenn die Erkennung versagt: "
     "`2k`, `4k`, `f8`, `f6`, `f4`, `fe`, `e0`, `e7`, `3f`, `fa`, `dpc` …"],
], [0.14, 0.22, 0.64])
h2("Amiga")
table(["Schlüssel", "Werte"], [
    ["`kickstart`", "`1.2`, `1.3`, `2.0`, `3.0`, `3.1` (der A1200 nimmt sein eigenes "
     "3.1-Abbild, wenn vorhanden)"],
    ["`chip`", "`256k`, `512k`, `1m`, `2m`"],
    ["`slow`", "`512k`, `0`"],
    ["`fast`", "`0`, `512k` … `8m` (Zorro-II-Autoconfig)"],
    ["`chipset`, `agnus`, `denise`", "`ocs`, `ecs`, `aga` (chipset); `ocs`, `ecs` "
     "für die Chips einzeln"],
    ["`drives`, `drivetype`", "`1` … `4` Diskettenlaufwerke; `dd`, `hd`"],
    ["`serial`", "`none`, `loopback`"],
    ["`wcs`", "A1000: `rom`, `disk` (Kickstart von einer Bootstrap-Diskette)"],
], [0.26, 0.74])
h2("Weitere Maschinen")
table(["Schlüssel", "Maschine", "Werte"], [
    ["`crtc`", "CPC", "`0`, `1`, `2`: der CRTC-Typ"],
    ["`dos`", "CPC", "`amsdos`, `parados`, `none`"],
    ["`ram`, `lc`, `aux`", "Apple II", "`4k` … `48k`; Language Card `on|off` (II/II+); "
     "Zusatzspeicher `64k|1k|none` (IIe)"],
    ["`slot4`, `slot6`", "Apple II", "`mockingboard|none`; `disk2|none`"],
    ["`ram`", "Mac", "`512k`, `1m`, `2m`, `4m` (Plus und SE)"],
    ["`serial`", "Mac", "`loopback`"],
    ["`os`, `ram`", "Archimedes", "`3.11`, `3.10`, `3.00`, `2.00`, `1.20`; `512k`, "
     "`1m`, `2m`, `4m`"],
    ["`traps`", "Spectrum, ZX81", "`off`: keine Band-Traps (nur Laden in Echtzeit)"],
    ["`joystick`, `mouse`", "Spectrum", "`kempston`, `sinclair1`, `sinclair2`, "
     "`cursor`, `none`; `mouse=kempston`"],
    ["`issue`, `multiface`, `if1`, `microdrives`, `beta`", "Spectrum",
     "`issue=2`; `multiface=on`; `if1=on` mit `microdrives=1..8`; `beta=on` (TR-DOS)"],
    ["`ram`", "ZX81", "`1k`, `2k`, `16k`"],
    ["`ram`, `baud`, `reader`, `uppercase`, `mpu`, `load`", "IMSAI",
     "`1k` … `64k`; `9600`; `card|console`; `off`; `a|b` (Monitor-ROM); `HEX`"],
    ["`port1`, `port2`", "Vectrex", "`controller`, `lightpen`, `imager`; `eye`, "
     "`lines=fine|soft`"],
    ["`card`, `rtc`, `port0`, `port1`, `net`, `mac`", "Logophor 16", "`card=system` steckt die "
     "eingebaute Systemkarte ein (Boiler und der Desktop Panorama; das Profil `logophor16` setzt das, "
     "Schreibzugriffe gehen am Ende verloren; `--disk` tritt an ihre Stelle), `none` (Vorgabe); `rtc`: Sekunden seit 2000-01-01 "
     "(Start der Uhr; Vorgabe 2026-01-01); `mouse|joystick|none` (Vorgabe: Maus an "
     "Port 0, Joystick an Port 1); `net=host` baut das Gateway des Hubs in die Maschine ein (10.16.0.1: DHCP, DNS und das Internet über das Netz des Host-Rechners, ohne Hub; das Profil `logophor16` setzt das), `net=HOST:PORT` verbindet mit dem Netz eines "
     "`ada-asm hub` (zugleich Gateway des Netzes; `--no-gateway` nur Verteiler) (Vorgabe Port 6516); `mac=02:4c:31:36:00:01`. Die SD-Karte ist ein FAT32-Image per `--disk` "
     "(`ada-asm sd card.img --boot prog.bin`); Foundry startet davon "
     "`BOOT/BOILER.BIN`, eine Taste während des Startbilds öffnet den Monitor"],
], [0.26, 0.18, 0.56])
h2("Tastenfelder und Proportionalmäuse an den Commodore-Ports")
p("""Neben Joystick, Paddles und der Maus 1351 nehmen die
Commodore-Steuerports Geräte, die Software mit eigenem Treiber als
Joystick-Leitungen liest:""")
bullets(
    "`amigamouse`, `stmouse`, `cx22`: die Amiga-Maus, die Atari-ST-Maus (auch der "
    "spätere Trackball CX80) und der Atari-Trackball CX22. Die eingefangene Maus des "
    "Rechners treibt sie; ein Klick auf das Bild fängt sie ein.",
    "`rushware`, `coplin`, `cx85`, `cardkey`: Zifferntastenfelder. Der Ziffernblock des "
    "Rechners bedient sie (Ziffern, `.`, Enter, `+`, `-`, `*`, `/`), dazu Escape, N, Y, "
    "Entf, P und R für die Tasten, die manche davon haben. Solange ein Tastenfeld "
    "steckt, wandert der Tastatur-Joystick auf die Cursortasten.",
    "`cx21`: das Atari-Tastenfeld (1 2 3 / 4 5 6 / 7 8 9 / * 0 #) auf dem Ziffernblock, "
    "`*` auf Multiplizieren und `#` auf Dividieren; es braucht einen Port mit "
    "POT-Leitungen.",
)

# ---- 16 Profile -----------------------------------------------------------------------

h1("Startprofile (profiles.yaml)")
p("""Ein Startprofil ist eine benannte Maschinenzusammenstellung: die
Maschine, ihr Modell und Videoformat, die ROMs, die Maschinenoptionen, die
einzulegenden Medien, Monitor- und Fenstereinstellungen, Eingabeeinstellungen
und die Server. Statt jedes Mal eine lange Kommandozeile zu tippen, starten
Sie das Profil mit seinem Namen, wählen es aus einer Liste im
Desktop-Fenster oder auf der Web-Seite.""")
h2("Wo die Datei liegt")
bullets(
    "`profiles.yaml` im Arbeitsordner (dem Ordner, aus dem Sie mimic starten), oder",
    "im Konfigurationsordner des Benutzers: `%APPDATA%/mimic/profiles.yaml` unter "
    "Windows, `~/.config/mimic/profiles.yaml` unter Linux und macOS, oder",
    "jede mit `--profiles DATEI` genannte Datei.",
    "`docs/profiles.example.yaml` im Repository ist ein kommentierter Ausgangspunkt; "
    "kopieren Sie es an einen der Orte oben.",
)
h2("Ein Profil starten")
table(["Wo", "Wie"], [
    ["Kommandozeile", "`mimic-desktop --profile NAME` oder `mimic-headless --profile NAME`. "
     "Jede Option auf derselben Kommandozeile überschreibt den Wert des Profils; "
     "`--option`- und `--rom`-Werte werden je Schlüssel zusammengeführt, eine "
     "`--disk`-Liste ersetzt die Disketten des Profils."],
    ["Desktop-Fenster", "`mimic-desktop` ohne `--machine` und `--profile` starten: wird eine "
     "Profildatei gefunden, zeigt das Fenster ihren Namen und listet die Profile mit "
     "ihren Beschreibungen. Ein Klick startet eines. Kann ein Profil nicht starten "
     "(ROM fehlt, falsche Option), steht der Grund unter der Liste, und Sie können ein "
     "anderes wählen. Ohne Profildatei startet wie bisher der C64."],
    ["Web-Seite", "**Profiles…** liest eine `profiles.yaml` von Ihrer Platte; die Profile "
     "erscheinen dann in der Liste **start profile…** daneben, und die Wahl eines "
     "Eintrags startet es. Medien- und ROM-Pfade im Profil werden über den Dateinamen "
     "den auf die Seite hochgeladenen Dateien zugeordnet, also zuerst die Abbilder (und "
     "den ROM-Ordner) hochladen."],
    ["Auflisten", "`--list-profiles` gibt jedes Profil mit Beschreibung aus und beendet."],
    ["Speichern", "`--save-profile NAME` speichert die Zusammenstellung der aktuellen "
     "Kommandozeile als Profil in der benutzten Datei (angelegt, falls sie fehlt) und "
     "startet dann."],
], [0.18, 0.82])
h2("Die Datei")
p("""Zwei Schlüssel auf oberster Ebene: `settings`, gemeinsam für alle
Profile, und `profiles`, ein Eintrag je Name. Relative Pfade zählen vom
Ordner der Profildatei; Medienpfade von `settings.media`, wenn es gesetzt
ist.""")
code("""settings:
  roms: roms                 # ROM-Ordner, wenn ein Profil keinen nennt (dann $EMU_ROM_DIR)
  snapshots: snapshots       # wohin der Schnappschuss-Knopf des Desktops speichert
  media: disks               # Basisordner für relative Medienpfade

profiles:
  mac-se-755:
    description: Mac SE 4 MB mit System 7.5.5 auf einer SCSI-Platte
    machine: mac
    model: se
    options: { ram: 4m }
    media:
      disks: [system755.hda]   # jedes geht ins nächste Laufwerk, das es nimmt
    display: { crt: mac, scale: 2, footer: true, panel: false }

  c64-game:
    description: C64 NTSC mit Maus 1351 und einer Diskette, die sich selbst startet
    machine: c64
    videoformat: ntsc
    roms: c64roms            # der eigene ROM-Ordner dieses Profils
    rom: { kernal: kernal.901227-03.bin }   # oder einzelne Dateien nach Rolle
    options: { port1: mouse1351, drive8: "1541" }
    media: { disks: [game.d64], autostart: true }   # auch tape:, cart:, prg:
    input: { joykeys: cursor, joyport: 2, speed: 100 }

  amiga-dev:
    description: Amiga 500 mit 1 MB Chip-RAM und dem Debugger
    machine: amiga
    options: { chip: 1m, drives: 2 }
    media: { disks: [workbench.adf, extras.adf] }
    display: { deinterlace: weave }
    debug: { server: true, preview: "127.0.0.1:6580", vice: false }""")
table(["Schlüssel", "Bedeutung", "Entsprechung auf der Kommandozeile"], [
    ["`description`", "Eine Zeile, die die Auswahl und `--list-profiles` zeigen.", "–"],
    ["`machine`, `model`, `videoformat`", "Was laufen soll.", "`--machine`, `--model`, "
     "`--videoformat`"],
    ["`roms`", "Der ROM-Ordner dieses Profils.", "`--rom-dir`"],
    ["`rom`", "Einzelne ROM-Dateien nach Rolle (`kernal`, `basic`, `kick13` …).", "`--rom ROLLE=DATEI`"],
    ["`options`", "Maschinenoptionen, ein Schlüssel je Zeile oder in Klammern.", "`--option KEY=VALUE`"],
    ["`media.disks`", "Diskettenabbilder der Reihe nach; jedes geht ins nächste Laufwerk, das es nimmt.",
     "`--disk` (wiederholt)"],
    ["`media.tape`, `media.cart`, `media.prg`", "Ein Band, Modul oder Programm.",
     "`--tape`, `--cart`, `--prg`"],
    ["`media.autostart`", "Nach dem Booten laden und starten.", "`--autostart`"],
    ["`display.crt`", "Monitor-Voreinstellung oder `.toml`-Datei.", "`--crt`"],
    ["`display.scale`", "Fensterzeilen je emulierter Zeile.", "`--scale`"],
    ["`display.footer`, `display.panel`", "Statusfußzeile sichtbar; Konsolenpanel offen.",
     "`--footer`, `--no-panel`"],
    ["`display.deinterlace`", "`weave` oder `bob`.", "`--deinterlace`"],
    ["`input.joykeys`", "`cursor`, `numpad` oder `off`.", "`--joykeys`"],
    ["`input.joyport`", "Anschluss, den Tastatur-Joystick und Gamepad spielen.", "`--joyport`"],
    ["`input.speed`", "Prozent der Echtzeit.", "`--speed`"],
    ["`debug.server`", "`true` (127.0.0.1:6510) oder eine Adresse.", "`--debug [ADR]`"],
    ["`debug.preview`", "`true` (127.0.0.1:6580) oder eine Adresse.", "`--preview [ADR]`"],
    ["`debug.vice`", "`true` (127.0.0.1:6502) oder eine Adresse.", "`--vice [ADR]`"],
    ["`settings.roms`", "ROM-Ordner für Profile ohne `roms`.", "`$EMU_ROM_DIR`"],
    ["`settings.snapshots`", "Ordner für den Schnappschuss-Knopf der Fußzeile.", "–"],
    ["`settings.media`", "Basisordner für relative Medienpfade.", "–"],
], [0.3, 0.44, 0.26])
h2("Vorrang")
p("""Der ROM-Ordner kommt von der Kommandozeile, dann aus `roms` des
Profils, dann aus `settings.roms`, dann aus `EMU_ROM_DIR`, dann
`~/.mimic/roms`. Alles andere: ein
Profil wird in eine Kommandozeile verwandelt, und die getippten Optionen
werden darübergelegt, so dass ein getippter Wert den des Profils ersetzt;
Optionen und ROM-Rollen werden je Schlüssel zusammengeführt; eine getippte
Diskettenliste ersetzt die Disketten des Profils. Das Headless-Programm
nimmt dieselbe Datei und ignoriert die Schlüssel `display` und `input`, die
es nicht braucht.""")

# ---- 17 Web -------------------------------------------------------------------------------

h1("Die Web-Oberfläche")
p("""Die Web-Oberfläche ist der zu WebAssembly übersetzte Emulator mit einer
Seite darum. Nichts ist mitgeliefert: ROMs, Disketten, Kassetten und Module
werden im Browser hochgeladen und können dort behalten werden (IndexedDB),
nur in diesem Browser. Jede Maschine läuft; Druckerausgabe, Netz und die
Echte-Hardware-Backends des Desktops gibt es im Web nicht.""")
h2("Bauen und ausliefern")
code("""bash tools/build-web.sh        # -> target/web: mimic.wasm, index.html, mimic.js, mimic-worker.js
python -m http.server -b 127.0.0.1 -d target/web 8080
# dann http://127.0.0.1:8080/ öffnen
node tools/web-smoke.mjs       # jede Maschine startet im wasm-Modul (ohne Browser)""")
note("""Die Seite muss von einem HTTP-Server kommen, nicht von einer
`file:`-URL: der Browser weigert sich, das WebAssembly-Modul und den Worker
aus lokalen Dateien zu laden. Jeder statische Server genügt; auf der
Serverseite läuft nichts.""", "Hinweis")
h2("Die Seite")
table(["Bereich", "Was er tut"], [
    ["Machine", "Maschine, Modell und Videoformat; ein Optionsfeld für `KEY=VALUE`-Wörter; "
     "das Kästchen **built-in test ROMs** (`roms=stub`); **Start** und **Stop**. "
     "Darunter die ROMs, die die Maschine braucht, und die optionalen (ein DOS-ROM "
     "des Laufwerks macht aus dem virtuellen Laufwerk eine echte 1541)."],
    ["Files", "**Add files…** lädt einzelne Dateien hoch, **Add ROM folder…** einen ganzen "
     "Ordner (wie `--rom-dir`: Unterordner je Maschine, Namen und Größen wie bei der "
     "ROM-Suche). Auf die Seite gezogene Dateien werden hinzugefügt; auf das Bild "
     "gezogene kommen ins passende Laufwerk. **keep files in this browser** legt sie in "
     "IndexedDB ab, so dass sie beim nächsten Besuch da sind."],
    ["Drives and slots", "Eine Zeile je Laufwerk, Datasette und Modulschacht mit dem "
     "eingelegten Abbild und einem Knopf **Mount** / **Eject** (Mount öffnet einen "
     "Dateidialog)."],
    ["Cheats", "Die geladenen Freezes als Kästchen; **Import** (POKE-Liste, "
     "Adresse/Wert-Codes, VICE-Monitor-Befehle, TOML), Export, und die Speichersuche "
     "mit einem **Freeze**-Knopf je Treffer. `cheats/<maschine>/<crc32>.toml` in einem "
     "hochgeladenen Ordner gilt für das passende Abbild."],
    ["State", "**Save state** lädt eine `.mimicstate`-Datei herunter, **Load state…** "
     "stellt eine wieder her; **Profiles…** liest eine `profiles.yaml` und listet ihre "
     "Profile."],
    ["Kopf", "**Pause**, **Reset** (Umschalt+Klick: Aus- und Einschalten), das Kästchen "
     "**Sound**, die Wahl **Monitor** (rohe Pixel, der Monitor der Maschine, lite) und "
     "**Scale** (1–4)."],
    ["Konsolenpanel", "Unter dem Bild: die Knöpfe und Schalter der Maschine, wie im Desktop."],
    ["Statuszeile", "Was läuft und mit wie vielen Bildern je Sekunde, oder der letzte Fehler."],
], [0.2, 0.8])
h2("Eingabe")
bullets(
    "Klick auf das Bild: Tasten gehen nach Lage an die Maschine, wie im Desktop. Tasten, "
    "die der Browser selbst nähme (Blättern, Suchen), bleiben bei der Maschine, solange "
    "das Bild den Fokus hat.",
    "Gamepads mit der Standardbelegung spielen den Joystick (Steuerkreuz oder linker "
    "Stick, Knöpfe nach der Belegung der Maschine).",
    "Eine Maus oder Paddles in einem Anschluss fangen den Zeiger bei einem Klick ein "
    "(Esc gibt ihn frei); Lichtgriffel, Zapper oder Super Scope zielen mit dem Zeiger "
    "über dem Bild.",
    "Tastenfelder in einem Commodore-Port nehmen den Ziffernblock; die Joystick-Tasten "
    "wandern auf die Cursortasten.",
)
h2("URL-Parameter")
p("""Die Seite startet sofort eine Maschine, wenn die URL eine nennt:""")
code("""http://127.0.0.1:8080/?machine=c64&options=roms=stub&start=1
http://127.0.0.1:8080/?machine=atari8&model=130xe&videoformat=ntsc&options=ram=320k,basic=off&crt=default&start=1""")
table(["Parameter", "Bedeutung"], [
    ["`machine=ID`", "Die Maschinen-ID (nötig für einen automatischen Start)."],
    ["`model=NAME`", "Das Modell."],
    ["`videoformat=pal|ntsc`", "Das Videoformat."],
    ["`options=KEY=VALUE,...`", "Maschinenoptionen, durch Kommas getrennt."],
    ["`crt=raw|default|lite`", "Der Monitor: scharfe Pixel, das eigene Monitorprofil der "
     "Maschine oder die Voreinstellung lite."],
    ["`start=1`", "Ohne Knopfdruck starten (die ROMs müssen schon im Speicher des Browsers "
     "liegen, oder `options=roms=stub`)."],
], [0.3, 0.7])
h2("Wie die Seite läuft")
bullets(
    "Die Emulation läuft in einem **Web Worker** (`mimic-worker.js`), dem das "
    "WebAssembly-Modul gehört, der die Bilder im Takt der Maschine erzeugt und die "
    "Befehle der Seite beantwortet; die Seite bleibt bedienbar und zeichnet nur die "
    "Bilder, die sie bekommt.",
    "Der Ton geht vom Worker direkt an ein **AudioWorklet** über einen Port, den die "
    "Seite übergibt; das Kästchen Sound schaltet ihn stumm. Browser starten Ton erst "
    "nach einem Klick auf die Seite.",
    "Das Bild wird mit **WebGL2** gezeichnet: rohe Pixel durch einen Durchreich-Shader, "
    "ein Monitor durch den CRT-Shader (dasselbe Modell wie im Desktop und in der "
    "Vorschau). Die Composite-Signalstufe (PAL-Farbmischung, NTSC-Artefakte) läuft im "
    "WebAssembly-Modul. Ohne WebGL2 rendert das Modul auf der CPU, und die Seite zeigt "
    "es auf einer 2D-Zeichenfläche.",
    "Ein Savestate ist der vollständige Zustand der Maschine; er lädt nur in dieselbe "
    "Maschine mit denselben ROMs und Medien.",
)
h2("Die Live-Vorschau (etwas anderes)")
p("""Der Vorschau-Server von `mimic-desktop --preview` und `mimic-headless
--preview` ist nicht die Web-Oberfläche: dort läuft der Emulator nativ, und
der Browser zeigt nur sein Bild, leitet Tastatur, Gamepad und Maus weiter,
bedient das Konsolenpanel, blättert durch die Grafikansichten und
bearbeitet CRT-Profile. Öffnen Sie `http://127.0.0.1:6580/` für seinen
Betrachter; Kapitel 8 beschreibt seine Adressen.""")
