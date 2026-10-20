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

echo "done. export EMU_TESTDATA_DIR=$DEST"
