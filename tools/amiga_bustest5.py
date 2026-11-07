"""Build bustest5.adf: a boot block that measures when the 68000 takes a
Copper-raised interrupt. Companion of tools/amiga_bustest.py (same
assembler, font and screen); boot it on real hardware or in a cycle-exact
emulator and read the numbers off the screen. The test
`interrupt_probe_matches_the_bus_test` (tests/tests/amiga_boot.rs) boots
tests/data/amiga_bustest5.bootblock in mimic and prints the same numbers
(EMU_BUSTEST5_REF compares).

The Copper waits for line $E0 + 2k, colour clock $51 + 2k (k = 0..15, as
the vAmiga test suite's `Agnus/Registers/VPOS/probe1`) and writes INTREQ
SOFT; the level-1 handler reads VHPOSR and stores it. Meanwhile the CPU
runs a two-instruction loop in chip RAM. Four groups of 16 values, each on
two screen rows of 8 (4 hex digits: line, colour clock):
  group 0: no bitplanes, loop `bra.s` (10 clocks)
  group 1: 2 lowres planes (DDFSTRT $30, DDFSTOP $D8), loop `bra.s`
  group 2: no bitplanes, loop `nop` + `bra.s` (14 clocks)
  group 3: 2 lowres planes, loop `nop` + `bra.s`
The value minus the trigger position is the interrupt latency in colour
clocks: the loop's end, the 44-clock exception, the handler's first
instructions, and every wait for a chip bus slot in between.

usage: python tools/amiga_bustest5.py OUT.adf [OUT.bootblock]
"""
import importlib.util
import os
import struct
import sys

_spec = importlib.util.spec_from_file_location(
    "amiga_bustest", os.path.join(os.path.dirname(os.path.abspath(__file__)), "amiga_bustest.py"))
bt = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(bt)

Asm, D, A, IND, POSTINC, D16, ABSL, IMM = (
    bt.Asm, bt.D, bt.A, bt.IND, bt.POSTINC, bt.D16, bt.ABSL, bt.IMM)
CODE, COP_HEAD, LOOPMEM, PLANES = bt.CODE, bt.COP_HEAD, bt.LOOPMEM, bt.PLANES
ROWBYTES, PLANE_SIZE, CUSTOM = bt.ROWBYTES, bt.PLANE_SIZE, bt.CUSTOM
CIAB_ICR, CIAA_ICR = bt.CIAB_ICR, bt.CIAA_ICR

