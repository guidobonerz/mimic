#!/usr/bin/env python3
"""Fetch C-BIOS, the free MSX BIOS, into the ROM folder.

    python tools/fetch_cbios.py [target-dir]

Default target: the `msx` folder of the ROM directory (`EMU_ROM_DIR`, else
`~/.mimic/roms`). Downloads the C-BIOS 0.29a release archive from
SourceForge, checks its SHA-256 and unpacks the MSX1 ROMs (the main ROM in
its international 60 Hz, European 50 Hz, Japanese and Brazilian builds and
the logo ROM) and the MSX2 ROMs (main ROM in the same four builds, sub ROM,
logo ROM) with the C-BIOS licence text. The machine `msx` then starts
without a user-supplied BIOS, as an MSX1 or with `--model msx2`; C-BIOS
runs cartridges and has neither BASIC nor disk support.

C-BIOS is BSD-licensed (see the `cbios.txt` this script puts beside the
ROMs). It is an optional download, never committed (golden rule 6). Stdlib
only; files already there are kept.

Source: http://cbios.sourceforge.net/
"""

import hashlib
import io
import os
import pathlib
import sys
import urllib.request
import zipfile

VERSION = "0.29a"
URLS = [
    "https://sourceforge.net/projects/cbios/files/cbios/0.29/cbios-0.29a.zip/download",
    "https://downloads.sourceforge.net/project/cbios/cbios/0.29/cbios-0.29a.zip",
]
SHA256 = "c585a2f4a10b397e460a95043d59ac79cf4442fbb45a6e14a1e27ef9febaed73"
FILES = [
    "roms/cbios_main_msx1.rom",
    "roms/cbios_main_msx1_eu.rom",
    "roms/cbios_main_msx1_jp.rom",
    "roms/cbios_main_msx1_br.rom",
    "roms/cbios_logo_msx1.rom",
    "roms/cbios_main_msx2.rom",
    "roms/cbios_main_msx2_eu.rom",
    "roms/cbios_main_msx2_jp.rom",
    "roms/cbios_main_msx2_br.rom",
    "roms/cbios_sub.rom",
    "roms/cbios_logo_msx2.rom",
    "doc/cbios.txt",
]


def rom_dir():
    env = os.environ.get("EMU_ROM_DIR")
    if env:
        return pathlib.Path(env)
    return pathlib.Path.home() / ".mimic" / "roms"


def download():
    error = None
    for url in URLS:
        for _ in range(3):
            try:
                with urllib.request.urlopen(url, timeout=120) as r:
                    data = r.read()
            except OSError as e:
                error = e
                continue
            if hashlib.sha256(data).hexdigest() == SHA256:
                return data
            error = ValueError(f"{url}: checksum mismatch")
    raise SystemExit(f"error: C-BIOS {VERSION} not downloaded: {error}")


def main():
    dest = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else rom_dir() / "msx"
    names = [pathlib.PurePosixPath(f).name for f in FILES]
    if all((dest / n).is_file() for n in names):
        print(f"C-BIOS {VERSION} is already in {dest}")
        return 0
    archive = zipfile.ZipFile(io.BytesIO(download()))
    dest.mkdir(parents=True, exist_ok=True)
    for f, name in zip(FILES, names):
        target = dest / name
        if not target.is_file():
            target.write_bytes(archive.read(f"cbios-{VERSION}/{f}"))
    print(f"C-BIOS {VERSION} -> {dest}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
