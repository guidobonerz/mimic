#!/usr/bin/env bash
# Fetch CPU test-suite data into testdata/ (gitignored, never committed).
#
#   tools/fetch-testdata.sh [target-dir]      default: <repo>/testdata
#   export EMU_TESTDATA_DIR=<target-dir>
#
# Sources are pinned to exact commits; the Dormann image is checked by SHA-256.
# Already downloaded files are kept, so re-running is cheap.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="${1:-$ROOT/testdata}"

# Tom Harte SingleStepTests 65x02 (MIT), ~850 MB for 6502/v1.
HARTE_COMMIT=2f6980a2d95757486c7bee24355c360e40e2a224
HARTE_URL="https://raw.githubusercontent.com/SingleStepTests/65x02/$HARTE_COMMIT/6502/v1"

# Klaus Dormann 6502 functional test (GPL-3.0 test program; run, never committed).
DORMANN_COMMIT=7954e2dbb49c469ea286070bf46cdd71aeb29e4b
DORMANN_URL="https://raw.githubusercontent.com/Klaus2m5/6502_65C02_functional_tests/$DORMANN_COMMIT/bin_files"
DORMANN_SHA256=fa12bfc761e6f9057e4cc01a665a7b800ff01ae91f598af1e39a1201d01953fd

fetch() { # url file
  if [ ! -s "$2" ]; then
    curl -fsSL --retry 3 -o "$2.part" "$1"
    mv "$2.part" "$2"
  fi
}

# Like fetch, but a missing file (HTTP 404) is remembered and skipped.
fetch_if_present() { # url file
  if [ -s "$2" ] || [ -e "$2.missing" ]; then return; fi
  code="$(curl -sSL --retry 3 -o "$2.part" -w '%{http_code}' "$1")"
  case "$code" in
    200) mv "$2.part" "$2" ;;
    404) rm -f "$2.part"; touch "$2.missing" ;;
    *) rm -f "$2.part"; echo "error: $1: HTTP $code" >&2; exit 1 ;;
  esac
}

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

echo "Dormann functional test -> $DEST/dormann"
mkdir -p "$DEST/dormann"
fetch "$DORMANN_URL/6502_functional_test.bin" "$DEST/dormann/6502_functional_test.bin"
fetch "$DORMANN_URL/6502_functional_test.lst" "$DEST/dormann/6502_functional_test.lst"
actual="$(sha256 "$DEST/dormann/6502_functional_test.bin")"
if [ "$actual" != "$DORMANN_SHA256" ]; then
  echo "error: Dormann image checksum mismatch ($actual)" >&2
  exit 1
fi

echo "Tom Harte 6502 tests -> $DEST/65x02/6502/v1 (256 files)"
mkdir -p "$DEST/65x02/6502/v1"
for i in $(seq 0 255); do
  name="$(printf '%02x.json' "$i")"
  fetch "$HARTE_URL/$name" "$DEST/65x02/6502/v1/$name"
done
echo "$HARTE_COMMIT" > "$DEST/65x02/COMMIT"

echo "Tom Harte NES 2A03 tests -> $DEST/65x02/nes6502/v1 (256 files)"
mkdir -p "$DEST/65x02/nes6502/v1"
for i in $(seq 0 255); do
  name="$(printf '%02x.json' "$i")"
  fetch "https://raw.githubusercontent.com/SingleStepTests/65x02/$HARTE_COMMIT/nes6502/v1/$name" "$DEST/65x02/nes6502/v1/$name"
done

# The CMOS 65C02 without the Rockwell bit instructions (the NCR/GTE 65C02
# of the enhanced Apple IIe behaves the same): `synertek65c02`.
echo "Tom Harte 65C02 tests -> $DEST/65x02/synertek65c02/v1 (256 files)"
mkdir -p "$DEST/65x02/synertek65c02/v1"
for i in $(seq 0 255); do
  name="$(printf '%02x.json' "$i")"
  fetch "https://raw.githubusercontent.com/SingleStepTests/65x02/$HARTE_COMMIT/synertek65c02/v1/$name" "$DEST/65x02/synertek65c02/v1/$name"
done

