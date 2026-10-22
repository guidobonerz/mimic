"""Build bustest.adf: a boot block that measures how fast the 68000 runs a
chip-RAM-bound loop under display DMA, with the CIA-B timer A (E clock,
709379 Hz on PAL: 1 tick = 5 colour clocks). Boot it on real hardware or in
another emulator and read the four numbers off the screen; the test
`chip_bus_arbitration_matches_the_bus_test` (tests/tests/amiga_boot.rs)
boots tests/data/amiga_bustest.bootblock (the second output file) in mimic
and compares. Reference from a cycle-exact WinUAE A500 (2026-10-22): 5539,
6DD3, 7196, 5EC9. Changing anything before a measurement shifts the loop's
start phase and the counts by up to 1 %: keep the program as it is.

Four rows of 4 hex digits (E-clock ticks for 600 iterations of 16×
MOVE.W 0(A1,D1.W),4(A0) + DBRA, started at line 1 of a frame):
  row 0: bitplane DMA off, Copper on (pointer setup only)
  row 1: 5 lowres planes (DDFSTRT $30, DDFSTOP $D8, lines $1C–$133), no
         per-line Copper MOVEs
  row 2: as row 1 plus a Copper WAIT + 5 MOVEs on every line (Desert Dream's
         white-disk screen)
  row 3: as row 2 with 4 planes

usage: python tools/amiga_bustest.py OUT.adf [OUT.bootblock]
"""
import struct
import sys

# --- a minimal 68000 assembler for the subset used below ------------------

D = lambda n: ("ea", 0, n, [])
A = lambda n: ("ea", 1, n, [])
IND = lambda n: ("ea", 2, n, [])
POSTINC = lambda n: ("ea", 3, n, [])
PREDEC = lambda n: ("ea", 4, n, [])


def D16(disp, an):
    return ("ea", 5, an, [disp & 0xFFFF])


def IDX(d8, an, xn, xa=False, long=False):
    ext = (int(xa) << 15) | (xn << 12) | (int(long) << 11) | (d8 & 0xFF)
    return ("ea", 6, an, [ext])


def ABSL(addr):
    return ("ea", 7, 1, [(addr >> 16) & 0xFFFF, addr & 0xFFFF])


def IMM(v, size):
    if size == "l":
        return ("ea", 7, 4, [(v >> 16) & 0xFFFF, v & 0xFFFF])
    return ("ea", 7, 4, [v & 0xFFFF])


SIZE_MOVE = {"b": 1, "w": 3, "l": 2}
SIZE2 = {"b": 0, "w": 1, "l": 2}


