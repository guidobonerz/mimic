"""Build agatest.adf: which colour clocks does an AGA wide bitplane fetch
take, and where does its first word show?

Boot it on an A1200 (WinUAE, cycle-exact A1200 configuration, or a real
machine) and in mimic, and compare the pictures. The program sets FMODE
(default $F: 64-bit bitplane and sprite fetches), eight high-resolution
planes (BPLCON0 $8211) and sprites of 64 high-resolution pixels (BPLCON3
SPRES 70 ns), and shows 13 bands of 16 lines from line $2C with DDFSTRT
$48, $44, $40 … $18 (DDFSTOP $D0 throughout; the Copper changes DDFSTRT
two lines before each band):

- plane 1 holds one set pixel per 64-bit word, plane 2 is all ones, planes
  3-8 are empty: the fetched data shows as a blue area (COLOR02) with a
  white line (COLOR03) at the first pixel of every fetched 64-bit word, on
  dark grey COLOR00 (display window $2C71-$2CD1, wider than the data). The
  blue area's left edge is where the first fetched word shows.
- in each band all eight sprites stand side by side from HSTART $80, 10
  lines high: even sprites in colour 1 of their pair, odd ones in colour 3
  (pair 0 red/orange, 1 yellow/green, 2 cyan/light blue, 3 magenta/pink).
  A sprite whose DMA slots the bitplane fetch takes is missing or broken.

WinUAE (A1200, 2026-10-26) with the first version (four bands $38, $24,
$30, $18): all eight sprites at $38; sprites 0-3 at $24 and $30; sprite 0
at $18; the data starts at the same place for $30 and $38 (on sprite 0's
left edge) and earlier for $24 and $18, the white lines in the same
columns in every band.

Compare: cargo run -p emu-headless -- --machine amiga --model 1200 --rom-dir roms --disk agatest.adf --frames 1200 --snapshot aga.png

usage: python tools/amiga_agatest.py OUT.adf [FMODE]"""
import importlib.util
import os
import struct
import sys

spec = importlib.util.spec_from_file_location(
    "bt", os.path.join(os.path.dirname(os.path.abspath(__file__)), "amiga_bustest.py"))
bt = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bt)
Asm, D, A, POSTINC, D16, ABSL, IMM = bt.Asm, bt.D, bt.A, bt.POSTINC, bt.D16, bt.ABSL, bt.IMM
CODE, CUSTOM = bt.CODE, bt.CUSTOM

COP = 0x40000
SPR = 0x50000      # sprite n's list at SPR + n * $1000
P1 = 0x60000       # 32 KB each
P2 = 0x68000
ZERO = 0x70000
COPWORDS = CODE + 0x180

FIRST = 0x2C
HEIGHT = 16
SPRITE_LINES = 10
# Descending: a sprite that loses its DMA slots in one band misses its next
# control words and stays off below, where the fetch starts earlier anyway.
DDF = list(range(0x48, 0x14, -4))
FMODE = int(sys.argv[2], 16) if len(sys.argv) > 2 else 0xF

cop = [0x1FC, FMODE, 0x106, 0x0080, 0x10C, 0x0011,
       0x100, 0x8211, 0x102, 0x0000, 0x104, 0x0024, 0x108, 0x0000, 0x10A, 0x0000,
       0x08E, 0x2C71, 0x090, 0x2CD1, 0x092, DDF[0], 0x094, 0x00D0]
for i, p in enumerate([P1, P2] + [ZERO] * 6):
    cop += [0xE0 + 4 * i, p >> 16, 0xE2 + 4 * i, p & 0xFFFF]
for n in range(8):
    s = SPR + 0x1000 * n
    cop += [0x120 + 4 * n, s >> 16, 0x122 + 4 * n, s & 0xFFFF]
cop += [0x180, 0x0222, 0x182, 0x0888, 0x184, 0x000A, 0x186, 0x0FFF]
for i, c in enumerate([0xF00, 0xF80, 0xFF0, 0x0F0, 0x0FF, 0x08F, 0xF0F, 0xF8A]):
    cop += [0x1A2 + 4 * i, c]
for b, ddf in enumerate(DDF[1:], 1):
    cop += [((FIRST + HEIGHT * b - 2) << 8) | 0x01, 0xFFFE, 0x092, ddf]
cop += [0xFFFF, 0xFFFE]
assert FIRST + HEIGHT * len(DDF) <= 0x100