# Tom Harte SingleStepTests Z80 (MIT), ~1.4 GB: 1604 instruction files of
# 1000 tests each, named by their opcode bytes ("ed b0.json", "dd cb __ 46.json").
Z80_COMMIT=ebe1875d48f374bcfd4b505d8eb8ee751568b5f7
Z80_URL="https://raw.githubusercontent.com/SingleStepTests/z80/$Z80_COMMIT/v1"

echo "Tom Harte Z80 tests -> $DEST/z80/v1"
mkdir -p "$DEST/z80/v1"
for prefix in "" "cb " "dd " "ed " "fd " "dd cb __ " "fd cb __ "; do
  for i in $(seq 0 255); do
    name="$prefix$(printf '%02x' "$i").json"
    fetch_if_present "$Z80_URL/${name// /%20}" "$DEST/z80/v1/$name"
  done
done
echo "$Z80_COMMIT" > "$DEST/z80/COMMIT"

# Tom Harte SingleStepTests 68000 (MIT), ~190 MB: 125 gzipped JSON files,
# one per instruction (and size), 8000+ tests each with bus transactions.
M68K_COMMIT=e0d5ece9670205cc84a0101081837deb446f86a3
M68K_URL="https://raw.githubusercontent.com/SingleStepTests/680x0/$M68K_COMMIT/68000/v1"
M68K_FILES="
ABCD ADD.b ADD.l ADD.w ADDA.l ADDA.w ADDX.b ADDX.l ADDX.w AND.b AND.l
AND.w ANDItoCCR ANDItoSR ASL.b ASL.l ASL.w ASR.b ASR.l ASR.w BCHG BCLR
BSET BSR BTST Bcc CHK CLR.b CLR.l CLR.w CMP.b CMP.l CMP.w CMPA.l CMPA.w
DBcc DIVS DIVU EOR.b EOR.l EOR.w EORItoCCR EORItoSR EXG EXT.l EXT.w JMP
JSR LEA LINK LSL.b LSL.l LSL.w LSR.b LSR.l LSR.w MOVE.b MOVE.l MOVE.q
MOVE.w MOVEA.l MOVEA.w MOVEM.l MOVEM.w MOVEP.l MOVEP.w MOVEfromSR
MOVEfromUSP MOVEtoCCR MOVEtoSR MOVEtoUSP MULS MULU NBCD NEG.b NEG.l
NEG.w NEGX.b NEGX.l NEGX.w NOP NOT.b NOT.l NOT.w OR.b OR.l OR.w ORItoCCR
ORItoSR PEA README.md RESET ROL.b ROL.l ROL.w ROR.b ROR.l ROR.w ROXL.b
ROXL.l ROXL.w ROXR.b ROXR.l ROXR.w RTE RTR RTS SBCD SUB.b SUB.l SUB.w
SUBA.l SUBA.w SUBX.b SUBX.l SUBX.w SWAP Scc TAS TRAP TRAPV TST.b TST.l
TST.w UNLINK
"

echo "Tom Harte 68000 tests -> $DEST/680x0/68000/v1"
mkdir -p "$DEST/680x0/68000/v1"
for name in $M68K_FILES; do
  fetch "$M68K_URL/$name.json.gz" "$DEST/680x0/68000/v1/$name.json.gz"
done
echo "$M68K_COMMIT" > "$DEST/680x0/COMMIT"

# 8080 CPU tests: TST8080 (Microcosm), 8080PRE/8080EXM (Ian Bartholomew),
# CPUTEST (Supersoft), as collected in superzazu/8080; run, never committed.
I8080_COMMIT=274ffd700b81baabea99b0963bc1260b67132185
I8080_URL="https://raw.githubusercontent.com/superzazu/8080/$I8080_COMMIT/cpu_tests"
echo "8080 CPU tests -> $DEST/8080"
mkdir -p "$DEST/8080"
for pair in \
  "TST8080.COM 9561c6fb6c99efe3de00eb77e4044fd102151058b39ac2d7bce10483838a08e7" \
  "8080PRE.COM 18eb3c79cba42c0718f160be6a1853cb64cdce7aa47d65780189a57bdd98c4e0" \
  "CPUTEST.COM e61a9a75348c774486c2207080ea4effbf6c2367fdace31b0731081a4144030b" \
  "8080EXM.COM 6e3286e11bb1a8f47b8ee1280b4a067be813193363e3223c99b0d21912f44aeb"; do
  set -- $pair
  fetch "$I8080_URL/$1" "$DEST/8080/$1"
  [ "$(sha256 "$DEST/8080/$1")" = "$2" ] || { echo "error: $1: checksum" >&2; exit 1; }
