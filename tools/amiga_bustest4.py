"""Build bustest4.adf: the 68000 working next to a busy Copper and the
Blitter, the load of Sanity's "Interference" (its textured cube). Companion
of tools/amiga_bustest.py (same assembler, timer, font and screen). The
program is larger than a boot block: the boot block reads it from the disk
(trackdisk CMD_READ with the IORequest Kickstart passes in A1) and jumps to
it. The test `cpu_blitter_copper_bus_timing_matches_the_bus_test`
(tests/tests/amiga_boot.rs) boots tests/data/amiga_bustest4.bin (boot
block + program) in mimic and compares.

Screen: 5 lowres planes, DDF $30-$D0. Copper lists like Interference's:
every line $1C-$FF is a block WAIT line,$CE; MOVE COP1LCL (next block);
MOVE COP2LCH/COP2LCL; MOVE COPJMP2, and the sub-list sets the five
BPLxPTL ("6": 5 MOVEs + COPJMP1) or BPLxPTL and COLOR01-COLOR30 ("36": 35
MOVEs + COPJMP1); the long sub-list's tail runs into the next line's
bitplane fetch, where it competes with the CPU for the free slots, and
ends just before that line's WAIT. "36alt": the long sub-list on even
lines only (the cube has it on every second or third line). "head" list:
the display registers only.

Each row: E-clock ticks (CIA-B timer A, 1 tick = 5 colour clocks) for a
loop started at line 0, shown as 4 hex digits. Loops: "noblit" = 400 x
the cube's blit-setup block (index arithmetic, five MOVE.W to memory,
BTST BBUSY wait, BLTAPT/BLTDPT, a store to NO-OP instead of BLTSIZE);
"blit" = the same with BLTSIZE: every block starts a 1 x 30-word A->D
blit (BLTDMOD 2).
  row 0-3: noblit with the head list, "6", "36alt", "36"
  row 4-7: blit with the head list, "6", "36alt", "36"

usage: python tools/amiga_bustest4.py OUT.adf [OUT.bin]
"""
import importlib.util
import os
import struct
import sys

_spec = importlib.util.spec_from_file_location(
    "amiga_bustest", os.path.join(os.path.dirname(os.path.abspath(__file__)), "amiga_bustest.py"))
bt = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(bt)

Asm, D, A, IND, POSTINC, PREDEC, D16, IDX, ABSL, IMM = (
    bt.Asm, bt.D, bt.A, bt.IND, bt.POSTINC, bt.PREDEC, bt.D16, bt.IDX, bt.ABSL, bt.IMM)
CODE, LOOPMEM, PLANES = bt.CODE, bt.LOOPMEM, bt.PLANES
ROWBYTES, PLANE_SIZE, CUSTOM = bt.ROWBYTES, bt.PLANE_SIZE, bt.CUSTOM
CIAB_CRA, CIAB_TALO, CIAB_TAHI, CIAB_ICR, CIAA_ICR = (
    bt.CIAB_CRA, bt.CIAB_TALO, bt.CIAB_TAHI, bt.CIAB_ICR, bt.CIAA_ICR)

LISTS = {"6": 0x40000, "36alt": 0x42000, "36": 0x44000, "head": 0x46000}
SUBS = {"6": 0x48000, "36": 0x48100}
LIST_HEAD = LISTS["head"]
BLIT_SRC = 0x08000  # read only: whatever Kickstart left there
BLIT_DST = 0x70000
DIVS_ITER = 120
BLIT_ITER = 400
NOOP = 0x1FE  # custom NO-OP register
FONT_AT = 0xF80  # program offset of the font; head and test table follow
HEAD_WORDS = 20 + 8 + 8 + 12 + 2
TESTS = [  # BPLCON0, DMACON, Copper list, loop
    (0x5200, 0x83C0, "head", "noblit"),
    (0x5200, 0x83C0, "6", "noblit"),
    (0x5200, 0x83C0, "36alt", "noblit"),
    (0x5200, 0x83C0, "36", "noblit"),
    (0x5200, 0x83C0, "head", "blit"),
    (0x5200, 0x83C0, "6", "blit"),
    (0x5200, 0x83C0, "36alt", "blit"),
    (0x5200, 0x83C0, "36", "blit"),
]
LOOPS = ["divs", "noblit", "blit"]


