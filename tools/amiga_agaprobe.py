"""Build an AGA picture probe disk for a real A1200, WinUAE and mimic.

Like tools/amiga_agatest.py (whose layout it shares) the picture measures
itself: in each band of 16 lines from line $2C the eight sprites stand
side by side from HSTART $80 as a ruler (64 high-resolution pixels each,
10 lines high; pair 0 red/orange, 1 yellow/green, 2 cyan/light blue,
3 magenta/pink), and the bitplane data shows against them. Only one thing
changes from band to band, set by the Copper two lines before the band.
Photograph the screen of the real machine; compare with

  python tools/amiga_winuae.py probe.adf winuae.png --shot 25 --model a1200
  cargo run -p emu-headless -- --machine amiga --model 1200 --disk probe.adf --frames 600 --snapshot mimic.png

While it sets up, the CPU shows its progress in COLOR00: red (running,
DMA off), yellow (planes filled), blue (sprite lists built), magenta
(Copper list copied); the Copper's list then sets its own COLOR00. A
screen stuck in one of these colours tells how far a real machine got.

Cases (`python tools/amiga_agaprobe.py OUT.adf CASE`):

- `scroll`: BPLCON1's AGA scroll bits with FMODE $F (64-bit fetches),
  eight high-resolution planes, DDFSTRT $38. Plane 1 (odd: PF1H) holds one
  set pixel per 64-bit word, plane 2 (even: PF2H) is all ones: a blue
  area (COLOR02) with white lines (COLOR03) where both are set. Bands:
  BPLCON1 $0000, $0011 (1 lores pixel), $00FF (15), $4400 (16), $8800 (32,
  BigTime's AXIS picture), $CC00 (48), $CCFF (63), $8000 (PF2H7 alone: the
  blue area moves, the lines' plane 1 not), $0800 (PF1H7 alone). Where the
  scrolled data starts and what shows left of it (COLOR00 grey or data
  fetched before) is the question; mimic shows a COLOR00 band.
- `bypass`: BPLCON0 bit 5 (BYPASS) off and on in turn (four bands), the
  `scroll` picture otherwise. The AGA documentation says it sends the
  plane bits past the colour table (for a genlock); what a plain A1200's
  video output shows then is the question.
- `scan`: scan doubling, two low-resolution planes with 16-bit fetches
  (DDFSTRT $38, DDFSTOP $D0: 40 bytes a line), sprites 64 bits wide.
  Plane 1's lines alternate all ones and all zeros (yellow and grey
  stripes one line high), the sprites' lines alternate set and clear.
  Bands: FMODE $000C (no doubling: one-line stripes); $400C (BSCAN2) with
  BPL1MOD -40, BPL2MOD 0; $400C with BPL1MOD 0, BPL2MOD -40; $800C (SSCAN2:
  the sprites' lines doubled?); $C00C with BPL1MOD -40. With BSCAN2 the
  stripes should turn two lines high in one of the two modulo bands (the
  one whose repeating modulo is used on the first line of each pair).

- `ddf`: tools/amiga_agatest.py's 13 bands (DDFSTRT $48, $44 … $18) on a
  green COLOR00, to see on a real A1200 whether the Copper runs at all
  when agatest's screen stays black.

- `ddf-high`, `ddf-low`: the halves of `ddf` (DDFSTRT $48–$30 and
  $2C–$18), to find the value that turns the real A1200's screen black.

- `planes`: which plane's data lands where when DDFSTRT is $3C (the
  garbage the A1200 shows there): eight high-resolution planes, FMODE $F,
  no sprites; in every 64-pixel word plane k holds pixels 8k to 8k + 7, in
  its own colour (planes 1-8: red, green, blue, yellow, cyan, magenta,
  orange, pink; two planes on one pixel make it white, none dark grey).
  Three bands of 64 lines: DDFSTRT $40 (clean), $3C, $40 again (does the
  disorder last?). The planes lie at $100000 + $8000 k.

usage: python tools/amiga_agaprobe.py OUT.adf scroll|bypass|scan|ddf|ddf-high|ddf-low|planes
"""
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
COPWORDS = CODE + 0x1C0

FIRST = 0x2C
HEIGHT = 16
SPRITE_LINES = 10