done

# Altair BASIC 3.2 (4K) for the IMSAI's console as a paper tape image (Bruce
# Hall's port, bhall66/IMSAI-8080); run, never committed.
IMSAI_COMMIT=ba429db43c93137d63d99c7f972881803ee19580
echo "IMSAI BASIC -> $DEST/imsai"
mkdir -p "$DEST/imsai"
fetch "https://raw.githubusercontent.com/bhall66/IMSAI-8080/$IMSAI_COMMIT/Altair-Basic/basic-tape.hex" "$DEST/imsai/basic-tape.hex"
[ "$(sha256 "$DEST/imsai/basic-tape.hex")" = "d342f6978f9d5840bb629469dd38ca9652d231cab097b75bebb5ffe912dd6b1a" ] || { echo "error: basic-tape.hex: checksum" >&2; exit 1; }

# CP/M 2.2 for the IMSAI with the FIF floppy controller, z80pack's 8-inch
# disk image (Udo Munk's archive); run, never committed.
echo "IMSAI CP/M 2.2 -> $DEST/imsai/cpm22.dsk"
if [ ! -s "$DEST/imsai/cpm22.dsk" ]; then
  fetch "https://www.icl1900.co.uk/unix4fun/z80pack/ftp/imsai/cpm22.tgz" "$DEST/imsai/cpm22.tgz"
  tar -xzf "$DEST/imsai/cpm22.tgz" -C "$DEST/imsai" disks/library/cpm22.dsk
  mv "$DEST/imsai/disks/library/cpm22.dsk" "$DEST/imsai/cpm22.dsk"
  rm -rf "$DEST/imsai/disks" "$DEST/imsai/cpm22.tgz"
fi
[ "$(sha256 "$DEST/imsai/cpm22.dsk")" = "ddc9084f536845b3611f6c44d864fee7c123244a269e5a602d87bd3cc6a32bbe" ] || { echo "error: cpm22.dsk: checksum" >&2; exit 1; }

# ZEXDOC/ZEXALL Z80 instruction exercisers (Frank Cringle, GPL-2.0 test
# programs; run, never committed), CP/M .COM files, checked by SHA-256.
ZEX_COMMIT=8f71d418bae69a476a5a0e5c6e122c8801b8d9f4
ZEX_URL="https://raw.githubusercontent.com/agn453/ZEXALL/$ZEX_COMMIT"
ZEXDOC_SHA256=34923a7ed82285d3038b2d54bd64899e12173eebb61f9d07b4fc72e78af2ae8f
ZEXALL_SHA256=6e2da55147a04f28d303d5da6a1e6b771557ac244653590a0f24a2d39c8537e8

echo "ZEXDOC/ZEXALL -> $DEST/zex"
mkdir -p "$DEST/zex"
for f in zexdoc zexall; do
  fetch "$ZEX_URL/$f.com" "$DEST/zex/$f.com"
done
for pair in "zexdoc $ZEXDOC_SHA256" "zexall $ZEXALL_SHA256"; do
  set -- $pair
  actual="$(sha256 "$DEST/zex/$1.com")"
  if [ "$actual" != "$2" ]; then
    echo "error: $1.com checksum mismatch ($actual)" >&2
    exit 1
  fi
done

