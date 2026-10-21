#!/usr/bin/env python3
"""polydbg: client for the mimic native debug protocol.

The protocol is JSON lines over TCP (see crates/emu-debug/src/protocol.rs).
Start a machine with a debug server, e.g.

    cargo run -p emu-headless -- --machine c64 --debug --run
    cargo run -p emu-desktop -- --machine c64 --debug

then use polydbg from the command line,

    python tools/polydbg.py regs
    python tools/polydbg.py break '$0810'
    python tools/polydbg.py resume --wait
    python tools/polydbg.py peek '$0400' 64
    python tools/polydbg.py shell

or as a library:

    from polydbg import Client
    with Client() as c:
        c.break_add(0x0810)
        c.resume()
        print(c.wait_event("stopped")["pc"])

Numbers may be written as 1234, $04d2, 0x4d2 or %10011010010. Where an
address is expected, a loaded symbol name works too.

Standard library only; Python 3.8 or newer.
"""

import argparse
import collections
import json
import shlex
import socket
import sys

DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 6510


class DebugError(Exception):
    """The server answered a request with an error."""


def parse_number(text):
    """Parse 1234, $04d2, 0x4d2 or %10011010010."""
    t = str(text).strip()
    try:
        if t.startswith("$"):
            return int(t[1:], 16)
        if t.lower().startswith("0x"):
            return int(t[2:], 16)
        if t.startswith("%"):
            return int(t[1:], 2)
        return int(t, 10)
    except ValueError:
        raise ValueError("not a number: %r" % text) from None


def address_args(text):
    """Arguments naming an address: a number, or a symbol."""
    try:
        return {"addr": parse_number(text)}
    except ValueError:
        return {"symbol": text}


# Scan lines per mode line of ANTIC modes 2-F (Atari 8-bit).
ANTIC_LINES = {2: 8, 3: 10, 4: 8, 5: 16, 6: 8, 7: 16, 8: 8, 9: 4, 10: 4,
               11: 2, 12: 1, 13: 2, 14: 1, 15: 1}


def decode_display_list(read, start, max_lines=240):
    """An ANTIC display list from `start` as (addr, bytes, text) entries.

    `read(addr)` returns one byte. Runs of the same instruction without LMS
    are merged ("MODE 2 x23"). Stops after a JVB, a JMP back into the
    list, or `max_lines` scan lines.
    """
    entries = []
    addr = start
    lines = 0
    seen = set()
    while lines < max_lines and addr not in seen:
        seen.add(addr)
        ir = read(addr)
        mode = ir & 0x0F
        dli = " DLI" if ir & 0x80 else ""
        if mode == 0:
            n = ((ir >> 4) & 7) + 1
            entries.append((addr, [ir], "BLANK %d%s" % (n, dli)))
            lines += n
            addr = (addr & 0xFC00) | ((addr + 1) & 0x03FF)
        elif mode == 1:
            target = read(addr + 1) | (read(addr + 2) << 8)
            jvb = ir & 0x40
            entries.append((addr, [ir, read(addr + 1), read(addr + 2)],
                            "%s $%04x%s" % ("JVB" if jvb else "JMP", target, dli)))
            if jvb or target in seen:
                break
            addr = target
            lines += 1
        else:
            flags = dli
            if ir & 0x20:
                flags += " VSCROL"
            if ir & 0x10:
                flags += " HSCROL"
            if ir & 0x40:
                lms = read(addr + 1) | (read(addr + 2) << 8)
                data = [ir, read(addr + 1), read(addr + 2)]
                text = "MODE %X LMS $%04x%s" % (mode, lms, flags)
                nxt = addr + 3
            else:
                data = [ir]
                text = "MODE %X%s" % (mode, flags)
                nxt = addr + 1
            prev = entries[-1] if entries else None
            if not (ir & 0x40) and prev and prev[1][0] == ir and len(prev[1]) == 1:
                count = int(prev[2].split(" x")[1]) + 1 if " x" in prev[2] else 2
                entries[-1] = (prev[0], prev[1], "MODE %X%s x%d" % (mode, flags, count))
            else:
                entries.append((addr, data, text))
            lines += ANTIC_LINES[mode]
            addr = (addr & 0xFC00) | (nxt & 0x03FF)
    return entries


