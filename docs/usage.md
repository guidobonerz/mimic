# mimic – Bauen, Benutzen, Debuggen

mimic ist ein zyklengenauer Emulator für mehrere Heimcomputer auf einem
gemeinsamen Kern. Jede Maschine bekommt automatisch denselben Remote-Debugger
und einen Live-Bildstream.

Inhalt:

1. [Voraussetzungen](#1-voraussetzungen)
2. [Bauen](#2-bauen)
3. [ROMs](#3-roms)
4. [Maschinen, Modelle und Optionen](#4-maschinen-modelle-und-optionen)
5. [Desktop-Frontend `emu-desktop`](#5-desktop-frontend-emu-desktop)
6. [Kommandozeilen-Frontend `emu-headless`](#6-kommandozeilen-frontend-emu-headless)
7. [Medien: Disketten, Bänder, Module, Programme](#7-medien-disketten-bänder-module-programme)
   - [7a. Echte Hardware: Greaseweazle und ZoomFloppy](#7a-echte-hardware-greaseweazle-und-zoomfloppy)
8. [Monitorprofile (CRT)](#8-monitorprofile-crt)
9. [Live-Preview im Browser](#9-live-preview-im-browser)
10. [Debug-Schnittstelle](#10-debug-schnittstelle)
11. [Python-Client `polydbg`](#11-python-client-polydbg)
12. [VICE-Binary-Monitor (VS64 & Co.)](#12-vice-binary-monitor-vs64--co)
13. [Tests und Testdaten](#13-tests-und-testdaten)

---

## 1. Voraussetzungen

- **Rust** (stable, Edition 2021) mit `cargo`.
- **Linux:** für das Desktop-Frontend die Pakete `libasound2-dev` und
  `libudev-dev`.
- **Optional:** Python 3 für den Debug-Client `tools/polydbg.py` (nur
  Standardbibliothek), `bash` und `curl` für das Testdaten-Skript, 7z/bsdtar
  für Acid800, ein C++-Compiler für die CPC-Testsuite (baut den Assembler
  pasmo).
- **WASM-Build:** das Target `wasm32-unknown-unknown`
  (`rustup target add wasm32-unknown-unknown`).

## 2. Bauen

```bash
cargo build --workspace                 # alles (Debug)
cargo build --release -p emu-desktop    # Desktop-Emulator, optimiert
cargo build --release -p emu-headless   # Kommandozeilen-Frontend
cargo build -p emu-web --target wasm32-unknown-unknown
```

Für flüssiges Arbeiten immer `--release` verwenden; der Debug-Build ist
deutlich langsamer als Echtzeit.

Die fertigen Programme liegen danach in `target/release/` (unter Windows mit
`.exe`):

| Programm | Crate | Zweck |
| --- | --- | --- |
| `mimic-desktop` | `emu-desktop` | Emulator mit Fenster, Ton, Tastatur und Gamepad; optional Debug-Server |
| `mimic-headless` | `emu-headless` | ohne Fenster: Snapshots, Skripte, CI, Debug-Server |

Sie lassen sich direkt starten oder an einen beliebigen Ort kopieren (keine
weiteren Dateien nötig, ROMs über `--rom-dir` oder `EMU_ROM_DIR`):

```powershell
.\target\release\mimic-desktop.exe --machine c64 --rom-dir roms --disk spiel.d64 --autostart
.\target\release\mimic-headless.exe --machine c64 --rom-dir roms --debug
```

`cargo run --release -p emu-desktop -- …` bzw. `-p emu-headless` startet
dieselben Programme; die Beispiele unten nutzen diese Form.

Prüfungen vor einem Commit:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python tools/check-deps.py              # Abhängigkeitsrichtung der Crates
```

## 3. ROMs

ROMs sind **nicht** im Repository und werden auch nie eingecheckt. Lege sie
in einen Ordner und gib ihn mit `--rom-dir DIR` an oder setze
`EMU_ROM_DIR`. Gesucht wird zuerst im Unterordner der Maschine, dann in
`drives/`, dann im Ordner selbst. Dateien werden über den **Namensanfang**
(ohne Groß-/Kleinschreibung, `.` und `_` zählen als `-`) und die **exakte
Größe** erkannt.

| Unterordner | Maschine | Rollen (Beispielnamen) |
| --- | --- | --- |
| `c64/` | C64 | `kernal` (8 KB), `basic` (8 KB), `chargen`/`characters` (4 KB) |
| `vic20/` | VIC-20 | KERNAL, BASIC, Zeichensatz |
| `pet/` | PET | BASIC, Editor, KERNAL, Zeichensatz je Modell |
| `plus4/` (auch für `c16`) | C16/Plus/4 | KERNAL, BASIC, optional 3-plus-1 |
| `c128/` | C128 | KERNAL, BASIC, Zeichensatz, optional Funktions-ROM |
| `atari/` (`atari8/`, `atari800xl/`) | Atari 400/800/XL/XE | XL-OS (`atarixl.rom`, 16 KB), BASIC (`ataribas.rom`, 8 KB), für 400/800 das OS-A/OS-B (`atariosb.rom`, 10 KB) |
| `cpc/` (`amstrad/`) | Amstrad CPC | `os464`/`basic1-0`, `os664`/`basic664`, `os6128`/`basic1-1` (je 16 KB) oder 32-KB-Kombidateien `cpc464`/`cpc664`/`cpc6128`; `amsdos`, optional `parados` |
| `amiga/` | Amiga 500/1000/2000/500+/1200 | Kickstart als 256 KB oder 512 KB, Name z. B. `Kickstart 1.3.rom`, `kick13.rom`, `kick34005.rom`; je Version eine Datei (1.2, 1.3, 2.0, 3.0, 3.1), Auswahl mit `--option kickstart=`; für den A1200 passt das 3.1-Image 40.063 (A500/600/2000) oder ein A1200-Image (3.0 39.106, 3.1 40.068), nicht das 3.0 des A4000 (39.106 mit CRC 9E6AC152: sucht 32-Bit-RAM bei $08000000); optional das A1000-Bootstrap-ROM (8 KB, `a1000boot.rom`) für `--option wcs=disk` |
| `zx81/` (`sinclair/`) | Sinclair ZX81 | `zx81.rom` (8 KB; `zx81.edition3.rom` wird bevorzugt) |
| `spectrum/` (`zxspectrum/`, `sinclair/`) | ZX Spectrum | `Spec48.rom` (48K, 16 KB), `spec128.rom` (128K, 32 KB: Editor-ROM, dann 48 BASIC), `specp2.rom` (+2, 32 KB); optional `mf128.rom` (Multiface 128, 8 KB) und `specif1.edition2.rom` bzw. `specif1.rom` (Interface 1, 8 KB) |
| `snes/` (`sfc/`) | SNES | optional die 64 Byte IPL-ROM des Sound-Prozessors, `spc700.rom`; ohne sie lädt ein eigener Bootlader mit demselben Protokoll |
| `coleco/` (`colecovision/`) | ColecoVision | das BIOS, `coleco.rom` (8 KB); ohne BIOS läuft mit `--option roms=stub` ein eigenes Mini-BIOS, das Module mit dem Header $55AA/$AA55 direkt startet (Spiele, die BIOS-Routinen aufrufen, brauchen das echte BIOS) |
| `drives/` | Laufwerke | `dos1541`, `dos1571`, `dos1581` |

Der Atari 2600 hat kein System-ROM: das Programm kommt vom Modul (`--cart`).

Fehlt ein ROM, nennt die Fehlermeldung Rolle, erwartete Größe und Namen.
Einzelne ROMs lassen sich im Headless-Frontend auch direkt angeben:
`--rom kernal=pfad/kernal.bin`.

Viele Maschinen haben die Option `roms=stub`: ein winziges eingebautes
Programm statt der echten ROMs, nur für Tests.

## 4. Maschinen, Modelle und Optionen

`cargo run --release -p emu-headless -- --list` zeigt alle Maschinen:

| ID | Maschine | Modelle (`--model`) |
| --- | --- | --- |
| `c64` | Commodore 64 | `pal` (Standard), `ntsc` |
| `vic20` | VIC-20 | `pal`, `ntsc` |
| `pet` | PET | `4032` (Standard), `2001`, `3032`, `8032` |
| `c16`, `plus4` | C16, Plus/4 | `pal`, `ntsc` |
| `c128` | C128 | `pal`, `ntsc` |
| `atari8` | Atari 400/800/800XL/65XE/130XE | `800xl` (Standard), `65xe`, `130xe`, `800`, `400`, jeweils auch mit `-ntsc` (z. B. `130xe-ntsc`) |
| `atari2600` | Atari 2600 (VCS) | `auto` (Standard: NTSC oder PAL nach der Bildlänge des Programms, nach etwa 20 Bildern entschieden), `ntsc`, `pal` (erzwungen) |
| `cpc` | Amstrad CPC | `6128` (Standard), `664`, `464` |
| `amiga` | Commodore Amiga 500/1000/2000 (OCS), 500+ (ECS), 1200 (AGA) | `500` (Standard), `500-ntsc`, `1000`, `1000-ntsc`, `2000`, `2000-ntsc`, `500plus` (ECS-Chipsatz, 1 MB Chip-RAM, Kickstart 2.0), `500plus-ntsc`, `1200` (68EC020 mit 14 MHz, AGA, 2 MB Chip-RAM, Gayle ohne IDE, Kickstart 3.1), `1200-ntsc` |
| `zx81` | Sinclair ZX81 / Timex Sinclair 1000 | `zx81` (Standard, PAL), `ts1000` (NTSC, 60 Hz) |
| `spectrum` | Sinclair ZX Spectrum | `48k` (Standard), `128k`, `plus2` (der Amstrad-+2: 128K-Hardware mit eigenen ROMs) |
| `coleco` | ColecoVision | `ntsc` (Standard, TMS9918A), `pal` (TMS9929A) |
| `nes` | Nintendo Entertainment System / Famicom | `ntsc` (Standard, 2C02), `pal` (2C07) |
| `snes` | Super Nintendo / Super Famicom | `ntsc` (Standard), `pal` |
| `sim6502` | 6502-Referenzmaschine (Tests) | `default` |

Optionen werden mit `--option KEY=WERT` gesetzt (mehrfach möglich):

| Maschine | Optionen |
| --- | --- |
| C64 | `sid=6581\|8580`, `cia=6526\|6526a`, `drive8=1541\|1571\|1581\|virtual\|none`, `drive8_ppm=N`, `port1`/`port2=joystick\|paddles\|mouse1351\|none`, `autostart=on`, `uci=on` (Ultimate Command Interface, Abschnitt 7b), `userport=wic64\|none` (WiC64, Abschnitt 7b), `net=on\|off` |
| VIC-20 | `ram=none\|3k\|8k\|16k\|24k\|all`, `port1=joystick\|none`, `drive8=…`, `autostart=on` |
| PET | `ram=8k\|16k\|32k`, `keyboard=graphics\|business`, `hz=50\|60`, `drive8=virtual\|none` |
| C16/Plus/4 | `ram=16k\|64k`, `drive8=…`, `port1=joystick\|none`, `autostart=on` |
| C128 | `display=40\|80`, `vdc_ram=16k\|64k`, `sid=…`, `drive8=1571\|1541\|1581\|virtual\|none`, `port2=…`, `autostart=on` |
| Atari 8-Bit | `port1`/`port2=joystick\|paddles\|none` (400/800 auch `port3`/`port4`), `basic=on\|off` (XL/XE: OPTION beim Start; 400/800: BASIC-Modul im linken Schacht), `ram=8k\|16k\|32k\|48k` (nur 400/800; Standard 16k bzw. 48k), `drives=0..4`, `drive=1050\|810\|xf551` |
| SNES | `port1`/`port2=pad\|mouse\|superscope\|none` (Super Scope nur in Port 2; zielt mit der Maus, linke Taste = Abzug, rechte = Cursor), `multitap=on` (Multiplayer 5 in Port 2: Ports 2–5 mit Pads), `roms=stub` (eigener Sound-Bootlader statt der IPL-ROM) |
| NES | `port1`/`port2=controller\|zapper\|arkanoid\|none` (Standard `controller`; `zapper` = Lichtpistole, zielt mit der Maus, linke Taste = Abzug; `arkanoid` = Arkanoid-Controller, die Maus dreht den Knopf), `fourscore=on` (Four Score: Ports 3 und 4 mit Controllern, Ports 1 und 2 müssen Controller sein) |
| ColecoVision | `port1`/`port2=controller\|superaction\|driving\|roller\|none` (Standard `controller`: Joystick, zwei Feuerknöpfe, Keypad; `superaction` = Super Action Controller mit vier Knöpfen und Speed Roller; `driving` = Expansion Module #2 mit Lenkrad und Gaspedal; `roller` = Roller Controller (Trackball, belegt mit seinem zweiten Kabel auch Port 2)), `roms=stub` |
| Atari 2600 | `port1`/`port2=joystick\|paddles\|keypad\|driving\|none`, `tv=color\|bw`, `left`/`right=b\|a` (Difficulty), `bank=…` (Bankswitching erzwingen: `2k`, `4k`, `f8`, `f6`, `f4`, `f8sc`, `f6sc`, `f4sc`, `fa`, `fe`, `e0`, `e7`, `3f`, `dpc`) |
| ZX81 | `ram=16k\|1k\|2k` (Standard 16k), `traps=on\|off` (LOAD/SAVE der .P-Datei im Bandschacht über ROM-Traps) |
| ZX Spectrum | `traps=on\|off` (Standard on: `LOAD ""` kopiert Standardblöcke direkt aus TAP/TZX; off spielt das Band in Echtzeit), `autostart=on` (mit Band: tippt `LOAD ""` bzw. wählt im 128K-Menü den Tape Loader), `joystick=kempston\|sinclair1\|sinclair2\|cursor\|none` (Standard kempston), `mouse=kempston\|none`, `issue=3\|2` (Platinen-Ausgabe: EAR-Rückkopplung in Port $FE Bit 6), `multiface=on` (Multiface 128; roter Knopf = Bedienfeld-Taste MULTIFACE), `if1=on` (Interface 1 mit `microdrives=1..8`, Standard 2; Cartridges als MDR), `roms=stub` |
| CPC | `crtc=0\|1\|2` (Standard 464: 0, sonst 1), `dos=amsdos\|parados\|none`, `joy1=joystick\|amx\|none`, `joy2=joystick\|none` |
| Amiga | `kickstart=1.3\|1.2\|2.0\|3.0\|3.1` (Standard 1.3, A500+ 2.0, A1200 3.1), `chip=256k\|512k\|1m\|2m` (Chip-RAM; Standard A500 512k, A1000 256k, A2000 und A500+ 1m; 2m braucht den ECS-Agnus), `chipset=ocs\|ecs\|aga` (Standard je Modell; `agnus=ocs\|ecs` und `denise=ocs\|ecs` wählen die Chips einzeln, z. B. A500 Rev 6A mit ECS-Agnus und OCS-Denise: `agnus=ecs`), `slow=512k\|0` (Trapdoor-Erweiterung, Standard A500 512 KB, sonst 0), `fast=0\|512k\|1m\|2m\|4m\|8m` (Zorro-II-Fast-RAM per Autoconfig, Standard 0), `drives=1..4` (DF0:–DF3:, Standard 1), `drivetype=dd\|hd` (HD-Laufwerke für 1,76-MB-ADFs), `port0=mouse\|joystick\|none` (Standard Maus), `port1=joystick\|mouse\|none` (Standard Joystick), `serial=none\|loopback` (TXD mit RXD verbunden), `wcs=rom\|disk` (A1000: Kickstart vorgeladen oder per Bootstrap-ROM von der Kickstart-Diskette laden) |

`drive8=virtual` bedient Laufwerk 8 auf Protokollebene (schnell, nur
KERNAL-LOAD/SAVE), die anderen Werte emulieren das echte Laufwerk mit eigener
CPU (nötig für Schnelllader und Kopierschutz). Ein Ordner kann als virtuelle
Diskette dienen (`--disk ./ordner`).

## 5. Desktop-Frontend `emu-desktop`

```bash
cargo run --release -p emu-desktop -- --machine c64 --rom-dir roms
cargo run --release -p emu-desktop -- --machine c64 --disk spiel.d64 --autostart
cargo run --release -p emu-desktop -- --machine vic20 --tape spiel.tap
cargo run --release -p emu-desktop -- --machine atari8 --disk spiel.atr
cargo run --release -p emu-desktop -- --machine atari8 --model 130xe --disk spiel.atr
cargo run --release -p emu-desktop -- --machine atari8 --model 800 --option port3=joystick --cart spiel.car
cargo run --release -p emu-desktop -- --machine snes --cart spiel.sfc
cargo run --release -p emu-desktop -- --machine nes --cart spiel.nes
cargo run --release -p emu-desktop -- --machine nes --model pal --cart spiel.nes --option port2=zapper
cargo run --release -p emu-desktop -- --machine coleco --rom-dir roms --cart spiel.col
cargo run --release -p emu-desktop -- --machine coleco --model pal --rom-dir roms --cart spiel.rom --option port1=roller
cargo run --release -p emu-desktop -- --machine atari2600 --cart spiel.a26
cargo run --release -p emu-desktop -- --machine atari2600 --model pal --cart spiel.bin --option port1=paddles
cargo run --release -p emu-desktop -- --machine cpc --disk spiel.dsk
cargo run --release -p emu-desktop -- --machine cpc --model 464 --tape spiel.cdt
cargo run --release -p emu-desktop -- --machine zx81 --rom-dir roms --tape programm.p
cargo run --release -p emu-desktop -- --machine spectrum --rom-dir roms --tape spiel.tzx --autostart
cargo run --release -p emu-desktop -- --machine spectrum --model 128k --rom-dir roms --prg spiel.z80
cargo run --release -p emu-desktop -- --machine spectrum --rom-dir roms --option if1=on --option multiface=on --disk cartridge.mdr
cargo run --release -p emu-desktop -- --machine amiga --rom-dir roms --disk workbench13.adf
cargo run --release -p emu-desktop -- --machine amiga --rom-dir roms --disk demo-a.adf --disk demo-b.adf
cargo run --release -p emu-desktop -- --machine amiga --rom-dir roms --option drives=2 --disk demo-a.adf --disk demo-b.adf
cargo run --release -p emu-desktop -- --machine amiga --model 2000 --rom-dir roms --option fast=8m --disk spiel.dms
cargo run --release -p emu-desktop -- --machine amiga --rom-dir roms --option kickstart=3.1 --option drivetype=hd --disk gross.adf
```

Wichtige Optionen: `--machine`, `--model`, `--prg`, `--disk`, `--tape`,
`--cart`, `--autostart`, `--rom-dir`, `--option KEY=WERT`,
`--crt PROFIL` (Monitor, Standard: der der Maschine), `--scale N`,
`--debug [ADR]`, `--preview [ADR]`, `--vice [ADR]` (siehe unten).
`--help` zeigt alles.

**Tastatur.** Die Tasten liegen nach **Position** auf einer PC-Tastatur
(US-Belegung), nicht nach Aufdruck:

- C64: Esc = RUN/STOP, Tab = CTRL, linke Strg = C=, Einfg = £, Entf = ↑,
  Bild↑ = RESTORE, Pos1 = CLR/HOME, F2/F4/F6/F8 = Shift+F1/F3/F5/F7.
- Atari: `< >` auf `- =`, `- =` auf `[ ]`, `+ *` auf `' \`, rechte Alt =
  Atari-Taste, Pfeile = CONTROL + `- = + *`; F2 START, F3 SELECT, F4 OPTION,
  F5 RESET, F6 HELP, F7/Pause BREAK.
- Atari 2600: F1 SELECT, F2 RESET (beide solange gedrückt), F3 Farbe/Schwarzweiß,
  F4/F5 Difficulty links/rechts A/B (jeweils umschalten). Keypads
  (`port1=keypad`, `port2`): links `1 2 3 / Q W E / A S D / Z X C`, rechts
  `8 9 0 / I O P / K L ; / , . /` (US-Positionen).
- SNES: Steuerkreuz auf den Cursortasten, `Z` = B (auch Leertaste), `X` = A,
  `A` = Y, `S` = X, `Q` = L, `W` = R, rechte Umschalttaste = Select, Enter =
  Start; am Gamepad Süd = B, Ost = A, West = Y, Nord = X, Schultertasten = L/R.
- NES: Steuerkreuz von Controller 1 auf den Cursortasten, Leertaste und `X` =
  A, `Z` = B, rechte Umschalttaste = Select, Enter = Start; am Gamepad
  Süd = A, Ost = B, Select und Start.
- ColecoVision: Keypad von Controller 1 auf der Ziffernreihe (`1`–`9`, `0`,
  `-` = `*`, `=` = `#`, `[` `]` = lila und blauer Knopf des Super Action
  Controllers, `X` = rechter Feuerknopf), Keypad von Controller 2 auf dem
  Ziffernblock (`*` und `/` = `*` und `#`, `+` = rechter Feuerknopf);
  Joystick 1 auf den Cursortasten, Leertaste = linker Feuerknopf.
- CPC: `- ^` auf `- =`, `@ [` auf `[ ]`, `: ;` auf `; '`, `]` auf `\`,
  Rücktaste = DEL, Entf = CLR, linke Alt = COPY, Ziffernblock und F1–F10 =
  Funktionstasten f0–f9 und f., Ziffernblock-Enter = ENTER.
- Amiga: linke/rechte Windows-Taste = linke/rechte Amiga-Taste, Alt = Alt,
  beide Strg = CTRL, Einfg = HELP, Entf = DEL, Num-Taste und Rollen = `(`
  und `)` des Ziffernblocks, F1–F10 = F1–F10. **Strg + Win + Win** (beide
  Amiga-Tasten) ist der Reset der echten Tastatur.

**Konsolen-Panel:** Unter dem Bild zeigt eine Leiste die Tasten und Schalter
des Gehäuses (POWER, RESET, beim Atari 2600 die Konsolenschalter, beim 800XL
START/SELECT/OPTION, beim C64/VIC-20 RESTORE, beim C128 auch 40/80 und CAPS
LOCK). Eine Taste ist gedrückt, solange die Maustaste gehalten wird; ein Klick
auf eine Schalterstellung stellt den Schalter um. Ganz links steht
**PAUSE**: ein Klick hält die ganze Maschine an, der nächste lässt sie
weiterlaufen; der Knopf leuchtet, solange die Maschine steht (auch wenn ein
Debugger sie hält), die Titelleiste zeigt die Stelle. Für einen
angeschlossenen Debugger ist das dasselbe wie `pause`/`resume`. Strg+P
blendet die Leiste ein/aus, `--no-panel` startet ohne sie. Dieselbe Leiste
hat der Browser-Viewer (ohne PAUSE); der Debugger bedient sie mit
`controls`/`set_control` (`polydbg panel`).

**Disketten wechseln:** `--disk` lässt sich mehrfach angeben. Die
Disketten kommen der Reihe nach in die Laufwerke (Amiga: DF0:, DF1: … mit
`--option drives=N`); die übrigen bilden eine Wechselliste für das erste
Laufwerk. **Strg+D** legt die nächste Diskette dieser Liste ein, und eine
Image-Datei, die man auf das Fenster zieht, landet ebenfalls im ersten
Laufwerk. Wie von Hand bleibt das Laufwerk dabei etwa zwei Sekunden leer,
bevor die neue Diskette hineinkommt: Programme, die das Laufwerk nur ab und
zu abfragen (Desert Dream etwa einmal pro Sekunde), bemerken den Wechsel
nur, wenn sie es einmal leer vorfinden. Die Titelleiste zeigt, welche
Diskette gerade drin ist; gibt es keine weitere Diskette in der Liste, sagt
sie das. Beispiel
für eine Demo mit zwei Disketten und einem Laufwerk:
`--disk demo-a.adf --disk demo-b.adf`, dann Strg+D, wenn die Demo nach
Disk B fragt. Im Debugger geht dasselbe mit `insert_media`.

**Statuszeile:** Strg+Leertaste blendet unter dem Bild eine Statuszeile
ein (`--footer` startet mit ihr). Links sitzt ein **Geschwindigkeitsregler**:
ein Klick auf eine Stufe oder Ziehen des Knopfs stellt 10 %, 25 %, 50 %,
100 %, 150 %, 200 %, 400 % oder 800 % der Echtzeit ein (ist der Rechner zu
langsam für eine Stufe, läuft die Emulation so schnell sie kann); außerhalb von 100 %
ist der Ton stumm. Dahinter folgen die emulierte Zeit seit dem Einschalten
(`m:ss.t`) mit der Bildnummer (dieselbe, die der Debugger in `beam` meldet),
die Mausposition, die Joystick-Richtungen und der Zustand der Laufwerke.

**Weitere Tasten:** F12 wechselt zwischen den Bildschirmen einer Maschine
(C128: VIC und VDC). Datasette/Kassette: F9 PLAY, Shift+F9 RECORD, F10 STOP,
F11 REWIND. **Ctrl+R** (gedrückt halten) spult zurück: die Maschine läuft
mit doppelter Geschwindigkeit rückwärts durch die letzten Bilder im
Speicher (`--rewind-mb N`, Standard 128 MB, 0 = aus) und läuft beim
Loslassen von dort weiter.

**Cheats:** `--cheat DATEI` lädt eine Cheat-Datei (`.toml`, POKE-Liste
`.pokes`/`.bas`, VICE-Monitorbefehle `.mon`, Adresse/Wert-Codes `.txt`),
`--cheats ORDNER` die Cheat-Datenbank (`ORDNER/<maschine>/<crc32>.toml`
gilt automatisch für eingelegte Images, auch beim Disk-Wechsel; Standard
`EMU_CHEATS_DIR` oder `./cheats`). Die Konsolen-Leiste zeigt jeden Cheat
als Schalter OFF/ON.

**Joystick:** Erster Gamepad und eine Tastatur-Belegung (beim C64/C128
Port 2, bei VIC-20, C16/Plus/4, Atari und CPC Port 1). Welche Tasten den
Joystick spielen, stellt der Schalter **JOY KEYS** in der Konsolen-Leiste um
(oder `--joykeys`): **CURSOR** = Cursortasten, Leertaste = Feuer (Standard
beim Atari 2600, bei der ColecoVision, beim NES und beim SNES, die keine Tastatur haben), **NUMPAD** = Ziffernblock 8/2/4/6,
Feuer 0 oder rechte Strg (Standard bei den Computern), **OFF** = nur Gamepad
(Standard beim CPC, dessen Ziffernblock die Funktionstasten sind). Bei CURSOR
spielen die Cursortasten und die Leertaste den Joystick statt zu tippen.
**PAD FIRE** wählt die Gamepad-Taste für Feuer (SOUTH = A beim Xbox-Pad,
EAST, WEST, NORTH), **SAVE MAPPING** speichert JOY KEYS, JOY PORT und PAD
FIRE für die Maschine (`%APPDATA%\mimic\input\<maschine>.toml`); beim
nächsten Start gilt die gespeicherte Belegung. `--input DATEI.toml` lädt
eine andere Belegung (`[[joystick]]`-Tabellen wie gespeichert).

**Lightpen:** `--option port1=lightpen` (C64/C128, Atari 8-bit: `portN`),
`port0=lightpen` (Amiga), `expansion=lightpen` (CPC): Der Mauszeiger über
dem Bild zielt, die linke Maustaste ist der Knopf (kein Einfangen); ebenso
Zapper (NES) und Super Scope (SNES).

**Maus/Paddles:** Mit `--option port1=mouse1351` (oder `paddles`, beim
Atari 2600 auch `driving`, jeweils auch `port2`) bzw. beim CPC
`--option joy1=amx` fängt ein Klick ins Fenster die Maus ein; die mittlere
Maustaste gibt sie frei. Beim 2600 dreht die Maus Paddle 0 bzw. das Lenkrad.
Bei der ColecoVision (`port1=superaction`, `driving`, `roller`) dreht die
Maus den Speed Roller (senkrecht), das Lenkrad (waagrecht, linke Taste =
Gas) bzw. den Trackball (beide Achsen, Tasten = Feuerknöpfe); beim
Driving Module lenken auch die Joystick-Richtungen, oben ist Gas.
Beim Amiga steckt die Maus von Haus aus in Port 0 (Klick ins Fenster fängt
sie ein, mittlere Taste gibt sie frei), der Joystick in Port 1.

**Amiga.** Ohne Diskette zeigt die Kickstart den „Insert disk“-Bildschirm;
eine ADF in DF0: (`--disk`) bootet. Workbench 1.3 braucht von Diskette
etwa drei Minuten emulierte Zeit bis zum Workbench-Bildschirm. Der Ton ist Stereo (Paula, Kanäle 0/3 links,
1/2 rechts) mit dem festen Tiefpass des A500 und dem LED-Filter (Butterworth
zweiter Ordnung, etwa 3,3 kHz), solange die Power-LED-Leitung ihn einschaltet.
Interlace-Bilder (Workbench mit LACE) werden aus beiden Halbbildern
zusammengesetzt („weave“); `--deinterlace bob` verdoppelt stattdessen die
Zeilen des aktuellen Halbbilds (kein Kammeffekt bei Bewegung, flimmert),
**Strg+I** schaltet zur Laufzeit um. Der Preview-Stream liefert ebenfalls
zusammengesetzte Vollbilder.

Modelle (`--model`): `500` (Standard), `1000` (256 KB Chip-RAM, Kickstart im
„Writable Control Store“; mit `--option wcs=disk` und dem Bootstrap-ROM
lädt sie das ROM wie das Original von der Kickstart-Diskette in DF0:) und
`2000` (1 MB Chip-RAM); jeweils auch `-ntsc` (`500-ntsc`). Speicher über
`--option chip=`, `slow=`, `fast=` (Fast-RAM meldet sich wie eine
Zorro-II-Karte per Autoconfig, Kickstart 1.2 und neuer finden es).

`EMU_DESKTOP_DEBUG=1` gibt alle 5 Sekunden Bild- und Audio-Statistik aus.

## 6. Kommandozeilen-Frontend `emu-headless`

Ohne Fenster und Ton – für Skripte, CI und als Debug-Server.

```bash
# 300 Bilder laufen lassen und das letzte als PNG speichern
cargo run --release -p emu-headless -- --machine c64 --rom-dir roms \
    --prg demo.prg --frames 300 --snapshot out.png

# C128 im 80-Zeichen-Modus, Bild des VDC
cargo run --release -p emu-headless -- --machine c128 --option display=80 \
    --rom-dir roms --frames 600 --screen vdc --snapshot c128.png

# Amiga 500: Kickstart 1.3 bis zum Insert-Disk-Bildschirm
cargo run --release -p emu-headless -- --machine amiga --rom-dir roms \
    --frames 400 --snapshot amiga.png

# Bild durch ein Monitorprofil, Ton als WAV, Savestate schreiben
cargo run --release -p emu-headless -- --machine cpc --rom-dir roms \
    --frames 150 --crt ctm644 --snapshot cpc.png --wav cpc.wav --save-state cpc.state
```

| Option | Bedeutung |
| --- | --- |
| `--frames N` | Bilder (Felder) emulieren, Standard 1 |
| `--snapshot F.png`, `--snapshot-every N` | letztes Bild bzw. jedes N-te speichern |
| `--screen vic\|vdc` | welcher Bildschirm (C128) |
| `--crt PROFIL[:N]` | Snapshot durch ein Monitorprofil, N Pixelzeilen pro Rasterzeile |
| `--wav F.wav` | Ton des Laufs als 16-Bit-PCM |
| `--load-state F`, `--save-state F` | Savestate laden/schreiben |
| `--debug [ADR]` | Debug-Server statt Bilder laufen lassen (Standard `127.0.0.1:6510`) |
| `--preview [ADR]` | Live-Preview (Standard `127.0.0.1:6580`), schließt `--debug` ein |
| `--vice [ADR]` | VICE-Binary-Monitor (Standard `127.0.0.1:6502`), schließt `--debug` ein |
| `--run` | mit `--debug`: gleich laufen statt angehalten starten |
| `--no-realtime` | mit `--debug`: so schnell wie möglich |

## 7. Medien: Disketten, Bänder, Module, Programme

| Art | Formate | Frontend-Option |
| --- | --- | --- |
| Diskette (mehrfach: weitere Laufwerke, Rest per Strg+D) | D64, D71, D81, G64, D80/D82 (PET), ATR (Atari), DSK/EDSK (CPC), ADF (Amiga: DD 880 KB, HD 1,76 MB mit `--option drivetype=hd`; wird beim Einlegen in MFM-Spuren gewandelt und beim Zurückschreiben wieder zur ADF), extended ADF (`UAE-1ADF`/`UAE--ADF` mit Rohspuren, die bitgenau erhalten und zurückgeschrieben werden), DMS (DISK-Masher-Archiv, wird beim Einlegen entpackt, nur lesen); ein **Ordner** als virtuelle Diskette | `--disk` |
| Band | TAP (Commodore), CAS (Atari), CDT/TZX (CPC), P/81 (ZX81: der Programmspeicher ab $4009, geladen und gesichert über ROM-Traps mit `LOAD ""`/`SAVE "NAME"`), TAP/TZX (ZX Spectrum: `LOAD ""` drückt PLAY; mit `traps=on` werden Standardblöcke direkt kopiert, Turbo- und Sonderblöcke laufen in Echtzeit; TZX-Stoppblöcke halten das Band an); eine PRG wird beim Einlegen in das Bandformat der Maschine aufgenommen | `--tape` |
| Snapshot | ZX Spectrum: SNA (48K, 128K) und Z80 (Version 1–3; 48K/128K/+2) — wird beim nächsten Befehl geladen | `--prg` |
| Microdrive | ZX Spectrum mit Interface 1: MDR-Cartridges (254 Sektoren, Schreibschutzbyte), Schacht „microdrive 1“ … | `--disk` |
| Modul | CRT (C64/C128), Roh-Dumps VIC-20 und C16/Plus/4, CAR bzw. 8/16-KB-ROM (Atari), Roh-Dumps `.a26`/`.bin` 2 KB–512 KB (Atari 2600; Bankswitching wird erkannt, auch DPC von Pitfall II mit 10 240 oder 10 495 Bytes), `.col`/`.rom`/`.bin` bis 32 KB oder MegaCart 64 KB–1 MB (ColecoVision), iNES/NES 2.0 `.nes` mit Mapper 0–4 (NROM, MMC1, UxROM, CNROM, MMC3), SNES `.sfc`/`.smc` (LoROM, HiROM, ExHiROM, mit oder ohne Kopierer-Header; noch ohne Koprozessoren) | `--cart` |
| Programm | PRG, T64, XEX | `--prg` |

Images werden standardmäßig über einen **Copy-on-Write-Overlay** geöffnet: das
Original bleibt unverändert, bis Änderungen ausdrücklich zurückgeschrieben
werden (`flush_media` im Debugger). Beim CPC 664/6128 werden Disketten an
Laufwerk A und B angeboten; auf dem 464 gibt es nur Kassette. Auf dem
664/6128 schaltet `|TAPE` (in BASIC) auf Kassette um.

## 7a. Echte Hardware: Greaseweazle und ZoomFloppy

Optional, nur in `mimic-headless` und nur mit Cargo-Features gebaut (ohne
sie ändert sich am normalen Build nichts):

```bash
cargo build --release -p emu-headless --features greaseweazle   # Flux-Leser
cargo build --release -p emu-headless --features opencbm        # echte 1541/1571/1581
cargo build --release -p emu-headless --features real-hardware  # beides
```

**Greaseweazle** (USB, erscheint als serielle Schnittstelle: unter Windows
ein COM-Port, im Geräte-Manager unter „Anschlüsse“; Linux `/dev/ttyACM0`,
Benutzer in der Gruppe `dialout`). Kein Treiber nötig (USB-CDC); die
Firmware muss Version 1.x sein (sonst mit den Greaseweazle-Tools `gw update`).
Eine Diskette wird ganz gelesen und als Image geschrieben, das Format kommt
aus der Dateiendung:

```powershell
mimic-headless --import-flux spiel.d64 --gw-port COM5           # 1541, 5,25"-Laufwerk
mimic-headless --import-flux spiel.g64 --gw-port COM5           # 1541 mit Rohspuren (Kopierschutz, defekte Sektoren)
mimic-headless --import-flux disk.d71 --gw-port COM5            # 1571, beide Seiten
mimic-headless --import-flux disk.d81 --gw-port COM5            # 1581, 3,5"-DD-Laufwerk
mimic-headless --import-flux workbench.adf --gw-port COM5       # Amiga DD (--gw-format amiga-hd für HD)
mimic-headless --import-flux dos.img --gw-port COM5             # PC/Atari ST (IBM-MFM, DD oder HD erkannt)
```

| Option | Bedeutung |
| --- | --- |
| `--gw-port PORT` | COM-Port bzw. Gerätedatei; `auto` (Standard) nimmt den einzigen gefundenen Greaseweazle (USB 1209:4d69) |
| `--gw-drive D` | `a` oder `b` am PC-Kabel mit Dreher (Standard `a`), `0`–`2` am Shugart-Bus |
| `--gw-format F` | `1541`, `1571`, `1581`, `amiga`, `amiga-hd`, `ibm`, `ibm-dd`, `ibm-hd`, `ibm360` |
| `--gw-tracks N` | Spuren (1541: 35/40/42) bzw. Zylinder (IBM) |
| `--gw-step S` | `auto` (Standard), `1` oder `2`: eine 40-Spur-Diskette (1541/1571/360K) im 80-Spur-Laufwerk (1,2 MB) braucht Doppelschritte; `auto` liest Zylinder 1 und sieht nach, welche Spur dort liegt |
| `--gw-revs N` | Umdrehungen pro Lesevorgang (Standard 2) |
| `--gw-retries N` | weitere Versuche für Spuren mit fehlenden Sektoren (Standard 3) |

Die Drehzahl wird aus den Indeximpulsen gemessen, also funktionieren 300-
und 360-U/min-Laufwerke. Fehlende Sektoren werden gemeldet; ein D64 bekommt
dann eine Fehlerbyte-Tabelle (20 = Header fehlt, 23 = Prüfsumme), ein G64
behält die rohen Spuren. Die 1541-Seite ist Kopf 0 (Unterseite) des
PC-Laufwerks.

**ZoomFloppy / XUM1541 über OpenCBM**: OpenCBM (0.4.99.104 oder neuer, unter
Windows die 64-Bit-Ausgabe passend zu mimic) muss installiert sein, der
ZoomFloppy-Treiber eingerichtet und `cbmctrl detect` muss das Laufwerk
finden. mimic lädt die Bibliothek erst zur Laufzeit: `opencbm.dll`
(Windows; im `PATH` oder neben `mimic-headless.exe`), `libopencbm.so.0`
(Linux), `libopencbm.dylib` (macOS) — oder den Pfad in `MIMIC_OPENCBM`.
Die Sektoren laufen über die DOS-Befehle des Laufwerks (`U1`/`U2`, `B-P`),
also ohne Schnelllader: eine 1541-Diskette braucht einige Minuten.

```powershell
mimic-headless --import-drive kopie.d64                 # Diskette in Laufwerk 8 als D64 (.d71/.d81 für 1571/1581)
mimic-headless --import-drive kopie.d64 --cbm-tracks 40 # 40-Spur-Diskette
mimic-headless --export-drive spiel.d64 --cbm-device 9  # Image auf die (formatierte) Diskette in Laufwerk 9 schreiben
mimic-headless --machine c64 --rom-dir roms --real-drive 8 --frames 600 --snapshot c64.png   # echte Diskette im emulierten Laufwerk 8
```

| Option | Bedeutung |
| --- | --- |
| `--real-drive N` | das echte Laufwerk N als Diskette im ersten emulierten Laufwerk; gelesen wird spurweise beim ersten Zugriff, Schreibzugriffe bleiben im Speicher |
| `--real-drive-write` | geänderte Sektoren werden auf die echte Diskette geschrieben |
| `--cbm-device N` | Laufwerksnummer für `--import-drive`/`--export-drive` (Standard 8) |
| `--cbm-adapter NAME` | OpenCBM-Adapter, z. B. `xum1541` (Standard: aus der OpenCBM-Konfiguration) |
| `--cbm-type T` | `1541`, `1570`, `1571`, `1581`, falls die Erkennung (`cbm_identify`) versagt |
| `--cbm-tracks N` | 35 oder 40 (D64) |

## 7b. Netzwerk und Ordner: Ultimate Command Interface und WiC64 (C64)

Zwei Erweiterungen bringen den C64 ins Netz. Sie greifen nie selbst auf den
Rechner zu: Netzwerk und Ordner stellt das Frontend bereit (Goldene Regel 10).

**Ultimate Command Interface (UCI)** – `--option uci=on`: die Register
`$DF1C–$DF1F` des Ultimate 64 / 1541 Ultimate II+ (Steuerung/Status,
Kommandodaten/Kennung `$C9`, Antwortdaten, Statustext wie `00,OK`).
Unterstützt werden die DOS-Targets 1 und 2 (Datei öffnen/lesen/schreiben/
suchen, FILE_INFO/FILE_STAT, löschen, umbenennen, kopieren, Verzeichnis
wechseln/anlegen/lesen, GET_PATH, GET_TIME), das Network-Target 3
(TCP/UDP-Sockets öffnen, lesen, schreiben, schließen, MAC/IP abfragen,
TCP-Listener) und vom Control-Target 4 nur IDENTIFY und GET_HWINFO. Die
Ultimate-Steuerbefehle (Mount, Reset, REU, Palette …) fehlen bewusst. Ein
Steckmodul bleibt nutzbar; nur die vier UCI-Register gehören dann dem UCI.

Die DOS-Targets arbeiten in einem Ordner des Rechners: `--uci-dir ORDNER`
(Standard `./ultimate`, wird beim ersten Schreiben angelegt). Pfade können
den Ordner nicht verlassen (`..`, absolute Pfade, Laufwerksbuchstaben und
symbolische Links werden abgewiesen).

**WiC64** – `--option userport=wic64`: der WLAN-Adapter am Userport
(PB0–PB7, PA2 als Richtung, PC2/FLAG2 als Handshake), Firmware 2.x mit dem
Legacy-Protokoll (`W`) und Protokoll 2 (`R`, `E`). Befehle: HTTP-GET (auch
URL-kodiert), HTTP-POST, TCP öffnen/lesen/schreiben/schließen, IP, MAC,
SSID, RSSI, Uhrzeit und Zeitzone, Server, Firmware-Version, Statusmeldung,
Timeouts, Echo. `https:` meldet „nicht unterstützt“ (kein TLS im Build).
Über den Debugger lässt sich der WiC64 auch nachträglich stecken
(`connect_device` mit `wic64` am Port „user port“).

**Netzwerk:** Standard ist das echte Netz des Rechners; `--net off` (oder
`--option net=off`) schaltet es ab, dann melden beide Geräte „kein
Netzwerk“. TCP-Listener des UCI lauschen nur auf `127.0.0.1`. Die Uhrzeit
kommt vom Rechner (UTC; den Abstand zur Ortszeit setzt die
Umgebungsvariable `EMU_UTC_OFFSET`, z. B. `+01:00`). Das Web-Frontend hat
kein Netz (Browser erlauben keine Sockets); das UCI sieht dort die
hochgeladenen Dateien als Ordner (Änderungen bleiben im Speicher).

```powershell
cargo run --release -p emu-desktop -- --machine c64 --rom-dir roms --option userport=wic64
cargo run --release -p emu-headless -- --machine c64 --rom-dir roms --option uci=on --uci-dir C:\c64\ultimate --debug
```

**Aufnahme und Wiedergabe:** Läufe mit echtem Netz sind nicht
deterministisch. Deshalb nimmt `record_start`/`record_stop` neben den
Eingaben jede Antwort von Netz und Ordner auf (Feld `host` der Aufnahme);
`replay_input` beantwortet sie daraus, ohne Netz – die Wiedergabe ist
exakt. Offene Verbindungen und Dateien sind nicht Teil eines Savestates:
nach dem Laden sieht das Programm Fehler (in einer Wiedergabe antwortet
die Aufnahme).

## 8. Monitorprofile (CRT)

Maschinen liefern immer das rohe Bild; Monitor- und Signaleffekte (Composite,
S-Video, PAL-Laufzeitleitung, NTSC-Artefaktfarben, Scanlines) entstehen
nachgelagert.

| Profil | Verwendung |
| --- | --- |
| `raw` | scharfe Pixel, keine Effekte (Regressionstests) |
| `1702`, `1084s` | Commodore-Monitore (S-Video) |
| `1084s-rgb` | RGB-Eingang des 1084S (C128-VDC) |
| `tv`, `pal-tv`, `ntsc-tv` | Fernseher über Composite (Atari-Standard) |
| `pet-white`, `pet-green` | PET-Monitore |
| `ctm644` | Amstrad-Farbmonitor (CPC-Standard) |
| `gt65` | Amstrad-Grünmonitor |

Eigene Profile sind TOML-Dateien mit `base = "<preset>"` und überschriebenen
Werten (`--crt meinprofil.toml`).

## 9. Live-Preview im Browser

```bash
cargo run --release -p emu-headless -- --machine c64 --rom-dir roms --preview
# Browser: http://127.0.0.1:6580/
```

| Anfrage | Antwort |
| --- | --- |
| `GET /` | Viewer-Seite mit Live-Bild |
| `GET /snapshot.png` | aktuelles Bild als PNG (Header `X-Beam-Frame/Line/Cycle`, `X-Paused`) |
| `GET /preview` (WebSocket) | Bildstrom als PNG/QOI (auch mit CRT-Filter) |
| `GET /stream` (WebSocket) | Rohstrom: Farbindizes, nur geänderte Zeilen, optional Ton |
| `GET /overlays` | Debug-Overlays: `beam`, `grid` und die der Maschine (JSON) |
| `GET /gfx` | Grafikdaten-Ansichten der Maschine (JSON) |
| `GET /gfx/<view>.png` | eine Ansicht als Bild (`scale=1..8`, weitere Zahlen sind Optionen der Ansicht, z. B. `index=2&addr=$2000`) |

Parameter (Query-String): `filter=raw|crt`, `profile=<name>`,
`scale=1..4`, für den Strom `fps=1..50` (Standard 25), `format=png|qoi`,
`delta=0|1`; der Snapshot zusätzlich `machine=<id>` und `screen=vic|vdc`.
Snapshot und `/preview` nehmen `overlay=beam,grid,…`: Die Overlays werden
auf das Rohbild des Hauptschirms gezeichnet (nie auf das CRT-Bild):
`beam` markiert die Strahlposition (Zeile rot, Punkt gelb; bei angehaltener
Maschine das Teilbild), `grid` zieht alle 8 Rasterzeilen eine Linie mit
Zeilennummern. Maschinen-Overlays:

| Maschine | Overlays |
| --- | --- |
| C64, C128 | `badlines` (Bad Lines), `sprites` (wo jedes Sprite tatsächlich erschien, auch gemultiplext) |
| C16/Plus/4 | `badlines` (DMA-Zeilen des TED) |
| Atari 8-bit | `dlist` (Modus je Display-List-Zeile, LMS/DLI/HS/VS), `players` (Player/Missiles), `dli` |
| Atari 2600 | `objects` (RESPx/RESMx/RESBL/HMOVE-Strobes) |
| Amiga | `dma` (DMA-Slot-Karte je Farbtakt und Zeile mit Legende), `sprites`, `copper` (jeder Copper-MOVE) |
| CPC | `crtc` (CRTC- und Gate-Array-Schreibzugriffe im Bild), `vsync` |
| ColecoVision | `sprites`, `limit` (Zeilen mit 5. Sprite) |
| NES | `sprites`, `sprite0` (Sprite-0-Treffer), `irq` (NMI, Mapper-/APU-IRQ) |
| SNES | `sprites`, `hdma` (HDMA-Schreibzugriffe je Zeile), `irq` |
| Apple II | `modes` (Videomodus je Zeile) |

Maschinen-Overlays zeichnen das letzte vollständige Bild auf, bei
angehaltener Maschine das Bild bis zum Strahl; sie kosten nur Zeit,
solange ein Viewer sie eingeschaltet hat. Der Viewer zeigt sie als
Schalter in der Kopfzeile, die Grafikansichten unter „Graphics data“.

Der **Rohstrom** `/stream` (nach dem Vorbild des Video-Streams des
Ultimate 64) kodiert keine Bilder: Er schickt die Farbindizes der Maschine
(4 Bit pro Pixel bis 16 Farben, sonst 8 Bit, beim RGB-Bild 24 Bit) und nach
dem ersten Bild nur die Zeilen, die sich geändert haben, dazu das
Seitenverhältnis eines Pixels (VIC-20 ≈ 1,66, C64 ≈ 0,94), mit dem der Viewer
das Bild so breit zeigt wie ein Fernseher. Der Server braucht
dafür etwa ein Achtel der Zeit des PNG-Stroms; 50 fps sind lokal und im LAN
kein Problem. Parameter: `screen=`, `fps=1..50` (Standard 50), `audio=0|1`
(Ton als 16-Bit-PCM, 48 kHz). Das Nachrichtenformat steht in
`crates/emu-preview/src/raw.rs`. Der Viewer nutzt den Rohstrom beim Filter
„raw“ und spielt den Ton ab, wenn „Sound“ angehakt ist. Für gleichmäßigen
Ton muss die Maschine in Echtzeit laufen; das ist der Standard, nur
`emu-headless --no-realtime` läuft schneller und der Ton stottert dann.

Der Viewer schickt Tastatur-, Gamepad- und Mauseingaben als JSON zurück
(`{"type": "key", "code": "KeyA", "down": true}` usw.), dadurch ist das
Vorschaubild bedienbar. Der Server läuft nur auf `127.0.0.1`.

## 9a. Web-Frontend (alle Maschinen im Browser)

Das Web-Frontend (`crates/emu-web`) ist derselbe Emulator als
WebAssembly. Gebündelt wird nichts: ROMs, Disketten, Bänder und Module
lädt man im Browser hoch (auf Wunsch bleiben sie in diesem Browser
gespeichert, IndexedDB).

```bash
bash tools/build-web.sh                       # -> target/web (mimic.wasm, index.html, mimic.js)
python -m http.server -b 127.0.0.1 -d target/web 8080
# Browser: http://127.0.0.1:8080/
node tools/web-smoke.mjs                      # alle Maschinen im WASM-Modul (Test-ROMs, ohne Browser)
```

- **Maschine** wählen, Modell und Optionen (`ram=8k drives=2`) eintragen,
  **Start**. „built-in test ROMs“ startet mit den eingebauten Stub-ROMs.
- **Files**: einzelne Dateien oder einen ROM-Ordner hochladen (wie
  `--rom-dir`: Unterordner je Maschine, Dateinamen und Größen wie bei der
  ROM-Suche). Auf das Bild gezogene Dateien kommen in das passende
  Laufwerk.
- **Drives and slots**: Medien einlegen/auswerfen.
- **Cheats**: Liste der Freezes zum An- und Ausschalten, Import (POKE-Liste,
  Adresse/Wert, VICE-Monitor, TOML), Export, Speichersuche mit
  „Freeze“-Knopf. Cheat-Dateien `cheats/<maschine>/<crc32>.toml` im
  hochgeladenen Ordner gelten automatisch für das passende Image.
- **State**: Savestate herunterladen/laden; **Profiles…** startet ein
  Profil aus einer `profiles.yaml`.
- Eingabe: ins Bild klicken, dann gehen Tasten an die Maschine
  (positionsgleich wie im Desktop); Gamepads (Standard-Mapping) spielen den
  Joystick; Maus/Paddles fangen den Mauszeiger (Esc gibt ihn frei).
- Direktstart per URL: `?machine=c64&options=roms=stub&start=1`
  (Optionen mit Komma getrennt), `&model=`, `&crt=raw|default|lite`.
- Monitor: `raw` (scharfe Pixel), das Profil der Maschine oder `lite`;
  das CRT-Bild rechnet die CPU, `lite` ist für schwache Rechner.

## 10. Debug-Schnittstelle

### 10.1 Aufbau

```
 Werkzeug (polydbg, eigenes Skript, VS64 …)
        │  TCP: JSON-Zeilen (nativ) / VICE-Binary-Monitor
        ▼
 Debug-Server (emu-debug)  ──Befehlswarteschlange──►  Emulations-Thread
        ▲                   ◄──Ereigniskanal───────   (Machine + DebugHooks)
```

- Die Emulation läuft in einem eigenen Thread. Netzwerk-Tasks sperren ihn nie;
  Befehle gehen über eine Warteschlange, Ergebnisse und Ereignisse über einen
  Kanal zurück.
- Der Debugger kennt **keine konkrete Maschine**. Er spricht nur die
  Kern-Traits an (Register, Speicherräume, Chips, Geräte). Darum funktioniert
  er für jede Maschine gleich, auch für Zweit-CPUs (1541-Laufwerk, Z80 im
  C128) als eigene „Devices“.
- Speicherzugriffe des Debuggers sind immer **seiteneffektfrei** (`peek`): ein
  Blick auf ein Interruptregister löscht nichts.
- Ohne gesetzte Haltepunkte kostet der Debugger praktisch keine Zeit
  (Haltepunkte über Bitmaps je Speicherraum).
- Der Server lauscht nur auf `127.0.0.1`.

Starten:

```bash
# angehalten starten, Debug-Server auf 127.0.0.1:6510
cargo run --release -p emu-headless -- --machine c64 --rom-dir roms --debug
# mit Fenster und Ton, Debug-Server zusätzlich
cargo run --release -p emu-desktop -- --machine c64 --debug
# alles: nativ + VICE-Monitor + Live-Bild, sofort laufen lassen
cargo run --release -p emu-headless -- --machine c64 --rom-dir roms --vice --preview --run
```

### 10.2 Protokoll (nativ, JSON-Zeilen)

Ein JSON-Objekt pro Zeile in beide Richtungen, Protokollname
`mimic-debug`, aktuelle Version **9**. Anfragen haben eine `id` und einen
`cmd`; jede bekommt genau eine Antwort, in Reihenfolge:

```json
→ {"id": 1, "cmd": "regs", "device": "cpu"}
← {"id": 1, "ok": true, "result": {"PC": 2048, "A": 0, "X": 0, "Y": 0, "S": 255, "P": 36}}
→ {"id": 2, "cmd": "regs", "device": "z80"}
← {"id": 2, "ok": false, "error": "unknown device 'z80'"}
```

Ohne `device` gilt der Haupt-Prozessor, ohne `space` der erste
Speicherraum (die Sicht der Haupt-CPU). Mit `info` erfährt man Geräte und
Speicherräume der Maschine; `capabilities` listet alle unterstützten
Befehle, Haltepunktarten und Ereignisse.

Beispiel mit Python (ohne Client):

```python
import json, socket
s = socket.create_connection(("127.0.0.1", 6510))
f = s.makefile("rw")
print(f.readline())                      # hello-Ereignis
f.write(json.dumps({"id": 1, "cmd": "peek", "addr": 0xD020, "len": 1}) + "\n"); f.flush()
print(f.readline())
```

**Ereignisse** tragen `event` statt `id`:

| Ereignis | Wann |
| --- | --- |
| `hello` | beim Verbinden: Protokoll, Version, Maschine |
| `resumed` | Emulation läuft los |
| `stopped` | Lauf endet (Haltepunkt, Watchpoint, Schritt fertig, Absturz, `pause`) |
| `frame_ready` | nach jedem Bild, nur nach `subscribe` |
| `media_changed`, `device_connected` | Medium/Gerät über das Protokoll gewechselt |
| `freeze_blocked_write` | Schreibzugriff auf eine eingefrorene Adresse unterdrückt (nach `subscribe`) |
| `stopped_frame` | nach `stopped`, nur nach `subscribe`: das (Teil-)Bild als PNG mit markiertem Strahl und den Overlays des Debuggers |
| `cheats_loaded` | die Cheat-Datenbank hatte Cheats für ein eingelegtes Medium (`game`) |

### 10.3 Befehle

**Ausführung.** `step_instr` (`count`), `step_cycle`, `step_line`,
`step_frame`, `pause`, `reset` (`kind: soft|hard`) antworten synchron mit dem
Haltezustand (`reason`, `pc`, `beam`). `resume`, `step_over`, `step_out`,
`run_to` (`addr` oder `symbol`) antworten mit `{"running": true}`; das Ende
meldet ein `stopped`-Ereignis.

**Ansehen und Ändern.** `info`, `regs`, `set_reg`, `peek` (`addr`, `len`,
max. 64 KB), `poke` (`bytes`), `disasm` (`addr`, `count`), `beam`
(Bild/Zeile/Zyklus), `chips` (Registerdumps aller Chips, z. B. VIC-II, SID,
CIA, ANTIC, CRTC, Gate Array, FDC).

**Rückwärts.** `rewind_config` (`frames`: alle N Bilder einen Zustand
merken, 0 = aus; `budget_mb`: Speicher, Standard 128), `rewind_info`,
`rewind` (`count` Zustände zurück, die Maschine bleibt angehalten; neuere
Zustände entfallen), `step_back` (`count` Befehle der Haupt-CPU zurück:
mimic stellt den letzten früheren Zustand her und rechnet deterministisch
bis zum gewünschten Befehl vor). Änderungen von außen (`poke`, `set_reg`,
Eingaben, Medien, Freezes, `load_state`) stecken in keinem älteren
Zustand; über sie springt `step_back` nicht zurück.

**Bilder, Overlays, Grafikdaten.** `frame` liefert das aktuelle Bild (beim
Anhalten das Teilbild) als PNG/QOI (`scale`, `screen`, `overlays`).
`overlay_list`, `overlay_set` (`overlays`: die Overlays dieses Debuggers;
Maschinen-Overlays werden ab dann aufgezeichnet), `overlay_get` (`overlay`:
die Formen in Bildkoordinaten: Rechtecke, Bänder, Marken, Zellkarten mit
Legende). `gfx_list` nennt die Grafikdaten-Ansichten der Maschine (Sprites,
Zeichensätze, Bitmaps, Bildschirmspeicher, Kacheln, Paletten mit ihren
Optionen), `gfx_get` (`view`, `format`: `png` | `qoi` | `indexed`, `scale`,
`options`) dekodiert eine: Bild, Parameter (Basisadressen, Modus, Farben …)
und die Rohbytes.

**Haltepunkte.** `break_add` mit `kind`:

| `kind` | Felder |
| --- | --- |
| `exec` (Standard) | `addr` oder `symbol` |
| `read`, `write` | Watchpoint: `addr`/`symbol`, `len` |
| `raster` | `line`, `cycle` |
| `irq`, `nmi` | – |

Zusätzlich bei allen: `condition` (Ausdruck), `ignore` (Treffer überspringen),
`enabled`. Weitere Befehle: `break_list`, `break_remove`, `break_enable`,
`break_condition`.

Bedingungen sind kleine Ausdrücke:

```text
A == $FF && [$D012] > 100
(P & %00000010) != 0 || line == 51
```

Zahlen dezimal, `$hex`, `0xhex`, `%binär`; Bezeichner sind Registernamen des
Geräts sowie `line`, `cycle`, `frame`; `[expr]` liest ein Byte (seiteneffektfrei).
Operatoren: `|| && == != < <= > >= | ^ & + - * / %` und unär `! - ~`.
Ein Auswertungsfehler zählt als „wahr“, damit ein kaputter Haltepunkt anhält.

**Symbole.** `symbols_load` (`path` oder `text`, Format `vice` oder
`kickass`, sonst automatisch erkannt), `symbols_list`, `symbols_clear`.
Symbole erscheinen in `disasm` und sind in `break_add`/`run_to` nutzbar.

**Zustände und Trace.** `save_state`/`load_state` (Base64), `trace_start`
(`instructions`, `reads`/`writes` als Bereiche, `capacity`), `trace_read`,
`trace_stop`. Trace-Einträge tragen Bild, Zeile und Zyklus.

**Freezer und Cheats.** `freeze_add` (`addr`/`symbol`, `size`, `value`,
`mode`: `block_writes` – Standard, `reapply_per_frame`, `reapply_on_read`;
`label`), `freeze_list`, `freeze_toggle`, `freeze_remove`. Cheat-Suche:
`search_start` (`size`, Bereich), `search_next` (`op`: `eq ne lt gt` gegen
`value`, `increased decreased unchanged changed`, `increased_by
decreased_by`), `search_results`. Cheat-Dateien: `cheats_load` (`path`
oder `text`, `format`: `toml` – Standard, `pokes` – BASIC-POKE-Listen auch
mit `$`/`&`-Hex, `vice` – VICE-Monitorbefehle `>`/`f` als Code-Patches,
`codes` – Adresse/Wert-Listen), `cheats_save` (TOML). Cheat-Dateien können
neben Freezes auch Code-Patches (`[[patch]]`, Bytefolgen) enthalten.

**Cheat-Datenbank.** Liegt unter `cheats/<maschine>/<crc32>.toml` (CRC-32
des Images, acht Hex-Ziffern klein; Ordner: `--cheats DIR`,
`EMU_CHEATS_DIR` oder `./cheats`), werden die Cheats beim Einlegen des
Images automatisch geladen (`insert_media`, Kommandozeile, Web).

**Geräte, Medien, Eingaben.** `list_ports`, `connect_device`,
`disconnect_device`; `list_media_slots`, `insert_media` (Datei auf dem Host,
Copy-on-Write), `eject_media`, `flush_media`, `set_drive_mode`
(`true`/`virtual`), `media_control` (z. B. Datasette `play`, `record`,
`stop`, `rewind`, `forward`); `input` (zeitgestempelte Eingaben an Tastatur
oder Port), `record_start`/`record_stop`/`replay_input` (Eingaben
aufzeichnen und reproduzierbar abspielen; die Aufnahme enthält auch die
Antworten von Netz und Ordner, Abschnitt 7b).

## 11. Python-Client `polydbg`

`tools/polydbg.py` braucht nur die Python-Standardbibliothek:

```bash
python tools/polydbg.py info
python tools/polydbg.py regs
python tools/polydbg.py disasm
python tools/polydbg.py break '$0810'          # Haltepunkt
python tools/polydbg.py resume --wait          # bis zum Halt laufen
python tools/polydbg.py peek '$d000' 64
python tools/polydbg.py chips vic2             # Chipregister
python tools/polydbg.py dlist                  # Atari: Display-List dekodieren
python tools/polydbg.py symbols game.sym
python tools/polydbg.py freeze '$0400' 9       # Adresse einfrieren
python tools/polydbg.py search start           # Cheat-Suche
python tools/polydbg.py gfx                    # Grafikansichten auflisten
python tools/polydbg.py gfx sprites index=2 --scale 4 -o s.png
python tools/polydbg.py overlay beam,grid,badlines
python tools/polydbg.py picture --overlays beam -o now.png   # Teilbild
python tools/polydbg.py rewind --on 1          # Rückspulpuffer an
python tools/polydbg.py rewind 50              # 50 Bilder zurück
python tools/polydbg.py back 3                 # 3 Befehle zurück
python tools/polydbg.py save state.bin
python tools/polydbg.py shell                  # interaktive Eingabe
```

`--host`/`--port` wählen den Server, `--json` gibt die rohen Antworten aus,
`raw CMD '{...}'` schickt beliebige Befehle. `python tools/polydbg.py --help`
listet alle Unterbefehle.

## 12. VICE-Binary-Monitor (VS64 & Co.)

Mit `--vice` spricht mimic zusätzlich das Binärprotokoll des VICE-Monitors
(API-Version 2). Werkzeuge wie **VS64** (VS Code), C64 Studio oder IceBro
Lite debuggen damit jede Maschine – nicht nur den C64.

```bash
cargo run --release -p emu-headless -- --machine c64 --rom-dir roms --vice --preview --run
```

VS64 `launch.json`:

```json
{ "type": "vice", "request": "attach", "name": "mimic",
  "hostname": "127.0.0.1", "port": 6502 }
```

Wichtig: `127.0.0.1`, nicht `localhost` – der Server lauscht nur auf IPv4.
Speicherraum 0 ist die Haupt-CPU, 1–4 sind die Laufwerke 8–11; Speicherlesen
ist immer seiteneffektfrei; `LIN`/`CYC` liefern die Strahlposition.

## 13. Tests und Testdaten

```bash
cargo test --workspace                                   # ohne ROMs/Testdaten
EMU_ROM_DIR=~/roms cargo test --workspace -- --include-ignored   # mit ROMs
bash tools/fetch-testdata.sh                             # Testsuites nach testdata/
export EMU_TESTDATA_DIR=$PWD/testdata
```

`fetch-testdata.sh` lädt gepinnte, prüfsummengesicherte Testsuites (Tom Harte
6502/Z80, Klaus Dormann, ZEXDOC/ZEXALL, Lorenz, Acid800, Arnold-Acid-Tests für
den CPC). Sie werden nur ausgeführt, nie eingecheckt.

| Suite | Befehl |
| --- | --- |
| 6502 Harte (jeder 10. Test) | `cargo test -p cpu-6502 --release -- --ignored` |
| Z80 Harte | `cargo test -p cpu-z80 --release --test harte -- --ignored` |
| ZEXDOC/ZEXALL | `cargo test -p cpu-z80 --release --test zex -- --ignored --nocapture` |
| Lorenz (C64, mit ROMs) | `cargo test -p emu-tests --release --test lorenz lorenz_suite -- --ignored --exact` |
| Acid800 (Atari) | `cargo test -p emu-tests --release --test acid800 -- --ignored --nocapture` |
| Arnold-Acid-Tests (CPC; Fetch braucht einen C++-Compiler für pasmo) | `cargo test -p emu-tests --release --test cpc_acid -- --ignored --nocapture` (ca. 10 min; `EMU_CPC_FILTER=ppi`, `EMU_CPC_CRTC=0`, `EMU_CPC_FRAMES=<Felder>`, `EMU_CPC_TAIL=<Zeichen>`; Bericht in `target/tmp/cpc-acid.txt`) |

Referenzbilder liegen in `tests/reference/`; `EMU_UPDATE_REFERENCE=1`
erzeugt sie neu (nur mit Begründung im Commit).
