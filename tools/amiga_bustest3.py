"""Build bustest3.adf: how much of the chip bus the Copper takes when it
jumps (COPJMP written by the Copper itself), measured through the 68000.
Companion of tools/amiga_bustest.py (same loop, timer, font and screen):
600 iterations of 16x MOVE.W 0(A1,D1.W),4(A0) + DBRA from line 0, E-clock
ticks shown as 4 hex digits per row. The test
`copper_jump_bus_timing_matches_the_bus_test` (tests/tests/amiga_boot.rs)
boots tests/data/amiga_bustest3.bootblock in mimic and compares.

Every line $1C-$FF of the Copper list is a block: WAIT line,$DF; MOVE
COP1LCL (the next block); MOVE COP2LCH/COP2LCL; MOVE COPJMP2. The sub-list
COP2LC points to does n MOVEs to colour registers and MOVE COPJMP1, which
continues at the next block (the scheme of Sanity's "Interference").
  row 0: 5 MOVEs in the sub-list, no bitplanes
  row 1: 30 MOVEs (COLOR01-COLOR30), no bitplanes
  row 2: 5 MOVEs, 5 lowres planes
  row 3: 30 MOVEs, 5 lowres planes
The list's end sets COP1LCL back to its start (every block changes it).

usage: python tools/amiga_bustest3.py OUT.adf [OUT.bootblock]
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

LIST5 = 0x40000   # blocks with the 5-MOVE sub-list
LIST30 = 0x44000  # blocks with the 30-MOVE sub-list
SUB5 = 0x48000
SUB30 = 0x48100
FONT_AT = 0x2C0
HEAD_AT = FONT_AT + 0x80
TESTS_AT = HEAD_AT + 2 * bt.HEAD_WORDS + 4
TESTS = [  # BPLCON0, DMACON, COP1LC
    (0x0200, 0x8280, LIST5),
    (0x0200, 0x8280, LIST30),
    (0x5200, 0x8380, LIST5),
    (0x5200, 0x8380, LIST30),
]


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
    # Sub-lists: n MOVEs COLORxx = $888, then MOVE COPJMP1.
    for sub, n in ((SUB5, 5), (SUB30, 30)):
        a.lea(ABSL(sub), 0)
        a.moveq(n - 1, 3)
        a.move("w", IMM(0x182, "w"), D(4))
        a.label(f"s{n}")
        a.move("w", D(4), POSTINC(0))
        a.move("w", IMM(0x0888, "w"), POSTINC(0))
        a.addq("w", 2, D(4))
        a.dbra(3, f"s{n}")
        a.move("l", IMM(0x00880000, "l"), IND(0))  # MOVE #0,COPJMP1
    # Lists: head (incl. COP1LCH = 4), then the blocks, then the end.
    for base, sub in ((LIST5, SUB5), (LIST30, SUB30)):
        a.lea(ABSL(base), 0)
        a.lea(ABSL(CODE + HEAD_AT), 1)
        a.moveq(bt.HEAD_WORDS + 2 - 1, 0)
        a.label(f"hd{base}")
        a.move("w", POSTINC(1), POSTINC(0))
        a.dbra(0, f"hd{base}")
        a.move("w", IMM(0x1C, "w"), D(1))
        a.label(f"cl{base}")
        a.move("w", D(1), D(2))
        a.lsl_w(8, 2)
        a.ori("w", 0x00DF, D(2))
        a.move("w", D(2), POSTINC(0))
        a.move("w", IMM(0xFFFE, "w"), POSTINC(0))
        a.move("w", IMM(0x82, "w"), POSTINC(0))  # COP1LCL = this block + 20
        a.move("l", A(0), D(2))
        a.addi("w", 14, D(2))
        a.move("w", D(2), POSTINC(0))
        a.move("l", IMM(0x00840004, "l"), POSTINC(0))  # COP2LCH = 4
        a.move("w", IMM(0x86, "w"), POSTINC(0))
        a.move("w", IMM(sub & 0xFFFF, "w"), POSTINC(0))
        a.move("l", IMM(0x008A0000, "l"), POSTINC(0))  # COPJMP2
        a.addq("w", 1, D(1))
        a.cmpi("w", 0x100, D(1))
        a.bne(f"cl{base}")
        # Back to the list's start for the next field, then the end.
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
    a.move("w", IMM(0x7FFF, "w"), D16(0x96, 6))
    a.move("w", D(0), D16(0x96, 6))
    a.bsr("measure")
    a.bsr("show")
    a.addq("w", 1, D(6))
    a.cmpi("w", 4, D(6))
    a.bne("tests")
    # The last row's display (planes on) stays.
    a.label("halt")
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
    head += [0x80, 0x0004]  # COP1LCH: the blocks' COP1LCL MOVEs need it
    code += b"".join(struct.pack(">H", w) for w in head)
    assert len(code) == TESTS_AT, (hex(len(code)), hex(TESTS_AT))
    for con0, dma, cop in TESTS:
        code += struct.pack(">HHI", con0, dma, cop)
    return bytes(code)


def main():
    out = sys.argv[1]
    prog = build_main()
    if len(prog) % 4:
        prog += bytes(4 - len(prog) % 4)
    boot = bt.build_boot(prog)
    adf = bytearray(901120)
    adf[0:4] = b"DOS\0"
    adf[8:12] = struct.pack(">I", 880)
    adf[12:12 + len(boot)] = boot
    off = 12 + len(boot)
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
