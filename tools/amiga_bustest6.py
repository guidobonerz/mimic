"""Build the sixth bus test: where the ECS Agnus loses time on Copper jumps
next to bitplane DMA. The owner's A500 (ECS Agnus 8372A, 2026-11-07) took
69 % and 48 % longer for tools/amiga_bustest3.py's rows 2 and 3 (a Copper
jump into a sub-list and back on every line, 5 or 30 MOVEs, 5 lowres
planes) than mimic and WinUAE, and its row with 5 MOVEs longer than the one
with 30, while the real A1200 agreed with the emulators. This program
varies what that test does, one row each; the numbers come out as in bus
test 3 (E-clock ticks for 600 iterations of 16x MOVE.W 0(A1,D1.W),4(A0) +
DBRA from line 0, 4 hex digits per row). It runs from the hardware disk
(tools/amiga_hwdisk.py, `build_main(harness=True)`); `results` prints the
rows.

Every line $1C-$FF of a list is a block: WAIT line,$DF, then
  kind A ("sub n"): MOVE COP1LCL (next block), MOVE COP2LCH/COP2LCL (the
         sub-list: n MOVEs COLORxx and MOVE COPJMP1), MOVE COPJMP2
         (bus test 3's scheme; unlike bus test 3, every row strobes COPJMP1 after
         setting COP1LC, so its list really runs: bus test 3's blocks rewrite
         COP1LCL themselves, and as both its lists share the high word the
         Copper never left the first one in rows 1 and 3);
  kind B ("direct"): MOVE COP1LCL (next block), 5 MOVEs COLORxx, MOVE
         COPJMP1 (a jump inside the one list, no second list);
  kind C ("none"): 5 MOVEs COLORxx (no jump; bus test 1's row 2).
Rows:
  0-7: 5 planes, sub-list with 0, 1, 2, 5, 10, 20, 30, 60 MOVEs
  8:   5 planes, direct
  9:   1 plane, sub 5
  10:  no planes, sub 5 (bus test 3's row 0)
  11:  5 planes, none (bus test 1's row 2)

usage: python tools/amiga_bustest6.py [OUT.adf]   (used by tools/amiga_hwdisk.py;
       OUT.adf boots the program alone, e.g. for mimic)
"""
import importlib.util
import os
import struct
import sys

_spec = importlib.util.spec_from_file_location(
    "amiga_bustest", os.path.join(os.path.dirname(os.path.abspath(__file__)), "amiga_bustest.py"))
bt = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(bt)

Asm, D, A, IND, POSTINC, D16, IDX, ABSL, IMM = (
    bt.Asm, bt.D, bt.A, bt.IND, bt.POSTINC, bt.D16, bt.IDX, bt.ABSL, bt.IMM)
CODE, LOOPMEM, PLANES = bt.CODE, bt.LOOPMEM, bt.PLANES
ROWBYTES, PLANE_SIZE, CUSTOM = bt.ROWBYTES, bt.PLANE_SIZE, bt.CUSTOM
CIAB_CRA, CIAB_TALO, CIAB_TAHI, CIAB_ICR, CIAA_ICR = (
    bt.CIAB_CRA, bt.CIAB_TALO, bt.CIAB_TAHI, bt.CIAB_ICR, bt.CIAA_ICR)
ITER = bt.ITER

SUB_COUNTS = [0, 1, 2, 5, 10, 20, 30, 60]
LISTS = 0x32000       # the lists, 0x2000 each (kind B's blocks are 32 bytes)
LIST_SIZE = 0x2000
SUBS = 0x46000        # the sub-lists, 0x100 each
# (kind, n) of every list, in memory order.
LIST_KINDS = [("A", n) for n in SUB_COUNTS] + [("B", 5), ("C", 5)]
FONT_AT = 0x700
HEAD_AT = FONT_AT + 0x80
TESTS_AT = HEAD_AT + 2 * bt.HEAD_WORDS + 4


def bccw(a, cc, label):
    """Bcc with a word displacement."""
    a.fixups.append((len(a.words) + 1, label, "w"))
    a.emit(0x6000 | (cc << 8), 0)


def list_at(i):
    return LISTS + i * LIST_SIZE