a = Asm(CODE)
a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x9A))
a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x96))
# Planes: P1 one pixel per 64 bits, P2 all ones, ZERO empty.
a.lea(ABSL(P1), 0)
a.move("w", IMM(0x0FFF, "w"), D(0))
a.label("p1")
a.move("l", IMM(0x80000000, "l"), POSTINC(0))
a.move("l", IMM(0, "l"), POSTINC(0))
a.dbra(0, "p1")
a.move("w", IMM(0x1FFF, "w"), D(0))
a.label("p2")
a.move("l", IMM(0xFFFFFFFF, "l"), POSTINC(0))
a.dbra(0, "p2")
a.move("w", IMM(0x1FFF, "w"), D(0))
a.label("z")
a.move("l", IMM(0, "l"), POSTINC(0))
a.dbra(0, "z")
# Sprites: per band POS, 3 pad words, CTL, 3 pad words, then the lines of
# DATA (64 bits) and DATB (64 bits); a zero control pair ends the list.
# POS = VSTART << 8 | HSTART >> 1 (HSTART $80 + $20 n), CTL = VSTOP << 8.
a.lea(ABSL(SPR), 0)
a.moveq(0, 4)       # DATB of even sprites
a.moveq(-1, 3)      # DATB of odd sprites
a.move("w", IMM(((FIRST + 3) << 8) | 0x40, "w"), D(1))
a.moveq(7, 7)
a.label("spr")
a.movea("l", A(0), 2)
a.move("w", D(1), D(2))
a.move("w", IMM((FIRST + 3 + SPRITE_LINES) << 8, "w"), D(0))
a.moveq(len(DDF) - 1, 6)
a.label("band")
a.move("w", D(2), POSTINC(2))
a.move("w", IMM(0, "w"), POSTINC(2))
a.move("l", IMM(0, "l"), POSTINC(2))
a.move("w", D(0), POSTINC(2))
a.move("w", IMM(0, "w"), POSTINC(2))
a.move("l", IMM(0, "l"), POSTINC(2))
a.moveq(SPRITE_LINES - 1, 5)
a.label("line")
a.move("l", IMM(0xFFFFFFFF, "l"), POSTINC(2))
a.move("l", IMM(0xFFFFFFFF, "l"), POSTINC(2))
a.move("l", D(4), POSTINC(2))
a.move("l", D(4), POSTINC(2))
a.dbra(5, "line")
a.addi("w", HEIGHT << 8, D(2))
a.addi("w", HEIGHT << 8, D(0))
a.dbra(6, "band")
for _ in range(4):
    a.move("l", IMM(0, "l"), POSTINC(2))
a.lea(D16(0x1000, 0), 0)
a.addi("w", 0x10, D(1))
a.move("l", D(4), D(5))
a.move("l", D(3), D(4))
a.move("l", D(5), D(3))
a.dbra(7, "spr")
# Copper list.
a.lea(ABSL(COP), 0)
a.lea(ABSL(COPWORDS), 1)
a.move("w", IMM(len(cop) - 1, "w"), D(0))
a.label("cp")
a.move("w", POSTINC(1), POSTINC(0))
a.dbra(0, "cp")
a.move("l", IMM(COP, "l"), ABSL(CUSTOM + 0x80))
a.move("w", IMM(0, "w"), ABSL(CUSTOM + 0x88))
a.move("w", IMM(0x83A0, "w"), ABSL(CUSTOM + 0x96))  # DMAEN BPLEN COPEN SPREN
a.label("halt")
a.bra("halt")
code = bytearray(a.finish())
assert len(code) <= COPWORDS - CODE, hex(len(code))
code += bytes(COPWORDS - CODE - len(code))
code += b"".join(struct.pack(">H", w) for w in cop)
code += bytes(-len(code) % 4)

boot = bt.build_boot(bytes(code))
adf = bytearray(901120)
adf[0:4] = b"DOS\0"
adf[8:12] = struct.pack(">I", 880)
adf[12:12 + len(boot)] = boot
off = 12 + len(boot)
assert off + len(code) <= 1024, (off, len(code))
adf[off:off + len(code)] = code
s = 0
for i in range(0, 1024, 4):
    s += struct.unpack(">I", adf[i:i + 4])[0]
    s = (s & 0xFFFFFFFF) + (s >> 32)
adf[4:8] = struct.pack(">I", (~s) & 0xFFFFFFFF)
open(sys.argv[1], "wb").write(adf)
print("ok", len(code))
