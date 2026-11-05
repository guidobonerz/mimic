"""TED timing test programs for the C16/Plus/4, measured against another
emulator or real hardware and compared with mimic.

    python tools/ted_timing.py build OUT_DIR
        writes one PRG per test (ted-<name>.prg) into OUT_DIR
    python tools/ted_timing.py vice XPLUS4.EXE OUT_DIR [TEST ...]
        runs each PRG in VICE's xplus4 for a fixed number of cycles, takes
        the exit screenshot and writes the measured bytes as
        tests/data/ted-<name>.ref (raw bytes) next to the program as
        tests/data/ted-<name>.prg
    python tools/ted_timing.py show FILE.ref
        prints a reference as a table

Every program measures with interrupts off, keeps its results in RAM at
$2000 (RESULTS bytes) and then shows $2000–$3F3F as a hires bitmap, white
on black, so the bytes can be read back from a screenshot without a
monitor: byte i of the bitmap is row i // 320, column (i % 320) // 8,
pixel line i % 8; xplus4's PAL screenshot puts the 320 × 200 window at
(32, 40). mimic's test (tests/tests/ted_timing.rs) reads the bytes from RAM.

Tests (PAL; each run starts with SYNC, below):
  trace: 1024 iterations of
        LDA $FF1E / STA hpos,X / LDA $FF1D / STA line,X / INX / BNE
    (23 CPU cycles each, four pages of 256) from line 300 through the
    whole visible frame. Results: 1024 bytes of $FF1E, then 1024 of $FF1D.
    The step of $FF1E between samples is the time the iteration took
    (2 units a TED double clock): 23 CPU cycles stretched by whatever the
    TED took from the CPU.
  fine: 14 runs, run j delayed by j + 2 CPU cycles after SYNC, each 256
    iterations of LDA $FF1E / STA hpos,X / INX / BNE (14 cycles). Together
    they place every stolen cycle to the double clock. Results: 14 × 256
    bytes of $FF1E.
  start, startdma: eleven probe instructions (NOP, STA/INC/PHA/LDA in
    several modes) moved cycle by cycle across tx 3 of a fetch line
    (startdma: of an attribute DMA line), then $FF1E and $FF1D.
  yscroll: the CPU clock through display lines for YSCROLL 0-7.
  dma: a YSCROLL write cycle by cycle through line 102: when does it
    still make the line a DMA line?
  counters: $FF1A/$FF1B/$FF1F over a frame.
  split, rows, fli: pictures (xplus4's screenshot is the reference).

SYNC: wait for line 300 ($FF1C bit 0 set, $FF1D = 44; vertical blank,
where the CPU runs at double clock), wait for $FF1E < $10, then for
$FF1E >= $20; read $FF1E once more and jump into a chain of $C9 bytes
ending in $24 $EA (`CMP #$C9` pairs, then `BIT $EA` or `CMP #$24 / NOP`)
at an offset of half the excess, which delays by one cycle per entry
byte. Afterwards the CPU stands at the same double clock of line 300 in
every run, however the program got there.

Stdlib only.
"""
import os
import struct
import subprocess
import sys
import zlib

# --- a small 6502 assembler -----------------------------------------------

OPS = {
    # mnemonic: {mode: opcode}
    "ADC": {"imm": 0x69, "zp": 0x65, "abs": 0x6D},
    "SBC": {"imm": 0xE9},
    "AND": {"imm": 0x29, "zp": 0x25, "abs": 0x2D},
    "ASL": {"acc": 0x0A},
    "BCC": {"rel": 0x90}, "BCS": {"rel": 0xB0}, "BEQ": {"rel": 0xF0},
    "BMI": {"rel": 0x30}, "BNE": {"rel": 0xD0}, "BPL": {"rel": 0x10},
    "BIT": {"zp": 0x24, "abs": 0x2C},
    "CLC": {"imp": 0x18}, "CLI": {"imp": 0x58}, "SEC": {"imp": 0x38},
    "SEI": {"imp": 0x78},
    "CMP": {"imm": 0xC9, "zp": 0xC5, "abs": 0xCD},
    "CPX": {"imm": 0xE0, "abs": 0xEC}, "CPY": {"imm": 0xC0},
    "DEC": {"zp": 0xC6, "abs": 0xCE},
    "DEX": {"imp": 0xCA}, "DEY": {"imp": 0x88},
    "INX": {"imp": 0xE8}, "INY": {"imp": 0xC8},
    "EOR": {"imm": 0x49},
    "JMP": {"abs": 0x4C}, "JSR": {"abs": 0x20}, "RTS": {"imp": 0x60},
    "RTI": {"imp": 0x40},
    "LDA": {"imm": 0xA9, "zp": 0xA5, "abs": 0xAD, "abx": 0xBD, "aby": 0xB9},
    "LDX": {"imm": 0xA2, "zp": 0xA6, "abs": 0xAE},
    "LDY": {"imm": 0xA0, "zp": 0xA4, "abs": 0xAC},
    "LSR": {"acc": 0x4A},
    "NOP": {"imp": 0xEA},
    "ORA": {"imm": 0x09},
    "PHA": {"imp": 0x48}, "PLA": {"imp": 0x68},
    "STA": {"zp": 0x85, "abs": 0x8D, "abx": 0x9D, "aby": 0x99, "izy": 0x91},
    "INC": {"zp": 0xE6, "abs": 0xEE, "abx": 0xFE},
    "STX": {"zp": 0x86, "abs": 0x8E}, "STY": {"zp": 0x84, "abs": 0x8C},
    "TAX": {"imp": 0xAA}, "TAY": {"imp": 0xA8}, "TXA": {"imp": 0x8A},
    "TYA": {"imp": 0x98},
}
SIZE = {"imp": 1, "acc": 1, "imm": 2, "zp": 2, "rel": 2, "abs": 3, "abx": 3, "aby": 3, "izy": 2}


