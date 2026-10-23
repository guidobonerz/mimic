"""Build bustest2.adf: a boot block that measures how the Blitter and the
68000 share the chip bus. Companion of tools/amiga_bustest.py (same
assembler, font and screen); boot it on real hardware or in a cycle-exact
emulator and read the numbers off the screen. The test
`blitter_bus_arbitration_matches_the_bus_test` (tests/tests/amiga_boot.rs)
boots tests/data/amiga_bustest2.bootblock in mimic and compares.

Each row: a blit of 64 words x 256 rows started at line 0 of a frame (found
with one long read of VPOSR/VHPOSR, so line 256 cannot pass for it) while
the 68000 runs `8 x MOVE.W 0(A1,D1.W),4(A0)` + ADDQ + BTST BBUSY in a loop
until the Blitter is done. Two numbers per row (4 hex digits each): the
E-clock ticks until the blit ended (CIA-B timer A, 1 tick = 5 colour
clocks) and the loop iterations the CPU managed meanwhile.
  row 0: A->D copy, BLTPRI off, no bitplanes, Copper idle
  row 1: A->D copy, BLTPRI on (Blitter nasty)
  row 2: A->D copy, BLTPRI off, 5 lowres planes + a Copper WAIT and 5 MOVEs
         on every line (the second bus test's row 2)
  row 3: D-only clear, BLTPRI off, no bitplanes

usage: python tools/amiga_bustest2.py OUT.adf [OUT.bootblock]
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
CODE, COP_FULL, COP_HEAD, LOOPMEM, PLANES = bt.CODE, bt.COP_FULL, bt.COP_HEAD, bt.LOOPMEM, bt.PLANES
ROWBYTES, PLANE_SIZE, CUSTOM = bt.ROWBYTES, bt.PLANE_SIZE, bt.CUSTOM
CIAB_CRA, CIAB_TALO, CIAB_TAHI, CIAB_ICR, CIAA_ICR = (
    bt.CIAB_CRA, bt.CIAB_TALO, bt.CIAB_TAHI, bt.CIAB_ICR, bt.CIAA_ICR)

BLIT_A = 0x08000
BLIT_D = 0x70000
BLTSIZE = (256 << 6) | 0  # 256 rows of 64 words
FONT_AT = 0x240
HEAD_AT = FONT_AT + 0x80
TESTS_AT = HEAD_AT + 2 * bt.HEAD_WORDS
TESTS = [  # BPLCON0, DMACON, COP1LC, BLTCON0
    (0x0200, 0x82C0, COP_HEAD, 0x09F0),
    (0x0200, 0x86C0, COP_HEAD, 0x09F0),
    (0x5200, 0x83C0, COP_FULL, 0x09F0),
    (0x0200, 0x82C0, COP_HEAD, 0x0100),
]


def build_main():
    a = Asm(CODE)
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x9A))  # INTENA
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x96))  # DMACON
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x9C))  # INTREQ
    a.move("b", IMM(0x7F, "b"), ABSL(CIAB_ICR))
    a.move("b", IMM(0x7F, "b"), ABSL(CIAA_ICR))
    a.lea(ABSL(CUSTOM), 6)
    a.lea(ABSL(LOOPMEM), 0)
    a.move("w", IMM((PLANES + 5 * PLANE_SIZE - LOOPMEM) // 4 - 1, "w"), D(0))
    a.label("clr")
    a.move("l", IMM(0, "l"), POSTINC(0))
    a.dbra(0, "clr")
    for base in (COP_FULL, COP_HEAD):
        a.lea(ABSL(base), 0)
        a.lea(ABSL(CODE + HEAD_AT), 1)
        a.moveq(bt.HEAD_WORDS - 1, 0)
        a.label(f"hd{base}")
        a.move("w", POSTINC(1), POSTINC(0))
        a.dbra(0, f"hd{base}")
        if base == COP_HEAD:
            a.move("l", IMM(0xFFFFFFFE, "l"), IND(0))
        else:
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
    a.move("b", IMM(0, "b"), ABSL(CIAB_CRA))
    a.move("b", IMM(0xFF, "b"), ABSL(CIAB_TALO))
    a.move("b", IMM(0xFF, "b"), ABSL(CIAB_TAHI))
    a.move("b", IMM(0x01, "b"), ABSL(CIAB_CRA))
    # Blitter registers that stay: BLTCON1, masks, modulos.
    a.move("w", IMM(0, "w"), D16(0x42, 6))
    a.move("l", IMM(0xFFFFFFFF, "l"), D16(0x44, 6))
    a.move("l", IMM(0, "l"), D16(0x64, 6))  # BLTAMOD, BLTDMOD
    a.lea(ABSL(CODE + TESTS_AT), 5)
    a.moveq(0, 6)
    a.label("tests")
    a.move("w", POSTINC(5), D16(0x100, 6))
    a.move("w", POSTINC(5), D(0))
    a.move("l", POSTINC(5), D16(0x80, 6))
    a.move("w", POSTINC(5), D16(0x40, 6))  # BLTCON0
    a.move("w", IMM(0x7FFF, "w"), D16(0x96, 6))
    a.move("w", D(0), D16(0x96, 6))
    a.bsr("measure")
    a.moveq(4, 1)
    a.bsr("show")
    a.move("w", D(7), D(0))
    a.moveq(10, 1)
    a.bsr("show")
    a.addq("w", 1, D(6))
    a.cmpi("w", 4, D(6))
    a.bne("tests")
    a.move("w", IMM(0x5200, "w"), D16(0x100, 6))
    a.move("l", IMM(COP_FULL, "l"), D16(0x80, 6))
    a.move("w", IMM(0x8380, "w"), D16(0x96, 6))
    a.label("halt")
    a.bra("halt")

    # measure: at line 0 start the blit and the loop; D0 = ticks, D7 = loops.
    a.label("measure")
    # VPOSR and VHPOSR in one long read: line 256 cannot pass for line 0.
    a.label("w0")
    a.move("l", D16(0x04, 6), D(0))
    a.andi("l", 0x1FF00, D(0))
    a.beq("w0")
    a.label("w1")
    a.move("l", D16(0x04, 6), D(0))
    a.andi("l", 0x1FF00, D(0))
    a.bne("w1")
    a.move("l", IMM(BLIT_A, "l"), D16(0x50, 6))  # BLTAPT
    a.move("l", IMM(BLIT_D, "l"), D16(0x54, 6))  # BLTDPT
    a.bsr("timer")
    a.move("w", D(0), D(2))
    a.lea(ABSL(LOOPMEM), 0)
    a.lea(ABSL(LOOPMEM + 0x100), 1)
    a.moveq(0, 1)
    a.moveq(0, 7)
    a.move("w", IMM(BLTSIZE, "w"), D16(0x58, 6))
    a.label("loop")
    for _ in range(8):
        a.move("w", IDX(0, 1, 1), D16(4, 0))
    a.addq("w", 1, D(7))
    a.btst_imm(6, D16(0x02, 6))  # DMACONR high byte bit 6: BBUSY
    a.bne("loop")
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

    # show: D0 as 4 hex digits in plane 1, row 24 + 16*D6, byte column D1.
    a.label("show")
    a.move("w", D(0), D(5))
    a.lea(ABSL(PLANES), 0)
    a.emit(0xD0C1)  # ADDA.W D1,A0
    a.move("w", D(6), D(1))
    a.lsl_w(4, 1)
    a.addi("w", 24, D(1))
    a.move("w", D(1), D(2))
    a.lsl_w(5, 2)
    a.move("w", D(1), D(3))
    a.lsl_w(3, 3)
    a.emit(0xD443)  # ADD.W D3,D2
    a.move("w", D(1), D(3))
    a.lsl_w(2, 3)
    a.emit(0xD443)  # ADD.W D3,D2
    a.emit(0xD0C2)  # ADDA.W D2,A0
    a.moveq(3, 4)
    a.label("digit")
    a.rol_w(4, 5)
    a.move("w", D(5), D(1))
    a.andi("w", 0x000F, D(1))
    a.lsl_w(3, 1)
    a.lea(ABSL(CODE + FONT_AT), 1)
    a.emit(0xD2C1)  # ADDA.W D1,A1
    a.movea("l", A(0), 2)
    a.moveq(7, 3)
    a.label("row")
    a.move("b", POSTINC(1), IND(2))
    a.lea(D16(ROWBYTES, 2), 2)
    a.dbra(3, "row")
    a.addq("l", 1, A(0))
    a.dbra(4, "digit")
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
    assert len(head) == bt.HEAD_WORDS
    code += b"".join(struct.pack(">H", w) for w in head)
    assert len(code) == TESTS_AT
    for con0, dma, cop, bltcon0 in TESTS:
        code += struct.pack(">HHIH", con0, dma, cop, bltcon0)
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