def blit_block(a, size_reg, tag):
    """One block of the cube's colour loop: A0 index table (zeros), A1/A2/A6
    reset from D2/D3/D4 each block, A3 long table (zeros), A4 blit source
    (+D7 per block), A5 = $DFF002."""
    a.moveq(0, 0)
    a.moveq(0, 1)
    a.move("w", POSTINC(0), D(0))
    a.move("w", D(0), D(1))
    a.emit(0xD040)                       # ADD.W D0,D0
    a.lea(IDX(0, 6, 0), 6)               # LEA 0(A6,D0.W),A6
    a.lsl_w(4, 0)
    a.lea(IDX(0, 1, 0), 1)               # LEA 0(A1,D0.W),A1
    a.emit(0xD241)                       # ADD.W D1,D1
    a.emit(0xD241)
    a.move("l", IDX(0, 3, 1), D(1))      # MOVE.L 0(A3,D1.W),D1
    a.emit(0xD5C1)                       # ADDA.L D1,A2
    a.move("l", A(2), D(0))
    a.move("w", IND(6), D(1))
    a.emit(0xD246)                       # ADD.W D6,D1
    a.emit(0xD27A, 0xFFFE)               # ADD.W *(PC),D1 (its own extension word)
    a.moveq(0x50, 5)
    for k in range(5):
        if k:
            a.emit(0xD245)               # ADD.W D5,D1
        a.move("w", D(1), D16(2 + 4 * k, 2))
    a.lea(D16(0x16, 2), 2)
    a.moveq(6, 1)
    a.label(f"bb_{tag}")
    a.emit(0x0315)                       # BTST D1,(A5)
    a.bne(f"bb_{tag}")
    a.move("l", A(4), D16(0x4E, 5))      # BLTAPT
    a.move("l", A(2), D16(0x52, 5))      # BLTDPT
    a.move("w", IMM(0x0781, "w"), D16(size_reg - 2, 5))  # BLTSIZE 30 x 1 or NO-OP
    a.move("w", D(0), D16(4, 1))
    a.move("w", D(0), IND(1))
    a.emit(0x2242)                       # MOVEA.L D2,A1
    a.emit(0x2443)                       # MOVEA.L D3,A2
    a.emit(0x2C44)                       # MOVEA.L D4,A6
    a.emit(0xD8C7)                       # ADDA.W D7,A4


def bcc_w(a, cc, label):
    """Bcc with a 16-bit displacement."""
    a.fixups.append((len(a.words) + 1, label, "w"))
    a.emit(0x6000 | (cc << 8), 0)


def divs_block(a):
    """The cube's perspective division; A0 reads code words as data."""
    a.move("w", POSTINC(0), D(1))
    a.move("w", POSTINC(0), D(3))
    a.emit(0x9640)                       # SUB.W D0,D3
    a.move("w", D(4), D(2))
    a.emit(0x9642)                       # SUB.W D2,D3
    a.swap(1)
    a.emit(0xEE81)                       # ASR.L #7,D1
    a.emit(0x83C3)                       # DIVS.W D3,D1
    a.emit(0xEA41)                       # ASR.W #5,D1
    a.emit(0xD245)                       # ADD.W D5,D1
    a.move("w", D(1), POSTINC(3))


