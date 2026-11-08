"""Build the fourteenth bus test: where MOVEM samples IPL0-2, each probe
from a fixed start, for the hardware disk (tools/amiga_hwdisk.py).

Bus tests 12 and 13 let the main loop run on from one interrupt to the
next, so each probe's phase in the loop followed from the last probe's
latency and one wrong latency moved every later probe. Here every probe
starts the loop afresh at a fixed beam position:
  - the CPU waits in STOP #$2000;
  - the Copper writes INTREQ SET|COPER at line $40 + 4k, colour clock $21;
    the level-3 handler acknowledges it, reads VHPOSR (stored: the sync
    value) and returns into the group's loop;
  - the Copper writes INTREQ SET|SOFT on the same line at colour clock
    $85 + 2k: the trigger moves through the loop two colour clocks a probe;
  - the level-1 handler reads VHPOSR first (stored: the value), then
    returns to the STOP for the next probe (after 16 to the next group).
Groups (no bitplanes), one long instruction each:
  group 0: MOVEM.L D0/D1,(A0) + BRA.S
  group 1: MOVEM.L D0/D1/D3/D4/A1/A3,(A0) + BRA.S
  group 2: MOVEM.L D0-D7/A1/A3,(A0) + BRA.S
  group 3: LEA (abs).L,A0 + MOVEM.L D0/D1/D3/D4/A1/A3,-(A0) + BRA.S
The measurement runs in supervisor mode (STOP is privileged), entered
with TRAP #0. Each probe stores the sync value and the value (two words); the latency
is the value minus the trigger position in colour clocks. The colour
clock at which the latency jumps by a whole instruction is the sample
point (Motorola AN1012: the step that samples is instruction dependent).

usage: used by tools/amiga_hwdisk.py; `python tools/amiga_bustest14.py`
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
SAVE6C = LOOPMEM + 0x800    # the level-3 vector the test borrows
SAVE80 = LOOPMEM + 0x804    # the TRAP #0 vector, likewise
SCRATCH = LOOPMEM + 0x1000  # what the MOVEMs write
GROUPS = 4
PROBES = 16
LINE0 = 0x40                # probe k on line LINE0 + 4k
SYNC_H = 0x21               # the Copper's COPER (loop start)
TRIG_H = 0x85               # the Copper's SOFT at TRIG_H + 2k
COP_PROBES = COP_HEAD + 0x400
FONT_AT = 0x300
HEAD_AT = FONT_AT + 0x80
MOVEM_2 = 0x0003            # D0 D1 (bit 0 = D0 ... bit 15 = A7)
MOVEM_6 = 0x0A1B            # D0 D1 D3 D4 A1 A3
MOVEM_10 = 0x0AFF           # D0-D7 A1 A3
MOVEM_6_PREDEC = 0xD850     # D0 D1 D3 D4 A1 A3 for -(An) (bit 0 = A7 ... bit 15 = D0)


def trigger(k):
    """The SOFT trigger of probe k as a VHPOSR word."""
    return ((LINE0 + 4 * k) << 8) | (TRIG_H + 2 * k)


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
    """Bcc.W (op: 0x6600 BNE, 0x6700 BEQ, 0x6B00 BMI)."""
    a.fixups.append((len(a.words) + 1, label, "w"))
    a.emit(op, 0)


def lea_label(a, label, an):
    """LEA (abs).L of a label, patched by the caller's table."""
    a.lea(ABSL(a.pc), an)
    return (len(a.words) - 2, label)


