"""The Amiga hardware disk: one bootable ADF that checks the Greaseweazle
path end to end and takes the bus measurements on a real A500 or A1200
without a monitor. Results go onto the disk itself.

1. `build OUT.adf` makes the disk. Write it with the Greaseweazle
   (`gw write --format amiga.amigados OUT.adf`) and boot it.
2. The Amiga (any Kickstart, 512 KiB chip RAM is enough):
   - notes its chips: exec version, AttnFlags (CPU), chip RAM size,
     VPOSR (Agnus/Alice id), DENISEID (Denise/Lisa id), the ROM's
     version word;
   - reads every block through trackdisk and checks the pattern this
     tool put there: proves that a real Amiga reads what the Greaseweazle
     wrote;
   - runs the bus tests (tools/amiga_bustest.py, 2, 3, 4, 5, 6, 7, as
     `build_main(harness=True)`: they leave the OS's CIA masks alone and
     return) one after another, keeping plane 1 of each, where its numbers
     are drawn;
   - writes all of it through trackdisk to blocks 1000 on (proves that the
     Amiga writes what the Greaseweazle can read back);
   - then blinks the power LED: slowly (about 1 Hz) when everything
     worked, quickly when a read or write failed or memory was taken.
   The pattern check reads the whole disk (about a minute), the tests take
   seconds; wait for the slow blinking.
3. Read the disk back (`mimic-headless --import-flux back.adf --gw-port
   COM5 --gw-format amiga`, or `gw read`) and run `results back.adf`: the
   chips, the pattern check (also checked here again, block by block) and
   each test's numbers, decoded from the planes with the tests' font; with
   `--against other.adf` side by side with another run (mimic's: the test
   `hardware_disk_runs_and_writes_its_results` in tests/tests/amiga_boot.rs
   saves one).

Memory: the boot block reserves 0x30000-0x7E7FF with AllocAbs (the bus
tests' fixed addresses; the harness at 0x78000); the planes are kept in a
buffer from AllocMem until the write.

usage: python tools/amiga_hwdisk.py build OUT.adf
       python tools/amiga_hwdisk.py results IN.adf [--against OTHER.adf]
"""
import importlib.util
import os
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def load(name):
    spec = importlib.util.spec_from_file_location(name, os.path.join(HERE, name + ".py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


bt = load("amiga_bustest")
Asm, D, A, IND, POSTINC, PREDEC, D16, ABSL, IMM = (
    bt.Asm, bt.D, bt.A, bt.IND, bt.POSTINC, bt.PREDEC, bt.D16, bt.ABSL, bt.IMM)
CUSTOM, PLANES, PLANE_SIZE, ROWBYTES, CODE = bt.CUSTOM, bt.PLANES, bt.PLANE_SIZE, bt.ROWBYTES, bt.CODE

BLOCKS = 1760
TRACK = 11 * 512
HARN = 0x78000          # the harness (inside the reserved range)
HARN_BLOCK = 2           # on the disk from block 2
HARN_BLOCKS = 16
PROG_BLOCK = 32          # the tests from block 32, 16 blocks each
PROG_BLOCKS = 16
RESULT_BLOCK = 1000      # the results: info block, then the planes
TESTS = ["amiga_bustest", "amiga_bustest2", "amiga_bustest3", "amiga_bustest4", "amiga_bustest5",
         "amiga_bustest6", "amiga_bustest7"]
RESULT_LEN = (512 + len(TESTS) * PLANE_SIZE + 511) // 512 * 512
RESULT_BLOCKS = RESULT_LEN // 512
PATTERN_KEY = 0xA5A55A5A
MAGIC = b"MIMICHW1"   # MIMICHW0: the boot block failure record

# harness memory (all inside the reserved range, above the tests')
VARS = HARN + 0x2000
IOREQ, EXEC, CAPT, BADS, FIRSTBAD, READERR, WRITEERR, OLD64, SAVEINT, SAVEDMA, FLAGS = (
    VARS + 4 * i for i in range(11))
INFO = VARS + 0x100
TABLE = HARN + 0x1F00    # the tests' level-1 handlers (0 = none)
RBUF = VARS + 0x400

# exec and trackdisk
ALLOCMEM, DOIO, CACHECLEARU = -198, -456, -636
CMD_READ, CMD_WRITE, CMD_UPDATE, TD_MOTOR = 2, 3, 4, 9
IO_COMMAND, IO_ERROR, IO_LENGTH, IO_DATA, IO_OFFSET = 0x1C, 0x1F, 0x24, 0x28, 0x2C


def excluded(b):
    """Blocks the pattern does not cover: boot block, harness, tests, results."""
    return b < PROG_BLOCK + len(TESTS) * PROG_BLOCKS or RESULT_BLOCK <= b < RESULT_BLOCK + RESULT_BLOCKS


def pattern(b):
    return b"".join(struct.pack(">I", ((b << 16) | j) ^ PATTERN_KEY) for j in range(128))


def bccw(a, cc, label):
    """Bcc with a word displacement."""
    a.fixups.append((len(a.words) + 1, label, "w"))
    a.emit(0x6000 | (cc << 8), 0)


def jsr_lib(a, off):
    a.emit(0x4EAE, off & 0xFFFF)  # JSR off(A6)


def doio(a, cmd, data, length, offset_dn=None, offset=0):
    """trackdisk: io_Command, io_Data, io_Length, io_Offset (a constant or
    a data register), DoIO; A1 = the IORequest after it."""
    a.movea("l", ABSL(IOREQ), 1)
    a.move("w", IMM(cmd, "w"), D16(IO_COMMAND, 1))
    a.move("l", IMM(data, "l"), D16(IO_DATA, 1))
    a.move("l", IMM(length, "l"), D16(IO_LENGTH, 1))
    if offset_dn is None:
        a.move("l", IMM(offset, "l"), D16(IO_OFFSET, 1))
    else:
        a.move("l", D(offset_dn), D16(IO_OFFSET, 1))
    a.movea("l", ABSL(EXEC), 6)
    jsr_lib(a, DOIO)
    a.movea("l", ABSL(IOREQ), 1)


def copy_longs(a, src, dst, n):
    a.lea(ABSL(src), 0)
    a.lea(ABSL(dst), 1)
    a.move("w", IMM(n - 1, "w"), D(0))
    lbl = f"cp{len(a.words)}"
    a.label(lbl)
    a.move("l", POSTINC(0), POSTINC(1))
    a.dbra(0, lbl)


def build_harness(irqs):
    a = Asm(HARN)
    a.move("l", A(1), ABSL(IOREQ))
    a.move("l", A(6), ABSL(EXEC))
    for v in (BADS, READERR, WRITEERR, CAPT):
        a.move("l", IMM(0, "l"), ABSL(v))
    a.move("l", IMM(0xFFFFFFFF, "l"), ABSL(FIRSTBAD))
    # --- the chips
    a.lea(ABSL(INFO), 0)
    a.move("w", IMM(127, "w"), D(0))
    a.label("clr")
    a.move("l", IMM(0, "l"), POSTINC(0))
    a.dbra(0, "clr")
    a.move("l", IMM(struct.unpack(">I", MAGIC[:4])[0], "l"), ABSL(INFO))
    a.move("l", IMM(struct.unpack(">I", MAGIC[4:])[0], "l"), ABSL(INFO + 4))
    a.move("w", D16(20, 6), ABSL(INFO + 8))      # exec LIB_VERSION
    a.move("w", D16(296, 6), ABSL(INFO + 10))    # AttnFlags
    a.move("l", D16(62, 6), ABSL(INFO + 12))     # MaxLocMem
    a.move("w", ABSL(CUSTOM + 0x04), ABSL(INFO + 16))  # VPOSR
    a.move("w", ABSL(CUSTOM + 0x7C), ABSL(INFO + 18))  # DENISEID
    a.move("l", ABSL(0xF8000C), ABSL(INFO + 20))       # the ROM's version
    # --- every track: read, check the pattern
    a.moveq(0, 7)
    a.label("track")
    a.move("w", D(7), D(0))
    a.emit(0xC0FC, TRACK)                        # MULU #TRACK,D0
    doio(a, CMD_READ, RBUF, TRACK, offset_dn=0)
    a.tst("b", D16(IO_ERROR, 1))
    a.beq("readok")
    a.addq("l", 1, ABSL(READERR))
    bccw(a, 0, "nexttrack")
    a.label("readok")
    a.move("w", D(7), D(3))
    a.emit(0xC6FC, 11)                           # MULU #11,D3: first block
    a.lea(ABSL(RBUF), 0)
    a.moveq(10, 2)
    a.label("blk")
    a.movea("l", A(0), 2)
    a.cmpi("w", PROG_BLOCK + len(TESTS) * PROG_BLOCKS, D(3))
    bccw(a, 5, "nextblk")                        # BCS: below, not covered
    a.cmpi("w", RESULT_BLOCK, D(3))
    a.bcc(5, "check")
    a.cmpi("w", RESULT_BLOCK + RESULT_BLOCKS, D(3))
    bccw(a, 5, "nextblk")
    a.label("check")
    a.move("w", D(3), D(0))
    a.swap(0)
    a.emit(0x4240)                               # CLR.W D0
    a.move("w", IMM(127, "w"), D(1))
    a.label("lw")
    a.move("l", D(0), D(4))
    a.emit(0x0A84, PATTERN_KEY >> 16, PATTERN_KEY & 0xFFFF)  # EORI.L #K,D4
    a.emit(0xB898)                               # CMP.L (A0)+,D4
    a.bne("bad")
    a.addq("l", 1, D(0))
    a.dbra(1, "lw")
    a.bra("nextblk")
    a.label("bad")
    a.addq("l", 1, ABSL(BADS))
    a.tst("l", ABSL(FIRSTBAD))
    a.bcc(10, "nextblk")                         # BPL: one noted already (-1 = none)
    a.label("first")
    a.moveq(0, 0)
    a.move("w", D(3), D(0))
    a.move("l", D(0), ABSL(FIRSTBAD))
    a.label("nextblk")
    a.lea(D16(512, 2), 0)
    a.addq("w", 1, D(3))
    a.dbra(2, "blk")
    a.label("nexttrack")
    a.addq("w", 1, D(7))
    a.cmpi("w", BLOCKS // 11, D(7))
    bccw(a, 6, "track")                          # BNE
    doio(a, TD_MOTOR, 0, 0)
    a.move("l", ABSL(BADS), ABSL(INFO + 24))
    a.move("l", ABSL(FIRSTBAD), ABSL(INFO + 28))
    a.move("l", ABSL(READERR), ABSL(INFO + 32))
    # --- a buffer for the planes
    a.move("l", IMM(len(TESTS) * PLANE_SIZE, "l"), D(0))
    a.move("l", IMM(0x10001, "l"), D(1))         # MEMF_PUBLIC | MEMF_CLEAR
    a.movea("l", ABSL(EXEC), 6)
    jsr_lib(a, ALLOCMEM)
    a.move("l", D(0), ABSL(CAPT))
    bccw(a, 7, "write")                          # none: no tests
    # --- the tests
    a.moveq(0, 7)
    a.label("test")
    a.move("w", D(7), D(0))
    a.emit(0xC0FC, PROG_BLOCKS * 512)            # MULU
    a.addi("l", PROG_BLOCK * 512, D(0))
    doio(a, CMD_READ, CODE, PROG_BLOCKS * 512, offset_dn=0)
    doio(a, TD_MOTOR, 0, 0)
    a.movea("l", ABSL(EXEC), 6)
    a.cmpi("w", 37, D16(20, 6))
    a.bcc(5, "nocache")                          # BCS: no CacheClearU before V37
    jsr_lib(a, CACHECLEARU)
    a.label("nocache")
    a.move("l", ABSL(0x64), ABSL(OLD64))
    # the level-1 vector of the tests that have a handler
    a.lea(ABSL(TABLE), 0)                        # table of handlers (0 = none)
    a.move("w", D(7), D(0))
    a.lsl_w(2, 0)
    a.emit(0x2030, 0x0000)                       # MOVE.L 0(A0,D0.W),D0
    a.beq("novec")
    a.move("l", D(0), ABSL(0x64))
    a.label("novec")
    a.move("w", ABSL(CUSTOM + 0x1C), ABSL(SAVEINT))  # INTENAR
    a.move("w", ABSL(CUSTOM + 0x02), ABSL(SAVEDMA))  # DMACONR
    a.emit(0x48E7, 0xFFFE)                       # MOVEM.L D0-D7/A0-A6,-(SP)
    a.emit(0x4EB9, CODE >> 16, CODE & 0xFFFF)   # JSR CODE
    a.emit(0x4CDF, 0x7FFF)                       # MOVEM.L (SP)+,D0-D7/A0-A6
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x96))
    a.move("w", ABSL(SAVEDMA), D(0))
    a.ori("w", 0x8000, D(0))
    a.move("w", D(0), ABSL(CUSTOM + 0x96))
    a.move("l", ABSL(OLD64), ABSL(0x64))
    a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x9A))
    a.move("w", ABSL(SAVEINT), D(0))
    a.ori("w", 0xC000, D(0))
    a.move("w", D(0), ABSL(CUSTOM + 0x9A))
    # keep plane 1
    a.lea(ABSL(PLANES), 0)
    a.movea("l", ABSL(CAPT), 1)
    a.move("w", D(7), D(0))
    a.emit(0xC0FC, PLANE_SIZE)                   # MULU
    a.emit(0xD3C0)                               # ADDA.L D0,A1
    a.move("w", IMM(PLANE_SIZE // 4 - 1, "w"), D(0))
    a.label("keep")
    a.move("l", POSTINC(0), POSTINC(1))
    a.dbra(0, "keep")
    a.addq("w", 1, D(7))
    a.cmpi("w", len(TESTS), D(7))
    bccw(a, 6, "test")
    # --- everything onto the disk (from chip RAM at 0x30000)
    a.label("write")
    copy_longs(a, INFO, 0x30000, 128)
    a.tst("l", ABSL(CAPT))
    a.beq("nocapt")
    a.movea("l", ABSL(CAPT), 0)
    a.lea(ABSL(0x30200), 1)
    a.move("w", IMM(len(TESTS) * PLANE_SIZE // 4 - 1, "w"), D(0))
    a.label("cpc")
    a.move("l", POSTINC(0), POSTINC(1))
    a.dbra(0, "cpc")
    a.label("nocapt")
    doio(a, CMD_WRITE, 0x30000, RESULT_LEN, offset=RESULT_BLOCK * 512)
    a.move("b", D16(IO_ERROR, 1), D(0))
    a.move("b", D(0), ABSL(WRITEERR + 3))
    doio(a, CMD_UPDATE, 0, 0)
    a.move("b", D16(IO_ERROR, 1), D(0))
    a.emit(0x8139, (WRITEERR + 3) >> 16, (WRITEERR + 3) & 0xFFFF)  # OR.B D0,(abs).L
    doio(a, TD_MOTOR, 0, 0)
    # --- the power LED: slowly when all went well
    a.moveq(25, 5)                               # frames a half period
    a.tst("l", ABSL(WRITEERR))
    a.bne("fast")
    a.tst("l", ABSL(READERR))
    a.bne("fast")
    a.tst("l", ABSL(BADS))
    a.bne("fast")
    a.tst("l", ABSL(CAPT))
    a.bne("blink")
    a.label("fast")
    a.moveq(5, 5)
    a.label("blink")
    a.emit(0x0879, 0x0001, 0x00BF, 0xE001)       # BCHG #1,$BFE001
    a.move("w", D(5), D(6))
    a.label("frames")
    a.label("f0")                                # leave line 0, wait for it
    a.move("w", ABSL(CUSTOM + 0x06), D(0))
    a.lsr_w(8, 0)
    a.beq("f0")
    a.label("f1")
    a.move("w", ABSL(CUSTOM + 0x06), D(0))
    a.lsr_w(8, 0)
    a.bne("f1")
    a.dbra(6, "frames")
    a.bra("blink")
    code = bytearray(a.finish())
    # the handler table, part of the harness's image
    table_at = TABLE - HARN
    assert len(code) <= table_at, len(code)
    code += bytes(table_at - len(code))
    for irq in irqs:
        code += struct.pack(">I", irq)
    assert len(code) <= HARN_BLOCKS * 512
    return bytes(code)


def build_boot(harness_len):
    """Boot block: reserve 0x30000-0x7E7FF, read the harness, jump to it
    (A1 = trackdisk's IORequest, A6 = ExecBase); when the memory is taken,
    blink the power LED quickly."""
    b = Asm(0)
    b.move("l", A(1), PREDEC(7))
    # Up to $7E800 only: a 512 KB Kickstart 1.3 machine keeps the top
    # 6 KB for its stack (the owner's A500 refused $50000 bytes).
    b.move("l", IMM(0x4E800, "l"), D(0))
    b.lea(ABSL(0x30000), 1)
    jsr_lib(b, -204)                             # AllocAbs
    b.movea("l", POSTINC(7), 1)
    b.tst("l", D(0))
    b.beq("fail")
    b.move("l", A(1), PREDEC(7))
    b.move("w", IMM(CMD_READ, "w"), D16(IO_COMMAND, 1))
    b.move("l", IMM(HARN, "l"), D16(IO_DATA, 1))
    b.move("l", IMM(harness_len, "l"), D16(IO_LENGTH, 1))
    b.move("l", IMM(HARN_BLOCK * 512, "l"), D16(IO_OFFSET, 1))
    jsr_lib(b, DOIO)
    b.movea("l", POSTINC(7), 1)
    b.jmp(HARN)
    b.label("fail")
    # The memory is taken: write exec's memory list (AttnFlags, MaxLocMem,
    # every MemHeader with its first chunks) to the result block, so
    # `results` can say where the free memory is, then blink.
    b.move("l", A(1), PREDEC(7))
    b.move("l", IMM(512, "l"), D(0))
    b.move("l", IMM(0x10002, "l"), D(1))         # MEMF_CHIP | MEMF_CLEAR
    jsr_lib(b, -198)                             # AllocMem
    b.movea("l", POSTINC(7), 1)
    b.tst("l", D(0))
    bccw(b, 7, "blink")                          # BEQ
    b.emit(0x2440, 0x2640)                       # MOVEA.L D0,A2; MOVEA.L D0,A3
    b.move("l", IMM(0x4D494D49, "l"), POSTINC(2))  # "MIMI"
    b.move("l", IMM(0x43485730, "l"), POSTINC(2))  # "CHW0"
    b.move("w", D16(0x128, 6), POSTINC(2))       # AttnFlags
    b.move("w", D16(0x14, 6), POSTINC(2))        # SoftVer
    b.move("l", D16(0x3E, 6), POSTINC(2))        # MaxLocMem
    b.movea("l", D16(0x142, 6), 0)               # MemList head
    b.label("hdr")
    b.move("l", IND(0), D(0))                    # ln_Succ: 0 at the tail
    b.beq("done")
    b.move("l", D16(0x14, 0), POSTINC(2))        # mh_Lower
    b.move("l", D16(0x18, 0), POSTINC(2))        # mh_Upper
    b.move("l", D16(0x1C, 0), POSTINC(2))        # mh_Free
    b.move("w", D16(0x0E, 0), POSTINC(2))        # mh_Attributes
    b.movea("l", D16(0x10, 0), 4)                # mh_First
    b.moveq(15, 1)
    b.label("chunk")
    b.move("l", A(4), D(0))
    b.beq("endc")
    b.move("l", A(4), POSTINC(2))
    b.move("l", D16(4, 4), POSTINC(2))           # mc_Bytes
    b.movea("l", IND(4), 4)                      # mc_Next
    b.dbra(1, "chunk")
    b.label("endc")
    b.move("l", IMM(0xFFFFFFFF, "l"), POSTINC(2))
    b.move("l", A(2), D(0))
    b.emit(0x908B)                               # SUB.L A3,D0
    b.cmpi("l", 360, D(0))
    bccw(b, 4, "done")                           # BCC: the block is full
    b.movea("l", IND(0), 0)
    b.bra("hdr")
    b.label("done")
    b.move("l", IMM(0, "l"), POSTINC(2))
    b.move("l", A(1), PREDEC(7))
    b.move("w", IMM(CMD_WRITE, "w"), D16(IO_COMMAND, 1))
    b.move("l", A(3), D16(IO_DATA, 1))
    b.move("l", IMM(512, "l"), D16(IO_LENGTH, 1))
    b.move("l", IMM(RESULT_BLOCK * 512, "l"), D16(IO_OFFSET, 1))
    jsr_lib(b, DOIO)
    b.movea("l", IND(7), 1)
    b.move("w", IMM(CMD_UPDATE, "w"), D16(IO_COMMAND, 1))
    jsr_lib(b, DOIO)
    b.movea("l", IND(7), 1)
    b.move("w", IMM(TD_MOTOR, "w"), D16(IO_COMMAND, 1))
    b.move("l", IMM(0, "l"), D16(IO_LENGTH, 1))
    jsr_lib(b, DOIO)
    b.label("blink")
    b.emit(0x0879, 0x0001, 0x00BF, 0xE001)       # BCHG #1,$BFE001
    b.move("l", IMM(60000, "l"), D(0))
    b.label("wait")
    b.subq("l", 1, D(0))
    b.bne("wait")
    b.bra("blink")
    return b.finish()


def build(out):
    progs, irqs = [], []
    for name in TESTS:
        m = load(name)
        r = m.build_main(harness=True)
        prog, irq = (r if isinstance(r, tuple) else (r, 0))
        assert len(prog) <= PROG_BLOCKS * 512, (name, len(prog))
        progs.append(prog)
        irqs.append(irq)
    harness = build_harness(irqs)
    harness += bytes(-len(harness) % 512)
    boot = build_boot(len(harness))
    adf = bytearray(BLOCKS * 512)
    for blk in range(BLOCKS):
        if not excluded(blk) or RESULT_BLOCK <= blk < RESULT_BLOCK + RESULT_BLOCKS:
            adf[blk * 512:(blk + 1) * 512] = pattern(blk)
    adf[0:1024] = bytes(1024)
    adf[0:4] = b"DOS\0"
    adf[8:12] = struct.pack(">I", 880)
    adf[12:12 + len(boot)] = boot
    adf[HARN_BLOCK * 512:HARN_BLOCK * 512 + len(harness)] = harness
    for k, prog in enumerate(progs):
        at = (PROG_BLOCK + k * PROG_BLOCKS) * 512
        adf[at:at + len(prog)] = prog
    s = 0
    for i in range(0, 1024, 4):
        s += struct.unpack(">I", adf[i:i + 4])[0]
        s = (s & 0xFFFFFFFF) + (s >> 32)
    adf[4:8] = struct.pack(">I", (~s) & 0xFFFFFFFF)
    open(out, "wb").write(adf)
    print(f"{out}: boot {len(boot)} bytes, harness {len(harness)}, tests "
          + ", ".join(str(len(p)) for p in progs))


# --- reading the results back -------------------------------------------

GLYPHS = {bytes(rows): d for d, rows in bt.FONT.items()}


def numbers(plane):
    """The 4-digit hex numbers drawn in a plane, in reading order."""
    rows = len(plane) // ROWBYTES
    found = []
    for y in range(rows - 7):
        x = 0
        while x < ROWBYTES:
            digits = []
            while x < ROWBYTES:
                cell = bytes(plane[(y + r) * ROWBYTES + x] for r in range(8))
                if cell not in GLYPHS or not any(cell):
                    break
                digits.append(GLYPHS[cell])
                x += 1
            if len(digits) >= 4:
                found.append((y, x - len(digits), int("".join("%X" % d for d in digits), 16)))
            x += 1
    return [v for _, _, v in sorted(found)]


def read_results(path):
    adf = open(path, "rb").read()
    assert len(adf) == BLOCKS * 512, f"{path}: not a DD ADF"
    info = adf[RESULT_BLOCK * 512:RESULT_BLOCK * 512 + 512]
    if info[:8] != MAGIC:
        return None, adf
    return info, adf


def describe(info):
    ver, attn = struct.unpack(">HH", info[8:12])
    chip, = struct.unpack(">I", info[12:16])
    vposr, denise = struct.unpack(">HH", info[16:20])
    rom, bads, first, rderr = struct.unpack(">IiiI", info[20:36])
    cpu = "68000"
    for bit, name in ((7, "68060"), (3, "68040"), (2, "68030"), (1, "68020"), (0, "68010")):
        if attn & (1 << bit):
            cpu = name
            break
    agnus = {0x00: "OCS PAL", 0x10: "OCS NTSC", 0x20: "ECS PAL", 0x30: "ECS NTSC",
             0x22: "AGA PAL", 0x32: "AGA NTSC"}.get((vposr >> 8) & 0x7F, f"${(vposr >> 8) & 0x7F:02X}")
    lisa = {0xF8: "Lisa (AGA)", 0xFC: "ECS Denise"}.get(denise & 0xFF, f"no id (${denise:04X}: OCS)")
    return [f"exec V{ver}, ROM version word ${rom:08X}, CPU {cpu} (AttnFlags ${attn:04X})",
            f"chip RAM up to ${chip:06X}, Agnus/Alice {agnus} (VPOSR ${vposr:04X}), Denise {lisa}",
            f"pattern: {bads} bad blocks" + (f" (first {first})" if bads else "")
            + f", {rderr} tracks unreadable"]


def memory_map(block):
    """The boot block's failure record: AttnFlags, SoftVer, MaxLocMem, then
    per MemHeader lower, upper, free, attributes and its chunks (address,
    bytes) up to a -1."""
    attn, softver, maxloc = struct.unpack(">HHI", block[8:16])
    lines = [f"AttnFlags ${attn:04X}, SoftVer {softver}, MaxLocMem ${maxloc:06X}"]
    at = 16
    while at + 14 <= 512:
        lower, upper, free, attrs = struct.unpack(">IIIH", block[at:at + 14])
        if lower == 0:
            break
        kind = "chip" if attrs & 2 else "fast"
        lines.append(f"{kind} ${lower:06X}-${upper:06X}, free ${free:X} (attributes ${attrs:04X})")
        at += 14
        while at + 8 <= 512:
            addr, size = struct.unpack(">II", block[at:at + 8])
            at += 8
            if addr == 0xFFFFFFFF:
                break
            lines.append(f"    free ${addr:06X}-${addr + size:06X} ({size} bytes)")
    return lines


def results(path, against=None):
    info, adf = read_results(path)
    bad = [b for b in range(BLOCKS) if not excluded(b) and adf[b * 512:(b + 1) * 512] != pattern(b)]
    print(f"{path}: pattern on this image: {len(bad)} blocks differ"
          + (f" (first {bad[:8]})" if bad else ""))
    if info is None:
        if adf[RESULT_BLOCK * 512:RESULT_BLOCK * 512 + 8] == b"MIMICHW0":
            print("the boot block could not reserve $30000-$7E7FF; exec's memory list:")
            for line in memory_map(adf[RESULT_BLOCK * 512:RESULT_BLOCK * 512 + 512]):
                print("  " + line)
        else:
            print("no results: the Amiga has not written them (no slow blinking?)")
        return
    for line in describe(info):
        print("  " + line)
    other = None
    if against:
        oinfo, oadf = read_results(against)
        if oinfo is not None:
            other = oadf
            print(f"against {against}:")
            for line in describe(oinfo):
                print("  " + line)
    for k, name in enumerate(TESTS):
        at = RESULT_BLOCK * 512 + 512 + k * PLANE_SIZE
        vals = numbers(adf[at:at + PLANE_SIZE])
        line = f"{name}: " + " ".join("%04X" % v for v in vals)
        print(line if vals else f"{name}: no numbers (not run?)")
        if other is not None:
            ovals = numbers(other[at:at + PLANE_SIZE])
            print(" " * len(name) + "  " + " ".join("%04X" % v for v in ovals) + "  (against)")
            if name.endswith("5"):
                # beam positions: the difference in colour clocks
                diffs = [f"{(v & 0xFF) - (o & 0xFF):+d}" for v, o in zip(vals, ovals)]
            else:
                diffs = [f"{(v - o) * 100 / o:+.1f}%" for v, o in zip(vals, ovals) if o]
            print(" " * len(name) + "  " + " ".join(diffs))


def main():
    if len(sys.argv) >= 3 and sys.argv[1] == "build":
        build(sys.argv[2])
    elif len(sys.argv) >= 3 and sys.argv[1] == "results":
        against = sys.argv[4] if len(sys.argv) >= 5 and sys.argv[3] == "--against" else None
        results(sys.argv[2], against)
    else:
        print(__doc__)
        sys.exit(1)


if __name__ == "__main__":
    main()