class Asm:
    def __init__(self, base):
        self.base = base
        self.words = []
        self.labels = {}
        self.fixups = []  # (word index, label, kind)

    @property
    def pc(self):
        return self.base + 2 * len(self.words)

    def label(self, name):
        self.labels[name] = self.pc

    def emit(self, *ws):
        self.words.extend(w & 0xFFFF for w in ws)

    def ea(self, op):
        _, mode, reg, ext = op
        return (mode << 3) | reg, ext

    def move(self, size, src, dst):
        s, sx = self.ea(src)
        d, dx = self.ea(dst)
        dmode, dreg = d >> 3, d & 7
        self.emit((SIZE_MOVE[size] << 12) | (dreg << 9) | (dmode << 6) | s, *sx, *dx)

    def movea(self, size, src, an):
        self.move(size, src, A(an))

    def moveq(self, v, dn):
        self.emit(0x7000 | (dn << 9) | (v & 0xFF))

    def lea(self, src, an):
        s, sx = self.ea(src)
        self.emit(0x41C0 | (an << 9) | s, *sx)

    def addq(self, size, v, dst):
        d, dx = self.ea(dst)
        self.emit(0x5000 | ((v & 7) << 9) | (SIZE2[size] << 6) | d, *dx)

    def subq(self, size, v, dst):
        d, dx = self.ea(dst)
        self.emit(0x5100 | ((v & 7) << 9) | (SIZE2[size] << 6) | d, *dx)

    def addi(self, size, v, dst):
        d, dx = self.ea(dst)
        self.emit(0x0600 | (SIZE2[size] << 6) | d, *IMM(v, size)[3], *dx)

    def andi(self, size, v, dst):
        d, dx = self.ea(dst)
        self.emit(0x0200 | (SIZE2[size] << 6) | d, *IMM(v, size)[3], *dx)

    def ori(self, size, v, dst):
        d, dx = self.ea(dst)
        self.emit(0x0000 | (SIZE2[size] << 6) | d, *IMM(v, size)[3], *dx)

    def cmpi(self, size, v, dst):
        d, dx = self.ea(dst)
        self.emit(0x0C00 | (SIZE2[size] << 6) | d, *IMM(v, size)[3], *dx)

    def sub_dd(self, src_dn, dst_dn):  # SUB.W Dsrc,Ddst
        self.emit(0x9040 | (dst_dn << 9) | src_dn)

    def btst_imm(self, bit, dst):
        d, dx = self.ea(dst)
        self.emit(0x0800 | d, bit, *dx)

    def swap(self, dn):
        self.emit(0x4840 | dn)

    def lsr_w(self, n, dn):
        self.emit(0xE048 | ((n & 7) << 9) | dn)

    def lsl_w(self, n, dn):
        self.emit(0xE148 | ((n & 7) << 9) | dn)

    def rol_w(self, n, dn):
        self.emit(0xE158 | ((n & 7) << 9) | dn)

    def tst(self, size, dst):
        d, dx = self.ea(dst)
        self.emit(0x4A00 | (SIZE2[size] << 6) | d, *dx)

    def jmp(self, addr):
        self.emit(0x4EF9, addr >> 16, addr & 0xFFFF)

    def bsr(self, label):
        self.fixups.append((len(self.words) + 1, label, "w"))
        self.emit(0x6100, 0)

    def rts(self):
        self.emit(0x4E75)

    def nop(self):
        self.emit(0x4E71)

    def bcc(self, cc, label):
        self.fixups.append((len(self.words), label, "s"))
        self.emit(0x6000 | (cc << 8))

    bra = lambda self, l: self.bcc(0, l)
    bne = lambda self, l: self.bcc(6, l)
    beq = lambda self, l: self.bcc(7, l)

    def dbra(self, dn, label):
        self.fixups.append((len(self.words) + 1, label, "w"))
        self.emit(0x51C8 | dn, 0)

    def data(self, bs):
        assert len(bs) % 2 == 0
        for i in range(0, len(bs), 2):
            self.emit((bs[i] << 8) | bs[i + 1])

    def finish(self):
        for idx, label, kind in self.fixups:
            target = self.labels[label]
            if kind == "s":
                here = self.base + 2 * idx + 2
                disp = target - here
                assert -128 <= disp < 128 and disp != 0, (label, disp)
                self.words[idx] |= disp & 0xFF
            else:
                here = self.base + 2 * idx
                disp = target - here
                assert -32768 <= disp < 32768
                self.words[idx] = disp & 0xFFFF
        return b"".join(struct.pack(">H", w) for w in self.words)


# --- the program ------------------------------------------------------------

CODE = 0x30000
COP_FULL = 0x40000
COP_HEAD = 0x42000
LOOPMEM = 0x50000
PLANES = 0x60000
ROWBYTES = 44
PLANE_SIZE = ROWBYTES * 280
CUSTOM = 0xDFF000
CIAB_CRA = 0xBFDE00
CIAB_TALO = 0xBFD400
CIAB_TAHI = 0xBFD500
CIAB_ICR = 0xBFDD00
CIAA_ICR = 0xBFED01
ITER = 600

