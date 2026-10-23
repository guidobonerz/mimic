"""Build sprtest.adf: when does Denise show sprites without plane data?
A red sprite bar (lines 40-199, HSTART $90) next to one lowres plane (a
white box, plane rows 60-199 = lines 104-243, left half, in front of the
sprite). The Copper switches: line 100 BPLCON0 BPU = 0; line 120 BPU = 1;
line 140 DMACON BPLEN off; line 160 BPLEN on; line 180 BPU = 0 with another
COLOR00. WinUAE (OCS A500, 2026-10-23, `docs/sources.md`), and mimic: the
bar shows on lines 44-99 and 120-123 (the plane pointers did not advance
while BPU = 0, so the box starts at line 124), nothing on 100-119, 140-159
and 180-199, and the box hides it elsewhere. Boot it in mimic:
  cargo run -p emu-headless -- --machine amiga --rom-dir roms --disk sprtest.adf --frames 300 --snapshot spr.png

usage: python tools/amiga_sprtest.py OUT.adf"""
import importlib.util, os, struct, sys
spec = importlib.util.spec_from_file_location(
    "bt", os.path.join(os.path.dirname(os.path.abspath(__file__)), "amiga_bustest.py"))
bt = importlib.util.module_from_spec(spec); spec.loader.exec_module(bt)
Asm, D, A, IND, POSTINC, D16, ABSL, IMM = bt.Asm, bt.D, bt.A, bt.IND, bt.POSTINC, bt.D16, bt.ABSL, bt.IMM
CODE, CUSTOM = bt.CODE, bt.CUSTOM
PLANE = 0x60000; SPR = 0x50000; COP = 0x40000
a = Asm(CODE)
a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x9A))
a.move("w", IMM(0x7FFF, "w"), ABSL(CUSTOM + 0x96))
# plane: 40 bytes/line, 256 lines; lines 60-199 left 20 bytes $FF
a.lea(ABSL(PLANE), 0)
a.move("w", IMM(256 * 40 // 4 - 1, "w"), D(0))
a.label("clr"); a.move("l", IMM(0, "l"), POSTINC(0)); a.dbra(0, "clr")
a.lea(ABSL(PLANE + 60 * 40), 0)
a.move("w", IMM(139, "w"), D(1))
a.label("ln"); a.moveq(4, 0)
a.label("w"); a.move("l", IMM(0xFFFFFFFF, "l"), POSTINC(0)); a.dbra(0, "w")
a.lea(D16(20, 0), 0); a.dbra(1, "ln")
# sprite: POS $2848 (VSTART 40, HSTART $90), CTL $C800 (VSTOP 200), data $FFFF/$0000 x 160, end
a.lea(ABSL(SPR), 0)
a.move("l", IMM(0x2848C800, "l"), POSTINC(0))
a.move("w", IMM(159, "w"), D(0))
a.label("sd"); a.move("l", IMM(0xFFFF0000, "l"), POSTINC(0)); a.dbra(0, "sd")
a.move("l", IMM(0, "l"), IND(0))
# copper list
a.lea(ABSL(COP), 0)
a.lea(ABSL(CODE + 0x300), 1)
a.moveq(bt.HEAD_WORDS - 1, 0)  # not used; we copy a fixed count below
words = [0x8E, 0x2C81, 0x90, 0x2CC1, 0x92, 0x38, 0x94, 0xD0, 0x100, 0x1200, 0x102, 0, 0x104, 0,
         0x108, 0, 0x10A, 0, 0xE0, PLANE >> 16, 0xE2, PLANE & 0xFFFF, 0x120, SPR >> 16, 0x122, SPR & 0xFFFF,
         0x180, 0x0333, 0x182, 0x0FFF, 0x1A2, 0x0F00, 0x1A4, 0x0F00, 0x1A6, 0x0F00,
         0x6401, 0xFFFE, 0x100, 0x0200,
         0x7801, 0xFFFE, 0x100, 0x1200,
         0x8C01, 0xFFFE, 0x96, 0x0100,
         0xA001, 0xFFFE, 0x96, 0x8100,
         0xB401, 0xFFFE, 0x100, 0x0200, 0x180, 0x0666,
         0xC801, 0xFFFE, 0x100, 0x1200, 0x180, 0x0333,
         0xFFFF, 0xFFFE]
a.move("w", IMM(len(words) - 1, "w"), D(0))
a.label("cp"); a.move("w", POSTINC(1), POSTINC(0)); a.dbra(0, "cp")
a.move("l", IMM(COP, "l"), ABSL(CUSTOM + 0x80))
a.move("w", IMM(0, "w"), ABSL(CUSTOM + 0x88))
a.move("w", IMM(0x83A0, "w"), ABSL(CUSTOM + 0x96))  # DMAEN BPLEN COPEN SPREN
a.label("halt"); a.bra("halt")
code = bytearray(a.finish())
assert len(code) <= 0x300, hex(len(code))
code += bytes(0x300 - len(code))
code += b"".join(struct.pack(">H", w) for w in words)
code += bytes(-len(code) % 4)
boot = bt.build_boot(code)
adf = bytearray(901120); adf[0:4] = b"DOS\0"; adf[8:12] = struct.pack(">I", 880)
adf[12:12 + len(boot)] = boot; off = 12 + len(boot); assert off + len(code) <= 1024
adf[off:off + len(code)] = code
s = 0
for i in range(0, 1024, 4):
    s += struct.unpack(">I", adf[i:i + 4])[0]; s = (s & 0xFFFFFFFF) + (s >> 32)
adf[4:8] = struct.pack(">I", (~s) & 0xFFFFFFFF)
open(sys.argv[1], "wb").write(adf); print("ok", len(code))
