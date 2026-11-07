"""Build the ninth bus test: bus test 8's rows measured without the start
phase. Bus tests 6-8 time 600 iterations of the CPU loop; where the CPU's
share differs a lot between a line's regions (a Copper chain inside the
bitplane fetch window), where those iterations end makes the result swing
by up to 6 % from run to run, on the owner's A500 as in mimic (bus test
7's row 11: BC7C and B139 on the same machine). This program instead
counts the loop's iterations (16x MOVE.W 0(A1,D1.W),4(A0), plus one read
of VHPOSR) in exactly four fields, from line 0 to the fourth return
from the field's upper half into its lower: higher is faster, the rows are
those of bus test 8.
Rows (lowres, DDF $30-$D8, sub-lists of kind A unless said):
  0-6: 0, 1, 2, 3, 4, 5, 6 planes, sub 20 from $A0
  7: 5 planes, sub 10 from $A0
  8: 5 planes, sub 30 from $A0
  9: 5 planes, sub 20 from $60
  10: 5 planes, sub 20 from $C0
  11: 5 planes, direct 20 from $A0

usage: python tools/amiga_bustest9.py [OUT.adf [OUT.bin]]   (used by tools/amiga_hwdisk.py;
       OUT.adf boots the program alone, OUT.bin = tests/data/amiga_bustest9.bin for
       the hardware tests in tests/tests/amiga_boot.rs)
"""
import importlib.util
import os
import sys

_spec = importlib.util.spec_from_file_location(
    "amiga_bustest7", os.path.join(os.path.dirname(os.path.abspath(__file__)), "amiga_bustest7.py"))
bt7 = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(bt7)

bt7.FRAMES = 4
bt7.SUB_COUNTS = [10, 20, 30]
bt7.LIST_SPECS = [
    (0x32000, "A", 20, 0xA0),
    (0x34000, "A", 10, 0xA0),
    (0x36000, "A", 30, 0xA0),
    (0x38000, "A", 20, 0x60),
    (0x3A000, "A", 20, 0xC0),
    (0x3C000, "B", 20, 0xA0),   # 92 bytes a block
]
for (base, kind, n, _), nxt in zip(bt7.LIST_SPECS, bt7.LIST_SPECS[1:] + [(bt7.SUBS, "", 0, 0)]):
    assert base + 2 * bt7.bt.HEAD_WORDS + 4 + len(bt7.LINES) * bt7.block_len(kind, n) + 12 <= nxt[0], hex(base)
con0 = bt7.con0
bt7.TESTS = [(con0(p), 0x8380 if p else 0x8280, 0) for p in range(7)] + [
    (con0(5), 0x8380, 1),
    (con0(5), 0x8380, 2),
    (con0(5), 0x8380, 3),
    (con0(5), 0x8380, 4),
    (con0(5), 0x8380, 5),
]
bt7.ROWS = len(bt7.TESTS)
ROWS = bt7.ROWS


def build_main(harness=False):
    return bt7.build_main(harness)


def main():
    sys.argv = sys.argv[:]
    bt7.main()


if __name__ == "__main__":
    main()
