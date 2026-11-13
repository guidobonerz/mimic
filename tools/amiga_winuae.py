"""Run a hardware disk of tools/amiga_hwdisk.py in WinUAE as a reference.

WinUAE (cycle-exact A500: 68000, OCS, 512 KB chip RAM and 512 KB slow RAM,
Kickstart 1.3, floppy at normal speed) boots a copy of the disk; once the
harness has written its results and the image stops changing, WinUAE is
stopped and the image saved. WinUAE serves as an observable reference only
(no code taken; CLAUDE.md golden rule 7). It agreed with the owner's real
A500 on every value of bus tests 11-16 (2026-11-08), except the serial
port's frame time (bus test 11 group 2), where the A500 is the reference.

usage: python tools/amiga_winuae.py IN.adf OUT.adf [--seconds N]
  WinUAE from EMU_WINUAE (default: winuae64.exe on PATH), the Kickstart
  from EMU_KICKSTART (default: $EMU_ROM_DIR/amiga/Kickstart 1.3.rom).
  Then: python tools/amiga_hwdisk.py results OUT.adf

A picture instead of results (for the picture tests such as
tools/amiga_agatest.py):

  python tools/amiga_winuae.py IN.adf OUT.png --shot SECONDS [--model a1200]

boots the disk, waits SECONDS and saves WinUAE's window as it draws it
(PrintWindow with full content: only WinUAE's own window, even when other
windows cover it; no screen capture, no focus taken). The window is 720
high-resolution pixels wide with doubled lines, WinUAE's status bar at the
bottom. `--model a1200` is a cycle-exact A1200 (68EC020, AGA, 2 MB chip
RAM) with the A1200 Kickstart from EMU_KICKSTART (default:
$EMU_ROM_DIR/amiga/amiga-os-310-a1200.rom with its rom.key beside it).
"""
import hashlib
import os
import shutil
import subprocess
import sys
import tempfile
import time

RESULT_AT = 1000 * 512
MAGIC = b"MIMICHW1"

CONFIG = """config_description=mimic reference A500 (tools/amiga_winuae.py)
use_gui=no
kickstart_rom_file={kick}
floppy0={disk}
floppy0type=0
floppy0wp=false
nr_floppies=1
floppy_speed=100
cpu_type=68000
cpu_model=68000
cpu_compatible=true
cpu_cycle_exact=true
cpu_memory_cycle_exact=true
blitter_cycle_exact=true
cycle_exact=true
chipset=ocs
chipset_compatible=A500
chipmem_size=1
bogomem_size=2
fastmem_size=0
ntsc=false
immediate_blits=false
waiting_blits=disabled
"""


A1200_CONFIG = """config_description=mimic reference A1200 (tools/amiga_winuae.py)
use_gui=no
kickstart_rom_file={kick}
kickstart_key_file={key}
floppy0={disk}
floppy0type=0
floppy0wp=false
nr_floppies=1
floppy_speed=100
cpu_type=68ec020
cpu_model=68020
cpu_compatible=true
cpu_24bit_addressing=true
cpu_cycle_exact=true
cpu_memory_cycle_exact=true
blitter_cycle_exact=true
cycle_exact=true
chipset=aga
chipset_compatible=A1200
chipmem_size=4
bogomem_size=0
fastmem_size=0
ntsc=false
immediate_blits=false
waiting_blits=disabled
"""

# Picture settings: 720 high-resolution pixels, no filter, not centred.
SHOT_CONFIG = """gfx_width_windowed=720
gfx_height_windowed=568
gfx_lores=false
gfx_linemode=none
gfx_filter=no
gfx_center_horizontal=none
gfx_center_vertical=none
"""

# PrintWindow with PW_CLIENTONLY | PW_RENDERFULLCONTENT (3): WinUAE's own
# window only.
PRINT_WINDOW = r'''
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public struct RECT { public int L, T, R, B; }
public class W {
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
"@
[W]::SetProcessDPIAware() | Out-Null
$h = (Get-Process -Id PID).MainWindowHandle
$r = New-Object RECT
[W]::GetClientRect($h, [ref]$r) | Out-Null
$bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$dc = $g.GetHdc()
$ok = [W]::PrintWindow($h, $dc, 3)
$g.ReleaseHdc($dc)
if (-not $ok) { exit 1 }
$bmp.Save("OUT", [System.Drawing.Imaging.ImageFormat]::Png)
'''


def shot(exe, cfg, out, seconds):
    """Run WinUAE with `cfg`, save its window after `seconds`."""
    proc = subprocess.Popen([exe, "-f", cfg])
    try:
        time.sleep(seconds)
        script = PRINT_WINDOW.replace("PID", str(proc.pid)).replace("OUT", os.path.abspath(out))
        subprocess.run(["powershell", "-NoProfile", "-Command", script], check=True)
    finally:
        proc.kill()
        proc.wait()
    print(f"saved {out}")


def main():
    args = sys.argv[1:]
    if len(args) < 2:
        print(__doc__)
        sys.exit(1)
    src, out = args[0], args[1]
    seconds = int(args[args.index("--seconds") + 1]) if "--seconds" in args else 300
    exe = os.environ.get("EMU_WINUAE", "winuae64.exe")
    a1200 = "--model" in args and args[args.index("--model") + 1] == "a1200"
    roms = os.path.join(os.environ.get("EMU_ROM_DIR", os.path.expanduser("~/.mimic/roms")), "amiga")
    kick = os.environ.get("EMU_KICKSTART") or os.path.join(
        roms, "amiga-os-310-a1200.rom" if a1200 else "Kickstart 1.3.rom")
    with tempfile.TemporaryDirectory() as tmp:
        disk = os.path.join(tmp, "disk.adf")
        shutil.copyfile(src, disk)
        cfg = os.path.join(tmp, "mimic.uae")
        with open(cfg, "w", encoding="utf-8") as f:
            if a1200:
                key = os.path.join(os.path.dirname(os.path.abspath(kick)), "rom.key")
                f.write(A1200_CONFIG.format(kick=os.path.abspath(kick), key=key, disk=disk))
            else:
                f.write(CONFIG.format(kick=os.path.abspath(kick), disk=disk))
            if "--shot" in args:
                f.write(SHOT_CONFIG)
        if "--shot" in args:
            shot(exe, cfg, out, float(args[args.index("--shot") + 1]))
            return
        proc = subprocess.Popen([exe, "-f", cfg])
        last, stable, start = None, 0, time.time()
        try:
            while time.time() - start < seconds:
                time.sleep(5)
                data = open(disk, "rb").read()
                if data[RESULT_AT:RESULT_AT + len(MAGIC)] != MAGIC:
                    continue
                digest = hashlib.md5(data).hexdigest()
                stable = stable + 1 if digest == last else 0
                last = digest
                if stable >= 3:
                    break
            else:
                print("timed out: no results from WinUAE", file=sys.stderr)
        finally:
            proc.kill()
            proc.wait()
        shutil.copyfile(disk, out)
    print(f"saved {out}")


if __name__ == "__main__":
    main()