class Asm:
    def __init__(self, org):
        self.org = org
        self.code = bytearray()
        self.labels = {}
        self.fixups = []

    @property
    def pc(self):
        return self.org + len(self.code)

    def label(self, name):
        self.labels[name] = self.pc

    def byte(self, *bs):
        self.code += bytes(bs)

    def op(self, mnem, mode="imp", arg=None):
        opcode = OPS[mnem][mode]
        here = self.pc
        self.code.append(opcode)
        n = SIZE[mode] - 1
        if n == 0:
            return
        if isinstance(arg, str):
            self.fixups.append((len(self.code), mode, arg, here))
            self.code += bytes(n)
        elif mode == "rel":
            raise ValueError("branches take a label")
        elif n == 1:
            self.code.append(arg & 0xFF)
        else:
            self.code += struct.pack("<H", arg & 0xFFFF)

    def value(self, name):
        """A label, `label+n`, `<label` (low byte) or `label>` (high byte)."""
        if name.startswith("<"):
            return self.value(name[1:]) & 0xFF
        if name.endswith(">"):
            return self.value(name[:-1]) >> 8
        if "+" in name:
            base, off = name.split("+")
            return self.value(base) + int(off)
        return self.labels[name]

    def assemble(self):
        for at, mode, name, here in self.fixups:
            target = self.value(name)
            if mode == "rel":
                d = target - (here + 2)
                if not -128 <= d <= 127:
                    raise ValueError(f"branch to {name} out of range")
                self.code[at] = d & 0xFF
            elif SIZE[mode] == 2:
                self.code[at] = target & 0xFF
            else:
                self.code[at:at + 2] = struct.pack("<H", target)
        return bytes(self.code)


# --- program frame ----------------------------------------------------------

RESULTS = 0x2000
BITMAP_BYTES = 8000


def basic_stub(a):
    """10 SYS4109, then a JMP to the code (at $100D)."""
    a.byte(0x0B, 0x10, 0x0A, 0x00, 0x9E)
    a.code += b"4109"
    a.byte(0x00, 0x00, 0x00)
    a.op("JMP", "abs", "main")


def show_results(a):
    """Hires bitmap at $2000 (TED $FF12 bits 3–5 = 1), matrix at $0800:
    a 1 dot takes colour bits 4–7 of the matrix byte and luminance bits 0–2
    of the attribute, a 0 dot bits 0–3 and 4–6 (TED data sheet), so
    attribute $07 and matrix byte $10 give white 1-pixels on black."""
    a.op("LDX", "imm", 0)
    a.label("fill")
    for page in range(4):
        a.op("LDA", "imm", 0x07)
        a.op("STA", "abx", 0x0800 + page * 0x100)
        a.op("LDA", "imm", 0x10)
        a.op("STA", "abx", 0x0C00 + page * 0x100)
    a.op("INX")
    a.op("BNE", "rel", "fill")
    # Done marker in the bitmap's last two bytes (checked by `vice`).
    a.op("LDA", "imm", 0xA5)
    a.op("STA", "abs", RESULTS + BITMAP_BYTES - 2)
    a.op("LDA", "imm", 0x5A)
    a.op("STA", "abs", RESULTS + BITMAP_BYTES - 1)
    a.op("LDA", "imm", 0x00)
    a.op("STA", "abs", 0xFF15)
    a.op("STA", "abs", 0xFF19)
    a.op("LDA", "imm", 0x08)          # bitmap at $2000, voice 1 high bits 0
    a.op("STA", "abs", 0xFF12)
    a.op("LDA", "imm", 0x08)          # matrix at $0800
    a.op("STA", "abs", 0xFF14)
    a.op("LDA", "imm", 0x3B)          # BMM, DEN, 25 rows, YSCROLL 3
    a.op("STA", "abs", 0xFF06)
    a.op("LDA", "imm", 0x08)          # hires, 40 columns, XSCROLL 0
    a.op("STA", "abs", 0xFF07)
    a.label("done")
    a.op("JMP", "abs", "done")


def blank_screen(a):
    """Spaces with attribute $71 everywhere, the hardware cursor off the
    screen: the text the loading left differs between emulators."""
    a.op("LDX", "imm", 0)
    a.label("blank")
    for page in range(4):
        a.op("LDA", "imm", 0x71)
        a.op("STA", "abx", 0x0800 + page * 0x100)
        a.op("LDA", "imm", 0x20)
        a.op("STA", "abx", 0x0C00 + page * 0x100)
    a.op("INX")
    a.op("BNE", "rel", "blank")
    a.op("LDA", "imm", 0x03)
    a.op("STA", "abs", 0xFF0C)
    a.op("LDA", "imm", 0xFF)
    a.op("STA", "abs", 0xFF0D)


def clear_results(a):
    """Zero $2000–$3FFF (the bitmap) before measuring."""
    a.op("LDA", "imm", 0)
    a.op("LDX", "imm", 0)
    a.label("clr")
    for page in range(0x20):
        a.op("STA", "abx", RESULTS + page * 0x100)
    a.op("INX")
    a.op("BNE", "rel", "clr")


# --- tests -------------------------------------------------------------------

SYNC_LINE = 300
CHAIN = 12