def case(name):
    """(initial registers, bands: the registers each band sets, scan?)"""
    hires8 = {0x1FC: 0x000F, 0x100: 0x8211, 0x102: 0x0000, 0x092: 0x0038, 0x094: 0x00D0,
              0x108: 0, 0x10A: 0}
    if name == "scroll":
        values = [0x0000, 0x0011, 0x00FF, 0x4400, 0x8800, 0xCC00, 0xCCFF, 0x8000, 0x0800]
        return hires8, [[(0x102, v)] for v in values], False
    if name == "bypass":
        return hires8, [[(0x100, v)] for v in [0x8211, 0x8231, 0x8211, 0x8231]], False
    if name == "ddf":
        # agatest's bands (DDFSTRT $48 down to $18) on a green COLOR00: a
        # green screen shows the Copper ran.
        init = {**hires8, 0x092: 0x0048, 0x180: 0x0060}
        return init, [[(0x092, d)] for d in range(0x48, 0x14, -4)], False
    if name in ("ddf-high", "ddf-low"):
        # Halves of `ddf`, to find the DDFSTRT that blanks the real A1200.
        values = range(0x48, 0x2C, -4) if name == "ddf-high" else range(0x2C, 0x14, -4)
        values = list(values)
        init = {**hires8, 0x092: values[0], 0x180: 0x0060}
        return init, [[(0x092, d)] for d in values], False
    if name == "scan":
        init = {0x1FC: 0x000C, 0x100: 0x2200, 0x102: 0, 0x092: 0x0038, 0x094: 0x00D0,
                0x108: 0, 0x10A: 0}
        m40 = 0x10000 - 40
        bands = [[(0x1FC, 0x000C), (0x108, 0), (0x10A, 0)],
                 [(0x1FC, 0x400C), (0x108, m40), (0x10A, 0)],
                 [(0x1FC, 0x400C), (0x108, 0), (0x10A, m40)],
                 [(0x1FC, 0x800C), (0x108, 0), (0x10A, 0)],
                 [(0x1FC, 0xC00C), (0x108, m40), (0x10A, 0)]]
        return init, bands, True
    raise SystemExit(__doc__)