# Wolfgang Lorenz C64 Emulator Test Suite 2.15 (public domain), as kept in the
# VICE test programs, pinned to an SVN revision. Only the plain C64 builds;
# the *new variants target the 6526A and are selected by the runner.
LORENZ_REV=46290
LORENZ_URL="https://svn.code.sf.net/p/vice-emu/code/!svn/bc/$LORENZ_REV/testprogs/general/Lorenz-2.15/src"
LORENZ_TESTS="start ldab ldaz ldazx ldaa ldaax ldaay ldaix ldaiy staz stazx staa staax staay
staix staiy ldxb ldxz ldxzy ldxa ldxay stxz stxzy stxa ldyb ldyz ldyzx ldya ldyax styz
styzx stya taxn tayn txan tyan tsxn txsn phan plan phpn plpn inxn inyn dexn deyn incz inczx
inca incax decz deczx deca decax asln aslz aslzx asla aslax lsrn lsrz lsrzx lsra lsrax roln
rolz rolzx rola rolax rorn rorz rorzx rora rorax andb andz andzx anda andax anday andix andiy
orab oraz orazx oraa oraax oraay oraix oraiy eorb eorz eorzx eora eorax eoray eorix eoriy
clcn secn cldn sedn clin sein clvn adcb adcz adczx adca adcax adcay adcix adciy sbcb sbcz
sbczx sbca sbcax sbcay sbcix sbciy cmpb cmpz cmpzx cmpa cmpax cmpay cmpix cmpiy cpxb cpxz
cpxa cpyb cpyz cpya bitz bita brkn rtin jsrw rtsn jmpw jmpi beqr bner bmir bplr bcsr bccr
bvsr bvcr nopn nopb nopz nopzx nopa nopax asoz asozx asoa asoax asoay asoix asoiy rlaz rlazx
rlaa rlaax rlaay rlaix rlaiy lsez lsezx lsea lseax lseay lseix lseiy rraz rrazx rraa rraax
rraay rraix rraiy dcmz dcmzx dcma dcmax dcmay dcmix dcmiy insz inszx insa insax insay insix
insiy laxz laxzy laxa laxay laxix laxiy axsz axszy axsa axsix alrb arrb aneb lxab sbxb shaay
shaiy shxay shyax shsay lasay ancb sbcb-eb trap1 trap2 trap3 trap4 trap5 trap6 trap7 trap8
trap9 trap10 trap11 trap12 trap13 trap14 trap15 trap16 trap17 branchwrap mmufetch mmu
cpuport cputiming irq nmi cia1tb123 cia2tb123 cia1pb6 cia1pb7 cia2pb6 cia2pb7 cia1tab
loadth cnto2 icr01 imr flipos oneshot cntdef cia1ta cia1tb cia2ta cia2tb
irqnew nminew icr01new imrnew cia1tanew cia1tbnew cia1tabnew cia2tanew cia2tbnew"

echo "Lorenz test suite -> $DEST/lorenz"
mkdir -p "$DEST/lorenz"
for t in $LORENZ_TESTS; do
  fetch "$LORENZ_URL/$t.prg" "$DEST/lorenz/$t.prg"
done
echo "$LORENZ_REV" > "$DEST/lorenz/REVISION"

# Acid800 1.2 (Avery Lee): Atari 8-bit hardware test suite, the Phase 5 gate.
# Run only, never committed. A .7z archive: needs 7z/7za or bsdtar (Windows'
# tar.exe is bsdtar). Keeps the boot disk and the standalone tests with
# their label files.
ACID_URL="https://www.virtualdub.org/downloads/Acid800-1.2.7z"
ACID_SHA256=4074e6a40db1833e97603e334ca130f5923bff34a0d4f8a83e2ecf9617de1b20

