"""Random 68020 instruction tests on a bootable Amiga disk.

    python tools/m68020_random.py build OUT.adf [--seed N] [--cases N] [--bin OUT.bin] [--kinds chk2,cas]
    python tools/m68020_random.py extract RUN.adf OUT.ref

`build` generates test cases (a fixed seed gives the same disk; `--kinds`
limits them to some generators, the method names of `Gen`) and writes
a boot disk. Boot it on an A1200 (WinUAE: A1200, 68EC020 or 68020, no FPU,
no JIT, 2 MB chip RAM, no fast RAM, Kickstart 3.1, the ADF writable): the
boot block reserves $100000-$1A8000 with AllocAbs, reads the program, and
the program runs every case in supervisor mode with interrupts off. Each
case restores a 2 KB test area and a small stack zone, applies its patches
(pointers for memory indirect modes, return addresses), writes the
instruction into a buffer of ILLEGAL opcodes, clears the instruction cache
and enters it through RTE with the case's registers and CCR. The ILLEGAL
after it (or any exception the instruction raises) ends the case: the
vector number, D0-D7/A0-A6, A7, 16 bytes from A7 (the exception frame and
what the instruction pushed) and up to 24 changed bytes of the test area
are recorded. When all cases ran, the results are written back to the
disk (trackdisk CMD_WRITE + CMD_UPDATE) and the screen turns green (red:
AllocAbs or a disk error).

`extract` copies the results from such a disk into a reference file. The
test `random_68020_matches_the_reference` (tests/tests/m68020_random.rs)
runs the same program on mimic's 68020 and compares; `--bin` writes the
program image it loads (tests/data/m68020_random.bin).

Instruction encodings: Motorola M68000 Family Programmer's Reference
Manual (M68000PM/AD rev. 1); exec/trackdisk calls: Amiga ROM Kernel
Reference Manual (Libraries, Devices). Cases, layout and program are this
project's own.
"""
import random
import struct
import sys

# --- memory layout (chip RAM of a 2 MB A1200) -------------------------------

CODE = 0x100000          # header, harness, exception stubs, vector table
PRISTINE = CODE + 0x2000  # the test area's initial contents
CASES = CODE + 0x2800
CASE_SIZE = 128
RESULTS = 0x143000
REC_SIZE = 192
AREA = 0x1A4000
AREA_SIZE = 0x800
CODEBUF = 0x1A5000       # ILLEGAL opcodes with the instruction at INSN_AT
CODEBUF_SIZE = 0x200
INSN_AT = CODEBUF + 0x100
MAX_WORDS = 11
STACK = 0x1A5F80         # A7 when the instruction starts
STACK_ZONE = (STACK - 0x40, 0x80)  # cleared before every case
VARS = 0x1A6000
HSTACK = 0x1A8000        # the harness's own stack
ALLOC_START, ALLOC_END = CODE, HSTACK
MAX_DIFFS = 24
DEFAULT_CASES = 2048

V_VECNUM = VARS + 0x00
V_REGSAVE = VARS + 0x04   # 15 longs
V_SPSAVE = VARS + 0x40
V_CASEPTR = VARS + 0x44
V_RESPTR = VARS + 0x48
V_COUNT = VARS + 0x4C
V_SAVED_SSP = VARS + 0x50
V_SAVED_VBR = VARS + 0x54
V_SAVED_CACR = VARS + 0x58
V_IOREQ = VARS + 0x5C
V_DONE = VARS + 0x60

MAGIC = b"M020"
HEADER_FMT = ">4sHHIIIIIIIIIIIII"  # see header()
HEADER_AT = 8            # in the image, after two BRA.W
HARNESS_AT = 0x50
STUB_SIZE = 12
ILLEGAL = 0x4AFC

# exec.library / trackdisk.device
LVO_SUPERVISOR = -30
LVO_DISABLE = -120
LVO_ENABLE = -126
LVO_ALLOCABS = -204
LVO_DOIO = -456
IO_COMMAND, IO_ERROR, IO_LENGTH, IO_DATA, IO_OFFSET = 0x1C, 0x1F, 0x24, 0x28, 0x2C
CMD_READ, CMD_WRITE, CMD_UPDATE, TD_MOTOR = 2, 3, 4, 9
COLOR00 = 0xDFF180


def s16(v):
    v &= 0xFFFF
    return v - 0x10000 if v & 0x8000 else v


def s8(v):
    v &= 0xFF
    return v - 0x100 if v & 0x80 else v


def words_of_long(v):
    v &= 0xFFFFFFFF
    return [v >> 16, v & 0xFFFF]


# --- a small assembler for the harness ---------------------------------------

class Asm:
    def __init__(self, base):
        self.base = base
        self.words = []
        self.labels = {}
        self.fixups = []

    @property
    def pc(self):
        return self.base + 2 * len(self.words)

    def label(self, name):
        assert name not in self.labels, name
        self.labels[name] = self.pc

    def w(self, *ws):
        self.words.extend(x & 0xFFFF for x in ws)

    def l(self, v):
        self.w(*words_of_long(v))

    def branch(self, opcode, label, size="w"):
        """Bcc/BRA/BSR (opcode with the condition) or DBcc (size 'db')."""
        if size == "s":
            self.fixups.append((len(self.words), label, "s"))
            self.w(opcode)
        else:
            self.w(opcode)
            self.fixups.append((len(self.words), label, "w"))
            self.w(0)

    def finish(self):
        for idx, label, kind in self.fixups:
            target = self.labels[label]
            if kind == "s":
                disp = target - (self.base + 2 * idx + 2)
                assert -128 <= disp < 128 and disp not in (0, -1), (label, disp)
                self.words[idx] |= disp & 0xFF
            else:
                disp = target - (self.base + 2 * idx)
                assert -32768 <= disp < 32768, label
                self.words[idx] = disp & 0xFFFF
        return b"".join(struct.pack(">H", x) for x in self.words)


BRA, BNE, BEQ, BHI, BPL = 0x6000, 0x6600, 0x6700, 0x6200, 0x6A00


def dbra(a, dn, label):
    a.branch(0x51C8 | dn, label)