def build_main():
    a = Asm(CODE)
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x9A))
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x96))
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x9C))
    a.move("b", IMM(0x7F, "b"), ABSL(CIAB_ICR))
    a.move("b", IMM(0x7F, "b"), ABSL(CIAA_ICR))
    a.lea(ABSL(CUSTOM), 6)
    a.lea(ABSL(LOOPMEM), 0)
    a.move("w", IMM((PLANES + 5 * PLANE_SIZE - LOOPMEM) // 4 - 1, "w"), D(0))
    a.label("clr")
    a.move("l", IMM(0, "l"), POSTINC(0))
    a.dbra(0, "clr")
    # Sub-lists like Interference's: 5 MOVEs BPLxPTL (the planes' start),
    # for "36" 30 MOVEs COLOR01-COLOR30 = $888, then COPJMP1.
    for name, sub in SUBS.items():
        a.lea(ABSL(sub), 0)
        for p in range(5):
            a.move("w", IMM(0xE2 + 4 * p, "w"), POSTINC(0))
            a.move("w", IMM((PLANES + p * PLANE_SIZE) & 0xFFFF, "w"), POSTINC(0))
        if name == "36":
            a.moveq(29, 3)
            a.move("w", IMM(0x182, "w"), D(4))
            a.label("s36")
            a.move("w", D(4), POSTINC(0))
            a.move("w", IMM(0x0888, "w"), POSTINC(0))
            a.addq("w", 2, D(4))
            a.dbra(3, "s36")
        a.move("l", IMM(0x00880000, "l"), IND(0))
    # Every list starts with the head (incl. COP1LCH = 4).
    for name, base in LISTS.items():
        a.lea(ABSL(base), 0)
        a.lea(ABSL(CODE + FONT_AT + 0x80), 1)
        a.moveq(HEAD_WORDS - 1, 0)
        a.label(f"hd{base}")
        a.move("w", POSTINC(1), POSTINC(0))
        a.dbra(0, f"hd{base}")
        if name != "head":
            a.move("w", IMM(0x1C, "w"), D(1))
            a.label(f"cl{name}")
            a.move("w", D(1), D(2))
            a.lsl_w(8, 2)
            a.ori("w", 0x00CF, D(2))
            a.move("w", D(2), POSTINC(0))
            a.move("w", IMM(0xFFFE, "w"), POSTINC(0))
            a.move("w", IMM(0x82, "w"), POSTINC(0))  # COP1LCL = this block + 20
            a.move("l", A(0), D(2))
            a.addi("w", 14, D(2))
            a.move("w", D(2), POSTINC(0))
            a.move("l", IMM(0x00840004, "l"), POSTINC(0))  # COP2LCH = 4
            a.move("w", IMM(0x86, "w"), POSTINC(0))
            if name == "36alt":
                # even lines the long sub-list, odd lines the short one
                a.move("w", IMM(SUBS["36"] & 0xFFFF, "w"), D(2))
                a.emit(0x0801, 0)                      # BTST #0,D1
                a.beq(f"ce{name}")
                a.move("w", IMM(SUBS["6"] & 0xFFFF, "w"), D(2))
                a.label(f"ce{name}")
                a.move("w", D(2), POSTINC(0))
            else:
                a.move("w", IMM(SUBS[name] & 0xFFFF, "w"), POSTINC(0))
            a.move("l", IMM(0x008A0000, "l"), POSTINC(0))  # COPJMP2
            a.addq("w", 1, D(1))
            a.cmpi("w", 0x100, D(1))
            a.bne(f"cl{name}")
            a.move("l", IMM(0x00820000 | (base & 0xFFFF), "l"), POSTINC(0))
        a.move("l", IMM(0xFFFFFFFE, "l"), IND(0))
    a.move("b", IMM(0, "b"), ABSL(CIAB_CRA))
    a.move("b", IMM(0xFF, "b"), ABSL(CIAB_TALO))
    a.move("b", IMM(0xFF, "b"), ABSL(CIAB_TAHI))
    a.move("b", IMM(0x01, "b"), ABSL(CIAB_CRA))
    # Blitter: A->D, masks, BLTAMOD 0, BLTDMOD 2 (every other word).
    a.move("l", IMM(0x09F00000, "l"), D16(0x40, 6))
    a.move("l", IMM(0xFFFFFFFF, "l"), D16(0x44, 6))
    a.move("w", IMM(0, "w"), D16(0x64, 6))
    a.move("w", IMM(2, "w"), D16(0x66, 6))
    a.moveq(0, 6)
    a.label("tests")
    # Row D6's entry: 10 bytes at TESTS_AT + 10 * D6.
    a.move("w", D(6), D(0))
    a.emit(0xC0FC, 10)                   # MULU.W #10,D0
    a.lea(ABSL(CODE + FONT_AT + 0x80 + 2 * HEAD_WORDS), 5)
    a.emit(0xDAC0)                       # ADDA.W D0,A5
    a.lea(ABSL(CUSTOM), 4)
    # Switch lists in the vertical blank (line >= 256): the full list's
    # blocks rewrite COP1LC on every line, the head list does not.
    a.label("vb")
    a.move("l", D16(0x04, 4), D(0))
    a.andi("l", 0x10000, D(0))
    a.beq("vb")
    a.move("w", IMM(0x7FFF, "w"), D16(0x96, 4))  # Copper off: its list rewrites COP1LC
    a.move("w", POSTINC(5), D16(0x100, 4))
    a.move("w", POSTINC(5), D(0))
    a.move("l", POSTINC(5), D16(0x80, 4))
    a.move("w", D(0), D16(0x88, 4))               # COPJMP1
    a.move("w", D(0), D16(0x96, 4))
    a.move("w", IND(5), D(0))            # loop number
    a.move("w", D(6), PREDEC(7))
    a.bsr("measure")
    a.move("w", POSTINC(7), D(6))
    a.bsr("show")
    a.addq("w", 1, D(6))
    a.cmpi("w", len(TESTS), D(6))
    a.bne("tests")
    # Show the numbers: planes on, the head list.
    a.lea(ABSL(CUSTOM), 6)
    a.move("w", IMM(0x7FFF, "w"), D16(0x96, 6))
    a.move("w", IMM(0x5200, "w"), D16(0x100, 6))
    a.move("l", IMM(LIST_HEAD, "l"), D16(0x80, 6))
    a.move("w", D(0), D16(0x88, 6))
    a.move("w", IMM(0x8380, "w"), D16(0x96, 6))
    a.label("halt")
    a.bra("halt")

    # measure: D0 = loop number; returns the ticks in D0.
    a.label("measure")
    a.lea(ABSL(CUSTOM), 6)
    a.move("w", D(0), PREDEC(7))
    a.label("w0")
    a.move("l", D16(0x04, 6), D(0))
    a.andi("l", 0x1FF00, D(0))
    a.beq("w0")
    a.label("w1")
    a.move("l", D16(0x04, 6), D(0))
    a.andi("l", 0x1FF00, D(0))
    a.bne("w1")
    a.bsr("timer")
    a.move("w", IND(7), D(1))            # loop number
    a.move("w", D(0), IND(7))            # start time replaces it
    a.tst("w", D(1))
    bcc_w(a, 7, "l_divs")
    a.subq("w", 1, D(1))
    bcc_w(a, 7, "l_noblit")
    bcc_w(a, 0, "l_blit")
    a.label("done")
    a.bsr("timer")
    a.move("w", POSTINC(7), D(2))
    a.sub_dd(0, 2)
    a.move("w", D(2), D(0))
    a.rts()

    for name in LOOPS:
        a.label(f"l_{name}")
        if name == "divs":
            a.move("w", IMM(DIVS_ITER, "w"), PREDEC(7))
            a.moveq(0x21, 0)
            a.move("w", IMM(0x200, "w"), D(4))
            a.moveq(0x40, 5)
            a.label("dv")
            a.lea(ABSL(CODE), 0)
            a.lea(ABSL(LOOPMEM), 3)
            for _ in range(8):
                divs_block(a)
            a.subq("w", 1, IND(7))
            bcc_w(a, 6, "dv")
        else:
            a.move("w", IMM(BLIT_ITER, "w"), PREDEC(7))
            a.lea(ABSL(LOOPMEM), 0)
            a.lea(ABSL(LOOPMEM + 0x1000), 3)
            a.lea(ABSL(BLIT_SRC), 4)
            a.lea(ABSL(CUSTOM + 2), 5)
            a.move("l", IMM(LOOPMEM + 0x2000, "l"), D(2))
            a.move("l", IMM(BLIT_DST, "l"), D(3))
            a.move("l", IMM(LOOPMEM + 0x3000, "l"), D(4))
            a.emit(0x2C44)  # MOVEA.L D4,A6
            a.emit(0x2242)  # MOVEA.L D2,A1
            a.emit(0x2443)  # MOVEA.L D3,A2
            a.move("w", IMM(0x8000, "w"), D(6))
            a.moveq(0x3C, 7)
            a.label(f"lp_{name}")
            blit_block(a, 0x58 if name == "blit" else NOOP, name)
            a.subq("w", 1, IND(7))
            bcc_w(a, 6, f"lp_{name}")
            # Let the last blit end.
            a.label(f"end_{name}")
            a.emit(0x0315)  # BTST D1,(A5)
            a.bne(f"end_{name}")
        a.addq("l", 2, A(7))
        bcc_w(a, 0, "done")

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

    # show: D0 as 4 hex digits in plane 1, row 24 + 16 * D6, column 4.
    a.label("show")
    a.move("w", D(0), D(5))
    a.move("w", D(6), D(1))
    a.lsl_w(4, 1)
    a.addi("w", 24, D(1))
    a.move("w", D(1), D(2))
    a.lsl_w(5, 2)
    a.move("w", D(1), D(3))
    a.lsl_w(3, 3)
    a.emit(0xD443)
    a.move("w", D(1), D(3))
    a.lsl_w(2, 3)
    a.emit(0xD443)
    a.lea(ABSL(PLANES + 4), 0)
    a.emit(0xD0C2)
    a.moveq(3, 7)
    a.label("digit")
    a.rol_w(4, 5)
    a.move("w", D(5), D(1))
    a.andi("w", 0x000F, D(1))
    a.lsl_w(3, 1)
    a.lea(ABSL(CODE + FONT_AT), 1)
    a.emit(0xD2C1)
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
    assert len(code) <= FONT_AT, hex(len(code))
    code += bytes(FONT_AT - len(code))
    for i in range(16):
        code += bytes(bt.FONT[i])
    head = []
    for p in range(5):
        pt = PLANES + p * PLANE_SIZE
        head += [0xE0 + 4 * p, pt >> 16, 0xE2 + 4 * p, pt & 0xFFFF]
    head += [0x8E, 0x1C71, 0x90, 0x34D1, 0x92, 0x0030, 0x94, 0x00D0,
             0x102, 0, 0x104, 0, 0x108, 2, 0x10A, 2,
             0x180, 0x0024, 0x182, 0x0FFF, 0x184, 0x0F00, 0x186, 0x00F0,
             0x188, 0x000F, 0x18A, 0x0FF0]
    head += [0x80, 0x0004]  # COP1LCH: the blocks' COP1LCL MOVEs need it
    assert len(head) == HEAD_WORDS
    code += b"".join(struct.pack(">H", w) for w in head)
    for con0, dma, lst, loop in TESTS:
        code += struct.pack(">HHIH", con0, dma, LISTS[lst], LOOPS.index(loop))
    return bytes(code)


def build_boot(length):
    """Boot block: read the program (from byte 1024 of the disk) to CODE
    with the trackdisk IORequest in A1, then jump to it."""
    b = Asm(0)
    b.move("l", IMM(CODE, "l"), D16(0x28, 1))    # io_Data
    b.move("l", IMM(length, "l"), D16(0x24, 1))  # io_Length
    b.move("l", IMM(1024, "l"), D16(0x2C, 1))    # io_Offset
    b.move("w", IMM(2, "w"), D16(0x1C, 1))       # io_Command = CMD_READ
    b.emit(0x4EAE, (-456) & 0xFFFF)              # JSR DoIO(A6)
    b.jmp(CODE)
    return b.finish()


def main():
    out = sys.argv[1]
    prog = build_main()
    prog += bytes(-len(prog) % 512)
    boot = build_boot(len(prog))
    adf = bytearray(901120)
    adf[0:4] = b"DOS\0"
    adf[8:12] = struct.pack(">I", 880)
    adf[12:12 + len(boot)] = boot
    adf[1024:1024 + len(prog)] = prog
    s = 0
    for i in range(0, 1024, 4):
        s += struct.unpack(">I", adf[i:i + 4])[0]
        s = (s & 0xFFFFFFFF) + (s >> 32)
    adf[4:8] = struct.pack(">I", (~s) & 0xFFFFFFFF)
    open(out, "wb").write(adf)
    if len(sys.argv) > 2:
        open(sys.argv[2], "wb").write(adf[:1024 + len(prog)])
    print("program bytes", len(prog), "boot code", len(boot))


if __name__ == "__main__":
    main()