FONT = {  # 8x8, one byte per row
    0: [0x3C, 0x66, 0x6E, 0x76, 0x66, 0x66, 0x3C, 0],
    1: [0x18, 0x38, 0x18, 0x18, 0x18, 0x18, 0x3C, 0],
    2: [0x3C, 0x66, 0x06, 0x1C, 0x30, 0x60, 0x7E, 0],
    3: [0x3C, 0x66, 0x06, 0x1C, 0x06, 0x66, 0x3C, 0],
    4: [0x0C, 0x1C, 0x3C, 0x6C, 0x7E, 0x0C, 0x0C, 0],
    5: [0x7E, 0x60, 0x7C, 0x06, 0x06, 0x66, 0x3C, 0],
    6: [0x1C, 0x30, 0x60, 0x7C, 0x66, 0x66, 0x3C, 0],
    7: [0x7E, 0x06, 0x0C, 0x18, 0x30, 0x30, 0x30, 0],
    8: [0x3C, 0x66, 0x66, 0x3C, 0x66, 0x66, 0x3C, 0],
    9: [0x3C, 0x66, 0x66, 0x3E, 0x06, 0x0C, 0x38, 0],
    10: [0x18, 0x3C, 0x66, 0x66, 0x7E, 0x66, 0x66, 0],
    11: [0x7C, 0x66, 0x66, 0x7C, 0x66, 0x66, 0x7C, 0],
    12: [0x3C, 0x66, 0x60, 0x60, 0x60, 0x66, 0x3C, 0],
    13: [0x78, 0x6C, 0x66, 0x66, 0x66, 0x6C, 0x78, 0],
    14: [0x7E, 0x60, 0x60, 0x7C, 0x60, 0x60, 0x7E, 0],
    15: [0x7E, 0x60, 0x60, 0x7C, 0x60, 0x60, 0x60, 0],
}


