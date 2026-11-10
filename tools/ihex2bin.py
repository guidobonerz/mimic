#!/usr/bin/env python3
"""Intel HEX to a binary ROM image.

    python tools/ihex2bin.py mpu-b-rom.hex ~/.mimic/roms/imsai/mpu-b.rom

The image starts at the file's lowest address and ends at its highest.
"""
import pathlib
import sys


def convert(text):
    data = {}
    for line in text.splitlines():
        line = line.strip()
        if not line.startswith(":"):
            continue
        record = bytes.fromhex(line[1:])
        if sum(record) & 0xFF:
            raise ValueError(f"bad checksum: {line}")
        length, kind = record[0], record[3]
        address = record[1] << 8 | record[2]
        if kind == 0:
            for i, byte in enumerate(record[4:4 + length]):
                data[address + i] = byte
        elif kind == 1:
            break
    if not data:
        raise ValueError("no data records")
    low, high = min(data), max(data)
    return bytes(data.get(a, 0) for a in range(low, high + 1))


def main():
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    image = convert(pathlib.Path(sys.argv[1]).read_text(encoding="ascii"))
    pathlib.Path(sys.argv[2]).write_bytes(image)
    print(f"{sys.argv[2]}: {len(image)} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
