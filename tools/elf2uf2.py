#!/usr/bin/env python3
"""Minimal ELF -> UF2 converter for the RP2350 (Arm, secure image) and the RP2040.

Fallback for machines without picotool: the resulting file can be dropped on
the USB drive that appears in BOOTSEL mode ("RP2350" or "RPI-RP2").
Standard library only.

    tools/elf2uf2.py <firmware.elf> <firmware.uf2> [rp2350|rp2040]   (default rp2350)

NOTE: picotool is the tested path. This converter writes plain UF2 blocks
with the chip's family ID and no partition-table handling.
"""

import struct
import sys

UF2_MAGIC0, UF2_MAGIC1, UF2_MAGIC_END = 0x0A324655, 0x9E5D5157, 0x0AB16F30
FLAG_FAMILY_ID = 0x00002000
FAMILY = {"rp2350": 0xE48BFF59, "rp2040": 0xE48BFF56}  # rp2350-arm-s, rp2040
FLASH_START, FLASH_END = 0x10000000, 0x14000000
PAGE = 256


def flash_segments(elf):
    if elf[:4] != b"\x7fELF" or elf[4] != 1 or elf[5] != 1:
        sys.exit("not a 32-bit little-endian ELF file")
    phoff, = struct.unpack_from("<I", elf, 28)
    phentsize, phnum = struct.unpack_from("<HH", elf, 42)
    for i in range(phnum):
        p_type, offset, _vaddr, paddr, filesz = struct.unpack_from("<IIIII", elf, phoff + i * phentsize)
        if p_type == 1 and filesz > 0 and FLASH_START <= paddr < FLASH_END:  # PT_LOAD in flash
            yield paddr, elf[offset:offset + filesz]


def main():
    if len(sys.argv) not in (3, 4) or (len(sys.argv) == 4 and sys.argv[3] not in FAMILY):
        sys.exit(__doc__)
    family = FAMILY[sys.argv[3] if len(sys.argv) == 4 else "rp2350"]
    with open(sys.argv[1], "rb") as f:
        elf = f.read()

    pages = {}  # page address -> bytearray(256)
    for addr, data in flash_segments(elf):
        for i, byte in enumerate(data):
            a = addr + i
            pages.setdefault(a - a % PAGE, bytearray(PAGE))[a % PAGE] = byte
    if not pages:
        sys.exit("no loadable flash segments found")

    blocks = []
    for n, addr in enumerate(sorted(pages)):
        head = struct.pack("<IIIIIIII", UF2_MAGIC0, UF2_MAGIC1, FLAG_FAMILY_ID, addr, PAGE, n, len(pages), family)
        blocks.append(head + bytes(pages[addr]).ljust(476, b"\0") + struct.pack("<I", UF2_MAGIC_END))

    with open(sys.argv[2], "wb") as f:
        f.write(b"".join(blocks))
    print(f"{sys.argv[2]}: {len(blocks)} blocks, {len(blocks) * PAGE} bytes of flash from 0x{min(pages):08x}")


if __name__ == "__main__":
    main()
