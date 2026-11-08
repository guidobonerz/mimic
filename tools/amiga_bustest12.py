"""Build the twelfth bus test: where in a long instruction the 68000
samples IPL0-2 for its interrupt decision, for the hardware disk
(tools/amiga_hwdisk.py).

Motorola's AN1012 ("A Discussion of Interrupts for the MC68000", 1988,
see docs/sources.md) says the step that samples the interrupt lines is
instruction dependent: MOVEM.L (An)+ samples at its start, DIVS at its
end. mimic samples every instruction at the start of its last bus cycle,
which the owner's A500 confirmed for NOP and BRA.S (bus tests 5, 10, 11).
This test runs bus test 11's Copper-raised SOFT probes against main loops
of one long instruction each:
  group 0: MOVEM.L (A0),D0/D1/D3/D4/A1/A3 + BRA.S (60 + 10 clocks)
  group 1: MOVEM.L D0/D1/D3/D4/A1/A3,(A0) + BRA.S (56 + 10 clocks)
  group 2: MOVE.L #$00012345,D1 + DIVS.W D0,D1 (D0 = $0123) + BRA.S
  group 3: MOVE.W #$FFFF,D1 + MULU.W D0,D1 (D0 = $FFFF, 70 clocks) + BRA.S
No bitplanes. The Copper writes INTREQ SET|SOFT at line $E0 + 2k, colour
clock $51 + 4k (k = 0..15): each probe lies 916 CPU clocks after the last,
which moves it through the loops' phases (6, -8 and 36 clocks a probe for
groups 0, 1 and 3). Each probe stores the trigger position (the anchor)
and VHPOSR as the level-1 handler's first instruction reads it; the
latency is their difference in colour clocks (227 a line). Where the
sample point lies in an instruction shows in the phase at which the
latency jumps by a whole loop.

usage: used by tools/amiga_hwdisk.py; `python tools/amiga_bustest12.py`
       only checks that the program builds
"""
import importlib.util
import os
import struct

_spec = importlib.util.spec_from_file_location(
    "amiga_bustest", os.path.join(os.path.dirname(os.path.abspath(__file__)), "amiga_bustest.py"))
bt = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(bt)

Asm, D, A, IND, POSTINC, D16, ABSL, IMM = (
    bt.Asm, bt.D, bt.A, bt.IND, bt.POSTINC, bt.D16, bt.ABSL, bt.IMM)
CODE, COP_HEAD, LOOPMEM, PLANES = bt.CODE, bt.COP_HEAD, bt.LOOPMEM, bt.PLANES
ROWBYTES, PLANE_SIZE, CUSTOM = bt.ROWBYTES, bt.PLANE_SIZE, bt.CUSTOM
CIAB_ICR, CIAA_ICR = bt.CIAB_ICR, bt.CIAA_ICR

RESULTS = LOOPMEM           # 4 groups of 32 words
SCRATCH = LOOPMEM + 0x1000  # what the MOVEMs read and write
GROUPS = 4
PROBES = 16
FIRST = 0xE051              # the first trigger (line, colour clock)
STEP = 0x0204               # 2 lines and 4 colour clocks a probe
COP_PROBES = COP_HEAD + 0x400
FONT_AT = 0x300
HEAD_AT = FONT_AT + 0x80
MOVEM_REGS = 0x0A1B         # D0 D1 D3 D4 A1 A3 (bit 0 = D0 ... bit 15 = A7)


def wait_line0(a, name):
    """Wait for the start of the next field (VPOSR/VHPOSR line 0)."""
    a.label(name + "a")
    a.move("l", D16(0x04, 6), D(0))
    a.andi("l", 0x1FF00, D(0))
    a.beq(name + "a")
    a.label(name + "b")
    a.move("l", D16(0x04, 6), D(0))
    a.andi("l", 0x1FF00, D(0))
    a.bne(name + "b")


def bcc_w(a, op, label):
    """Bcc.W (op: 0x6600 BNE, 0x6700 BEQ)."""
    a.fixups.append((len(a.words) + 1, label, "w"))
    a.emit(op, 0)