# (BPLCON0, DMACON, list index): the rows.
TESTS = ([(0x5200, 0x8380, i) for i in range(len(SUB_COUNTS))]
         + [(0x5200, 0x8380, len(SUB_COUNTS)),      # direct
            (0x1200, 0x8380, SUB_COUNTS.index(5)),  # 1 plane, sub 5
            (0x0200, 0x8280, SUB_COUNTS.index(5)),  # no planes, sub 5
            (0x5200, 0x8380, len(SUB_COUNTS) + 1)])  # none
ROWS = len(TESTS)


def build_main(harness=False):
    a = Asm(CODE)
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x9A))
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x96))
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x9C))
    if not harness:
        a.move("b", IMM(0x7F, "b"), ABSL(CIAB_ICR))
        a.move("b", IMM(0x7F, "b"), ABSL(CIAA_ICR))
    a.lea(ABSL(CUSTOM), 6)
    a.lea(ABSL(LOOPMEM), 0)
    a.move("w", IMM((PLANES + 5 * PLANE_SIZE - LOOPMEM) // 4 - 1, "w"), D(0))
    a.label("clr")
    a.move("l", IMM(0, "l"), POSTINC(0))
    a.dbra(0, "clr")
    # Sub-lists: n MOVEs COLORxx = $888, then MOVE COPJMP1.
    for k, n in enumerate(SUB_COUNTS):
        a.lea(ABSL(SUBS + k * 0x100), 0)
        if n:
            a.move("w", IMM(n - 1, "w"), D(3))
            a.move("w", IMM(0x182, "w"), D(4))
            a.label(f"s{k}")
            a.move("w", D(4), POSTINC(0))
            a.move("w", IMM(0x0888, "w"), POSTINC(0))
            a.addq("w", 2, D(4))
            a.cmpi("w", 0x1C0, D(4))             # COLOR01-COLOR31, then again
            a.bne(f"t{k}")
            a.move("w", IMM(0x182, "w"), D(4))
            a.label(f"t{k}")
            a.dbra(3, f"s{k}")
        a.move("l", IMM(0x00880000, "l"), IND(0))  # MOVE #0,COPJMP1
    # Lists: head (incl. COP1LCH = 3), then the blocks, then the end.
    for i, (kind, n) in enumerate(LIST_KINDS):
        base = list_at(i)
        a.lea(ABSL(base), 0)
        a.lea(ABSL(CODE + HEAD_AT), 1)
        a.moveq(bt.HEAD_WORDS + 2 - 1, 0)
        a.label(f"hd{i}")
        a.move("w", POSTINC(1), POSTINC(0))
        a.dbra(0, f"hd{i}")
        a.move("w", IMM(base >> 16, "w"), D16(-2, 0))  # this list's COP1LCH
        a.move("w", IMM(0x1C, "w"), D(1))
        a.label(f"cl{i}")
        a.move("w", D(1), D(2))
        a.lsl_w(8, 2)
        a.ori("w", 0x00DF, D(2))
        a.move("w", D(2), POSTINC(0))
        a.move("w", IMM(0xFFFE, "w"), POSTINC(0))
        if kind == "A":
            sub = SUBS + SUB_COUNTS.index(n) * 0x100
            a.move("w", IMM(0x82, "w"), POSTINC(0))  # COP1LCL = this block + 20
            a.move("l", A(0), D(2))
            a.addi("w", 14, D(2))
            a.move("w", D(2), POSTINC(0))
            a.move("l", IMM(0x00840000 | (SUBS >> 16), "l"), POSTINC(0))  # COP2LCH
            a.move("w", IMM(0x86, "w"), POSTINC(0))
            a.move("w", IMM(sub & 0xFFFF, "w"), POSTINC(0))
            a.move("l", IMM(0x008A0000, "l"), POSTINC(0))  # COPJMP2
        elif kind == "B":
            a.move("w", IMM(0x82, "w"), POSTINC(0))  # COP1LCL = this block + 32
            a.move("l", A(0), D(2))
            a.addi("w", 26, D(2))
            a.move("w", D(2), POSTINC(0))
            for k in range(n):
                a.move("l", IMM((0x182 + 2 * k) << 16 | 0x0888, "l"), POSTINC(0))
            a.move("l", IMM(0x00880000, "l"), POSTINC(0))  # COPJMP1
        else:
            for k in range(n):
                a.move("l", IMM((0x182 + 2 * k) << 16 | 0x0888, "l"), POSTINC(0))
        a.addq("w", 1, D(1))
        a.cmpi("w", 0x100, D(1))
        bccw(a, 6, f"cl{i}")                         # BNE
        # Back to the list's start for the next field, then the end.
        a.move("l", IMM(0x00800000 | (base >> 16), "l"), POSTINC(0))
        a.move("l", IMM(0x00820000 | (base & 0xFFFF), "l"), POSTINC(0))
        a.move("l", IMM(0xFFFFFFFE, "l"), IND(0))
    a.move("b", IMM(0, "b"), ABSL(CIAB_CRA))
    a.move("b", IMM(0xFF, "b"), ABSL(CIAB_TALO))
    a.move("b", IMM(0xFF, "b"), ABSL(CIAB_TAHI))
    a.move("b", IMM(0x01, "b"), ABSL(CIAB_CRA))
    a.lea(ABSL(CODE + TESTS_AT), 5)
    a.moveq(0, 6)
    a.label("tests")
    a.move("w", POSTINC(5), D16(0x100, 6))
    a.move("w", POSTINC(5), D(0))
    a.move("l", POSTINC(5), D16(0x80, 6))
    a.move("w", D(0), D16(0x88, 6))              # COPJMP1: into the row's list now
    a.move("w", IMM(0x7FFF, "w"), D16(0x96, 6))
    a.move("w", D(0), D16(0x96, 6))
    a.bsr("measure")
    a.bsr("show")
    a.addq("w", 1, D(6))
    a.cmpi("w", ROWS, D(6))
    bccw(a, 6, "tests")                              # BNE
    a.label("halt")
    if harness:  # back to tools/amiga_hwdisk.py's harness
        a.rts()
    else:
        a.bra("halt")

    a.label("measure")
    a.label("w0")
    a.move("l", D16(0x04, 6), D(0))
    a.andi("l", 0x1FF00, D(0))
    a.beq("w0")
    a.label("w1")
    a.move("l", D16(0x04, 6), D(0))
    a.andi("l", 0x1FF00, D(0))
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
    a.sub_dd(0, 2)
    a.move("w", D(2), D(0))
    a.rts()

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
    head += [0x8E, 0x1C71, 0x90, 0x34D1, 0x92, 0x0030, 0x94, 0x00D8,
             0x102, 0, 0x104, 0, 0x108, 0, 0x10A, 0,
             0x180, 0x0024, 0x182, 0x0FFF, 0x184, 0x0F00, 0x186, 0x00F0,
             0x188, 0x000F, 0x18A, 0x0FF0]
    head += [0x80, LISTS >> 16]  # COP1LCH: the blocks' COP1LCL MOVEs need it
    code += b"".join(struct.pack(">H", w) for w in head)
    assert len(code) == TESTS_AT, (hex(len(code)), hex(TESTS_AT))
    for con0, dma, i in TESTS:
        code += struct.pack(">HHI", con0, dma, list_at(i))
    return bytes(code)


def build_boot(length):
    """Boot block: read the program (from byte 1024 of the disk) to CODE
    with the trackdisk IORequest in A1, then jump to it (bus test 4's)."""
    b = Asm(0)
    b.move("l", IMM(CODE, "l"), D16(0x28, 1))    # io_Data
    b.move("l", IMM(length, "l"), D16(0x24, 1))  # io_Length
    b.move("l", IMM(1024, "l"), D16(0x2C, 1))    # io_Offset
    b.move("w", IMM(2, "w"), D16(0x1C, 1))       # io_Command = CMD_READ
    b.emit(0x4EAE, (-456) & 0xFFFF)              # JSR DoIO(A6)
    b.jmp(CODE)
    return b.finish()


def main():
    """`OUT.adf`: a bootable disk of the program alone (the boot block loads
    it as bus test 4's does); without arguments only a build check."""
    prog = build_main(harness=not sys.argv[1:])
    print("program bytes", len(prog), "rows", ROWS)
    if not sys.argv[1:]:
        return
    prog += bytes(-len(prog) % 512)
    boot = build_boot(len(prog))
    adf = bytearray(901120)
    adf[0:4] = b"DOS" + bytes(1)
    adf[8:12] = struct.pack(">I", 880)
    adf[12:12 + len(boot)] = boot
    adf[1024:1024 + len(prog)] = prog
    s = 0
    for i in range(0, 1024, 4):
        s += struct.unpack(">I", adf[i:i + 4])[0]
        s = (s & 0xFFFFFFFF) + (s >> 32)
    adf[4:8] = struct.pack(">I", (~s) & 0xFFFFFFFF)
    open(sys.argv[1], "wb").write(adf)


if __name__ == "__main__":
    main()