def hexdump(addr, data, frozen=(), width=16):
    """Lines of `addr: hex bytes  ascii`; frozen offsets are marked with *."""
    frozen = set(frozen)
    lines = []
    for off in range(0, len(data), width):
        chunk = data[off:off + width]
        cells = []
        for i, b in enumerate(chunk):
            mark = "*" if off + i in frozen else " "
            cells.append("%02x%s" % (b, mark))
        text = "".join(chr(b) if 32 <= b < 127 else "." for b in chunk)
        lines.append("%04x: %-*s %s" % (addr + off, width * 3, "".join(cells), text))
    return lines


def format_stop(state):
    """One line for a stop state (response of a step, or a stopped event)."""
    reason = state.get("reason") or {}
    kind = reason.get("kind", "?")
    if kind == "breakpoint":
        kind = "breakpoint %s" % reason.get("bp")
    elif kind == "watchpoint":
        kind = "watchpoint %s (%s $%04x)" % (
            reason.get("bp"), "write" if reason.get("write") else "read", reason.get("addr", 0))
    pc = state.get("pc")
    beam = state.get("beam") or {}
    where = "pc=$%04x" % pc if pc is not None else "pc=?"
    if beam:
        where += " frame %s line %s cycle %s" % (beam.get("frame"), beam.get("line"), beam.get("cycle"))
    return "%s: %s" % (kind, where)


class Client:
    """A connection to a mimic debug server.

    `call` sends one command and returns its `result`; events pushed in the
    meantime are queued and read with `wait_event` or `take_events`.
    """

    def __init__(self, host=DEFAULT_HOST, port=DEFAULT_PORT, timeout=10.0):
        self.sock = socket.create_connection((host, port), timeout=timeout)
        self.file = self.sock.makefile("rwb")
        self.next_id = 1
        self.pending = collections.deque()
        self.hello = self._read()
        if self.hello.get("event") != "hello":
            raise DebugError("unexpected greeting: %r" % self.hello)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def close(self):
        try:
            self.file.close()
        finally:
            self.sock.close()

    def _read(self):
        line = self.file.readline()
        if not line:
            raise ConnectionError("server closed the connection")
        return json.loads(line)

    def call(self, cmd, **args):
        """Send `cmd` with `args`; return the result or raise DebugError."""
        req = {"id": self.next_id, "cmd": cmd}
        self.next_id += 1
        req.update({k: v for k, v in args.items() if v is not None})
        self.file.write((json.dumps(req) + "\n").encode())
        self.file.flush()
        while True:
            msg = self._read()
            if "event" in msg:
                self.pending.append(msg)
                continue
            if msg.get("id") != req["id"]:
                raise DebugError("response out of order: %r" % msg)
            if not msg.get("ok"):
                raise DebugError(msg.get("error", "unknown error"))
            return msg.get("result")

    def take_events(self):
        """Events received so far, oldest first."""
        out = list(self.pending)
        self.pending.clear()
        return out

    def wait_event(self, name=None, timeout=None):
        """The next event (named `name`, if given); earlier others are dropped."""
        if timeout is not None:
            self.sock.settimeout(timeout)
        try:
            while True:
                ev = self.pending.popleft() if self.pending else self._read()
                if name is None or ev.get("event") == name:
                    return ev
        finally:
            if timeout is not None:
                self.sock.settimeout(10.0)

    # ---- convenience -----------------------------------------------------

    def info(self):
        return self.call("info")

    def regs(self, device=None):
        """Registers as an ordered dict name -> value."""
        r = self.call("regs", device=device)
        return collections.OrderedDict((x["name"], x["value"]) for x in r["regs"])

    def set_reg(self, name, value, device=None):
        self.call("set_reg", name=name, value=value, device=device)

    def peek(self, addr, length, space=None):
        return bytes(self.call("peek", addr=addr, len=length, space=space)["bytes"])

    def poke(self, addr, data, space=None):
        self.call("poke", addr=addr, bytes=list(data), space=space)

    def disasm(self, addr=None, count=10, device=None):
        return self.call("disasm", addr=addr, count=count, device=device)["lines"]

    def break_add(self, addr=None, symbol=None, **kw):
        """A breakpoint; `kind`, `len`, `condition`, `ignore` … as keywords."""
        return self.call("break_add", addr=addr, symbol=symbol, **kw)["bp"]

    def resume(self):
        self.call("resume")

    def pause(self):
        return self.call("pause")

    def step(self, count=1):
        return self.call("step_instr", count=count)

    def _async(self, cmd, wait, **args):
        self.take_events()
        self.call(cmd, **args)
        return self.wait_event("stopped") if wait else None

    def step_over(self, wait=True):
        return self._async("step_over", wait)

    def step_out(self, wait=True):
        return self._async("step_out", wait)

    def run_to(self, addr=None, symbol=None, wait=True):
        return self._async("run_to", wait, addr=addr, symbol=symbol)

    def save_state(self):
        """Machine state and freezes, as stored by `load_state`."""
        return self.call("save_state")

    def load_state(self, state):
        return self.call("load_state", data=state["data"], freezes=state.get("freezes"))