def build_main(harness=False):
    a = Asm(CODE)
    patches = []
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
    a.move("l", ABSL(0x6C), ABSL(SAVE6C))
    patches.append(lea_label(a, "irq3", 0))
    a.move("l", A(0), ABSL(0x6C))
    # STOP is privileged and the hardware disk's harness runs as a task in
    # user mode: the measurement runs in a TRAP #0 handler and returns
    # with RTE (the same from supervisor mode).
    a.move("l", ABSL(0x80), ABSL(SAVE80))
    patches.append(lea_label(a, "super", 0))
    a.move("l", A(0), ABSL(0x80))
    a.emit(0x4E40)                               # TRAP #0
    a.move("l", ABSL(SAVE80), ABSL(0x80))
    a.move("l", ABSL(SAVE6C), ABSL(0x6C))
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
    a.label("super")
    # Two Copper lists: the head table and the end; the head table, the
    # 16 probes (sync and trigger) and the end.
    for base, probes in ((COP_HEAD, False), (COP_PROBES, True)):
        a.lea(ABSL(base), 0)
        a.lea(ABSL(CODE + HEAD_AT), 1)
        a.moveq(bt.HEAD_WORDS - 1, 0)
        a.label(f"hd{probes}")
        a.move("w", POSTINC(1), POSTINC(0))
        a.dbra(0, f"hd{probes}")
        if probes:
            a.move("w", IMM((LINE0 << 8) | SYNC_H, "w"), D(1))
            a.move("w", IMM((LINE0 << 8) | TRIG_H, "w"), D(3))
            a.moveq(PROBES - 1, 4)
            a.label("cp")
            a.move("w", D(1), POSTINC(0))
            a.move("w", IMM(0xFFFE, "w"), POSTINC(0))
            a.move("w", IMM(0x009C, "w"), POSTINC(0))  # INTREQ
            a.move("w", IMM(0x8010, "w"), POSTINC(0))  # SET | COPER
            a.move("w", D(3), POSTINC(0))
            a.move("w", IMM(0xFFFE, "w"), POSTINC(0))
            a.move("w", IMM(0x009C, "w"), POSTINC(0))  # INTREQ
            a.move("w", IMM(0x8004, "w"), POSTINC(0))  # SET | SOFT
            a.addi("w", 0x0400, D(1))
            a.addi("w", 0x0402, D(3))
            a.dbra(4, "cp")
        a.move("l", IMM(0xFFFFFFFE, "l"), IND(0))
    a.moveq(0, 6)                                # D6 = group
    a.label("tests")
    a.move("w", IMM(0x0200, "w"), D16(0x100, 6))  # BPLCON0: no planes
    a.move("w", IMM(0x7FFF, "w"), D16(0x96, 6))
    a.move("w", IMM(0x8280, "w"), D16(0x96, 6))  # DMACON: Copper
    a.move("w", IMM(0x0014, "w"), D16(0x9C, 6))  # INTREQ: COPER, SOFT off
    # COP1LC takes the probe list at the next field; the first field with
    # it is thrown away, then the requests it left are cleared.
    a.move("l", IMM(COP_PROBES, "l"), D16(0x80, 6))  # COP1LC
    patches.append(lea_label(a, "gdone", 4))     # A4 = gdone
    wait_line0(a, "c1")
    wait_line0(a, "c2")
    a.move("w", IMM(0x0014, "w"), D16(0x9C, 6))  # INTREQ: COPER, SOFT off
    a.lea(ABSL(RESULTS), 2)
    a.move("w", D(6), D(0))
    a.lsl_w(6, 0)                                # 64 bytes a group
    a.emit(0xD4C0)                               # ADDA.W D0,A2
    a.moveq(PROBES, 2)
    a.lea(ABSL(SCRATCH), 0)
    # A5 = the group's STOP, where the level-1 handler returns to.
    a.move("w", D(6), D(5))
    patches.append(lea_label(a, "s0", 5))
    a.subq("w", 1, D(5))
    bcc_w(a, 0x6B00, "go")               # BMI
    patches.append(lea_label(a, "s1", 5))
    a.subq("w", 1, D(5))
    bcc_w(a, 0x6B00, "go")               # BMI
    patches.append(lea_label(a, "s2", 5))
    a.subq("w", 1, D(5))
    bcc_w(a, 0x6B00, "go")               # BMI
    patches.append(lea_label(a, "s3", 5))
    a.label("go")
    a.move("w", IMM(0xC014, "w"), D16(0x9A, 6))  # INTENA: INTEN | COPER | SOFT
    a.emit(0x4ED5)                               # JMP (A5)
    a.label("s0")
    a.emit(0x4E72, 0x2000)                       # STOP #$2000
    a.label("g0")
    a.emit(0x48D0, MOVEM_2)                      # MOVEM.L D0/D1,(A0)
    a.bra("g0")
    a.label("s1")
    a.emit(0x4E72, 0x2000)
    a.label("g1")
    a.emit(0x48D0, MOVEM_6)                      # MOVEM.L 6 regs,(A0)
    a.bra("g1")
    a.label("s2")
    a.emit(0x4E72, 0x2000)
    a.label("g2")
    a.emit(0x48D0, MOVEM_10)                     # MOVEM.L 10 regs,(A0)
    a.bra("g2")
    a.label("s3")
    a.emit(0x4E72, 0x2000)
    a.label("g3")
    a.lea(ABSL(SCRATCH + 0x100), 0)
    a.emit(0x48E0, MOVEM_6_PREDEC)               # MOVEM.L 6 regs,-(A0)
    a.bra("g3")
    a.label("gdone")
    a.move("w", IMM(0x0014, "w"), D16(0x9A, 6))  # INTENA: COPER, SOFT off
    a.move("l", IMM(COP_HEAD, "l"), D16(0x80, 6))
    a.addq("w", 1, D(6))
    a.cmpi("w", GROUPS, D(6))
    bcc_w(a, 0x6600, "tests")
    a.emit(0x4E73)                               # RTE: back from TRAP #0

    # Level 3 (the sync): acknowledge COPER, store VHPOSR, back into the
    # loop after the STOP.
    a.label("irq3")
    a.move("w", IMM(0x0010, "w"), D16(0x9C, 6))
    a.move("w", D16(0x06, 6), POSTINC(2))
    a.emit(0x4E73)                               # RTE
    # Level 1 (the probe): VHPOSR first, acknowledge SOFT, store; back to
    # the group's STOP, after D2 probes to A4 (gdone).
    a.label("irq1")
    a.move("w", D16(0x06, 6), D(5))
    a.move("w", IMM(0x0004, "w"), D16(0x9C, 6))
    a.move("w", D(5), POSTINC(2))
    a.emit(0x2F4D, 0x0002)                       # MOVE.L A5,2(SP)
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
    for at, label in patches:
        a.words[at] = a.labels[label] >> 16
        a.words[at + 1] = a.labels[label] & 0xFFFF
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


def cck(v):
    return (v >> 8) * 227 + (v & 0xFF)


def latencies(words):
    """128 words (sync, value pairs) to 64 latencies: value minus trigger
    in colour clocks."""
    return [cck(words[i + 1]) - cck(trigger((i // 2) % PROBES)) for i in range(0, len(words) - 1, 2)]


def sync_offsets(words):
    """The trigger's distance from the sync read, per probe (constant when
    every loop starts alike)."""
    return [cck(trigger((i // 2) % PROBES)) - cck(words[i]) for i in range(0, len(words) - 1, 2)]


if __name__ == "__main__":
    prog, irq1 = build_main()
    print(f"bus test 14: {len(prog)} bytes, handler at ${irq1:06X}")
