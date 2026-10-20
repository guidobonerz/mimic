"""Unit tests for polydbg (run: python -m unittest discover -s tools).

The end-to-end test against the real debug server is the Rust test
tests/tests/python_client.rs.
"""

import json
import socket
import threading
import unittest

import polydbg


class Parsing(unittest.TestCase):
    def test_numbers(self):
        self.assertEqual(polydbg.parse_number("1234"), 1234)
        self.assertEqual(polydbg.parse_number("$04d2"), 1234)
        self.assertEqual(polydbg.parse_number("0x4D2"), 1234)
        self.assertEqual(polydbg.parse_number("%101"), 5)
        for bad in ["", "$", "0xZZ", "%2", "start"]:
            with self.assertRaises(ValueError):
                polydbg.parse_number(bad)

    def test_addresses_or_symbols(self):
        self.assertEqual(polydbg.address_args("$0810"), {"addr": 0x0810})
        self.assertEqual(polydbg.address_args("sprites.init"), {"symbol": "sprites.init"})

    def test_hexdump_marks_frozen_bytes(self):
        lines = polydbg.hexdump(0x0400, bytes([0x41, 0x00, 0xFF]), frozen=[1])
        self.assertEqual(len(lines), 1)
        self.assertTrue(lines[0].startswith("0400: 41 00*ff "), lines[0])
        self.assertTrue(lines[0].endswith("A.."), lines[0])

    def test_json_flag_before_or_after_the_command(self):
        parser = polydbg.build_parser()
        self.assertTrue(parser.parse_args(["--json", "regs"]).json)
        self.assertTrue(parser.parse_args(["regs", "--json"]).json)
        self.assertFalse(parser.parse_args(["regs"]).json)

    def test_stop_lines(self):
        line = polydbg.format_stop({
            "reason": {"kind": "breakpoint", "bp": 3},
            "pc": 0x0810,
            "beam": {"frame": 1, "line": 50, "cycle": 7},
        })
        self.assertEqual(line, "breakpoint 3: pc=$0810 frame 1 line 50 cycle 7")
        line = polydbg.format_stop({
            "reason": {"kind": "watchpoint", "bp": 1, "write": True, "addr": 0xD020},
            "pc": None,
        })
        self.assertEqual(line, "watchpoint 1 (write $d020): pc=?")


class FakeServer:
    """Speaks just enough of the protocol: hello, then scripted replies."""

    def __init__(self, replies):
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(1)
        self.port = self.listener.getsockname()[1]
        self.requests = []
        self.replies = replies
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()

    def serve(self):
        conn, _ = self.listener.accept()
        self.listener.close()
        f = conn.makefile("rwb")
        f.write(b'{"event":"hello","protocol":"polytrace-debug","version":5,"machine":"fake"}\n')
        f.flush()
        for reply in self.replies:
            req = json.loads(f.readline())
            self.requests.append(req)
            for line in reply(req):
                f.write((json.dumps(line) + "\n").encode())
            f.flush()
        f.close()
        conn.close()


class ClientTests(unittest.TestCase):
    def test_calls_queue_events_and_raise_errors(self):
        server = FakeServer([
            lambda r: [{"event": "resumed", "pc": 1},
                       {"id": r["id"], "ok": True, "result": {"running": True}}],
            lambda r: [{"id": r["id"], "ok": False, "error": "no breakpoint 9"}],
            lambda r: [{"event": "stopped", "pc": 2},
                       {"id": r["id"], "ok": True, "result": {"regs": [
                           {"name": "PC", "value": 2, "bits": 16}]}}],
        ])
        with polydbg.Client(port=server.port) as c:
            self.assertEqual(c.hello["machine"], "fake")
            self.assertEqual(c.call("resume"), {"running": True})
            with self.assertRaises(polydbg.DebugError) as e:
                c.call("break_remove", bp=9)
            self.assertIn("no breakpoint", str(e.exception))
            self.assertEqual(c.regs(), {"PC": 2})
            self.assertEqual(c.wait_event("stopped")["pc"], 2)
            self.assertEqual(c.take_events(), [])
        self.assertEqual(server.requests[1], {"id": 2, "cmd": "break_remove", "bp": 9})
        self.assertNotIn("device", server.requests[2], "None arguments are left out")


if __name__ == "__main__":
    unittest.main()
