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


def main():
    args = sys.argv[1:]
    if len(args) < 2:
        print(__doc__)
        sys.exit(1)
    src, out = args[0], args[1]
    seconds = int(args[args.index("--seconds") + 1]) if "--seconds" in args else 300
    exe = os.environ.get("EMU_WINUAE", "winuae64.exe")
    kick = os.environ.get("EMU_KICKSTART") or os.path.join(
        os.environ.get("EMU_ROM_DIR", "roms"), "amiga", "Kickstart 1.3.rom")
    with tempfile.TemporaryDirectory() as tmp:
        disk = os.path.join(tmp, "disk.adf")
        shutil.copyfile(src, disk)
        cfg = os.path.join(tmp, "a500.uae")
        with open(cfg, "w", encoding="utf-8") as f:
            f.write(CONFIG.format(kick=os.path.abspath(kick), disk=disk))
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
