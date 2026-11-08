"""Build the seventeenth bus test: the time of short instruction
sequences on the chip bus, measured with two VHPOSR reads, for the
hardware disk (tools/amiga_hwdisk.py).

Bus tests 14-16 timed interrupt handlers against the owner's A500 and
WinUAE (which agrees with the A500 there value for value) and found
mimic a few colour clocks off in a handler's tail. This test times
sequences directly: for each snippet it waits for the next line and
colour clock $18 (past the refresh slots), reads VHPOSR, runs the
snippet, reads VHPOSR again and stores both words. No DMA, interrupts
off. A0 and A1 point to chip RAM, A6 to the custom chips; the snippets
may use D0-D5, A0, A1, A4, A5.

The difference of the two reads, in colour clocks, minus the empty
snippet's is the snippet's time including every wait the chip bus
imposes; `latencies` gives the plain differences (the hardware disk
tool prints them, 16 to a group).

usage: used by tools/amiga_hwdisk.py; `python tools/amiga_bustest17.py`
       lists the snippets
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

RESULTS = LOOPMEM           # 64 pairs of words
SCRATCH = LOOPMEM + 0x1000  # what the snippets read and write
SAVE80 = LOOPMEM + 0x800    # the TRAP #0 vector (for RTE)
FONT_AT = 0x1A00            # the program is long: 64 snippets inline
HEAD_AT = FONT_AT + 0x80
START_H = 0x18              # each snippet starts after this colour clock

NOP = 0x4E71
LSL1 = 0xE388               # LSL.L #1,D0 (10 clocks)
CR = (0x302E, 0x001C)       # MOVE.W $1C(A6),D0: read INTENAR
CW = (0x3D40, 0x01FE)       # MOVE.W D0,$1FE(A6): write NO-OP
RD = (0x3010,)              # MOVE.W (A0),D0
WR = (0x3080,)              # MOVE.W D0,(A0)
RDL = (0x2010,)             # MOVE.L (A0),D0
WRL = (0x2080,)             # MOVE.L D0,(A0)

# (name, words); "BRAW", "JMPN", "RTS", "RTE" are built specially.
SNIPPETS = [
    ("empty", ()),
    ("nop", (NOP,)),
    ("nop x2", (NOP, NOP)),
    ("nop x3", (NOP, NOP, NOP)),
    ("chip read", RD),
    ("chip read x2", RD + RD),
    ("chip write", WR),
    ("chip write x2", WR + WR),
    ("custom read", CR),
    ("custom read x2", CR + CR),
    ("custom write", CW),
    ("custom write x2", CW + CW),
    ("nop, chip read", (NOP,) + RD),
    ("nop, chip write", (NOP,) + WR),
    ("nop, custom read", (NOP,) + CR),
    ("nop, custom write", (NOP,) + CW),
    ("lsl.l #1", (LSL1,)),
    ("lsl.l #1 x2", (LSL1, LSL1)),
    ("lsl.l #1, chip read", (LSL1,) + RD),
    ("lsl.l #1, chip write", (LSL1,) + WR),
    ("lsl.l #1, custom read", (LSL1,) + CR),
    ("lsl.l #1, custom write", (LSL1,) + CW),
    ("lsl.l #2", (0xE588,)),
    ("lsl.l #3", (0xE788,)),
    ("chip read.l", RDL),
    ("chip write.l", WRL),
    ("custom read, chip write", CR + WR),
    ("custom read, chip read", CR + RD),
    ("custom write, chip write", CW + WR),
    ("custom write, chip read", CW + RD),
    ("chip write, custom read", WR + CR),
    ("chip read, custom read", RD + CR),
    ("add.l d1,d0", (0xD081,)),
    ("adda.w d1,a4", (0xD8C1,)),
    ("mulu.w d1,d0 (d1=0)", (0x7200, 0xC0C1)),
    ("move.w #0,(a0)", (0x30BC, 0x0000)),
    ("move.w (a0),(a1)", (0x3290,)),
    ("move.w (a0)+,d0", (0x3018,)),
    ("move.w d0,-(a0)", (0x3100,)),
    ("move.w 2(a0),d0", (0x3028, 0x0002)),
    ("tst.w (a0)", (0x4A50,)),
    ("clr.w (a0)", (0x4250,)),
    ("addq.w #1,(a0)", (0x5250,)),
    ("bra.w next", "BRAW"),
    ("jmp (abs).l next", "JMPN"),
    ("pea, rts", "RTS"),
    ("frame, rte", "RTE"),
    ("movem.l d0/d1,(a0)", (0x48D0, 0x0003)),
    ("movem.l (a0),d0/d1", (0x4CD0, 0x0003)),
    ("movem.l d0/d1,-(a1)", (0x48E1, 0xC000)),
    ("move.w (a0),d0 x4", RD * 4),
    ("move.w d0,(a0) x4", WR * 4),
    ("custom read x4", CR * 4),
    ("custom write x4", CW * 4),
    ("lsl.l #1, custom read x2", (LSL1,) + CR + CR),
    ("nop, custom read x2", (NOP,) + CR + CR),
    ("custom read, nop, chip write", CR + (NOP,) + WR),
    ("custom read, lsl.l #1, chip write", CR + (LSL1,) + WR),
    ("move.w $06(a6),d0", (0x302E, 0x0006)),
    ("move.w $06(a6),(a0)", (0x30AE, 0x0006)),
    ("move.w $06(a6),(a0)+", (0x30EE, 0x0006)),
    ("move.w #$10,$9c(a6)", (0x3D7C, 0x0010, 0x009C)),
    ("move.w #$10,$9c(a6), custom read", (0x3D7C, 0x0010, 0x009C) + CR),
    ("move.w #$10,$9c(a6), chip write", (0x3D7C, 0x0010, 0x009C) + WR),
]
assert len(SNIPPETS) <= 64, len(SNIPPETS)


def build_main(harness=False):
    a = Asm(CODE)
    patches = []
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x9A))  # INTENA
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x96))  # DMACON: no DMA
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
    # RTE needs supervisor mode: the snippets run in a TRAP #0 handler.
    a.move("l", ABSL(0x80), ABSL(SAVE80))
    a.lea(ABSL(a.pc), 0)
    patches.append((len(a.words) - 2, "super"))
    a.move("l", A(0), ABSL(0x80))
    a.emit(0x4E40)                               # TRAP #0
    a.move("l", ABSL(SAVE80), ABSL(0x80))
    # Show the 128 words, 8 a row, and stop with the planes on.
    a.move("w", IMM(0x8380, "w"), D16(0x96, 6))
    a.lea(ABSL(COP_HEAD), 0)
    a.lea(ABSL(CODE + HEAD_AT), 1)
    a.moveq(bt.HEAD_WORDS - 1, 0)
    a.label("hd")
    a.move("w", POSTINC(1), POSTINC(0))
    a.dbra(0, "hd")
    a.move("l", IMM(0xFFFFFFFE, "l"), IND(0))
    a.move("l", IMM(COP_HEAD, "l"), D16(0x80, 6))
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
    a.cmpi("w", 16, D(6))
    a.bne("rows")
    a.move("w", IMM(0x1200, "w"), D16(0x100, 6))
    a.label("halt")
    if harness:  # back to tools/amiga_hwdisk.py's harness
        a.rts()
    else:
        a.bra("halt")

    a.label("super")
    a.lea(ABSL(RESULTS), 2)
    a.lea(D16(0x06, 6), 3)                       # A3 = VHPOSR
    for k, (name, body) in enumerate(SNIPPETS):
        a.lea(ABSL(SCRATCH), 0)
        a.lea(ABSL(SCRATCH + 0x100), 1)
        a.moveq(0, 1)
        # the next line, then colour clock START_H or later
        a.move("b", IND(3), D(0))
        a.label(f"w{k}a")
        a.emit(0xB013)                           # CMP.B (A3),D0
        a.beq(f"w{k}a")
        a.label(f"w{k}b")
        a.move("b", D16(1, 3), D(0))
        a.cmpi("b", START_H, D(0))
        a.emit(0x6500 | 0xF6)                    # BCS.S w{k}b (back 10)
        a.emit(0x3C13)                           # MOVE.W (A3),D6
        if body == "BRAW":
            a.fixups.append((len(a.words) + 1, f"n{k}", "w"))
            a.emit(0x6000, 0)                    # BRA.W next
        elif body == "JMPN":
            a.emit(0x4EF9, 0, 0)
            patches.append((len(a.words) - 2, f"n{k}"))
        elif body == "RTS":
            a.emit(0x4879, 0, 0)                 # PEA next
            patches.append((len(a.words) - 2, f"n{k}"))
            a.emit(0x4E75)                       # RTS
        elif body == "RTE":
            a.emit(0x4879, 0, 0)                 # PEA next
            patches.append((len(a.words) - 2, f"n{k}"))
            a.emit(0x40E7)                       # MOVE SR,-(SP)
            a.emit(0x4E73)                       # RTE
        else:
            a.emit(*body) if body else None
        a.label(f"n{k}")
        a.emit(0x3E13)                           # MOVE.W (A3),D7
        a.move("w", D(6), POSTINC(2))
        a.move("w", D(7), POSTINC(2))
    a.emit(0x4E73)                               # RTE: back from TRAP #0

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
    return bytes(code)


def cck(v):
    return (v >> 8) * 227 + (v & 0xFF)


def latencies(words):
    """Per snippet: the second VHPOSR read minus the first, colour clocks."""
    return [cck(words[i + 1]) - cck(words[i]) for i in range(0, len(words) - 1, 2)]


if __name__ == "__main__":
    prog = build_main()
    print(f"bus test 17: {len(prog)} bytes, {len(SNIPPETS)} snippets")