def build_main(harness=False):
    a = Asm(CODE)
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x9A))  # INTENA
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x96))  # DMACON
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x9C))  # INTREQ
    if not harness:  # (tools/amiga_hwdisk.py: the OS keeps its CIA masks)
        a.move("b", IMM(0x7F, "b"), ABSL(CIAB_ICR))
        a.move("b", IMM(0x7F, "b"), ABSL(CIAA_ICR))
    a.lea(ABSL(CUSTOM), 6)
    a.lea(ABSL(LOOPMEM), 0)
    a.move("w", IMM((PLANES + 5 * PLANE_SIZE - LOOPMEM) // 4 - 1, "w"), D(0))
    a.label("clr")
    a.move("l", IMM(0, "l"), POSTINC(0))
    a.dbra(0, "clr")
    # Two Copper lists: the head table and the end; the head table, the
    # 16 probes and the end.
    for base, probes in ((COP_HEAD, False), (COP_PROBES, True)):
        a.lea(ABSL(base), 0)
        a.lea(ABSL(CODE + HEAD_AT), 1)
        a.moveq(bt.HEAD_WORDS - 1, 0)
        a.label(f"hd{probes}")
        a.move("w", POSTINC(1), POSTINC(0))
        a.dbra(0, f"hd{probes}")
        if probes:
            a.move("w", IMM(FIRST, "w"), D(1))
            a.moveq(PROBES - 1, 3)
            a.label("cp")
            a.move("w", D(1), POSTINC(0))
            a.move("w", IMM(0xFFFE, "w"), POSTINC(0))
            a.move("w", IMM(0x009C, "w"), POSTINC(0))  # INTREQ
            a.move("w", IMM(0x8004, "w"), POSTINC(0))  # SET | SOFT
            a.addi("w", STEP, D(1))
            a.dbra(3, "cp")
        a.move("l", IMM(0xFFFFFFFE, "l"), IND(0))
    a.moveq(0, 6)                                # D6 = group
    a.label("tests")
    a.move("w", IMM(0x0200, "w"), D16(0x100, 6))  # BPLCON0: no planes
    a.move("w", IMM(0x7FFF, "w"), D16(0x96, 6))
    a.move("w", IMM(0x8280, "w"), D16(0x96, 6))  # DMACON: Copper
    a.move("w", IMM(0x0004, "w"), D16(0x9C, 6))  # INTREQ: SOFT off
    # COP1LC takes the probe list at the next field; as in bus tests 10
    # and 11 the first field with it is thrown away, then the request it
    # left is cleared.
    a.move("l", IMM(COP_PROBES, "l"), D16(0x80, 6))  # COP1LC
    a.lea(ABSL(a.pc), 4)                         # (patched: A4 = gdone)
    lea_gdone = len(a.words) - 2
    wait_line0(a, "c1")
    wait_line0(a, "c2")
    a.move("w", IMM(0x0004, "w"), D16(0x9C, 6))  # INTREQ: SOFT off
    a.lea(ABSL(RESULTS), 2)
    a.move("w", D(6), D(0))
    a.lsl_w(6, 0)                                # 64 bytes a group
    a.emit(0xD4C0)                               # ADDA.W D0,A2
    a.moveq(PROBES, 2)
    a.move("w", IMM(FIRST, "w"), D(7))           # the first trigger
    a.lea(ABSL(SCRATCH), 0)
    a.move("w", D(6), D(5))                      # the group's loop
    a.move("w", IMM(0xC004, "w"), D16(0x9A, 6))  # INTENA: INTEN | SOFT
    a.subq("w", 1, D(5))
    bcc_w(a, 0x6700, "g1")
    a.subq("w", 1, D(5))
    bcc_w(a, 0x6700, "g2")
    a.subq("w", 1, D(5))
    bcc_w(a, 0x6700, "g3")
    a.label("g0")
    a.emit(0x4CD0, MOVEM_REGS)                   # MOVEM.L (A0),regs
    a.bra("g0")
    a.label("g1")
    a.emit(0x48D0, MOVEM_REGS)                   # MOVEM.L regs,(A0)
    a.bra("g1")
    a.label("g2")
    a.move("w", IMM(0x0123, "w"), D(0))
    a.label("g2l")
    a.move("l", IMM(0x00012345, "l"), D(1))
    a.emit(0x83C0)                               # DIVS.W D0,D1
    a.bra("g2l")
    a.label("g3")
    a.move("w", IMM(0xFFFF, "w"), D(0))
    a.label("g3l")
    a.move("w", IMM(0xFFFF, "w"), D(1))
    a.emit(0xC2C0)                               # MULU.W D0,D1
    a.bra("g3l")
    a.label("gdone")
    a.move("w", IMM(0x0004, "w"), D16(0x9A, 6))  # INTENA: SOFT off
    a.move("l", IMM(COP_HEAD, "l"), D16(0x80, 6))
    a.addq("w", 1, D(6))
    a.cmpi("w", GROUPS, D(6))
    bcc_w(a, 0x6600, "tests")
    # Show the 128 words, 8 a row, and stop with the planes on.
    a.lea(ABSL(RESULTS), 5)
    a.moveq(0, 6)
    a.label("rows")
    a.moveq(2, 1)
    a.label("cols")
    a.move("w", POSTINC(5), D(0))
    a.bsr("show")
    a.addq("w", 5, D(1))
    a.cmpi("w", 42, D(1))
    a.bne("cols")
    a.addq("w", 1, D(6))
    a.cmpi("w", GROUPS * 4, D(6))
    a.bne("rows")
    a.move("w", IMM(0x1200, "w"), D16(0x100, 6))
    a.move("w", IMM(0x8380, "w"), D16(0x96, 6))
    a.label("halt")
    if harness:  # back to tools/amiga_hwdisk.py's harness
        a.rts()
    else:
        a.bra("halt")

    # The handler: VHPOSR first, then acknowledge SOFT, store anchor and
    # value; the anchor moves on by STEP. After D2 values return to A4.
    a.label("irq1")
    a.move("w", D16(0x06, 6), D(5))
    a.move("w", IMM(0x0004, "w"), D16(0x9C, 6))
    a.move("w", D(7), POSTINC(2))
    a.move("w", D(5), POSTINC(2))
    a.addi("w", STEP, D(7))
    a.subq("w", 1, D(2))
    a.bne("rte")
    a.emit(0x2F4C, 0x0002)                       # MOVE.L A4,2(SP)
    a.label("rte")
    a.emit(0x4E73)                               # RTE

    # show: D0 as 4 hex digits in plane 1, row 24 + 16*D6, byte column D1.
    a.label("show")
    a.move("w", D(0), D(5))
    a.lea(ABSL(PLANES), 0)
    a.emit(0xD0C1)  # ADDA.W D1,A0
    a.move("w", D(6), D(2))
    a.lsl_w(4, 2)
    a.addi("w", 24, D(2))
    a.move("w", D(2), D(3))
    a.lsl_w(5, 3)
    a.emit(0xD0C3)  # ADDA.W D3,A0
    a.move("w", D(2), D(3))
    a.lsl_w(3, 3)
    a.emit(0xD0C3)
    a.move("w", D(2), D(3))
    a.lsl_w(2, 3)
    a.emit(0xD0C3)
    a.moveq(3, 4)
    a.label("digit")
    a.rol_w(4, 5)
    a.move("w", D(5), D(2))
    a.andi("w", 0x000F, D(2))
    a.lsl_w(3, 2)
    a.lea(ABSL(CODE + FONT_AT), 1)
    a.emit(0xD2C2)  # ADDA.W D2,A1
    a.movea("l", A(0), 2)
    a.moveq(7, 3)
    a.label("row")
    a.move("b", POSTINC(1), IND(2))
    a.lea(D16(ROWBYTES, 2), 2)
    a.dbra(3, "row")
    a.addq("l", 1, A(0))
    a.dbra(4, "digit")
    a.rts()
    # The patch: LEA gdone (absolute).
    a.words[lea_gdone] = a.labels["gdone"] >> 16
    a.words[lea_gdone + 1] = a.labels["gdone"] & 0xFFFF
    code = bytearray(a.finish())
    assert len(code) <= FONT_AT, len(code)
    code += bytes(FONT_AT - len(code))
    for i in range(16):
        code += bytes(bt.FONT[i])
    head = []
    for p in range(5):
        pt = PLANES + p * PLANE_SIZE
        head += [0xE0 + 4 * p, pt >> 16, 0xE2 + 4 * p, pt & 0xFFFF]
    head += [0x8E, 0x1C71, 0x90, 0x34D1, 0x92, 0x0030, 0x94, 0x00D8,
             0x102, 0, 0x104, 0, 0x108, 0, 0x10A, 0,
             0x180, 0x0000, 0x182, 0x0FFF, 0x184, 0x0F00, 0x186, 0x00F0,
             0x188, 0x000F, 0x18A, 0x0FF0]
    assert len(head) == bt.HEAD_WORDS
    code += b"".join(struct.pack(">H", w) for w in head)
    while len(code) % 4:
        code += b"\0"
    return bytes(code), a.labels["irq1"]


def latencies(words):
    """128 words (anchor, value pairs) to 64 latencies in colour clocks."""
    def cck(v):
        return (v >> 8) * 227 + (v & 0xFF)
    return [cck(words[i + 1]) - cck(words[i]) for i in range(0, len(words) - 1, 2)]


if __name__ == "__main__":
    prog, irq1 = build_main()
    print(f"bus test 12: {len(prog)} bytes, handler at ${irq1:06X}")
