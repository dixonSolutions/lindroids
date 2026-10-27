#!/usr/bin/env python3
"""Clear DF_1_PIE in DT_FLAGS_1 of ELF files, in place.

NDK r27's lld marks PIE executables with DF_1_PIE; Android 7's linker does not know the flag and
prints "unsupported flags DT_FLAGS_1=0x8000001" on every run of the program (Android 8 and later
know it). Android needs no flag to treat a PIE as one, so clearing it changes nothing else. Termux
does the same to its packages with termux-elf-cleaner.

    scripts/elf-clear-pie-flag.py <elf>...
"""
import struct
import sys

DT_FLAGS_1 = 0x6FFFFFFB
DF_1_PIE = 0x08000000
PT_DYNAMIC = 2


def clear(path):
    with open(path, "r+b") as f:
        data = bytearray(f.read())
        if data[:4] != b"\x7fELF":
            raise SystemExit(f"{path}: not an ELF file")
        is64 = data[4] == 2
        end = "<" if data[5] == 1 else ">"
        if is64:
            phoff, = struct.unpack_from(end + "Q", data, 0x20)
            phentsize, phnum = struct.unpack_from(end + "HH", data, 0x36)
            ph = end + "IIQQQQQQ"  # type, flags, offset, vaddr, paddr, filesz, memsz, align
            dyn = end + "qQ"
        else:
            phoff, = struct.unpack_from(end + "I", data, 0x1C)
            phentsize, phnum = struct.unpack_from(end + "HH", data, 0x2A)
            ph = end + "IIIIIIII"  # type, offset, vaddr, paddr, filesz, memsz, flags, align
            dyn = end + "iI"
        changed = False
        for i in range(phnum):
            fields = struct.unpack_from(ph, data, phoff + i * phentsize)
            if fields[0] != PT_DYNAMIC:
                continue
            offset, size = (fields[2], fields[5]) if is64 else (fields[1], fields[4])
            step = struct.calcsize(dyn)
            for at in range(offset, offset + size, step):
                tag, val = struct.unpack_from(dyn, data, at)
                if tag == 0:
                    break
                if tag == DT_FLAGS_1 and val & DF_1_PIE:
                    struct.pack_into(dyn, data, at, tag, val & ~DF_1_PIE)
                    changed = True
        if changed:
            f.seek(0)
            f.write(data)
        return changed


if __name__ == "__main__":
    for p in sys.argv[1:]:
        print(f"{p}: {'DF_1_PIE cleared' if clear(p) else 'no DF_1_PIE'}")