echo "Acid800 1.2 -> $DEST/acid800"
mkdir -p "$DEST/acid800"
if [ ! -s "$DEST/acid800/standalone/cpu_insn.xex" ]; then
  fetch "$ACID_URL" "$DEST/acid800/Acid800-1.2.7z"
  actual="$(sha256 "$DEST/acid800/Acid800-1.2.7z")"
  if [ "$actual" != "$ACID_SHA256" ]; then
    echo "error: Acid800 archive checksum mismatch ($actual)" >&2
    exit 1
  fi
  tmp="$DEST/acid800/extract"
  rm -rf "$tmp"
  mkdir -p "$tmp"
  if command -v 7z >/dev/null; then
    7z x -y -o"$tmp" "$DEST/acid800/Acid800-1.2.7z" >/dev/null
  elif command -v 7za >/dev/null; then
    7za x -y -o"$tmp" "$DEST/acid800/Acid800-1.2.7z" >/dev/null
  elif command -v bsdtar >/dev/null; then
    bsdtar -xf "$DEST/acid800/Acid800-1.2.7z" -C "$tmp"
  elif [ -x /c/Windows/System32/tar.exe ]; then
    /c/Windows/System32/tar.exe -xf "$DEST/acid800/Acid800-1.2.7z" -C "$tmp"
  else
    echo "error: no 7z, 7za or bsdtar to unpack Acid800" >&2
    exit 1
  fi
  rm -rf "$DEST/acid800/standalone"
  cp "$tmp/out/Release/Acid800/acid800.atr" "$DEST/acid800/"
  cp -r "$tmp/out/Release/Acid800/standalone" "$DEST/acid800/standalone"
  rm -rf "$tmp" "$DEST/acid800/Acid800-1.2.7z"
  echo "1.2" > "$DEST/acid800/VERSION"
fi

# Kevin Thacker's Arnold "Acid tests" for the Amstrad CPC: the Phase 6 gate.
# "(c) Kevin Thacker, part of the Arnold emulator distribution, free to
# distribute without modification" -- run only, never committed. Sources
# only: taken from ColinPitrat/CPCMisc (pinned commit) and assembled with
# pasmo 0.5.3 (GPL; Debian's original source tarball, built here with the
# host's C++ compiler). Without a compiler the suite is skipped.
CPCMISC_COMMIT=9e0ad714baee3548c6effc22044861a3f401799b
CPCMISC_URL="https://codeload.github.com/ColinPitrat/CPCMisc/tar.gz/$CPCMISC_COMMIT"
CPCMISC_SHA256=6342175d5694a2cce734bbc311c526160e5bcdb761dabc3e90e81e7c16cac7f7
PASMO_URL="https://deb.debian.org/debian/pool/main/p/pasmo/pasmo_0.5.3.orig.tar.gz"
PASMO_SHA256=db3c41cac16df5966fcca4719bb218af78224de13785b222d1d8e292fc87f1eb
# Suites with a files.txt (asm, binary name[, disc]) and their pasmo options.
CPC_ACID_SUITES="cpc: cpcrom: crtc: ppi: psg:--equ,CPC=1,--equ,SPEC=0 z80tests: disc/fdc:"

echo "CPC Acid tests -> $DEST/cpc-acid"
CXX_BIN="${CXX:-}"
if [ -z "$CXX_BIN" ]; then
  for c in c++ g++ clang++; do
    if command -v "$c" >/dev/null; then CXX_BIN="$c"; break; fi
  done
fi
if [ -z "$CXX_BIN" ]; then
  echo "warning: no C++ compiler for pasmo: CPC Acid tests skipped" >&2