# The results: 4 groups of 16 words.
RESULTS = LOOPMEM
GROUPS = 4
PROBES = 16
FONT_AT = 0x200
HEAD_AT = FONT_AT + 0x80
TESTS_AT = HEAD_AT + 2 * bt.HEAD_WORDS
TESTS = [  # BPLCON0, DMACON, loop kind
    (0x0200, 0x8280, 0),
    (0x2200, 0x8380, 0),
    (0x0200, 0x8280, 1),
    (0x2200, 0x8380, 1),
]


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
    # The Copper list: the head table (planes, DDF, DIW, colours), then
    # the 16 probes, then the end.
    a.lea(ABSL(COP_HEAD), 0)
    a.lea(ABSL(CODE + HEAD_AT), 1)
    a.moveq(bt.HEAD_WORDS - 1, 0)
    a.label("hd")
    a.move("w", POSTINC(1), POSTINC(0))
    a.dbra(0, "hd")
    a.move("w", IMM(0xE051, "w"), D(1))
    a.moveq(PROBES - 1, 3)
    a.label("cp")
    a.move("w", D(1), POSTINC(0))
    a.move("w", IMM(0xFFFE, "w"), POSTINC(0))
    a.move("w", IMM(0x009C, "w"), POSTINC(0))  # INTREQ
    a.move("w", IMM(0x8004, "w"), POSTINC(0))  # SET | SOFT
    a.addi("w", 0x0202, D(1))
    a.dbra(3, "cp")
    a.move("l", IMM(0xFFFFFFFE, "l"), IND(0))
    a.move("l", IMM(COP_HEAD, "l"), D16(0x80, 6))  # COP1LC
    # The level-1 handler and the interrupt mask.
    # The boot code runs as a task in user mode with the interrupt mask
    # at 0: a level-1 request is taken as soon as INTENA allows it.
    a.lea(ABSL(RESULTS), 2)
    a.lea(ABSL(CODE + TESTS_AT), 5)
    a.moveq(0, 6)
    a.label("tests")
    a.move("w", POSTINC(5), D16(0x100, 6))  # BPLCON0
    a.move("w", POSTINC(5), D(0))
    a.move("w", POSTINC(5), D(4))  # loop kind
    a.move("w", IMM(0x7FFF, "w"), D16(0x96, 6))
    a.move("w", D(0), D16(0x96, 6))  # DMACON
    a.move("w", IMM(0x0004, "w"), D16(0x9C, 6))  # INTREQ: SOFT off
    a.move("w", IMM(0xC004, "w"), D16(0x9A, 6))  # INTENA: INTEN | SOFT
    # Wait for line 0 (VPOSR and VHPOSR in one read), then loop until the
    # handler has 16 values and diverts the return to `done`.
    a.label("w0")
    a.move("l", D16(0x04, 6), D(0))
    a.andi("l", 0x1FF00, D(0))
    a.beq("w0")
    a.label("w1")
    a.move("l", D16(0x04, 6), D(0))
    a.andi("l", 0x1FF00, D(0))
    a.bne("w1")
    a.tst("w", D(4))
    a.bne("loopb")
    a.label("loopa")
    a.bra("loopa")
    a.label("loopb")
    a.nop()
    a.bra("loopb")
    a.label("done")
    a.move("w", IMM(0x0004, "w"), D16(0x9A, 6))  # INTENA: SOFT off
    a.addq("w", 1, D(6))
    a.cmpi("w", GROUPS, D(6))
    a.bne("tests")
    # Show the 64 values, 8 a row, and stop with the planes on.
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
    a.cmpi("w", GROUPS * 2, D(6))
    a.bne("rows")
    a.move("w", IMM(0x1200, "w"), D16(0x100, 6))
    a.move("w", IMM(0x8380, "w"), D16(0x96, 6))
    a.label("halt")
    if harness:  # back to tools/amiga_hwdisk.py's harness
        a.rts()
    else:
        a.bra("halt")

    # The handler: store VHPOSR, acknowledge; after the 16th value return
    # to `done` instead.
    a.label("irq1")
    a.move("w", D16(0x06, 6), D(5))
    a.move("w", D(5), POSTINC(2))
    a.move("w", IMM(0x0004, "w"), D16(0x9C, 6))
    a.move("w", A(2), D(5))
    a.andi("w", 2 * PROBES - 1, D(5))
    a.bne("rte")
    a.emit(0x2F7C, a.labels["done"] >> 16, a.labels["done"] & 0xFFFF, 2)  # MOVE.L #done,2(SP)
    a.label("rte")
    a.emit(0x4E73)  # RTE

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
    for con0, dma, kind in TESTS:
        code += struct.pack(">HHH", con0, dma, kind)
    while len(code) % 4:
        code += b"\0"
    return bytes(code), a.labels["irq1"]


def main():
    out = sys.argv[1]
    prog, irq1 = build_main()
    # The boot code sets the level-1 vector before jumping to the program.
    b = Asm(0)
    b.lea(D16(0, 0), 0)  # patched to LEA prog(PC),A0
    b.lea(ABSL(CODE), 1)
    b.move("w", IMM(len(prog) // 4 - 1, "w"), D(0))
    b.label("copy")
    b.move("l", POSTINC(0), POSTINC(1))
    b.dbra(0, "copy")
    b.move("l", IMM(irq1, "l"), ABSL(0x64))
    b.jmp(CODE)
    boot = bytearray(b.finish())
    prog_off = 12 + len(boot)
    boot[0:4] = struct.pack(">HH", 0x41FA, (prog_off - (12 + 2)) & 0xFFFF)
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
    print("program bytes", len(prog), "boot code", len(boot), "irq1", hex(irq1))


if __name__ == "__main__":
    main()