# ---- command line -----------------------------------------------------------

def out(args, value, text_lines):
    """Print raw JSON with --json, else the human-readable lines."""
    if args.json:
        print(json.dumps(value, indent=2))
    else:
        for line in text_lines:
            print(line)


def cmd_info(c, args):
    i = c.info()
    spaces = ", ".join("%s=%s" % (s["id"], s["name"]) for s in i["memory_spaces"])
    out(args, i, [
        "machine   %s (protocol %s)" % (i["machine"], i["protocol_version"]),
        "devices   %s" % ", ".join(i["devices"]),
        "spaces    %s" % spaces,
        "state     %s" % ("running" if i["running"] else "paused"),
    ])


def cmd_regs(c, args):
    r = c.regs(args.device)
    out(args, r, ["  ".join("%s=$%x" % kv for kv in r.items())])


def cmd_set(c, args):
    c.set_reg(args.name, parse_number(args.value), args.device)


def cmd_peek(c, args):
    addr = parse_number(args.addr)
    r = c.call("peek", addr=addr, len=parse_number(args.len), space=args.space)
    out(args, r, hexdump(addr, bytes(r["bytes"]), r.get("frozen", ())))


def cmd_poke(c, args):
    c.poke(parse_number(args.addr), [parse_number(b) & 0xff for b in args.bytes], args.space)


def cmd_disasm(c, args):
    addr = parse_number(args.addr) if args.addr else None
    lines = c.disasm(addr, args.count, args.device)
    text = []
    for l in lines:
        label = (l["label"] + ":") if l.get("label") else ""
        target = ("  ; " + l["target_label"]) if l.get("target_label") else ""
        code = " ".join("%02x" % b for b in l["bytes"])
        text.append("%-12s %04x  %-9s %s%s" % (label, l["addr"], code, l["text"], target))
    out(args, lines, text)


