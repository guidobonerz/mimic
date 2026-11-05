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
    "CPX": {"imm": 0xE0}, "CPY": {"imm": 0xC0},
    "DEC": {"zp": 0xC6, "abs": 0xCE}, "INC": {"zp": 0xE6, "abs": 0xEE},
    "DEX": {"imp": 0xCA}, "DEY": {"imp": 0x88},
    "INX": {"imp": 0xE8}, "INY": {"imp": 0xC8},
    "EOR": {"imm": 0x49},
    "JMP": {"abs": 0x4C}, "JSR": {"abs": 0x20}, "RTS": {"imp": 0x60},
    "LDA": {"imm": 0xA9, "zp": 0xA5, "abs": 0xAD, "abx": 0xBD, "aby": 0xB9},
    "LDX": {"imm": 0xA2, "zp": 0xA6, "abs": 0xAE},
    "LDY": {"imm": 0xA0, "zp": 0xA4, "abs": 0xAC},
    "LSR": {"acc": 0x4A},
    "NOP": {"imp": 0xEA},
    "ORA": {"imm": 0x09},
    "PHA": {"imp": 0x48}, "PLA": {"imp": 0x68},
    "STA": {"zp": 0x85, "abs": 0x8D, "abx": 0x9D, "aby": 0x99},
    "STX": {"zp": 0x86, "abs": 0x8E}, "STY": {"zp": 0x84, "abs": 0x8C},
    "TAX": {"imp": 0xAA}, "TAY": {"imp": 0xA8}, "TXA": {"imp": 0x8A},
    "TYA": {"imp": 0x98},
}
SIZE = {"imp": 1, "acc": 1, "imm": 2, "zp": 2, "rel": 2, "abs": 3, "abx": 3, "aby": 3}


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
        """A label, `label+n` or `<label` (low byte)."""
        if name.startswith("<"):
            return self.value(name[1:]) & 0xFF
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


TESTS = {
    "trace": build_trace,
    "fine": build_fine,
}

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
            # xplus4's autostart now and then reports FILE NOT FOUND: retry
            # until the program's done marker is in the picture.
            for attempt in range(5):
                subprocess.run([exe, "-default", "+sound", "-limitcycles", str(LIMIT_CYCLES),
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