def sync(a, tag):
    """Stand at a fixed double clock of line SYNC_LINE (see the docstring)."""
    a.label(f"{tag}s1")
    a.op("LDA", "abs", 0xFF1C)
    a.op("AND", "imm", 0x01)
    a.op("BEQ", "rel", f"{tag}s1")
    a.op("LDA", "abs", 0xFF1D)
    a.op("CMP", "imm", SYNC_LINE - 256)
    a.op("BNE", "rel", f"{tag}s1")
    a.label(f"{tag}s2")
    a.op("LDA", "abs", 0xFF1E)
    a.op("CMP", "imm", 0x10)
    a.op("BCS", "rel", f"{tag}s2")
    # Still line SYNC_LINE? (A late first sight of the line would otherwise
    # land the sync a line further on.)
    a.op("LDA", "abs", 0xFF1D)
    a.op("CMP", "imm", SYNC_LINE - 256)
    a.op("BNE", "rel", f"{tag}s1")
    a.label(f"{tag}s3")
    a.op("LDA", "abs", 0xFF1E)
    a.op("CMP", "imm", 0x20)
    a.op("BCC", "rel", f"{tag}s3")
    # The read 8 cycles (16 units) after the one that left the loop:
    # $30–$42 for exits at $20–$32.
    a.op("LDA", "abs", 0xFF1E)
    a.op("SEC")
    a.op("SBC", "imm", 0x30)
    a.op("LSR", "acc")
    a.op("CLC")
    a.op("ADC", "imm", f"<{tag}chain")
    a.op("STA", "abs", f"{tag}jmp+1")
    a.label(f"{tag}jmp")
    a.op("JMP", "abs", f"{tag}chain")
    # Keep the chain inside one page (its high byte is fixed).
    if (a.pc & 0xFF) + CHAIN + 2 > 0xFF:
        a.code += bytes(0x100 - (a.pc & 0xFF))
    a.label(f"{tag}chain")
    a.byte(*([0xC9] * CHAIN), 0x24, 0xEA)