def build_harness(n_cases, seed):
    a = Asm(CODE)
    a.branch(BRA, "amiga")                  # CODE+0: from the boot block
    a.branch(BRA, "bare")                   # CODE+4: bare CPU (tests)
    assert a.pc == CODE + HEADER_AT
    res_off = 1024 + (CASES + n_cases * CASE_SIZE - CODE)
    res_off += -res_off % 512
    header = struct.pack(
        HEADER_FMT, MAGIC, 1, n_cases, seed, CASES, CASE_SIZE, RESULTS,
        REC_SIZE, res_off, V_DONE, AREA, AREA_SIZE, INSN_AT, HSTACK, 0, 0)
    loops_at = len(a.words) + len(header) // 2 - 4   # green, fail (patched)
    for i in range(0, len(header), 2):
        a.w(struct.unpack(">H", header[i:i + 2])[0])
    while a.pc < CODE + HARNESS_AT:
        a.w(0)

    # Amiga entry: user mode, A1 = trackdisk IORequest, A6 = ExecBase.
    a.label("amiga")
    a.w(0x23C9); a.l(V_IOREQ)               # MOVE.L A1,ioreq
    a.w(0x4EAE, LVO_DISABLE)
    a.w(0x4BFA)                             # LEA super(PC),A5
    a.fixups.append((len(a.words), "super", "w"))
    a.w(0)
    a.w(0x4EAE, LVO_SUPERVISOR)
    a.w(0x2C78, 0x0004)                     # MOVEA.L 4.W,A6
    a.w(0x4EAE, LVO_ENABLE)
    a.w(0x2279); a.l(V_IOREQ)               # MOVEA.L ioreq,A1
    a.w(0x237C); a.l(RESULTS); a.w(IO_DATA)
    a.w(0x237C); a.l(n_cases * REC_SIZE); a.w(IO_LENGTH)
    a.w(0x237C); a.l(res_off); a.w(IO_OFFSET)
    a.w(0x337C, CMD_WRITE, IO_COMMAND)
    a.w(0x4EAE, LVO_DOIO)
    a.w(0x4A29, IO_ERROR)                   # TST.B io_Error(A1)
    a.branch(BNE, "fail")
    a.w(0x337C, CMD_UPDATE, IO_COMMAND)
    a.w(0x4EAE, LVO_DOIO)
    a.w(0x4A29, IO_ERROR)
    a.branch(BNE, "fail")
    a.w(0x337C, TD_MOTOR, IO_COMMAND)
    a.w(0x42A9, IO_LENGTH)                  # CLR.L io_Length(A1): motor off
    a.w(0x4EAE, LVO_DOIO)
    a.label("green")
    a.w(0x33FC, 0x00F0); a.l(COLOR00)
    a.branch(BRA, "green", "s")
    a.label("fail")
    a.w(0x33FC, 0x0F00); a.l(COLOR00)
    a.branch(BRA, "fail", "s")

    # Bare entry: an RTE frame so `super` returns to a STOP.
    a.label("bare")
    a.w(0x46FC, 0x2700)                     # MOVE.W #$2700,SR
    a.w(0x4FF9); a.l(HSTACK - 0x100)        # LEA HSTACK-$100,A7
    a.w(0x3F3C, 0x0000)                     # MOVE.W #0,-(A7): format 0
    a.w(0x487A)                             # PEA bare_done(PC)
    a.fixups.append((len(a.words), "bare_done", "w"))
    a.w(0)
    a.w(0x40E7)                             # MOVE.W SR,-(A7)
    a.branch(BRA, "super")
    a.label("bare_done")
    a.w(0x4E72, 0x2700)                     # STOP #$2700
    a.branch(BRA, "bare_done", "s")

    # Supervisor part: runs every case, returns with RTE.
    a.label("super")
    a.w(0x23CF); a.l(V_SAVED_SSP)           # MOVE.L A7,saved_ssp
    a.w(0x4E7A, 0x0801)                     # MOVEC VBR,D0
    a.w(0x23C0); a.l(V_SAVED_VBR)
    a.w(0x4E7A, 0x0002)                     # MOVEC CACR,D0
    a.w(0x23C0); a.l(V_SAVED_CACR)
    a.w(0x4FF9); a.l(HSTACK)                # LEA HSTACK,A7
    a.w(0x203C); a.l(9)                     # MOVE.L #9,D0: cache on + clear
    a.w(0x4E7B, 0x0002)                     # MOVEC D0,CACR
    a.w(0x203C); a.l(0)                     # MOVE.L #vbrtab,D0 (patched)
    vbr_fix = len(a.words) - 2
    a.w(0x4E7B, 0x0801)                     # MOVEC D0,VBR
    # results cleared, instruction buffer filled with ILLEGAL
    a.w(0x43F9); a.l(RESULTS)               # LEA RESULTS,A1
    a.w(0x203C); a.l(n_cases * REC_SIZE // 4)
    a.label("clr")
    a.w(0x4299)                             # CLR.L (A1)+
    a.w(0x5380)                             # SUBQ.L #1,D0
    a.branch(BNE, "clr", "s")
    a.w(0x43F9); a.l(CODEBUF)
    a.w(0x303C, CODEBUF_SIZE // 2 - 1)      # MOVE.W #n,D0
    a.label("fill")
    a.w(0x32FC, ILLEGAL)                    # MOVE.W #$4AFC,(A1)+
    dbra(a, 0, "fill")
    a.w(0x23FC); a.l(CASES); a.l(V_CASEPTR)
    a.w(0x23FC); a.l(RESULTS); a.l(V_RESPTR)
    a.w(0x33FC, n_cases - 1); a.l(V_COUNT)

    a.label("loop")
    a.w(0x41F9); a.l(PRISTINE)              # LEA PRISTINE,A0
    a.w(0x43F9); a.l(AREA)                  # LEA AREA,A1
    a.w(0x303C, AREA_SIZE // 4 - 1)
    a.label("copy")
    a.w(0x22D8)                             # MOVE.L (A0)+,(A1)+
    dbra(a, 0, "copy")
    a.w(0x43F9); a.l(STACK_ZONE[0])
    a.w(0x7000 | (STACK_ZONE[1] // 4 - 1))  # MOVEQ #n,D0
    a.label("zone")
    a.w(0x4299)
    dbra(a, 0, "zone")
    a.w(0x2079); a.l(V_CASEPTR)             # MOVEA.L caseptr,A0
    a.w(0x3028, 86)                         # MOVE.W 86(A0),D0: patches
    a.w(0x43E8, 88)                         # LEA 88(A0),A1
    a.branch(BRA, "pchk", "s")
    a.label("patch")
    a.w(0x2459)                             # MOVEA.L (A1)+,A2
    a.w(0x2499)                             # MOVE.L (A1)+,(A2)
    a.label("pchk")
    dbra(a, 0, "patch")
    a.w(0x43F9); a.l(INSN_AT)
    a.w(0x45E8, 2)                          # LEA 2(A0),A2
    a.w(0x3010)                             # MOVE.W (A0),D0
    a.branch(BRA, "ichk", "s")
    a.label("insn")
    a.w(0x32DA)                             # MOVE.W (A2)+,(A1)+
    a.label("ichk")
    dbra(a, 0, "insn")
    a.w(0x203C); a.l(9)
    a.w(0x4E7B, 0x0002)                     # MOVEC D0,CACR: clear
    a.w(0x4FF9); a.l(STACK)                 # LEA STACK,A7
    a.w(0x3F3C, 0x0000)                     # format 0
    a.w(0x4879); a.l(INSN_AT)               # PEA INSN_AT
    a.w(0x3F28, 84)                         # MOVE.W 84(A0),-(A7): SR
    a.w(0x4CE8, 0x7FFF, 24)                 # MOVEM.L 24(A0),D0-D7/A0-A6
    a.w(0x4E73)                             # RTE

    a.label("common")
    a.w(0x48F9, 0x7FFF); a.l(V_REGSAVE)     # MOVEM.L D0-D7/A0-A6,regsave
    a.w(0x23CF); a.l(V_SPSAVE)              # MOVE.L A7,spsave
    a.w(0x2079); a.l(V_RESPTR)              # MOVEA.L resptr,A0
    a.w(0x30F9); a.l(V_VECNUM)              # MOVE.W vecnum,(A0)+
    a.w(0x2648)                             # MOVEA.L A0,A3
    a.w(0x4258)                             # CLR.W (A0)+
    a.w(0x43F9); a.l(V_REGSAVE)
    a.w(0x700E)                             # MOVEQ #14,D0
    a.label("regs")
    a.w(0x20D9)                             # MOVE.L (A1)+,(A0)+
    dbra(a, 0, "regs")
    a.w(0x20F9); a.l(V_SPSAVE)              # MOVE.L spsave,(A0)+
    a.w(0x2279); a.l(V_SPSAVE)              # MOVEA.L spsave,A1
    a.w(0x7007)                             # MOVEQ #7,D0
    a.label("frame")
    a.w(0x30D9)                             # MOVE.W (A1)+,(A0)+
    dbra(a, 0, "frame")
    a.w(0x2848)                             # MOVEA.L A0,A4
    a.w(0x43F9); a.l(AREA)
    a.w(0x45F9); a.l(PRISTINE)
    a.w(0x303C, AREA_SIZE - 1)
    a.w(0x7200)                             # MOVEQ #0,D1
    a.label("diff")
    a.w(0x1419)                             # MOVE.B (A1)+,D2
    a.w(0xB41A)                             # CMP.B (A2)+,D2
    a.branch(BEQ, "same", "s")
    a.w(0x5241)                             # ADDQ.W #1,D1
    a.w(0x0C41, MAX_DIFFS)                  # CMPI.W #24,D1
    a.branch(BHI, "same", "s")
    a.w(0x2609)                             # MOVE.L A1,D3
    a.w(0x0483); a.l(AREA + 1)              # SUBI.L #AREA+1,D3
    a.w(0x38C3)                             # MOVE.W D3,(A4)+
    a.w(0x18C2)                             # MOVE.B D2,(A4)+
    a.w(0x421C)                             # CLR.B (A4)+
    a.label("same")
    dbra(a, 0, "diff")
    a.w(0x3681)                             # MOVE.W D1,(A3)
    a.w(0x06B9); a.l(REC_SIZE); a.l(V_RESPTR)
    a.w(0x06B9); a.l(CASE_SIZE); a.l(V_CASEPTR)
    a.w(0x43F9); a.l(INSN_AT)
    a.w(0x7000 | (MAX_WORDS - 1))
    a.label("restore")
    a.w(0x32FC, ILLEGAL)
    dbra(a, 0, "restore")
    a.w(0x4FF9); a.l(HSTACK)
    a.w(0x5379); a.l(V_COUNT)               # SUBQ.W #1,count
    a.branch(BPL, "loop")

    a.w(0x33FC, 1); a.l(V_DONE)             # MOVE.W #1,done
    a.w(0x2039 | 0); a.l(V_SAVED_CACR)      # MOVE.L saved_cacr,D0
    a.w(0x4E7B, 0x0002)
    a.w(0x2039); a.l(V_SAVED_VBR)
    a.w(0x4E7B, 0x0801)
    a.w(0x2E79); a.l(V_SAVED_SSP)           # MOVEA.L saved_ssp,A7
    a.w(0x4E73)                             # RTE

    # One stub per vector: note the vector, go to `common`.
    while len(a.words) % 2:
        a.w(0x4E71)
    stubs = a.pc
    for v in range(256):
        a.w(0x33FC, v); a.l(V_VECNUM)       # MOVE.W #v,vecnum (CCR is in the frame)
        a.branch(BRA, "common")
    while a.pc % 4:
        a.w(0)
    vbrtab = a.pc
    for v in range(256):
        a.l(stubs + v * STUB_SIZE)
    a.words[vbr_fix:vbr_fix + 2] = words_of_long(vbrtab)
    a.words[loops_at:loops_at + 4] = words_of_long(a.labels["green"]) + words_of_long(a.labels["fail"])
    code = a.finish()
    assert CODE + len(code) <= PRISTINE, hex(len(code))
    return code + bytes(PRISTINE - CODE - len(code)), res_off


# --- test cases ----------------------------------------------------------------

SPECIAL = [0, 1, 2, 0x7F, 0x80, 0xFF, 0x7FFF, 0x8000, 0xFFFF, 0x10000,
           0x7FFFFFFF, 0x80000000, 0x80000001, 0xFFFFFFFF, 0xFFFFFFFE,
           0x99, 0x09, 0x90, 0x9999, 0x0F0F, 0x00FF00FF]


class Case:
    def __init__(self, gen):
        self.g = gen
        self.rng = gen.rng
        self.regs = {}           # ("d"|"a", n) -> value
        self.used = set()
        self.patches = []
        self.words = []

    # registers
    def alloc(self, kind=None):
        kinds = [kind] if kind else ["d", "a"]
        free = [(k, n) for k in kinds for n in range(8 if k == "d" else 7)
                if (k, n) not in self.used]
        r = self.rng.choice(free)
        self.used.add(r)
        return r

    def dreg(self):
        return self.alloc("d")[1]

    def areg(self):
        return self.alloc("a")[1]

    def set(self, r, v):
        self.regs[r] = v & 0xFFFFFFFF

    # memory
    def mem(self, addr, size):
        v = 0
        for i in range(size):
            v = v << 8 | self.byte(addr + i)
        return v

    def byte(self, addr):
        b = None
        if AREA <= addr < AREA + AREA_SIZE:
            b = self.g.pristine[addr - AREA]
        elif STACK_ZONE[0] <= addr < STACK_ZONE[0] + STACK_ZONE[1]:
            b = 0
        for pa, pv in self.patches:
            if pa <= addr < pa + 4:
                b = (pv >> (8 * (3 - (addr - pa)))) & 0xFF
        return b

    def patch(self, addr, v):
        assert len(self.patches) < 5
        self.patches.append((addr, v & 0xFFFFFFFF))

    def target(self, region, span, align=None):
        lo, hi = {"src": (0x400, 0x5C0), "dst": (0x5C0, 0x780),
                  "ptr": (0x40, 0x3C0), "any": (0x400, 0x780)}[region]
        if region == "ptr":
            lo = lo + 0x40 * self.g.ptr_slot
            hi = lo + 0x40
            self.g.ptr_slot = (self.g.ptr_slot + 1) % 14
        align = align or (2 if self.rng.random() < 0.7 else 1)
        while True:
            t = self.rng.randrange(lo, hi - span)
            if t % align == 0:
                return AREA + t

    @property
    def ext_addr(self):
        return INSN_AT + 2 * len(self.words)

    # effective addresses: return the 6-bit mode/reg field, extension words
    # are appended to self.words
    def ea_dn(self, n=None):
        n = self.dreg() if n is None else n
        return n, ("d", n)

    def ea(self, size, kinds, region="any", span=None, align=None):
        """A memory (or register) operand; `kinds` from: dn an ind postinc
        predec d16 idx full abs pcd16 pcidx pcfull imm."""
        rng = self.rng
        kind = rng.choice(kinds)
        span = span or size
        if kind == "dn":
            n = self.dreg()
            return n
        if kind == "an":
            return 0o10 | self.areg()
        if kind == "imm":
            v = self.g.data()
            if size == 1:
                self.words.append(v & 0xFF)
            elif size == 2:
                self.words.append(v & 0xFFFF)
            else:
                self.words.extend(words_of_long(v))
            return 0o74
        if kind.startswith("pc"):
            t = CODEBUF + rng.randrange(0, CODEBUF_SIZE - span)
            if kind == "pcd16":
                self.words.append((t - self.ext_addr) & 0xFFFF)
                return 0o72
            if kind == "pcidx":
                self.brief(t, base=self.ext_addr)
                return 0o73
            self.full(t, base_fixed=self.ext_addr)
            return 0o73
        t = self.target(region, span, align)
        self.last_target = t
        if kind == "abs":
            self.words.extend(words_of_long(t))
            return 0o71
        an = self.areg()
        if kind == "ind":
            self.set(("a", an), t)
            return 0o20 | an
        if kind == "postinc":
            self.set(("a", an), t)
            return 0o30 | an
        if kind == "predec":
            self.set(("a", an), t + size)
            return 0o40 | an
        if kind == "d16":
            d = rng.randrange(-0x8000, 0x8000)
            self.words.append(d & 0xFFFF)
            self.set(("a", an), t - d)
            return 0o50 | an
        if kind == "idx":
            self.brief(t, an=an)
            return 0o60 | an
        self.full(t, an=an)
        return 0o60 | an

    def index(self):
        """An index register with its value: (ext bits, scaled value)."""
        rng = self.rng
        k, n = self.alloc()
        scale = rng.randrange(4)
        long = rng.random() < 0.5
        raw = self.g.data() if rng.random() < 0.3 else rng.randrange(-300, 300)
        if not long:
            raw = (rng.getrandbits(16) << 16) | (raw & 0xFFFF)
            eff = s16(raw)
        else:
            eff = raw if raw < 0x80000000 else raw - 0x100000000
        self.set((k, n), raw)
        bits = (int(k == "a") << 15) | (n << 12) | (int(long) << 11) | (scale << 9)
        return bits, eff << scale

    def brief(self, t, an=None, base=None):
        bits, x = self.index()
        d8 = self.rng.randrange(-128, 128)
        self.words.append(bits | (d8 & 0xFF))
        if an is not None:
            self.set(("a", an), t - d8 - x)
        else:
            # PC base: move the needed difference into the index register
            k, n = (("a", bits >> 12 & 7) if bits & 0x8000 else ("d", bits >> 12 & 7))
            need = t - base - d8
            self.words[-1] = (bits & ~0x0E00 & ~0xFF) | 0x0800 | (d8 & 0xFF)
            self.set((k, n), need)

    def full(self, t, an=None, base_fixed=None):
        """Full-format extension word; base register `an`, or a fixed base
        (the PC); some cases suppress the base or index, some go through a
        pointer in memory (pre- or post-indexed)."""
        rng = self.rng
        bs = an is None and base_fixed is None or rng.random() < 0.15
        if bs and an is not None:
            self.used.discard(("a", an))    # stays free, value random
        is_ = rng.random() < 0.2
        if is_:
            iis = rng.choice([0, 1, 2, 3])
        else:
            iis = rng.choice([0, 1, 2, 3, 5, 6, 7])
        bdsz = rng.choice([1, 2, 3])
        if is_:
            ibits, x = 0, 0
        else:
            ibits, x = self.index()
        od = {1: 0, 2: s16(rng.getrandbits(16)), 3: self.g.data()}[iis & 3 or 1] if iis else 0
        if iis:
            p = self.target("ptr", 4)
        # address the base + bd (+ index if pre/none) must reach
        if iis == 0:
            need = t - x
        elif not is_ and iis < 4:           # pre-indexed
            need = p - x
            self.patch(p, t - od)
        else:                               # post-indexed or no index
            need = p
            self.patch(p, t - (0 if is_ else x) - od)
        fixed = 0 if bs else base_fixed
        if fixed is not None:
            bdsz = 3
            bd = need - fixed
        else:
            bd = {1: 0, 2: s16(rng.getrandbits(16)), 3: self.g.data()}[bdsz]
            self.set(("a", an), need - bd)
        ext = ibits | 0x0100 | (int(bs) << 7) | (int(is_) << 6) | (bdsz << 4) | iis
        self.words.append(ext)
        if bdsz == 2:
            self.words.append(bd & 0xFFFF)
        elif bdsz == 3:
            self.words.extend(words_of_long(bd))
        if iis & 3 == 2:
            self.words.append(od & 0xFFFF)
        elif iis & 3 == 3:
            self.words.extend(words_of_long(od))


MEM_ALT = ["ind", "postinc", "predec", "d16", "idx", "full", "full", "abs"]
DATA_ALT = ["dn", "dn"] + MEM_ALT
SRC = ["dn", "dn", "imm", "pcd16", "pcidx", "pcfull"] + MEM_ALT
CONTROL = ["ind", "d16", "idx", "full", "full", "abs", "pcd16", "pcidx", "pcfull"]
CONTROL_ALT = ["ind", "d16", "idx", "full", "full", "abs"]
SIZES = [(0, 1), (1, 2), (2, 4)]


def srcs(size):
    return SRC + (["an"] if size > 1 else [])


class Gen:
    def __init__(self, seed, kinds=None):
        self.rng = random.Random(seed)
        self.pristine = bytes(self.data() & 0xFF if self.rng.random() < 0.2
                              else self.rng.getrandbits(8) for _ in range(AREA_SIZE))
        self.ptr_slot = 0
        self.kinds = [
            (self.alu_to_dn, 8), (self.alu_to_mem, 4), (self.eor, 2), (self.adda, 3),
            (self.immediate, 6), (self.quick, 3), (self.moveq, 1), (self.x_ops, 4),
            (self.bcd, 3), (self.pack, 2), (self.single, 6), (self.ext, 2),
            (self.mul_div_w, 5), (self.mul_l, 4), (self.div_l, 5), (self.shift_reg, 5),
            (self.shift_mem, 2), (self.bitop, 4), (self.bitfield, 8), (self.chk, 2),
            (self.chk2, 4), (self.cas, 3), (self.cas2, 2), (self.scc, 2),
            (self.move, 10), (self.movea, 2), (self.sr_ccr, 3), (self.lea_pea, 3),
            (self.movem, 4), (self.movep, 2), (self.exg_swap, 1), (self.tas, 1),
            (self.link_unlk, 2), (self.branch, 4), (self.dbcc, 2), (self.jump, 2),
            (self.returns, 2), (self.traps, 3), (self.lines, 1),
        ]
        if kinds:
            known = {f.__name__ for f, _ in self.kinds}
            unknown = set(kinds) - known
            if unknown:
                sys.exit(f"unknown kinds {sorted(unknown)}; known: {sorted(known)}")
            self.kinds = [(f, w) for f, w in self.kinds if f.__name__ in kinds]

    def data(self):
        r = self.rng.random()
        if r < 0.25:
            v = self.rng.choice(SPECIAL)
            if self.rng.random() < 0.3:
                v ^= self.rng.getrandbits(16) << 16
            return v & 0xFFFFFFFF
        if r < 0.4:
            return self.rng.randrange(0, 64)
        if r < 0.5:
            return int("".join(str(self.rng.randrange(10)) for _ in range(8)), 16)
        return self.rng.getrandbits(32)

    def case(self):
        while True:
            c = Case(self)
            fn = self.rng.choices([k for k, _ in self.kinds], [w for _, w in self.kinds])[0]
            fn(c)
            if len(c.words) <= MAX_WORDS:
                return c

    # each generator appends the instruction to c.words

    def op(self, c, opcode):
        """Start an instruction: the opcode word, patched by `fix`."""
        c.words.append(opcode)
        return len(c.words) - 1

    def alu_to_dn(self, c):
        base = self.rng.choice([0xD000, 0x9000, 0xC000, 0x8000, 0xB000])
        sz, size = self.rng.choice(SIZES)
        i = self.op(c, 0)
        kinds = srcs(size) if base in (0xD000, 0x9000, 0xB000) else SRC
        ea = c.ea(size, kinds, "src")
        dn = c.dreg()
        c.words[i] = base | dn << 9 | sz << 6 | ea

    def alu_to_mem(self, c):
        base = self.rng.choice([0xD000, 0x9000, 0xC000, 0x8000])
        sz, size = self.rng.choice(SIZES)
        i = self.op(c, 0)
        ea = c.ea(size, MEM_ALT, "dst")
        c.words[i] = base | c.dreg() << 9 | (4 + sz) << 6 | ea

    def eor(self, c):
        sz, size = self.rng.choice(SIZES)
        i = self.op(c, 0)
        ea = c.ea(size, DATA_ALT, "dst")
        c.words[i] = 0xB100 | c.dreg() << 9 | sz << 6 | ea

    def adda(self, c):
        base = self.rng.choice([0xD0C0, 0x90C0, 0xB0C0])
        long = self.rng.random() < 0.5
        i = self.op(c, 0)
        ea = c.ea(4 if long else 2, srcs(2), "src")
        c.words[i] = base | c.areg() << 9 | int(long) << 8 | ea

    def immediate(self, c):
        base = self.rng.choice([0x0000, 0x0200, 0x0400, 0x0600, 0x0A00, 0x0C00])
        sz, size = self.rng.choice(SIZES)
        i = self.op(c, 0)
        c.ea(size, ["imm"])
        kinds = DATA_ALT + (["pcd16", "pcidx"] if base == 0x0C00 else [])
        ea = c.ea(size, kinds, "dst")
        c.words[i] = base | sz << 6 | ea

    def quick(self, c):
        sz, size = self.rng.choice(SIZES)
        i = self.op(c, 0)
        kinds = DATA_ALT + (["an"] if size > 1 else [])
        ea = c.ea(size, kinds, "dst")
        c.words[i] = self.rng.choice([0x5000, 0x5100]) | self.rng.randrange(8) << 9 | sz << 6 | ea

    def moveq(self, c):
        c.words.append(0x7000 | c.dreg() << 9 | self.rng.getrandbits(8))

    def x_ops(self, c):
        base = self.rng.choice([0xD100, 0x9100, 0xB108])  # ADDX SUBX CMPM
        sz, size = self.rng.choice(SIZES)
        if base == 0xB108:
            ay, ax = c.areg(), c.areg()
            c.set(("a", ay), c.target("src", size))
            c.set(("a", ax), c.target("dst", size))
            c.words.append(base | ax << 9 | sz << 6 | ay)
            return
        if self.rng.random() < 0.5:
            c.words.append(base | c.dreg() << 9 | sz << 6 | c.dreg())
        else:
            ay, ax = c.areg(), c.areg()
            c.set(("a", ay), c.target("src", size) + size)
            c.set(("a", ax), c.target("dst", size) + size)
            c.words.append(base | ax << 9 | sz << 6 | 8 | ay)

    def bcd(self, c):
        r = self.rng.random()
        if r < 0.25:
            i = self.op(c, 0)
            c.words[i] = 0x4800 | c.ea(1, DATA_ALT, "dst")      # NBCD
            return
        base = self.rng.choice([0xC100, 0x8100])               # ABCD SBCD
        if r < 0.6:
            c.words.append(base | c.dreg() << 9 | c.dreg())
        else:
            ay, ax = c.areg(), c.areg()
            c.set(("a", ay), c.target("src", 1) + 1)
            c.set(("a", ax), c.target("dst", 1) + 1)
            c.words.append(base | ax << 9 | 8 | ay)

    def pack(self, c):
        base = self.rng.choice([0x8140, 0x8180])               # PACK UNPK
        adj = self.rng.getrandbits(16)
        if self.rng.random() < 0.5:
            c.words += [base | c.dreg() << 9 | c.dreg(), adj]
        else:
            src_size, dst_size = (2, 1) if base == 0x8140 else (1, 2)
            ay, ax = c.areg(), c.areg()
            c.set(("a", ay), c.target("src", src_size) + src_size)
            c.set(("a", ax), c.target("dst", dst_size) + dst_size)
            c.words += [base | ax << 9 | 8 | ay, adj]

    def single(self, c):
        base = self.rng.choice([0x4400, 0x4000, 0x4600, 0x4200, 0x4A00])  # NEG NEGX NOT CLR TST
        sz, size = self.rng.choice(SIZES)
        i = self.op(c, 0)
        kinds = DATA_ALT
        if base == 0x4A00:
            kinds = srcs(size)
        c.words[i] = base | sz << 6 | c.ea(size, kinds, "dst")

    def ext(self, c):
        c.words.append(self.rng.choice([0x4880, 0x48C0, 0x49C0]) | c.dreg())

    def mul_div_w(self, c):
        base = self.rng.choice([0xC0C0, 0xC1C0, 0x80C0, 0x81C0])
        i = self.op(c, 0)
        ea = c.ea(2, SRC, "src")
        dn = c.dreg()
        if base & 0xF000 == 0x8000 and self.rng.random() < 0.5:
            c.set(("d", dn), self.rng.getrandbits(18))    # quotient often fits
        c.words[i] = base | dn << 9 | ea

    def mul_l(self, c):
        i = self.op(c, 0)
        ext = len(c.words)
        c.words.append(0)
        ea = c.ea(4, SRC, "src")
        dl = c.dreg()
        wide = self.rng.random() < 0.5
        dh = c.dreg() if wide else 0
        c.words[i] = 0x4C00 | ea
        c.words[ext] = dl << 12 | int(self.rng.random() < 0.5) << 11 | int(wide) << 10 | dh

    def div_l(self, c):
        i = self.op(c, 0)
        ext = len(c.words)
        c.words.append(0)
        ea = c.ea(4, SRC, "src")
        dq = c.dreg()
        form = self.rng.choice(["32", "32r", "64"])
        dr = dq if form == "32" else c.dreg()
        if form == "64" and self.rng.random() < 0.6:
            c.set(("d", dr), self.rng.randrange(0, 0x10000))  # no overflow
        c.words[i] = 0x4C40 | ea
        c.words[ext] = dq << 12 | int(self.rng.random() < 0.5) << 11 | int(form == "64") << 10 | dr

    def shift_reg(self, c):
        sz, _ = self.rng.choice(SIZES)
        kind = self.rng.randrange(4)
        left = self.rng.randrange(2)
        if self.rng.random() < 0.5:
            cnt = self.rng.randrange(8)
            c.words.append(0xE000 | cnt << 9 | left << 8 | sz << 6 | kind << 3 | c.dreg())
        else:
            cr = c.dreg()
            c.set(("d", cr), self.rng.choice([self.data(), self.rng.randrange(70)]))
            c.words.append(0xE020 | cr << 9 | left << 8 | sz << 6 | kind << 3 | c.dreg())

    def shift_mem(self, c):
        i = self.op(c, 0)
        ea = c.ea(2, MEM_ALT, "dst")
        c.words[i] = 0xE0C0 | self.rng.randrange(4) << 9 | self.rng.randrange(2) << 8 | ea

    def bitop(self, c):
        kind = self.rng.randrange(4)
        i = self.op(c, 0)
        static = self.rng.random() < 0.5
        if static:
            c.words.append(self.rng.getrandbits(8))
        to_dn = self.rng.random() < 0.4
        if to_dn:
            ea = c.dreg()
        else:
            kinds = MEM_ALT + (["pcd16", "pcidx"] if kind == 0 else [])
            if kind == 0 and not static:
                kinds = kinds + ["imm"]
            ea = c.ea(1, kinds, "dst")
        if static:
            c.words[i] = 0x0800 | kind << 6 | ea
        else:
            dn = c.dreg()
            c.set(("d", dn), self.rng.choice([self.data(), self.rng.randrange(64)]))
            c.words[i] = 0x0100 | dn << 9 | kind << 6 | ea

    def bitfield(self, c):
        kind = self.rng.randrange(8)   # TST EXTU CHG EXTS CLR FFO SET INS
        i = self.op(c, 0)
        ext = len(c.words)
        c.words.append(0)
        do = self.rng.random() < 0.4
        dw = self.rng.random() < 0.3
        to_dn = self.rng.random() < 0.35
        if do:
            o = c.dreg()
            c.set(("d", o), self.rng.randrange(-120, 200) if not to_dn else self.data())
            offs = o
        else:
            offs = self.rng.randrange(32)
        if dw:
            w = c.dreg()
            c.set(("d", w), self.data())
            width = w
        else:
            width = self.rng.randrange(32)
        if to_dn:
            ea = c.dreg()
        else:
            kinds = ["ind", "d16", "idx", "full", "abs"]
            if kind in (0, 1, 3, 5):
                kinds += ["pcd16", "pcidx"]
            # the field may start 15 bytes before and end 30 bytes after
            ea = c.ea(1, kinds, "any", span=30)
        reg = c.dreg() if kind in (1, 3, 5, 7) else 0
        c.words[i] = 0xE8C0 | kind << 8 | ea
        c.words[ext] = reg << 12 | int(do) << 11 | (offs & 31) << 6 | int(dw) << 5 | (width & 31)

    def chk(self, c):
        long = self.rng.random() < 0.5
        size = 4 if long else 2
        i = self.op(c, 0)
        ea = c.ea(size, SRC, "src")
        dn = c.dreg()
        c.words[i] = (0x4100 if long else 0x4180) | dn << 9 | ea

    def chk2(self, c):
        sz, size = self.rng.choice(SIZES)
        i = self.op(c, 0)
        ext = len(c.words)
        c.words.append(0)
        ea = c.ea(size, CONTROL, "src", span=2 * size)
        k, n = c.alloc()
        if ea not in (0o72, 0o73) and self.rng.random() < 0.6:
            t = c.last_target
            lo, hi = c.mem(t, size), c.mem(t + size, size)
            c.set((k, n), self.rng.choice([lo, hi, (lo + hi) // 2, lo - 1, hi + 1, self.data()]))
        chk = self.rng.random() < 0.5
        c.words[i] = sz << 9 | 0x00C0 | ea
        c.words[ext] = int(k == "a") << 15 | n << 12 | int(chk) << 11

    def cas(self, c):
        sz, size = self.rng.choice(SIZES)
        i = self.op(c, 0)
        ext = len(c.words)
        c.words.append(0)
        ea = c.ea(size, MEM_ALT, "dst")
        dc, du = c.dreg(), c.dreg()
        if self.rng.random() < 0.5:
            c.set(("d", dc), c.mem(c.last_target, size) | self.rng.getrandbits(16) << 16 * (size < 4))
        c.words[i] = 0x08C0 | (sz + 1) << 9 | ea
        c.words[ext] = du << 6 | dc

    def cas2(self, c):
        long = self.rng.random() < 0.5
        size = 4 if long else 2
        exts = []
        for region in ("src", "dst"):
            k, rn = c.alloc()
            t = c.target(region, size)
            c.set((k, rn), t)
            dc, du = c.dreg(), c.dreg()
            if self.rng.random() < 0.6:
                c.set(("d", dc), c.mem(t, size))
            exts.append(int(k == "a") << 15 | rn << 12 | du << 6 | dc)
        c.words += [0x0EFC if long else 0x0CFC] + exts

    def scc(self, c):
        i = self.op(c, 0)
        c.words[i] = 0x50C0 | self.rng.randrange(16) << 8 | c.ea(1, DATA_ALT, "dst")

    def move(self, c):
        sz, size = self.rng.choice(SIZES)
        i = self.op(c, 0)
        src = c.ea(size, srcs(size), "src")
        dst = c.ea(size, DATA_ALT, "dst")
        code = {1: 1, 2: 3, 4: 2}[size]
        c.words[i] = code << 12 | (dst & 7) << 9 | (dst >> 3) << 6 | src

    def movea(self, c):
        long = self.rng.random() < 0.5
        i = self.op(c, 0)
        src = c.ea(4 if long else 2, srcs(2), "src")
        c.words[i] = (0x2040 if long else 0x3040) | c.areg() << 9 | src

    def sr_ccr(self, c):
        r = self.rng.randrange(6)
        if r < 3:
            c.words += [[0x023C, 0x003C, 0x0A3C][r], self.rng.getrandbits(8)]
            return
        i = self.op(c, 0)
        if r == 3:
            c.words[i] = 0x44C0 | c.ea(2, SRC, "src")          # MOVE to CCR
        else:
            c.words[i] = (0x40C0 if r == 4 else 0x42C0) | c.ea(2, DATA_ALT, "dst")

    def lea_pea(self, c):
        i = self.op(c, 0)
        ea = c.ea(4, CONTROL, "any")
        if self.rng.random() < 0.5:
            c.words[i] = 0x41C0 | c.areg() << 9 | ea
        else:
            c.words[i] = 0x4840 | ea

    def movem(self, c):
        long = self.rng.random() < 0.5
        size = 4 if long else 2
        to_mem = self.rng.random() < 0.5
        mask = self.rng.getrandbits(15) or 1          # never A7
        n = bin(mask).count("1")
        i = self.op(c, 0)
        c.words.append(0)
        if to_mem and self.rng.random() < 0.4:
            an = c.areg()
            c.set(("a", an), c.target("dst", n * size) + n * size)
            ea = 0o40 | an
            mask = int(f"{mask:016b}"[::-1], 2)       # -(An): bit 0 = A7
        elif not to_mem and self.rng.random() < 0.4:
            an = c.areg()
            c.set(("a", an), c.target("src", n * size))
            ea = 0o30 | an
        else:
            kinds = CONTROL_ALT if to_mem else CONTROL
            ea = c.ea(size, kinds, "dst" if to_mem else "src", span=n * size)
        c.words[i] = 0x4880 | int(not to_mem) << 10 | int(long) << 6 | ea
        c.words[i + 1] = mask

    def movep(self, c):
        an = c.areg()
        opmode = self.rng.randrange(4, 8)
        d = self.rng.randrange(-200, 200)
        t = c.target("dst" if opmode >= 6 else "src", 8)
        c.set(("a", an), t - d)
        c.words += [0x0008 | c.dreg() << 9 | opmode << 6 | an, d & 0xFFFF]

    def exg_swap(self, c):
        r = self.rng.randrange(4)
        if r == 0:
            c.words.append(0x4840 | c.dreg())
        elif r == 1:
            c.words.append(0xC140 | c.dreg() << 9 | c.dreg())
        elif r == 2:
            c.words.append(0xC148 | c.areg() << 9 | c.areg())
        else:
            c.words.append(0xC188 | c.dreg() << 9 | c.areg())

    def tas(self, c):
        i = self.op(c, 0)
        c.words[i] = 0x4AC0 | c.ea(1, DATA_ALT, "dst")

    def link_unlk(self, c):
        an = c.areg()
        r = self.rng.randrange(3)
        if r == 2:
            p = STACK - 8 - 2 * self.rng.randrange(8)
            c.set(("a", an), p)
            c.patch(p, self.data())
            c.words.append(0x4E58 | an)
            return
        d = -2 * self.rng.randrange(16)
        if r == 0:
            c.words += [0x4E50 | an, d & 0xFFFF]
        else:
            c.words += [0x4808 | an] + words_of_long(d)

    def flow_target(self, c, length):
        """A word address in the ILLEGAL buffer outside the instruction."""
        while True:
            t = CODEBUF + 2 * self.rng.randrange(2, CODEBUF_SIZE // 2 - 2)
            if not INSN_AT - 2 <= t < INSN_AT + 2 * length + 2:
                return t

    def branch(self, c):
        cc = self.rng.randrange(16)
        form = self.rng.choice(["b", "w", "l"])
        length = {"b": 1, "w": 2, "l": 3}[form]
        t = self.flow_target(c, length)
        if form == "b":
            while not -128 <= t - INSN_AT - 2 < 128:
                t = self.flow_target(c, length)
        d = t - INSN_AT - 2
        if form == "b":
            c.words.append(0x6000 | cc << 8 | d & 0xFF)
        elif form == "w":
            c.words += [0x6000 | cc << 8, d & 0xFFFF]
        else:
            c.words += [0x60FF | cc << 8] + words_of_long(d)

    def dbcc(self, c):
        t = self.flow_target(c, 2)
        dn = c.dreg()
        c.set(("d", dn), self.rng.choice([0, 1, 0xFFFF, 0x10000, self.data()]))
        c.words += [0x50C8 | self.rng.randrange(16) << 8 | dn, (t - INSN_AT - 2) & 0xFFFF]

    def jump(self, c):
        base = self.rng.choice([0x4EC0, 0x4E80])     # JMP JSR
        r = self.rng.randrange(3)
        i = self.op(c, 0)
        if r == 0:
            an = c.areg()
            c.set(("a", an), self.flow_target(c, 1))
            ea = 0o20 | an
        elif r == 1:
            an = c.areg()
            d = self.rng.randrange(-0x8000, 0x8000)
            c.words.append(d & 0xFFFF)
            c.set(("a", an), self.flow_target(c, 2) - d)
            ea = 0o50 | an
        else:
            c.words.append(0)
            t = self.flow_target(c, 2)
            c.words[-1] = (t - INSN_AT - 2) & 0xFFFF
            ea = 0o72
        c.words[i] = base | ea

    def returns(self, c):
        t = self.flow_target(c, 2)
        r = self.rng.randrange(3)
        if r == 0:
            c.patch(STACK, t)
            c.words.append(0x4E75)                   # RTS
        elif r == 1:
            ccr = self.rng.getrandbits(16)
            c.patch(STACK, ccr << 16 | t >> 16)
            c.patch(STACK + 4, (t & 0xFFFF) << 16)
            c.words.append(0x4E77)                   # RTR
        else:
            c.patch(STACK, t)
            c.words += [0x4E74, (2 * self.rng.randrange(-8, 24)) & 0xFFFF]  # RTD

    def traps(self, c):
        r = self.rng.randrange(4)
        if r == 0:
            c.words.append(0x4E40 | self.rng.randrange(16))   # TRAP
        elif r == 1:
            c.words.append(0x4E76)                           # TRAPV
        else:
            cc = self.rng.randrange(16) << 8
            form = self.rng.randrange(3)
            c.words.append(0x50FA + form | cc)                # TRAPcc.W/.L/-
            c.words += [[self.rng.getrandbits(16)], words_of_long(self.data()), []][form]

    def lines(self, c):
        r = self.rng.randrange(3)
        if r == 0:
            c.words.append(0xA000 | self.rng.getrandbits(12))
        elif r == 1:
            c.words.append(0x4AFC)
        else:
            c.words.append(0x4E71)                            # NOP


def build_cases(gen, n):
    out = bytearray()
    for _ in range(n):
        c = gen.case()
        regs = []
        for k in ("d", "a"):
            for r in range(8 if k == "d" else 7):
                regs.append(c.regs.get((k, r), gen.data()))
        sr = 0x2700 | gen.rng.randrange(32)
        rec = struct.pack(">H", len(c.words))
        rec += struct.pack(f">{MAX_WORDS}H", *(c.words + [ILLEGAL] * (MAX_WORDS - len(c.words))))
        rec += struct.pack(">15I", *regs)
        rec += struct.pack(">HH", sr, len(c.patches))
        for addr, v in c.patches:
            rec += struct.pack(">II", addr, v)
        rec += bytes(CASE_SIZE - len(rec))
        assert len(rec) == CASE_SIZE
        out += rec
    return bytes(out)


def build_boot(image_len):
    a = Asm(0)
    a.w(0x2F09)                              # MOVE.L A1,-(A7)
    a.w(0x203C); a.l(ALLOC_END - ALLOC_START)
    a.w(0x43F9); a.l(ALLOC_START)            # LEA start,A1
    a.w(0x4EAE, LVO_ALLOCABS & 0xFFFF)
    a.w(0x225F)                              # MOVEA.L (A7)+,A1
    a.w(0x4A80)                              # TST.L D0
    a.branch(BEQ, "fail", "s")
    a.w(0x237C); a.l(CODE); a.w(IO_DATA)
    a.w(0x237C); a.l(image_len); a.w(IO_LENGTH)
    a.w(0x237C); a.l(1024); a.w(IO_OFFSET)
    a.w(0x337C, CMD_READ, IO_COMMAND)
    a.w(0x4EAE, LVO_DOIO & 0xFFFF)
    a.w(0x4A29, IO_ERROR)
    a.branch(BNE, "fail", "s")
    a.w(0x4EF9); a.l(CODE)                   # JMP CODE
    a.label("fail")
    a.w(0x33FC, 0x0F00); a.l(COLOR00)
    a.branch(BRA, "fail", "s")
    return a.finish()


def build(seed, n, kinds=None):
    gen = Gen(seed, kinds)
    harness, res_off = build_harness(n, seed)
    image = harness + gen.pristine + bytes(CASES - PRISTINE - AREA_SIZE) + build_cases(gen, n)
    assert len(image) == CASES - CODE + n * CASE_SIZE
    image += bytes(-len(image) % 512)
    assert 1024 + len(image) <= res_off
    assert res_off + n * REC_SIZE <= 901120, "too many cases for one disk"
    assert RESULTS + n * REC_SIZE <= AREA
    assert CASES + n * CASE_SIZE <= RESULTS
    boot = build_boot(len(image))
    adf = bytearray(901120)
    adf[0:4] = b"DOS\0"
    adf[8:12] = struct.pack(">I", 880)
    adf[12:12 + len(boot)] = boot
    adf[1024:1024 + len(image)] = image
    s = 0
    for i in range(0, 1024, 4):
        s += struct.unpack(">I", adf[i:i + 4])[0]
        s = (s & 0xFFFFFFFF) + (s >> 32)
    adf[4:8] = struct.pack(">I", (~s) & 0xFFFFFFFF)
    return bytes(adf), image


def header(adf):
    h = adf[1024 + HEADER_AT:1024 + HEADER_AT + struct.calcsize(HEADER_FMT)]
    f = struct.unpack(HEADER_FMT, h)
    if f[0] != MAGIC:
        sys.exit("not a m68020_random disk")
    return dict(zip(["magic", "version", "cases", "seed", "case_at", "case_size",
                     "results", "rec_size", "res_off", "done", "area", "area_size",
                     "insn_at", "hstack", "green", "fail"], f))


def main():
    args = sys.argv[1:]
    if len(args) >= 2 and args[0] == "build":
        out = args[1]
        opts = dict(zip(args[2::2], args[3::2]))
        seed = int(opts.get("--seed", 1))
        n = int(opts.get("--cases", DEFAULT_CASES))
        kinds = opts["--kinds"].split(",") if "--kinds" in opts else None
        adf, image = build(seed, n, kinds)
        open(out, "wb").write(adf)
        if "--bin" in opts:
            open(opts["--bin"], "wb").write(image)
        print(f"{n} cases, seed {seed}: {out} (program {len(image)} bytes)")
    elif len(args) == 3 and args[0] == "extract":
        adf = open(args[1], "rb").read()
        h = header(adf)
        res = adf[h["res_off"]:h["res_off"] + h["cases"] * h["rec_size"]]
        if not any(res):
            sys.exit("no results on the disk: did the program run (green screen)?")
        open(args[2], "wb").write(res)
        print(f"{h['cases']} results (seed {h['seed']}) -> {args[2]}")
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