def build(name):
    init, bands, scan = case(name)
    cop = []
    for reg in [0x1FC, 0x106, 0x10C, 0x100, 0x102, 0x104, 0x108, 0x10A, 0x08E, 0x090, 0x092, 0x094]:
        value = {0x106: 0x0080, 0x10C: 0x0011, 0x104: 0x0024, 0x08E: 0x2C71, 0x090: 0x2CD1}.get(reg)
        cop += [reg, init.get(reg, value)]
    for reg, v in bands[0]:
        cop += [reg, v]
    for i, p in enumerate([P1, P2] + [ZERO] * 6):
        cop += [0xE0 + 4 * i, p >> 16, 0xE2 + 4 * i, p & 0xFFFF]
    for n in range(8):
        s = SPR + 0x1000 * n
        cop += [0x120 + 4 * n, s >> 16, 0x122 + 4 * n, s & 0xFFFF]
    cop += [0x180, init.get(0x180, 0x0222), 0x182, 0x0FF0 if scan else 0x0888, 0x184, 0x000A, 0x186, 0x0FFF]
    for i, c in enumerate([0xF00, 0xF80, 0xFF0, 0x0F0, 0x0FF, 0x08F, 0xF0F, 0xF8A]):
        cop += [0x1A2 + 4 * i, c]
    for b, regs in enumerate(bands[1:], 1):
        cop += [((FIRST + HEIGHT * b - 2) << 8) | 0x01, 0xFFFE]
        for reg, v in regs:
            cop += [reg, v]
    cop += [0xFFFF, 0xFFFE]
    assert FIRST + HEIGHT * len(bands) <= 0x100

    a = Asm(CODE)
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x9A))
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x96))
    # Stage colours, set by the CPU (the Copper's list sets COLOR00 once it
    # runs): red = running with DMA off, yellow = planes filled, blue =
    # sprite lists built, magenta = Copper list copied.
    a.move("w", IMM(0x0F00, "w"), ABSL(CUSTOM + 0x180))
    a.lea(ABSL(P1), 0)
    if scan:
        # Plane 1: lines of 40 bytes, all ones and all zeros in turn.
        a.move("w", IMM(32768 // 80 - 1, "w"), D(0))
        a.label("p1")
        a.moveq(9, 1)
        a.label("p1a")
        a.move("l", IMM(0xFFFFFFFF, "l"), POSTINC(0))
        a.dbra(1, "p1a")
        a.moveq(9, 1)
        a.label("p1b")
        a.move("l", IMM(0, "l"), POSTINC(0))
        a.dbra(1, "p1b")
        a.dbra(0, "p1")
        a.move("w", IMM(0x3FFF, "w"), D(0))  # plane 2 and the rest: empty
        a.label("p2")
        a.move("l", IMM(0, "l"), POSTINC(0))
        a.dbra(0, "p2")
    else:
        # Plane 1: one pixel per 64 bits; plane 2 all ones; ZERO empty.
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
    a.move("w", IMM(0x0FF0, "w"), ABSL(CUSTOM + 0x180))
    # Sprites: per band POS, 3 pad words, CTL, 3 pad words, then the lines
    # of DATA (64 bits) and DATB (64 bits); a zero control pair ends the
    # list. POS = VSTART << 8 | HSTART >> 1 (HSTART $80 + $20 n).
    a.lea(ABSL(SPR), 0)
    a.moveq(0, 4)       # DATB of even sprites
    a.moveq(-1, 3)      # DATB of odd sprites
    a.move("w", IMM(((FIRST + 3) << 8) | 0x40, "w"), D(1))
    a.moveq(7, 7)
    a.label("spr")
    a.movea("l", A(0), 2)
    a.move("w", D(1), D(2))
    a.move("w", IMM((FIRST + 3 + SPRITE_LINES) << 8, "w"), D(0))
    a.moveq(len(bands) - 1, 6)
    a.label("band")
    a.move("w", D(2), POSTINC(2))
    a.move("w", IMM(0, "w"), POSTINC(2))
    a.move("l", IMM(0, "l"), POSTINC(2))
    a.move("w", D(0), POSTINC(2))
    a.move("w", IMM(0, "w"), POSTINC(2))
    a.move("l", IMM(0, "l"), POSTINC(2))
    a.moveq(SPRITE_LINES // 2 - 1 if scan else SPRITE_LINES - 1, 5)
    a.label("line")
    a.move("l", IMM(0xFFFFFFFF, "l"), POSTINC(2))
    a.move("l", IMM(0xFFFFFFFF, "l"), POSTINC(2))
    a.move("l", D(4), POSTINC(2))
    a.move("l", D(4), POSTINC(2))
    if scan:
        # A clear line after each set one.
        for _ in range(4):
            a.move("l", IMM(0, "l"), POSTINC(2))
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
    a.move("w", IMM(0x000F, "w"), ABSL(CUSTOM + 0x180))
    a.lea(ABSL(COP), 0)
    a.lea(ABSL(COPWORDS), 1)
    a.move("w", IMM(len(cop) - 1, "w"), D(0))
    a.label("cp")
    a.move("w", POSTINC(1), POSTINC(0))
    a.dbra(0, "cp")
    a.move("w", IMM(0x0F0F, "w"), ABSL(CUSTOM + 0x180))
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
    return bytes(code)


PLANES8 = 0x100000   # `planes`: plane k at PLANES8 + k * $8000 (32 KB each)
# `planes`: the colour of each plane's own stripe (plane 1 first).
PLANE_COLOURS = [0xF00, 0x0F0, 0x00F, 0xFF0, 0x0FF, 0xF0F, 0xF80, 0xF8A]


def build_planes():
    """The `planes` case: eight planes with a stripe each, three bands."""
    bands = [0x40, 0x3C, 0x40]
    height = 64
    cop = [0x1FC, 0x000F, 0x106, 0x0080, 0x10C, 0x0011, 0x100, 0x8211, 0x102, 0, 0x104, 0x0024,
           0x108, 0, 0x10A, 0, 0x08E, 0x2C71, 0x090, 0x2CD1, 0x092, bands[0], 0x094, 0x00D0]
    for k in range(8):
        pt = PLANES8 + k * 0x8000
        cop += [0xE0 + 4 * k, pt >> 16, 0xE2 + 4 * k, pt & 0xFFFF]
    cop += [0x180, 0x0222]  # after the CPU's stage colours
    for b, ddf in enumerate(bands[1:], 1):
        cop += [((FIRST + height * b - 2) << 8) | 0x01, 0xFFFE, 0x092, ddf]
    cop += [0xFFFF, 0xFFFE]
    a = Asm(CODE)
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x9A))
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x96))
    a.move("w", IMM(0x0F00, "w"), ABSL(CUSTOM + 0x180))
    # Plane k: in every 64-bit word only byte k set.
    for k in range(8):
        v = 0xFF << (56 - 8 * k)
        a.lea(ABSL(PLANES8 + k * 0x8000), 0)
        a.move("l", IMM(v >> 32, "l"), D(1))
        a.move("l", IMM(v & 0xFFFFFFFF, "l"), D(2))
        a.move("w", IMM(0x8000 // 8 - 1, "w"), D(0))
        a.label(f"p{k}")
        a.move("l", D(1), POSTINC(0))
        a.move("l", D(2), POSTINC(0))
        a.dbra(0, f"p{k}")
    a.move("w", IMM(0x0FF0, "w"), ABSL(CUSTOM + 0x180))
    # The palette: all 256 white, then COLOR00 and the eight one-plane
    # colours (index 1 << k), through BPLCON3's bank (bits 15-13).
    a.moveq(7, 3)
    a.move("w", IMM(0xE000, "w"), D(4))
    a.label("bank")
    a.move("w", D(4), ABSL(CUSTOM + 0x106))
    a.lea(ABSL(CUSTOM + 0x180), 0)
    a.moveq(31, 0)
    a.label("white")
    a.move("w", IMM(0x0FFF, "w"), POSTINC(0))
    a.dbra(0, "white")
    a.addi("w", 0xE000, D(4))  # the next bank down (- $2000)
    a.dbra(3, "bank")
    fixed = [(0, 0x222)] + [(1 << k, c) for k, c in enumerate(PLANE_COLOURS)]
    for idx, c in fixed:
        a.move("w", IMM((idx >> 5) << 13, "w"), ABSL(CUSTOM + 0x106))
        a.move("w", IMM(c, "w"), ABSL(CUSTOM + 0x180 + 2 * (idx & 31)))
    a.move("w", IMM(0x0080, "w"), ABSL(CUSTOM + 0x106))
    a.move("w", IMM(0x000F, "w"), ABSL(CUSTOM + 0x180))
    a.lea(ABSL(COP), 0)
    a.lea(ABSL(CODE + 0x260), 1)
    a.move("w", IMM(len(cop) - 1, "w"), D(0))
    a.label("cp")
    a.move("w", POSTINC(1), POSTINC(0))
    a.dbra(0, "cp")
    a.move("w", IMM(0x0F0F, "w"), ABSL(CUSTOM + 0x180))
    a.move("l", IMM(COP, "l"), ABSL(CUSTOM + 0x80))
    a.move("w", IMM(0, "w"), ABSL(CUSTOM + 0x88))
    a.move("w", IMM(0x8380, "w"), ABSL(CUSTOM + 0x96))  # DMAEN BPLEN COPEN
    a.label("halt")
    a.bra("halt")
    code = bytearray(a.finish())
    assert len(code) <= 0x260, hex(len(code))
    code += bytes(0x260 - len(code))
    code += b"".join(struct.pack(">H", w) for w in cop)
    code += bytes(-len(code) % 4)
    return bytes(code)


def adf(code):
    boot = bt.build_boot(code)
    d = bytearray(901120)
    d[0:4] = b"DOS\0"
    d[8:12] = struct.pack(">I", 880)
    d[12:12 + len(boot)] = boot
    off = 12 + len(boot)
    assert off + len(code) <= 1024, (off, len(code))
    d[off:off + len(code)] = code
    s = 0
    for i in range(0, 1024, 4):
        s += struct.unpack(">I", d[i:i + 4])[0]
        s = (s & 0xFFFFFFFF) + (s >> 32)
    d[4:8] = struct.pack(">I", (~s) & 0xFFFFFFFF)
    return bytes(d)


def main():
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    code = build_planes() if sys.argv[2] == "planes" else build(sys.argv[2])
    open(sys.argv[1], "wb").write(adf(code))
    print("ok", len(code))


if __name__ == "__main__":
    main()
