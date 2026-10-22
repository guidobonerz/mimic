"""Unit tests for tools/gdb/mimic.py (runs without GDB)."""

import importlib.util
import os
import unittest

_spec = importlib.util.spec_from_file_location(
    "mimic_gdb", os.path.join(os.path.dirname(__file__), "gdb", "mimic.py")
)
mimic = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(mimic)


class MimicGdbTest(unittest.TestCase):
    def test_unescape(self):
        self.assertEqual(mimic.unescape(b"a}\x03}\x5db"), b"a#}b")
        self.assertEqual(mimic.unescape(b"\x89PNG"), b"\x89PNG")

    def test_frame_packet(self):
        self.assertEqual(mimic.frame_packet(), "qMimic.frame")
        self.assertEqual(
            mimic.frame_packet(crt=True, screen="vdc"),
            "qMimic.frame:screen=vdc;filter=crt",
        )

    def test_big_endian_only_for_a_little_endian_68000(self):
        little = "The target endianness is set automatically (currently little endian)."
        self.assertTrue(mimic.needs_big_endian("m68k", little))
        self.assertFalse(mimic.needs_big_endian("m68k", little.replace("little", "big")))
        self.assertFalse(mimic.needs_big_endian("z80", little))

    def test_viewer_reloads_the_png(self):
        html = mimic.viewer_html("/tmp/x/mimic-frame.png")
        self.assertIn('src="mimic-frame.png"', html)
        self.assertIn("setInterval", html)


if __name__ == "__main__":
    unittest.main()