def build_main():
    a = Asm(CODE)
    # Take the machine over.
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x9A))  # INTENA
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x96))  # DMACON
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x9C))  # INTREQ
    a.move("b", IMM(0x7F, "b"), ABSL(CIAB_ICR))
    a.move("b", IMM(0x7F, "b"), ABSL(CIAA_ICR))
    a.lea(ABSL(CUSTOM), 6)
    # Clear the planes and the loop memory.
    a.lea(ABSL(LOOPMEM), 0)
    a.move("w", IMM((PLANES + 5 * PLANE_SIZE - LOOPMEM) // 4 - 1, "w"), D(0))
    a.label("clr")
    a.move("l", IMM(0, "l"), POSTINC(0))
    a.dbra(0, "clr")
    # Copper lists: the head table into both, the per-line part into one.
    for base in (COP_FULL, COP_HEAD):
        a.lea(ABSL(base), 0)
        a.lea(ABSL(CODE + 0x300), 1)  # head table (filled in below)
        a.moveq(HEAD_WORDS - 1, 0)
        a.label(f"hd{base}")
        a.move("w", POSTINC(1), POSTINC(0))
        a.dbra(0, f"hd{base}")
        if base == COP_HEAD:
            a.move("l", IMM(0xFFFFFFFE, "l"), IND(0))
        else:
            # Per line $1C..$FF: WAIT line,$DF then 5 MOVEs (COLOR06..10).
            a.move("w", IMM(0x1C, "w"), D(1))
            a.label("cl")
            a.move("w", D(1), D(2))
            a.lsl_w(8, 2)
            a.ori("w", 0x00DF, D(2))
            a.move("w", D(2), POSTINC(0))
            a.move("w", IMM(0xFFFE, "w"), POSTINC(0))
            a.moveq(4, 3)
            a.move("w", IMM(0x18C, "w"), D(4))
            a.label("cm")
            a.move("w", D(4), POSTINC(0))
            a.move("w", IMM(0x0888, "w"), POSTINC(0))
            a.addq("w", 2, D(4))
            a.dbra(3, "cm")
            a.addq("w", 1, D(1))
            a.cmpi("w", 0x100, D(1))
            a.bne("cl")
            a.move("l", IMM(0xFFFFFFFE, "l"), IND(0))
    # CIA-B timer A: continuous from $FFFF, E clock.
    a.move("b", IMM(0, "b"), ABSL(CIAB_CRA))
    a.move("b", IMM(0xFF, "b"), ABSL(CIAB_TALO))
    a.move("b", IMM(0xFF, "b"), ABSL(CIAB_TAHI))
    a.move("b", IMM(0x01, "b"), ABSL(CIAB_CRA))
    # Tests from the table at CODE+$380: BPLCON0, DMACON, COP1LC (3 longs).
    a.lea(ABSL(CODE + 0x380), 5)
    a.moveq(0, 6)
    a.label("tests")
    a.move("w", POSTINC(5), D16(0x100, 6))
    a.move("w", POSTINC(5), D(0))
    a.move("l", POSTINC(5), D16(0x80, 6))
    a.move("w", IMM(0x7FFF, "w"), D16(0x96, 6))
    a.move("w", D(0), D16(0x96, 6))
    a.bsr("measure")
    a.bsr("show")
    a.addq("w", 1, D(6))
    a.cmpi("w", 4, D(6))
    a.bne("tests")
    # Show everything with the full list and stop.
    a.move("w", IMM(0x5200, "w"), D16(0x100, 6))
    a.move("l", IMM(COP_FULL, "l"), D16(0x80, 6))
    a.move("w", IMM(0x8380, "w"), D16(0x96, 6))
    a.label("halt")
    a.bra("halt")

    # measure: wait for the start of a frame, run the loop, D0 = ticks.
    a.label("measure")
    a.label("w0")  # leave line 0 first
    a.move("w", D16(0x06, 6), D(0))
    a.lsr_w(8, 0)
    a.beq("w0")
    a.label("w1")  # then wait for line 0 (VHPOSR high byte 0, VPOSR bit 0 clear)
    a.move("w", D16(0x04, 6), D(0))
    a.andi("w", 1, D(0))
    a.bne("w1")
    a.move("w", D16(0x06, 6), D(0))
    a.lsr_w(8, 0)
    a.bne("w1")
    a.bsr("timer")
    a.move("w", D(0), D(2))
    a.lea(ABSL(LOOPMEM), 0)
    a.lea(ABSL(LOOPMEM + 0x100), 1)
    a.moveq(0, 1)
    a.move("w", IMM(ITER - 1, "w"), D(7))
    a.label("loop")
    for _ in range(16):
        a.move("w", IDX(0, 1, 1), D16(4, 0))
    a.dbra(7, "loop")
    a.bsr("timer")
    a.sub_dd(0, 2)  # D2 = start - end = ticks
    a.move("w", D(2), D(0))
    a.rts()

    # timer: D0 = TAHI:TALO (re-read while the high byte changes).
    a.label("timer")
    a.move("b", ABSL(CIAB_TAHI), D(0))
    a.move("b", ABSL(CIAB_TALO), D(3))
    a.move("b", ABSL(CIAB_TAHI), D(4))
    a.emit(0xB800)  # CMP.B D0,D4
    a.bne("timer")
    a.lsl_w(8, 0)
    a.andi("w", 0x00FF, D(3))
    a.emit(0x8043)  # OR.W D3,D0
    a.rts()

    # show: draw D0 as 4 hex digits in plane 1 at row 24 + 16*D6, column 4.
    a.label("show")
    a.move("w", D(0), D(5))
    a.move("w", D(6), D(1))
    a.lsl_w(4, 1)
    a.addi("w", 24, D(1))
    a.move("w", D(1), D(2))  # D2 = row * 44 = row*32 + row*8 + row*4
    a.lsl_w(5, 2)
    a.move("w", D(1), D(3))
    a.lsl_w(3, 3)
    a.emit(0xD443)  # ADD.W D3,D2
    a.move("w", D(1), D(3))
    a.lsl_w(2, 3)
    a.emit(0xD443)  # ADD.W D3,D2
    a.lea(ABSL(PLANES + 4), 0)
    a.emit(0xD0C2)  # ADDA.W D2,A0
    a.moveq(3, 7)
    a.label("digit")
    a.rol_w(4, 5)
    a.move("w", D(5), D(1))
    a.andi("w", 0x000F, D(1))
    a.lsl_w(3, 1)
    a.lea(ABSL(CODE + 0x280), 1)  # font
    a.emit(0xD2C1)  # ADDA.W D1,A1
    a.movea("l", A(0), 2)
    a.moveq(7, 4)
    a.label("row")
    a.move("b", POSTINC(1), IND(2))
    a.lea(D16(ROWBYTES, 2), 2)
    a.dbra(4, "row")
    a.addq("l", 1, A(0))
    a.dbra(7, "digit")
    a.rts()
    code = bytearray(a.finish())
    assert len(code) <= 0x280, len(code)
    code += bytes(0x280 - len(code))
    for i in range(16):  # font at +$280 (128 bytes)
        code += bytes(FONT[i])
    # head table at +$300
    head = []
    for p in range(5):
        pt = PLANES + p * PLANE_SIZE
        head += [0xE0 + 4 * p, pt >> 16, 0xE2 + 4 * p, pt & 0xFFFF]
    head += [0x8E, 0x1C71, 0x90, 0x34D1, 0x92, 0x0030, 0x94, 0x00D8,
             0x102, 0, 0x104, 0, 0x108, 0, 0x10A, 0,
             0x180, 0x0024, 0x182, 0x0FFF, 0x184, 0x0F00, 0x186, 0x00F0,
             0x188, 0x000F, 0x18A, 0x0FF0]
    assert len(head) == HEAD_WORDS
    code += b"".join(struct.pack(">H", w) for w in head)
    code += bytes(CODE + 0x380 - CODE - len(code))
    # tests at +$380: BPLCON0, DMACON, COP1LC
    for con0, dma, cop in [(0x0200, 0x8280, COP_HEAD), (0x5200, 0x8380, COP_HEAD),
                           (0x5200, 0x8380, COP_FULL), (0x4200, 0x8380, COP_FULL)]:
        code += struct.pack(">HHI", con0, dma, cop)
    return bytes(code)


HEAD_WORDS = 20 + 8 + 8 + 12


def build_boot(main):
    # Boot block: copy the program to CODE and jump there.
    b = Asm(0)
    b.lea(D16(0, 0), 0)  # placeholder, patched below to LEA prog(PC),A0
    b.lea(ABSL(CODE), 1)
    b.move("w", IMM(len(main) // 4 - 1, "w"), D(0))
    b.label("copy")
    b.move("l", POSTINC(0), POSTINC(1))
    b.dbra(0, "copy")
    b.jmp(CODE)
    boot = bytearray(b.finish())
    # LEA d16(PC),A0: 0x41FA, disp from the extension word's address.
    prog_off = 12 + len(boot)
    boot[0:4] = struct.pack(">HH", 0x41FA, (prog_off - (12 + 2)) & 0xFFFF)
    return bytes(boot)


def main():
    out = sys.argv[1]
    prog = build_main()
    assert len(prog) % 4 == 0
    boot = build_boot(prog)
    adf = bytearray(901120)
    adf[0:4] = b"DOS\0"
    adf[8:12] = struct.pack(">I", 880)
    adf[12:12 + len(boot)] = boot
    off = 12 + len(boot)
    # The program follows the boot code; it must stay within the 1 KB boot
    # block … it does not (about 2 KB), so it lives in the next sectors and
    # the boot code copies from there: the whole 2 blocks are loaded by
    # Kickstart (the boot block is 1024 bytes = 2 sectors). Put the program
    # into sectors 2–5 and read them? Simpler: Kickstart loads only the boot
    # block, so keep the program small enough or load via trackdisk. We
    # instead store the program right after the boot code and require the
    # total to fit in 1024 bytes.
    assert off + len(prog) <= 1024, (off, len(prog))
    adf[off:off + len(prog)] = prog
    s = 0
    for i in range(0, 1024, 4):
        s += struct.unpack(">I", adf[i:i + 4])[0]
        s = (s & 0xFFFFFFFF) + (s >> 32)
    adf[4:8] = struct.pack(">I", (~s) & 0xFFFFFFFF)
    open(out, "wb").write(adf)
    if len(sys.argv) > 2:
        open(sys.argv[2], "wb").write(adf[:1024])
    print("program bytes", len(prog), "boot code", len(boot))


if __name__ == "__main__":
    main()
