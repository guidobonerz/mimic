"""GDB extension for mimic's GDB stub: shows the emulated machine's picture.

Load it in GDB (gdb-multiarch, m68k-elf-gdb, z80 GDB …), before or after
connecting:

    (gdb) source tools/gdb/mimic.py
    (gdb) target remote 127.0.0.1:2159
    (gdb) mimic live on        # refresh the picture on every stop
    (gdb) mimic frame          # fetch it once (and open the viewer)
    (gdb) mimic preview        # open the live preview stream in a browser

It also switches a 68000 target to big-endian (GDB without an ELF file
assumes the host's byte order, and a target description cannot say it).

Commands:

    mimic frame [FILE] [crt]   write the current (partial) frame as PNG and
                               open a viewer page that reloads it
    mimic live on|off          rewrite the PNG whenever the target stops
    mimic preview              start the stub's live preview server
                               (`monitor preview`) and open its URL
    mimic beam                 frame, raster line and cycle

The picture comes from the stub's `qMimic.frame` packet (GDB 14 or newer
can send it from Python: `RemoteTargetConnection.send_packet`); older GDBs
fall back to `monitor snapshot FILE`, which the stub writes itself (same
host only). Standard library only.
"""

import os
import tempfile
import webbrowser

try:
    import gdb  # noqa: F401  (only inside GDB)
except ImportError:  # unit tests
    gdb = None
# Outside GDB, `import gdb` may find this very folder (tools/gdb).
if gdb is not None and not hasattr(gdb, "Command"):
    gdb = None

DEFAULT_FILE = os.path.join(tempfile.gettempdir(), "mimic-frame.png")

VIEWER = """<!doctype html>
<html><head><meta charset="utf-8"><title>mimic frame</title>
<style>
body {{ margin: 0; background: #111; color: #ccc; font: 13px sans-serif; }}
img {{ display: block; margin: 8px auto; image-rendering: pixelated;
      max-width: 100%; }}
p {{ text-align: center; }}
</style></head><body>
<img id="f" src="{png}" alt="mimic frame">
<p>refreshed by GDB on every stop (<code>mimic live on</code>)</p>
<script>
const img = document.getElementById("f");
setInterval(() => {{ img.src = "{png}?t=" + Date.now(); }}, 500);
</script>
</body></html>
"""


def unescape(data):
    """Undo the RSP binary escape: '}' followed by byte ^ 0x20."""
    out = bytearray()
    it = iter(data)
    for b in it:
        if b == 0x7D:
            nxt = next(it, None)
            if nxt is not None:
                out.append(nxt ^ 0x20)
        else:
            out.append(b)
    return bytes(out)


def frame_packet(crt=False, screen=None):
    """The qMimic.frame packet text."""
    params = []
    if screen:
        params.append("screen=" + screen)
    if crt:
        params.append("filter=crt")
    return "qMimic.frame" + (":" + ";".join(params) if params else "")


def viewer_html(png_path):
    """A page that shows the PNG and reloads it twice a second."""
    return VIEWER.format(png=os.path.basename(png_path))


class State:
    def __init__(self):
        self.file = DEFAULT_FILE
        self.crt = False
        self.live = False
        self.viewer_open = False


STATE = State()


def _connection():
    inferior = gdb.selected_inferior()
    return getattr(inferior, "connection", None)


def _send_packet(text):
    """The reply as bytes, or None if this GDB cannot send packets."""
    conn = _connection()
    if conn is None or not hasattr(conn, "send_packet"):
        return None
    reply = conn.send_packet(text)
    if isinstance(reply, str):
        reply = reply.encode("latin-1")
    return reply


def write_frame(path, crt):
    """Fetch the picture into `path`; True on success."""
    reply = None
    try:
        reply = _send_packet(frame_packet(crt))
    except gdb.error:
        reply = None
    if reply is not None and reply[:1] != b"E":
        with open(path, "wb") as f:
            f.write(unescape(reply))
        return True
    out = gdb.execute(
        "monitor snapshot %s%s" % (path, " crt" if crt else ""), to_string=True
    )
    return out.startswith("wrote")


def open_viewer(path):
    if STATE.viewer_open:
        return
    page = os.path.splitext(path)[0] + ".html"
    with open(page, "w", encoding="utf-8") as f:
        f.write(viewer_html(path))
    webbrowser.open("file://" + os.path.abspath(page))
    STATE.viewer_open = True


def needs_big_endian(arch_name, endian_text):
    """A 68000 target shown little-endian (no ELF file loaded)."""
    return arch_name.startswith("m68k") and "little" in endian_text


def _fix_endian():
    try:
        arch = gdb.selected_inferior().architecture().name()
        endian = gdb.execute("show endian", to_string=True)
    except (gdb.error, RuntimeError):
        return
    if needs_big_endian(arch, endian):
        gdb.execute("set endian big", to_string=True)


def _on_stop(_event):
    _fix_endian()
    if STATE.live:
        try:
            write_frame(STATE.file, STATE.crt)
        except gdb.error as e:
            print("mimic: %s" % e)


if gdb is not None:

    class Mimic(gdb.Command):
        """Show the emulated machine's picture (mimic GDB stub).

Usage: mimic frame [FILE] [crt] | mimic live on|off | mimic preview | mimic beam"""

        def __init__(self):
            super().__init__("mimic", gdb.COMMAND_USER, prefix=True)

    class MimicFrame(gdb.Command):
        """Write the current frame as PNG and open a viewer: mimic frame [FILE] [crt]"""

        def __init__(self):
            super().__init__("mimic frame", gdb.COMMAND_USER)

        def invoke(self, arg, from_tty):
            words = gdb.string_to_argv(arg)
            STATE.crt = "crt" in words
            files = [w for w in words if w != "crt"]
            if files:
                STATE.file = os.path.abspath(files[0])
            if write_frame(STATE.file, STATE.crt):
                print("frame written to %s" % STATE.file)
                open_viewer(STATE.file)
            else:
                print("mimic: no picture (is this a mimic stub?)")

    class MimicLive(gdb.Command):
        """Refresh the frame PNG on every stop: mimic live on|off"""

        def __init__(self):
            super().__init__("mimic live", gdb.COMMAND_USER)

        def invoke(self, arg, from_tty):
            arg = arg.strip()
            if arg not in ("on", "off"):
                raise gdb.GdbError("usage: mimic live on|off")
            STATE.live = arg == "on"
            if STATE.live and write_frame(STATE.file, STATE.crt):
                open_viewer(STATE.file)
            print("live picture %s (%s)" % (arg, STATE.file))

    class MimicPreview(gdb.Command):
        """Start the stub's live preview server and open it: mimic preview"""

        def __init__(self):
            super().__init__("mimic preview", gdb.COMMAND_USER)

        def invoke(self, arg, from_tty):
            out = gdb.execute("monitor preview", to_string=True).strip()
            if out.startswith("http"):
                webbrowser.open(out)
            print(out)

    class MimicBeam(gdb.Command):
        """Frame, raster line and cycle: mimic beam"""

        def __init__(self):
            super().__init__("mimic beam", gdb.COMMAND_USER)

        def invoke(self, arg, from_tty):
            gdb.execute("monitor beam")

    Mimic()
    MimicFrame()
    MimicLive()
    MimicPreview()
    MimicBeam()
    gdb.events.stop.connect(_on_stop)
    gdb.events.before_prompt.connect(_fix_endian)
    _fix_endian()