elif [ ! -s "$DEST/cpc-acid/COMMIT" ] || [ "$(cat "$DEST/cpc-acid/COMMIT")" != "$CPCMISC_COMMIT" ]; then
  work="$DEST/cpc-acid/work"
  rm -rf "$DEST/cpc-acid"
  mkdir -p "$work"
  fetch "$PASMO_URL" "$work/pasmo.tar.gz"
  [ "$(sha256 "$work/pasmo.tar.gz")" = "$PASMO_SHA256" ] || {
    echo "error: pasmo source checksum mismatch" >&2; exit 1; }
  tar -xzf "$work/pasmo.tar.gz" -C "$work"
  pasmo="$work/pasmo"
  (cd "$work/pasmo-0.5.3" && "$CXX_BIN" -O2 -std=c++11 -DVERSION='"0.5.3"' -o "$pasmo" ./*.cpp)
  fetch "$CPCMISC_URL" "$work/cpcmisc.tar.gz"
  [ "$(sha256 "$work/cpcmisc.tar.gz")" = "$CPCMISC_SHA256" ] || {
    echo "error: CPCMisc archive checksum mismatch" >&2; exit 1; }
  tar -xzf "$work/cpcmisc.tar.gz" -C "$work" "CPCMisc-$CPCMISC_COMMIT/assembly/007_arnold_acid"
  src="$work/CPCMisc-$CPCMISC_COMMIT/assembly/007_arnold_acid"
  for entry in $CPC_ACID_SUITES; do
    suite="${entry%%:*}"
    opts="$(echo "${entry#*:}" | tr ',' ' ')"
    out="$DEST/cpc-acid/$(echo "$suite" | tr '/' '_')"
    mkdir -p "$out"
    while IFS=, read -r asm name _; do
      asm="$(echo "$asm" | tr -d '\r ')"
      name="$(echo "$name" | tr -d '\r ')"
      [ -n "$asm" ] || continue
      # shellcheck disable=SC2086
      if ! (cd "$src/$suite" && "$pasmo" --amsdos $opts "$asm.asm" "$out/$name.bin" "$out/$name.sym" >/dev/null 2>&1); then
        echo "  $suite/$asm: does not assemble, skipped" >&2
        rm -f "$out/$name.bin" "$out/$name.sym"
      fi
    done < "$src/$suite/files.txt"
  done
  cp "$src/readme.txt" "$DEST/cpc-acid/README.txt"
  rm -rf "$work"
  echo "$CPCMISC_COMMIT" > "$DEST/cpc-acid/COMMIT"
fi

# Tom Harte SingleStepTests 65816 (MIT), ~3 GB, and SPC700 (MIT) — Phase 12b.
W65816_COMMIT=307c63f5673c8c6b861c52f71ddb89212a227480
echo "Tom Harte 65816 tests -> $DEST/65816/v1 (512 files)"
mkdir -p "$DEST/65816/v1"
for i in $(seq 0 255); do
  for m in e n; do
    name="$(printf '%02x.%s.json' "$i" "$m")"
    fetch "https://raw.githubusercontent.com/SingleStepTests/65816/$W65816_COMMIT/v1/$name" "$DEST/65816/v1/$name"
  done
done
echo "$W65816_COMMIT" > "$DEST/65816/COMMIT"
SPC700_COMMIT=67d15f492b2740964abd4efd1229e0ec9c342228
echo "Tom Harte SPC700 tests -> $DEST/spc700/v1 (256 files)"
mkdir -p "$DEST/spc700/v1"
for i in $(seq 0 255); do
  name="$(printf '%02x.json' "$i")"
  fetch "https://raw.githubusercontent.com/SingleStepTests/spc700/$SPC700_COMMIT/v1/$name" "$DEST/spc700/v1/$name"
done
echo "$SPC700_COMMIT" > "$DEST/spc700/COMMIT"

# NES test ROMs (blargg's suites, nestest …), Phase 12; no licence given:
# run only, never committed.
NESROMS_COMMIT=95d8f621ae55cee0d09b91519a8989ae0e64753b
NESROMS_SHA256=78aab1d5e5d1fb717fd4200c0525ec08a32e4c7e4094590cb6ace95a8ae81d7f
if [ ! -e "$DEST/nes-test-roms/COMMIT" ]; then
  echo "NES test ROMs -> $DEST/nes-test-roms"
  mkdir -p "$DEST/nes-test-roms"
  fetch "https://codeload.github.com/christopherpow/nes-test-roms/tar.gz/$NESROMS_COMMIT" "$DEST/nes-test-roms.tar.gz"
  [ "$(sha256 "$DEST/nes-test-roms.tar.gz")" = "$NESROMS_SHA256" ] || {
    echo "error: nes-test-roms archive checksum mismatch" >&2; exit 1; }
  tar -xzf "$DEST/nes-test-roms.tar.gz" -C "$DEST/nes-test-roms" --strip-components=1
  rm "$DEST/nes-test-roms.tar.gz"
  echo "$NESROMS_COMMIT" > "$DEST/nes-test-roms/COMMIT"
fi

# Fuse Test 2008-03-28 (Philip Kendall, GPL): ZX Spectrum timing tests —
# frame length, contended memory and I/O, floating bus, LDIR at the
# contended boundary, $7FFD reads; Phase 15. Run only, never committed.
FUSETEST_URL="https://zxe.io/depot/software/ZX%20Spectrum/Fuse%20Test%20%282008-03-28%29%28Kendall,%20Philip%29%5B%21%5D.zip"
FUSETEST_SHA256=2044f70b77411a9656f58a85047ed18548f0b18dfb93efa8a79a338483efd5f0
if [ ! -s "$DEST/spectrum/fusetest.tap" ]; then
  echo "Fuse Test -> $DEST/spectrum"
  mkdir -p "$DEST/spectrum"
  fetch "$FUSETEST_URL" "$DEST/spectrum/fusetest.zip"
  [ "$(sha256 "$DEST/spectrum/fusetest.zip")" = "$FUSETEST_SHA256" ] || {
    echo "error: Fuse Test archive checksum mismatch" >&2; exit 1; }
  python3 -c "import sys, zipfile; z = zipfile.ZipFile(sys.argv[1]); open(sys.argv[2], 'wb').write(z.read('fusetest/fusetest.tap'))"     "$DEST/spectrum/fusetest.zip" "$DEST/spectrum/fusetest.tap" 2>/dev/null     || python -c "import sys, zipfile; z = zipfile.ZipFile(sys.argv[1]); open(sys.argv[2], 'wb').write(z.read('fusetest/fusetest.tap'))"     "$DEST/spectrum/fusetest.zip" "$DEST/spectrum/fusetest.tap"
  rm "$DEST/spectrum/fusetest.zip"
fi

# gilyon's snes-tests v1.4 (MIT): 65C816 (cputest-basic/-full) and SPC700
# (spctest) instruction tests that report on screen; Phase 12b. Run only,
# never committed.
SNESTESTS_URL="https://github.com/gilyon/snes-tests/releases/download/v1.4/snes-tests.zip"
SNESTESTS_SHA256=c3a39e15a9f3789fe143e54dfa035c988204576eafacf97df8ff3546a917c5e9
if [ ! -s "$DEST/snes-tests/cputest/cputest-full.sfc" ]; then
  echo "snes-tests -> $DEST/snes-tests"
  mkdir -p "$DEST/snes-tests"
  fetch "$SNESTESTS_URL" "$DEST/snes-tests/snes-tests.zip"
  [ "$(sha256 "$DEST/snes-tests/snes-tests.zip")" = "$SNESTESTS_SHA256" ] || {
    echo "error: snes-tests archive checksum mismatch" >&2; exit 1; }
  python3 -c "import sys, zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])" "$DEST/snes-tests/snes-tests.zip" "$DEST/snes-tests" 2>/dev/null     || python -c "import sys, zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])" "$DEST/snes-tests/snes-tests.zip" "$DEST/snes-tests"
  rm "$DEST/snes-tests/snes-tests.zip"
fi

# PeterLemon's SNES test and demo ROMs with reference PNGs (no licence
# given): CPU/SPC700/PPU/bank tests for tests/tests/snes_peterlemon.rs;
# Phase 12b. Run only, never committed.
PLSNES_COMMIT=350b394e86ec5d62f600b5cbf64cdce3721bb6ef
PLSNES_SHA256=acc9c50f6201bfb0c4c0c6b4f2e159820d3c11b99cffe8d61aed34c391883548
if [ ! -e "$DEST/peterlemon-snes/COMMIT" ]; then
  echo "PeterLemon SNES -> $DEST/peterlemon-snes"
  mkdir -p "$DEST/peterlemon-snes"
  fetch "https://codeload.github.com/PeterLemon/SNES/tar.gz/$PLSNES_COMMIT" "$DEST/peterlemon-snes.tar.gz"
  [ "$(sha256 "$DEST/peterlemon-snes.tar.gz")" = "$PLSNES_SHA256" ] || {
    echo "error: PeterLemon SNES archive checksum mismatch" >&2; exit 1; }
  tar -xzf "$DEST/peterlemon-snes.tar.gz" -C "$DEST/peterlemon-snes" --strip-components=1
  rm "$DEST/peterlemon-snes.tar.gz"
  echo "$PLSNES_COMMIT" > "$DEST/peterlemon-snes/COMMIT"
fi

echo "done. export EMU_TESTDATA_DIR=$DEST"