def cmd_chips(c, args):
    chips = c.call("chips")
    if args.name:
        chips = [x for x in chips if x["name"] == args.name]
        if not chips:
            raise ValueError("no chip %r" % args.name)
    text = []
    for chip in chips:
        text.append("%s:" % chip["name"])
        for r in chip["regs"]:
            where = "$%04x" % r["addr"] if r.get("addr") is not None else "     "
            text.append("  %s %-8s $%0*x" % (where, r["name"], (r["bits"] + 3) // 4, r["value"]))
    out(args, chips, text)


def cmd_dlist(c, args):
    """Atari 8-bit: the ANTIC display list (from DLISTL/H unless given)."""
    if args.addr:
        start = parse_number(args.addr)
    else:
        antic = [x for x in c.call("chips") if x["name"] == "antic"]
        if not antic:
            raise ValueError("this machine has no ANTIC")
        regs = {r["name"]: r["value"] for r in antic[0]["regs"]}
        start = regs["DLISTL"] | (regs["DLISTH"] << 8)
    cache = {}

    def read(a):
        a &= 0xFFFF
        page = a & 0xFF00
        if page not in cache:
            cache[page] = c.peek(page, 256)
        return cache[page][a & 0xFF]

    entries = decode_display_list(read, start)
    out(args, [{"addr": a, "bytes": b, "text": t} for a, b, t in entries],
        ["%04x  %-9s %s" % (a, " ".join("%02x" % x for x in b), t) for a, b, t in entries])


def cmd_break(c, args):
    kw = {"kind": args.kind, "condition": args.condition, "ignore": args.ignore,
          "len": args.len, "space": args.space}
    if args.kind in ("raster",):
        kw["line"] = parse_number(args.where)
    elif args.kind not in ("irq", "nmi"):
        kw.update(address_args(args.where))
    bp = c.call("break_add", **kw)["bp"]
    out(args, {"bp": bp}, ["breakpoint %s" % bp])


def cmd_breaks(c, args):
    lst = c.call("break_list")
    text = []
    for b in lst:
        where = "$%04x" % b["addr"] if "addr" in b else "line %s" % b.get("line", "")
        cond = ("  if " + b["condition"]) if b.get("condition") else ""
        state = "" if b["enabled"] else "  (disabled)"
        text.append("%3d %-6s %s  hits %d%s%s" % (b["bp"], b["kind"], where, b["hits"], cond, state))
    out(args, lst, text or ["no breakpoints"])


def cmd_unbreak(c, args):
    c.call("break_remove", bp=parse_number(args.bp))


def cmd_pause(c, args):
    s = c.pause()
    out(args, s, [format_stop(s)])


def cmd_resume(c, args):
    c.take_events()
    c.resume()
    if args.wait:
        ev = c.wait_event("stopped", timeout=args.timeout)
        out(args, ev, [format_stop(ev)])


def cmd_step(c, args):
    s = c.step(args.count)
    out(args, s, [format_stop(s)])


def cmd_next(c, args):
    s = c.step_over()
    out(args, s, [format_stop(s)])


def cmd_finish(c, args):
    s = c.step_out()
    out(args, s, [format_stop(s)])


def cmd_until(c, args):
    s = c.run_to(**address_args(args.where))
    out(args, s, [format_stop(s)])


def cmd_frame(c, args):
    s = None
    for _ in range(args.count):
        s = c.call("step_frame")
    out(args, s, [format_stop(s)])


def cmd_reset(c, args):
    s = c.call("reset", kind="hard" if args.hard else "soft")
    out(args, s, [format_stop(s)])


def cmd_save(c, args):
    with open(args.file, "w") as f:
        json.dump(c.save_state(), f)


def cmd_load(c, args):
    with open(args.file) as f:
        s = c.load_state(json.load(f))
    out(args, s, [format_stop(s)])


def cmd_symbols(c, args):
    r = c.call("symbols_load", path=args.file, format=args.format)
    out(args, r, ["%s: %d symbols (%d skipped), %d in total" % (
        r["format"], r["loaded"], r["skipped"], r["total"])])


def cmd_record_start(c, args):
    c.call("record_start")


def cmd_record_stop(c, args):
    replay = c.call("record_stop")
    with open(args.file, "w") as f:
        json.dump(replay, f)
    print("%d events" % len(replay["events"]))


def cmd_replay(c, args):
    with open(args.file) as f:
        r = c.call("replay_input", replay=json.load(f))
    out(args, r, ["%d events queued, run until cycle %d" % (r["events"], r["end"])])


def cmd_search(c, args):
    if args.op == "start":
        r = c.call("search_start", size=args.size, space=args.space)
        out(args, r, ["%d candidates" % r["candidates"]])
    elif args.op == "results":
        r = c.call("search_results", max=args.max)
        text = ["$%04x = %s (was %s)" % (x["addr"], x["value"], x["last"]) for x in r["results"]]
        out(args, r, text + ["%d candidates" % r["total"]])
    else:
        value = parse_number(args.value) if args.value is not None else None
        r = c.call("search_next", op=args.op, value=value)
        out(args, r, ["%d candidates" % r["candidates"]])


def cmd_freeze(c, args):
    kw = address_args(args.where)
    r = c.call("freeze_add", value=parse_number(args.value) if args.value else None,
               size=args.size, mode=args.mode, label=args.label, space=args.space, **kw)
    out(args, r, ["freeze %s (value %s)" % (r["freeze"], r["value"])])


def cmd_freezes(c, args):
    lst = c.call("freeze_list")
    text = ["%3d $%04x = %s  %s%s%s" % (
        f["freeze"], f["addr"], f["value"], f["mode"],
        ("  " + f["label"]) if f.get("label") else "",
        "" if f["enabled"] else "  (off)") for f in lst]
    out(args, lst, text or ["no freezes"])


def cmd_unfreeze(c, args):
    c.call("freeze_remove", freeze=parse_number(args.freeze))


def cmd_cheats(c, args):
    if args.op == "save":
        r = c.call("cheats_save", path=args.file)
        print("%d freezes" % r["freezes"])
    else:
        r = c.call("cheats_load", path=args.file)
        print("%d freezes" % len(r["freezes"]))


def cmd_input(c, args):
    """input joystick PORT [up down left right fire] / input key N up|down"""
    if args.what == "key":
        event = {"type": "key", "key": parse_number(args.values[0]),
                 "down": args.values[1] == "down"}
        target = "keyboard"
    else:
        port = parse_number(args.values[0])
        dirs = [d for d in args.values[1:] if d in ("up", "down", "left", "right")]
        event = {"type": "joystick", "dirs": dirs, "buttons": 1 if "fire" in args.values else 0}
        target = {"port": port}
    c.call("input", events=[{"target": target, "event": event, "delay": args.delay}])


def cmd_events(c, args):
    if args.subscribe:
        c.call("subscribe", events=args.subscribe)
    try:
        while True:
            print(json.dumps(c.wait_event(timeout=3600)))
            sys.stdout.flush()
    except KeyboardInterrupt:
        pass


def cmd_raw(c, args):
    """raw CMD [JSON-ARGS]: any protocol command."""
    extra = json.loads(args.args) if args.args else {}
    print(json.dumps(c.call(args.cmd, **extra), indent=2))


def cmd_shell(c, args):
    parser = build_parser(shell=True)
    print("polydbg shell for %s; 'help' lists commands, 'quit' leaves." % c.hello.get("machine"))
    while True:
        try:
            line = input("(polydbg) ")
        except (EOFError, KeyboardInterrupt):
            print()
            return
        words = shlex.split(line)
        if not words:
            continue
        if words[0] in ("quit", "exit", "q"):
            return
        if words[0] == "help":
            parser.print_help()
            continue
        try:
            sub = parser.parse_args(words)
            sub.json = sub.json or args.json
            sub.func(c, sub)
        except SystemExit:
            pass
        except (DebugError, ValueError, OSError) as e:
            print("error: %s" % e)
        for ev in c.take_events():
            if ev.get("event") == "stopped":
                print("stopped " + format_stop(ev))


def build_parser(shell=False):
    p = argparse.ArgumentParser(
        prog="polydbg" if shell else None,
        description="Client for the mimic debug protocol.",
        add_help=not shell)
    if not shell:
        p.add_argument("--host", default=DEFAULT_HOST)
        p.add_argument("--port", type=int, default=DEFAULT_PORT)
    p.add_argument("--json", action="store_true", help="print raw JSON results")
    sub = p.add_subparsers(dest="command", metavar="COMMAND")
    sub.required = True

    def add(name, func, help_text, **kw):
        sp = sub.add_parser(name, help=help_text, **kw)
        sp.set_defaults(func=func)
        # Also accepted after the command; SUPPRESS keeps an earlier --json.
        sp.add_argument("--json", action="store_true", default=argparse.SUPPRESS,
                        help=argparse.SUPPRESS)
        return sp

    add("info", cmd_info, "machine, devices, memory spaces, run state")
    sp = add("regs", cmd_regs, "registers")
    sp.add_argument("--device")
    sp = add("set", cmd_set, "set a register")
    sp.add_argument("name")
    sp.add_argument("value")
    sp.add_argument("--device")
    sp = add("peek", cmd_peek, "hex dump of memory (* = frozen)")
    sp.add_argument("addr")
    sp.add_argument("len", nargs="?", default="64")
    sp.add_argument("--space", type=int)
    sp = add("poke", cmd_poke, "write bytes")
    sp.add_argument("addr")
    sp.add_argument("bytes", nargs="+")
    sp.add_argument("--space", type=int)
    sp = add("disasm", cmd_disasm, "disassemble (default at PC)")
    sp.add_argument("addr", nargs="?")
    sp.add_argument("-n", "--count", type=int, default=10)
    sp.add_argument("--device")
    sp = add("chips", cmd_chips, "chip registers (all, or one chip by name)")
    sp.add_argument("name", nargs="?")
    sp = add("dlist", cmd_dlist, "Atari: decode the ANTIC display list (default: at DLIST)")
    sp.add_argument("addr", nargs="?")
    sp = add("break", cmd_break, "add a breakpoint: address/symbol, raster line, or irq/nmi")
    sp.add_argument("where", nargs="?", default="0")
    sp.add_argument("--kind", default="exec", choices=["exec", "read", "write", "raster", "irq", "nmi"])
    sp.add_argument("--condition", "-c")
    sp.add_argument("--ignore", type=int)
    sp.add_argument("--len", type=int)
    sp.add_argument("--space", type=int)
    add("breaks", cmd_breaks, "list breakpoints")
    sp = add("unbreak", cmd_unbreak, "remove a breakpoint")
    sp.add_argument("bp")
    add("pause", cmd_pause, "stop the machine")
    sp = add("resume", cmd_resume, "run (--wait: until it stops)")
    sp.add_argument("--wait", action="store_true")
    sp.add_argument("--timeout", type=float)
    sp = add("step", cmd_step, "step instructions")
    sp.add_argument("count", nargs="?", type=int, default=1)
    add("next", cmd_next, "step over a subroutine call")
    add("finish", cmd_finish, "run until the current subroutine returns")
    sp = add("until", cmd_until, "run to an address or symbol")
    sp.add_argument("where")
    sp = add("frame", cmd_frame, "run to the end of the frame")
    sp.add_argument("count", nargs="?", type=int, default=1)
    sp = add("reset", cmd_reset, "reset (--hard: power cycle)")
    sp.add_argument("--hard", action="store_true")
    sp = add("save", cmd_save, "save machine state and freezes to a file")
    sp.add_argument("file")
    sp = add("load", cmd_load, "load a state file")
    sp.add_argument("file")
    sp = add("symbols", cmd_symbols, "load a symbol file (VICE labels, KickAss .sym)")
    sp.add_argument("file")
    sp.add_argument("--format", choices=["vice", "kickass"])
    add("record-start", cmd_record_start, "start recording input")
    sp = add("record-stop", cmd_record_stop, "stop recording; write the replay file")
    sp.add_argument("file")
    sp = add("replay", cmd_replay, "replay a recording")
    sp.add_argument("file")
    sp = add("search", cmd_search, "cheat search: start, results, or a narrowing operation")
    sp.add_argument("op", choices=["start", "results", "eq", "ne", "lt", "gt", "increased",
                                   "decreased", "unchanged", "changed", "increased_by",
                                   "decreased_by"])
    sp.add_argument("value", nargs="?")
    sp.add_argument("--size", type=int, choices=[1, 2, 4])
    sp.add_argument("--space", type=int)
    sp.add_argument("--max", type=int)
    sp = add("freeze", cmd_freeze, "freeze an address (default: its current value)")
    sp.add_argument("where")
    sp.add_argument("value", nargs="?")
    sp.add_argument("--size", type=int, choices=[1, 2, 4])
    sp.add_argument("--mode", choices=["block_writes", "reapply_per_frame", "reapply_on_read"])
    sp.add_argument("--label")
    sp.add_argument("--space", type=int)
    add("freezes", cmd_freezes, "list freezes")
    sp = add("unfreeze", cmd_unfreeze, "remove a freeze")
    sp.add_argument("freeze")
    sp = add("cheats", cmd_cheats, "save or load a TOML cheat file")
    sp.add_argument("op", choices=["save", "load"])
    sp.add_argument("file")
    sp = add("input", cmd_input, "queue input: 'joystick PORT up fire …' or 'key N down|up'")
    sp.add_argument("what", choices=["joystick", "key"])
    sp.add_argument("values", nargs="+")
    sp.add_argument("--delay", type=int, default=0, help="cycles from now")
    sp = add("events", cmd_events, "print pushed events until Ctrl-C")
    sp.add_argument("--subscribe", nargs="*", default=[],
                    help="optional events, e.g. frame_ready freeze_blocked_write")
    sp = add("raw", cmd_raw, "send any command with JSON arguments")
    sp.add_argument("cmd")
    sp.add_argument("args", nargs="?")
    if not shell:
        add("shell", cmd_shell, "interactive prompt")
    return p


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        with Client(args.host, args.port) as c:
            args.func(c, args)
    except (DebugError, ValueError, OSError) as e:
        print("error: %s" % e, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