def delay(a, cycles):
    """Exactly `cycles` CPU cycles (0 or at least 2)."""
    if cycles % 2:
        a.op("BIT", "zp", 0xEA)
        cycles -= 3
    for _ in range(cycles // 2):
        a.op("NOP")


TRACE_PAGES = 4


def build_trace():
    a = Asm(0x1001)
    basic_stub(a)
    a.label("main")
    a.op("SEI")
    clear_results(a)
    sync(a, "t")
    for page in range(TRACE_PAGES):
        hpos = RESULTS + page * 0x100
        lines = RESULTS + TRACE_PAGES * 0x100 + page * 0x100
        a.op("LDX", "imm", 0)
        a.label(f"t{page}")
        a.op("LDA", "abs", 0xFF1E)
        a.op("STA", "abx", hpos)
        a.op("LDA", "abs", 0xFF1D)
        a.op("STA", "abx", lines)
        a.op("INX")
        a.op("BNE", "rel", f"t{page}")
    show_results(a)
    return a.assemble(), 2 * TRACE_PAGES * 0x100


FINE_RUNS = 14


def build_fine():
    a = Asm(0x1001)
    basic_stub(a)
    a.label("main")
    a.op("SEI")
    clear_results(a)
    for run in range(FINE_RUNS):
        sync(a, f"f{run}")
        delay(a, run + 2)
        a.op("LDX", "imm", 0)
        a.label(f"f{run}l")
        a.op("LDA", "abs", 0xFF1E)
        a.op("STA", "abx", RESULTS + run * 0x100)
        a.op("INX")
        a.op("BNE", "rel", f"f{run}l")
    show_results(a)
    return a.assemble(), FINE_RUNS * 0x100


IRQ_LINES = [0, 2, 3, 4, 5, 100, 203, 204, 250, 311]


def build_irqall():
    """Raster IRQ entry for every line 0-311 (one per frame): $FF1E at
    $2000+line... as two pages (lines 0-255, 256-311), $FF1D likewise at
    $2200. Same frame as `irq`."""
    a = Asm(0x1001)
    basic_stub(a)
    a.label("main")
    a.op("SEI")
    clear_results(a)
    a.op("STA", "abs", 0xFF3F)
    a.op("LDA", "imm", "<irqa")
    a.op("STA", "abs", 0xFFFE)
    a.op("LDA", "imm", "irqa>")
    a.op("STA", "abs", 0xFFFF)
    # line counter in $F2 (low) / $F3 (high)
    a.op("LDA", "imm", 0)
    a.op("STA", "zp", 0xF2)
    a.op("STA", "zp", 0xF3)
    a.label("next")
    a.op("LDA", "imm", 0)
    a.op("STA", "zp", 0xF1)
    a.op("LDA", "zp", 0xF2)
    a.op("STA", "abs", 0xFF0B)
    a.op("LDA", "zp", 0xF3)
    a.op("ORA", "imm", 0x02)
    a.op("STA", "abs", 0xFF0A)
    sync(a, "ia")
    a.op("LDA", "imm", 0xFF)
    a.op("STA", "abs", 0xFF09)
    a.op("CLI")
    a.label("iaw")
    for _ in range(20):
        a.op("NOP")
    a.op("LDA", "zp", 0xF1)
    a.op("BEQ", "rel", "iaw")
    a.op("SEI")
    a.op("INC", "zp", 0xF2)
    a.op("BNE", "rel", "nohi")
    a.op("INC", "zp", 0xF3)
    a.label("nohi")
    a.op("LDA", "zp", 0xF3)
    a.op("BEQ", "rel", "more")
    a.op("LDA", "zp", 0xF2)
    a.op("CMP", "imm", 312 - 256)
    a.op("BEQ", "rel", "fin")
    a.label("more")
    a.op("JMP", "abs", "next")
    a.label("fin")
    a.op("STA", "abs", 0xFF3E)
    show_results(a)
    a.label("irqa")
    a.op("LDA", "abs", 0xFF1E)
    a.op("LDX", "zp", 0xF2)
    a.op("LDY", "zp", 0xF3)
    a.op("BNE", "rel", "ihi")
    a.op("STA", "abx", RESULTS)
    a.op("LDA", "abs", 0xFF1D)
    a.op("STA", "abx", RESULTS + 0x200)
    a.op("JMP", "abs", "iend")
    a.label("ihi")
    a.op("STA", "abx", RESULTS + 0x100)
    a.op("LDA", "abs", 0xFF1D)
    a.op("STA", "abx", RESULTS + 0x300)
    a.label("iend")
    a.op("LDA", "imm", 0xFF)
    a.op("STA", "abs", 0xFF09)
    a.op("INC", "zp", 0xF1)
    a.op("RTI")
    return a.assemble(), 0x400


def build_irq():
    """Raster IRQ entry: for each compare line, the handler's first
    instructions read $FF1E, $FF1D and $FF1C while the main program runs a
    loop of NOPs (ROM switched off, the IRQ vector in RAM)."""
    a = Asm(0x1001)
    basic_stub(a)
    a.label("main")
    a.op("SEI")
    clear_results(a)
    a.op("STA", "abs", 0xFF3F)          # RAM at $8000-$FFFF
    a.op("LDA", "imm", "<irq")
    a.op("STA", "abs", 0xFFFE)
    a.op("LDA", "imm", "irq>")
    a.op("STA", "abs", 0xFFFF)
    for k, line in enumerate(IRQ_LINES):
        a.op("LDA", "imm", k)
        a.op("STA", "zp", 0xF0)
        a.op("LDA", "imm", 0)
        a.op("STA", "zp", 0xF1)
        a.op("LDA", "imm", line & 0xFF)
        a.op("STA", "abs", 0xFF0B)
        a.op("LDA", "imm", 0x02 | (line >> 8))
        a.op("STA", "abs", 0xFF0A)
        sync(a, f"i{k}")
        a.op("LDA", "imm", 0xFF)         # drop flags latched before
        a.op("STA", "abs", 0xFF09)
        a.op("CLI")
        a.label(f"i{k}w")
        for _ in range(20):
            a.op("NOP")
        a.op("LDA", "zp", 0xF1)
        a.op("BEQ", "rel", f"i{k}w")
        a.op("SEI")
    a.op("STA", "abs", 0xFF3E)          # ROM back
    show_results(a)
    a.label("irq")
    a.op("LDA", "abs", 0xFF1E)
    a.op("LDX", "zp", 0xF0)
    a.op("STA", "abx", RESULTS)
    a.op("LDA", "abs", 0xFF1D)
    a.op("STA", "abx", RESULTS + 0x10)
    a.op("LDA", "abs", 0xFF1C)
    a.op("STA", "abx", RESULTS + 0x20)
    a.op("LDA", "imm", 0xFF)
    a.op("STA", "abs", 0xFF09)
    a.op("INC", "zp", 0xF1)
    a.op("RTI")
    return a.assemble(), 0x30


SPLIT_WRITES = 2400


def build_split():
    """A picture test: every frame, from SYNC on, a straight run of
    STX/STY $FF19 (border) and $FF15 (background) alternating two colours,
    4 cycles each, through the top border and about 120 display lines. The
    stripes' edges show where a register write reaches the picture."""
    a = Asm(0x1001)
    basic_stub(a)
    a.label("main")
    a.op("SEI")
    blank_screen(a)
    a.op("LDX", "imm", 0x32)            # red
    a.op("LDY", "imm", 0x55)            # green
    a.label("frame")
    sync(a, "sp")
    regs = [0xFF19, 0xFF19, 0xFF15, 0xFF15]
    for i in range(SPLIT_WRITES):
        a.op("STX" if i % 2 == 0 else "STY", "abs", regs[(i // 2) % 2 * 2])
    a.op("JMP", "abs", "frame")
    return a.assemble(), 0


def build_rows():
    """A picture test: the text screen with every row in its own colour
    (attributes $31 + row) and every character a solid reversed space;
    shows which line takes which row's attributes (the attribute DMA runs
    on the last line of the row before)."""
    a = Asm(0x1001)
    basic_stub(a)
    a.label("main")
    a.op("SEI")
    blank_screen(a)
    # Row r: attribute ($31 + r) & $7F; character $A0 (reversed space).
    for r in range(25):
        for c in range(40):
            pass
    a.op("LDX", "imm", 0)
    a.label("rowsl")
    a.op("LDA", "abx", "rowattr")
    a.op("STA", "zp", 0xF0)
    a.op("TXA")
    a.op("PHA")
    # pointer = $0800 + 40 * row, from the table
    a.op("LDA", "abx", "rowlo")
    a.op("STA", "zp", 0xF2)
    a.op("LDA", "abx", "rowhi")
    a.op("STA", "zp", 0xF3)
    a.op("LDY", "imm", 39)
    a.label("colsl")
    a.op("LDA", "zp", 0xF0)
    a.op("STA", "izy", 0xF2)
    a.op("DEY")
    a.op("BPL", "rel", "colsl")
    a.op("LDA", "zp", 0xF3)
    a.op("CLC")
    a.op("ADC", "imm", 0x04)
    a.op("STA", "zp", 0xF3)
    a.op("LDY", "imm", 39)
    a.op("LDA", "imm", 0xA0)
    a.label("chrsl")
    a.op("STA", "izy", 0xF2)
    a.op("DEY")
    a.op("BPL", "rel", "chrsl")
    a.op("PLA")
    a.op("TAX")
    a.op("INX")
    a.op("CPX", "imm", 25)
    a.op("BNE", "rel", "rowsl")
    a.label("rowsend")
    a.op("JMP", "abs", "rowsend")
    a.label("rowattr")
    a.byte(*[((0x31 + r) & 0x7F) for r in range(25)])
    a.label("rowlo")
    a.byte(*[(0x0800 + 40 * r) & 0xFF for r in range(25)])
    a.label("rowhi")
    a.byte(*[(0x0800 + 40 * r) >> 8 for r in range(25)])
    return a.assemble(), 0


COUNTER_PAGES = 4
COUNTER_REGS = [0xFF1E, 0xFF1D, 0xFF1F, 0xFF1A, 0xFF1B]


def build_counters():
    """The TED's internal counters over a frame: from SYNC on, 1024
    samples of $FF1E, $FF1D, $FF1F (character line, flash counter),
    $FF1A/$FF1B (character position), 41 cycles apart, on the KERNAL's
    text screen. Results: per register 1024 bytes, in that order."""
    a = Asm(0x1001)
    basic_stub(a)
    a.label("main")
    a.op("SEI")
    clear_results(a)
    sync(a, "c")
    for page in range(COUNTER_PAGES):
        a.op("LDX", "imm", 0)
        a.label(f"c{page}")
        for k, reg in enumerate(COUNTER_REGS):
            a.op("LDA", "abs", reg)
            a.op("STA", "abx", RESULTS + k * COUNTER_PAGES * 0x100 + page * 0x100)
        a.op("INX")
        a.op("BNE", "rel", f"c{page}")
    show_results(a)
    return a.assemble(), len(COUNTER_REGS) * COUNTER_PAGES * 0x100


def build_fli():
    """A picture test of FLI: eight video matrices at $8000-$BFFF (ROM off
    for the CPU), matrix k with attributes ($42 + k) and character 0,
    solid in a charset in RAM at $C000. On every line L from 2 to 199 the program waits for the line
    counter to reach L, then sets YSCROLL = (L + 1) & 7 (an attribute DMA
    on line L + 1) and $FF14 to matrix (L + 1) & 7: each line takes its own
    matrix's colours."""
    a = Asm(0x1001)
    basic_stub(a)
    a.label("main")
    a.op("SEI")
    a.op("STA", "abs", 0xFF3F)
    # fill the matrices: attributes at m + 0..999, characters at m + $400.
    for k in range(8):
        base = 0x8000 + k * 0x800
        a.op("LDX", "imm", 0)
        a.label(f"fm{k}")
        for page in range(4):
            a.op("LDA", "imm", 0x40 | (k + 2))
            a.op("STA", "abx", base + page * 0x100)
            a.op("LDA", "imm", 0x00)
            a.op("STA", "abx", base + 0x400 + page * 0x100)
        a.op("INX")
        a.op("BNE", "rel", f"fm{k}")
    # character 0 of a RAM charset at $C000 is solid (no ROM fetches).
    a.op("LDA", "imm", 0xFF)
    a.op("LDX", "imm", 7)
    a.label("fcs")
    a.op("STA", "abx", 0xC000)
    a.op("DEX")
    a.op("BPL", "rel", "fcs")
    a.op("LDA", "abs", 0xFF12)
    a.op("AND", "imm", 0xFB)
    a.op("STA", "abs", 0xFF12)
    a.op("LDA", "abs", 0xFF13)
    a.op("AND", "imm", 0x03)
    a.op("ORA", "imm", 0xC0)
    a.op("STA", "abs", 0xFF13)
    a.op("LDA", "imm", 0x03)             # cursor off the screen
    a.op("STA", "abs", 0xFF0C)
    a.op("LDA", "imm", 0xFF)
    a.op("STA", "abs", 0xFF0D)
    a.label("frame")
    # wait for line 1 (high bit clear)
    a.label("fw0")
    a.op("LDA", "abs", 0xFF1C)
    a.op("AND", "imm", 0x01)
    a.op("BNE", "rel", "fw0")
    a.op("LDA", "abs", 0xFF1D)
    a.op("CMP", "imm", 1)
    a.op("BNE", "rel", "fw0")
    # unrolled: a DMA line leaves the CPU only some 25 cycles.
    for line in range(2, 200):
        nxt = (line + 1) & 7
        a.op("LDA", "imm", line)
        a.label(f"fl{line}")
        a.op("CMP", "abs", 0xFF1D)
        a.op("BNE", "rel", f"fl{line}")
        a.op("LDA", "imm", 0x18 | nxt)   # DEN, 25 rows
        a.op("STA", "abs", 0xFF06)
        a.op("LDA", "imm", (0x8000 + nxt * 0x800) >> 8)
        a.op("STA", "abs", 0xFF14)
    a.op("LDA", "imm", 0x1B)             # back to normal for the rest
    a.op("STA", "abs", 0xFF06)
    a.op("LDA", "imm", 0x80)
    a.op("STA", "abs", 0xFF14)
    a.op("JMP", "abs", "frame")
    return a.assemble(), 0


DMA_LINE = 102          # line & 7 = 6: in the middle of a character row
DMA_FRAMES = 80         # write positions, one cycle apart
DMA_SAMPLES = 24
DMA_BASE = int(os.environ.get("TED_DMA_BASE", "950"))
DMA_VALUES = [0x18 | (DMA_LINE & 7), 0x18 | ((DMA_LINE + 1) & 7)]


def build_dma():
    """When the TED decides a DMA line: frame after frame, from SYNC on,
    a fixed delay plus 0–79 cycles, then STX $FF06 with YSCROLL = 6 (the
    line counter's DMA_LINE & 7: does the current line still turn into a
    DMA line?) in the first 80 frames and YSCROLL = 7 (the next line) in
    the next 80, then the line counter and 23 samples of $FF1E, 12 cycles
    apart (stalls show the DMA); $FF06 back to $1B. Results: per frame
    DMA_SAMPLES bytes."""
    a = Asm(0x1001)
    basic_stub(a)
    a.label("main")
    a.op("SEI")
    clear_results(a)
    a.op("LDA", "imm", RESULTS & 0xFF)
    a.op("STA", "zp", 0xF2)
    a.op("LDA", "imm", RESULTS >> 8)
    a.op("STA", "zp", 0xF3)
    a.op("LDA", "imm", DMA_VALUES[0])
    a.op("STA", "zp", 0xF1)
    a.op("LDA", "imm", 0)
    a.op("STA", "zp", 0xF0)
    a.label("dframe")
    sync(a, "ys")
    a.op("LDA", "zp", 0xF0)
    a.op("CLC")
    a.op("ADC", "imm", "<dchain")
    a.op("STA", "abs", "djmp+1")
    a.op("LDX", "zp", 0xF1)
    a.op("LDY", "imm", 0)
    # fixed delay: 5 cycles a round (DMA_BASE rounds of 256 minus).
    for _ in range(DMA_BASE // 256):
        a.op("LDA", "imm", 0)
        a.label(f"dd{_}")
        a.op("SEC")
        a.op("SBC", "imm", 1)
        a.op("BNE", "rel", f"dd{_}")
    if DMA_BASE % 256:
        a.op("LDA", "imm", DMA_BASE % 256)
        a.label("ddr")
        a.op("SEC")
        a.op("SBC", "imm", 1)
        a.op("BNE", "rel", "ddr")
    a.label("djmp")
    a.op("JMP", "abs", "dchain")
    if (a.pc & 0xFF) + DMA_FRAMES + 2 > 0xFF:
        a.code += bytes(0x100 - (a.pc & 0xFF))
    a.label("dchain")
    a.byte(*([0xC9] * DMA_FRAMES), 0x24, 0xEA)
    a.op("STX", "abs", 0xFF06)
    a.op("LDA", "abs", 0xFF1D)
    a.op("STA", "izy", 0xF2)
    a.op("INY")
    for _ in range(DMA_SAMPLES - 1):
        a.op("LDA", "abs", 0xFF1E)
        a.op("STA", "izy", 0xF2)
        a.op("INY")
    a.op("LDA", "imm", 0x1B)
    a.op("STA", "abs", 0xFF06)
    a.op("LDA", "zp", 0xF2)
    a.op("CLC")
    a.op("ADC", "imm", DMA_SAMPLES)
    a.op("STA", "zp", 0xF2)
    a.op("LDA", "zp", 0xF3)
    a.op("ADC", "imm", 0)
    a.op("STA", "zp", 0xF3)
    a.op("INC", "zp", 0xF0)
    a.op("LDA", "zp", 0xF0)
    a.op("CMP", "imm", DMA_FRAMES)
    a.op("BEQ", "rel", "dnext")
    a.op("JMP", "abs", "dframe")
    a.label("dnext")
    a.op("LDA", "imm", 0)
    a.op("STA", "zp", 0xF0)
    a.op("LDA", "zp", 0xF1)
    a.op("CMP", "imm", DMA_VALUES[1])
    a.op("BEQ", "rel", "dshow")
    a.op("LDA", "imm", DMA_VALUES[1])
    a.op("STA", "zp", 0xF1)
    a.op("JMP", "abs", "dframe")
    a.label("dshow")
    show_results(a)
    return a.assemble(), 2 * DMA_FRAMES * DMA_SAMPLES


YSC_SAMPLES = 128


def build_yscroll():
    """The CPU clock in the display for every YSCROLL: for k = 0–7 one
    frame each with $FF06 = $18 + k from the border on, from SYNC a fixed
    delay into line ~100, then the line counter and 127 samples of $FF1E,
    15 cycles apart. Results: per k 128 bytes."""
    a = Asm(0x1001)
    basic_stub(a)
    a.label("main")
    a.op("SEI")
    clear_results(a)
    a.op("LDA", "imm", RESULTS & 0xFF)
    a.op("STA", "zp", 0xF2)
    a.op("LDA", "imm", RESULTS >> 8)
    a.op("STA", "zp", 0xF3)
    a.op("LDA", "imm", 0)
    a.op("STA", "zp", 0xF0)
    a.label("vframe")
    sync(a, "vs")
    a.op("LDA", "zp", 0xF0)
    a.op("ORA", "imm", 0x18)
    a.op("STA", "abs", 0xFF06)
    a.op("LDY", "imm", 0)
    for i in range(3):
        a.op("LDA", "imm", 0)
        a.label(f"vd{i}")
        a.op("SEC")
        a.op("SBC", "imm", 1)
        a.op("BNE", "rel", f"vd{i}")
    a.op("LDA", "imm", 160)
    a.label("vdr")
    a.op("SEC")
    a.op("SBC", "imm", 1)
    a.op("BNE", "rel", "vdr")
    a.op("LDA", "abs", 0xFF1D)
    a.op("STA", "izy", 0xF2)
    a.op("INY")
    a.label("vsamp")
    a.op("LDA", "abs", 0xFF1E)
    a.op("STA", "izy", 0xF2)
    a.op("INY")
    a.op("BPL", "rel", "vsamp")
    a.op("LDA", "imm", 0x1B)
    a.op("STA", "abs", 0xFF06)
    a.op("LDA", "zp", 0xF2)
    a.op("CLC")
    a.op("ADC", "imm", YSC_SAMPLES)
    a.op("STA", "zp", 0xF2)
    a.op("LDA", "zp", 0xF3)
    a.op("ADC", "imm", 0)
    a.op("STA", "zp", 0xF3)
    a.op("INC", "zp", 0xF0)
    a.op("LDA", "zp", 0xF0)
    a.op("CMP", "imm", 8)
    a.op("BEQ", "rel", "vshow")
    a.op("JMP", "abs", "vframe")
    a.label("vshow")
    show_results(a)
    return a.assemble(), 8 * YSC_SAMPLES


START_PROBES = [
    ("NOP", "imp", None),             # reads only
    ("STA", "abs", 0x00F8),           # write in the last cycle
    ("STA", "abx", 0x00F8),           # dummy read, then the write
    ("INC", "abs", 0x00F8),           # read, write, write
    ("PHA", "imp", None),             # dummy read, push
    ("LDA", "abs", 0x00F8),           # read in the last cycle
    ("STA", "zp", 0xF8),              # 3 cycles, write last
    ("STA", "izy", 0xFA),             # 6 cycles: dummy read, write
    ("INC", "zp", 0xF8),              # 5 cycles: read, write, write
    ("INC", "abx", 0x00F8),           # 7 cycles: dummy read, read, 2 writes
    ("STA", "aby", 0x00F8),           # 5 cycles like STA abs,X
]
START_OFFSETS = 20
START_BASE = int(os.environ.get("TED_START_BASE", "950"))


def build_start_dma():
    """`start` with YSCROLL = 5: line 101 is an attribute DMA line."""
    return build_start(0x1D, START_BASE + 11)


def build_start(yscroll=None, base=None):
    """The CPU at the start of a fetch line's single clock (tx 3): for
    every probe instruction of START_PROBES and every offset 0–23 cycles,
    one frame: SYNC, a fixed delay to the end of a display line, the
    offset, the probe, then LDA $FF1E and LDA $FF1D stored. Results: per
    probe and offset 2 bytes ($FF1E, $FF1D)."""
    a = Asm(0x1001)
    basic_stub(a)
    a.label("main")
    a.op("SEI")
    clear_results(a)
    a.op("LDX", "imm", 0)              # X = 0: STA abs,X writes $F8
    a.op("STX", "zp", 0xF0)            # offset
    a.op("STX", "zp", 0xF1)            # frame
    a.op("STX", "zp", 0xF6)            # probe
    a.op("LDA", "imm", 0xF8)           # ($FA) -> $00F8
    a.op("STA", "zp", 0xFA)
    a.op("STX", "zp", 0xFB)
    a.op("LDA", "imm", RESULTS & 0xFF)
    a.op("STA", "zp", 0xF2)
    a.op("LDA", "imm", RESULTS >> 8)
    a.op("STA", "zp", 0xF3)
    a.op("JMP", "abs", "snext")
    a.label("sframe")
    sync(a, "ss")
    if yscroll is not None:
        a.op("LDA", "imm", yscroll)
        a.op("STA", "abs", 0xFF06)
    a.op("LDA", "zp", 0xF0)
    a.op("CLC")
    a.op("ADC", "imm", "<schain")
    a.op("STA", "abs", "sjmp+1")
    a.op("LDY", "imm", 0)
    base = START_BASE if base is None else base
    for i in range(base // 256):
        a.op("LDA", "imm", 0)
        a.label(f"sd{i}")
        a.op("SEC")
        a.op("SBC", "imm", 1)
        a.op("BNE", "rel", f"sd{i}")
    if base % 256:
        a.op("LDA", "imm", base % 256)
        a.label("sdr")
        a.op("SEC")
        a.op("SBC", "imm", 1)
        a.op("BNE", "rel", "sdr")
    a.label("sjmp")
    a.op("JMP", "abs", "schain")
    if (a.pc & 0xFF) + START_OFFSETS + 2 > 0xFF:
        a.code += bytes(0x100 - (a.pc & 0xFF))
    a.label("schain")
    a.byte(*([0xC9] * START_OFFSETS), 0x24, 0xEA)
    # the probe (self-modified: three bytes)
    a.label("sprobe")
    a.byte(0xEA, 0xEA, 0xEA)
    a.op("LDA", "abs", 0xFF1E)
    a.op("STA", "zp", 0xF4)
    a.op("LDA", "abs", 0xFF1D)
    a.op("STA", "zp", 0xF5)
    if yscroll is not None:
        a.op("LDA", "imm", 0x1B)
        a.op("STA", "abs", 0xFF06)
    # results: two bytes a frame
    a.op("LDY", "imm", 0)
    a.op("LDA", "zp", 0xF4)
    a.op("STA", "izy", 0xF2)
    a.op("INY")
    a.op("LDA", "zp", 0xF5)
    a.op("STA", "izy", 0xF2)
    a.op("LDA", "zp", 0xF2)
    a.op("CLC")
    a.op("ADC", "imm", 2)
    a.op("STA", "zp", 0xF2)
    a.op("LDA", "zp", 0xF3)
    a.op("ADC", "imm", 0)
    a.op("STA", "zp", 0xF3)
    a.op("LDX", "imm", 0)
    a.op("INC", "zp", 0xF0)
    a.op("LDA", "zp", 0xF0)
    a.op("CMP", "imm", START_OFFSETS)
    a.op("BNE", "rel", "snext")
    a.op("LDA", "imm", 0)
    a.op("STA", "zp", 0xF0)
    a.op("INC", "zp", 0xF6)            # probe number
    a.op("LDA", "zp", 0xF6)
    a.op("CMP", "imm", len(START_PROBES))
    a.op("BEQ", "rel", "sshow")
    a.label("snext")
    # copy the probe's three bytes into place
    a.op("LDA", "zp", 0xF6)
    a.op("ASL", "acc")
    a.op("CLC")
    a.op("ADC", "zp", 0xF6)
    a.op("TAY")
    a.op("LDA", "aby", "sbytes")
    a.op("STA", "abs", "sprobe")
    a.op("LDA", "aby", "sbytes+1")
    a.op("STA", "abs", "sprobe+1")
    a.op("LDA", "aby", "sbytes+2")
    a.op("STA", "abs", "sprobe+2")
    a.op("JMP", "abs", "sframe")
    a.label("sshow")
    show_results(a)
    a.label("sbytes")
    for mnem, mode, arg in START_PROBES:
        b = [OPS[mnem][mode]]
        if SIZE[mode] == 3:
            b += [arg & 0xFF, arg >> 8]
        elif SIZE[mode] == 2:
            b += [arg & 0xFF]
        while len(b) < 3:
            b.append(0xEA)             # padding NOPs: the same for every offset
        a.byte(*b)
    return a.assemble(), 2 * START_OFFSETS * len(START_PROBES)


# Tests whose result is xplus4's picture (tests/data/ted-<name>.png).
PICTURES = {"split", "rows", "fli"}

TESTS = {
    "split": build_split,
    "rows": build_rows,
    "fli": build_fli,
    "dma": build_dma,
    "yscroll": build_yscroll,
    "start": build_start,
    "startdma": build_start_dma,
    "counters": build_counters,
    "trace": build_trace,
    "fine": build_fine,
    "irq": build_irq,
    "irqall": build_irqall,
}

# Tests that run longer than LIMIT_CYCLES.
LIMITS = {"irqall": 40_000_000, "split": 40_000_000, "rows": 30_000_000, "fli": 80_000_000, "dma": 40_000_000, "start": 40_000_000, "startdma": 40_000_000}

# xplus4 cycles until the screenshot (PAL, ~1.77 M per second: autostart
# takes about 3 s, the tests well under one).
LIMIT_CYCLES = 12_000_000


def prg(name):
    code, _ = TESTS[name]()
    return struct.pack("<H", 0x1001) + code


# --- reading a PNG (stdlib) --------------------------------------------------

def read_png(path):
    data = open(path, "rb").read()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", path
    i, idat, pal = 8, b"", None
    while i < len(data):
        n, kind = struct.unpack(">I4s", data[i:i + 8])
        chunk = data[i + 8:i + 8 + n]
        if kind == b"IHDR":
            w, h, depth, ctype = struct.unpack(">IIBB", chunk[:10])
        elif kind == b"PLTE":
            pal = [tuple(chunk[j:j + 3]) for j in range(0, n, 3)]
        elif kind == b"IDAT":
            idat += chunk
        i += 12 + n
    if depth != 8:
        raise ValueError(f"{path}: bit depth {depth}")
    bpp = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[ctype]
    raw = zlib.decompress(idat)
    stride = w * bpp
    rows, prev, pos = [], bytearray(stride), 0
    for _ in range(h):
        f = raw[pos]
        line = bytearray(raw[pos + 1:pos + 1 + stride])
        pos += 1 + stride
        for x in range(stride):
            a = line[x - bpp] if x >= bpp else 0
            b = prev[x]
            c = prev[x - bpp] if x >= bpp else 0
            if f == 1:
                line[x] = (line[x] + a) & 255
            elif f == 2:
                line[x] = (line[x] + b) & 255
            elif f == 3:
                line[x] = (line[x] + (a + b) // 2) & 255
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        if ctype == 3:
            rows.append([pal[v] for v in line])
        elif ctype == 0:
            rows.append([(v, v, v) for v in line])
        else:
            rows.append([tuple(line[x * bpp:x * bpp + 3]) for x in range(w)])
        prev = line
    return w, h, rows


# xplus4's PAL screenshot: the window's top left pixel.
WINDOW = (32, 40)


def bitmap_bytes(png, count):
    _, _, rows = read_png(png)
    x0, y0 = WINDOW
    out = bytearray()
    for i in range(count):
        row, col, line = i // 320, (i % 320) // 8, i % 8
        y = y0 + row * 8 + line
        v = 0
        for bit in range(8):
            r, g, b = rows[y][x0 + col * 8 + bit]
            v = v << 1 | (1 if r + g + b > 384 else 0)
        out.append(v)
    return bytes(out)


def show(name, data):
    """Print a reference: trace as line/hpos/step, fine as hpos/step rows."""
    def steps(seq):
        return [""] + [str((b - a) % 228 if b >= a else b - a + 228) for a, b in zip(seq, seq[1:])]
    if name == "trace":
        n = TRACE_PAGES * 0x100
        h, l = data[:n], data[n:2 * n]
        st = steps(h)
        for i in range(n):
            print(f"{i:4} line {l[i]:3} hpos ${h[i]:02X} {st[i]}")
    elif name == "fine":
        for run in range(FINE_RUNS):
            h = data[run * 0x100:(run + 1) * 0x100]
            print(f"run {run}: " + " ".join(f"{v:02X}" for v in h[:64]))
    else:
        print(data.hex())


def main(argv):
    if len(argv) >= 3 and argv[1] == "build":
        os.makedirs(argv[2], exist_ok=True)
        for name in TESTS:
            path = os.path.join(argv[2], f"ted-{name}.prg")
            open(path, "wb").write(prg(name))
            print(path)
    elif len(argv) >= 4 and argv[1] == "vice":
        exe, out = argv[2], argv[3]
        os.makedirs(out, exist_ok=True)
        for name in argv[4:] or list(TESTS):
            path = os.path.abspath(os.path.join(out, f"ted-{name}.prg"))
            open(path, "wb").write(prg(name))
            shot = os.path.abspath(os.path.join(out, f"ted-{name}.png"))
            if os.path.exists(shot):
                os.remove(shot)
            _, count = TESTS[name]()
            data_dir = os.path.join(os.path.dirname(__file__), "..", "tests", "data")
            if name in PICTURES:
                for attempt in range(5):
                    subprocess.run([exe, "-default", "+sound", "-limitcycles",
                                    str(LIMITS.get(name, LIMIT_CYCLES)),
                                    "-exitscreenshot", shot, path],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=600)
                    _, _, rows = read_png(shot)
                    # Stripes in the left border (many colour changes down
                    # column 5), not the BASIC screen of a failed autostart.
                    def changes(x):
                        col = [rows[y][x] for y in range(len(rows))]
                        return sum(1 for p, q in zip(col, col[1:]) if p != q)
                    if max(changes(5), changes(200)) >= 8:
                        break
                else:
                    raise SystemExit(f"{name}: no stripes in xplus4's picture")
                import shutil
                shutil.copy(shot, os.path.join(data_dir, f"ted-{name}.png"))
                open(os.path.join(data_dir, f"ted-{name}.prg"), "wb").write(prg(name))
                print(f"{name}: picture -> tests/data/ted-{name}.png")
                continue
            # xplus4's autostart now and then reports FILE NOT FOUND: retry
            # until the program's done marker is in the picture.
            for attempt in range(5):
                limit = LIMITS.get(name, LIMIT_CYCLES)
                subprocess.run([exe, "-default", "+sound", "-limitcycles", str(limit),
                                "-exitscreenshot", shot, path],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=600)
                every = bitmap_bytes(shot, BITMAP_BYTES)
                if every[-2:] == bytes([0xA5, 0x5A]):
                    break
            else:
                raise SystemExit(f"{name}: no done marker in xplus4's picture")
            data = every[:count]
            data_dir = os.path.join(os.path.dirname(__file__), "..", "tests", "data")
            ref = os.path.join(data_dir, f"ted-{name}.ref")
            open(ref, "wb").write(data)
            # The program mimic's test runs (tests/tests/ted_timing.rs).
            open(os.path.join(data_dir, f"ted-{name}.prg"), "wb").write(prg(name))
            print(f"{name}: {len(data)} bytes -> {os.path.normpath(ref)}")
    elif len(argv) >= 3 and argv[1] == "show":
        name = os.path.basename(argv[2]).removeprefix("ted-").split(".")[0]
        show(name, open(argv[2], "rb").read())
    else:
        print(__doc__)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
