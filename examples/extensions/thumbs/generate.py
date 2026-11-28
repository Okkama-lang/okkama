#!/usr/bin/env python3
"""Regenerate tiny RGB PNG inputs using only Python's standard library."""
from pathlib import Path
import struct
import zlib


def chunk(kind, data):
    return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))


for name, width, height, seed in [('landscape', 24, 16, 1), ('portrait', 12, 28, 2), ('square', 20, 20, 3)]:
    pixels = b''.join(b'\0' + bytes((x * 13 + y * 7 + seed * c * 29) % 256
                                  for x in range(width) for c in range(3)) for y in range(height))
    png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0))
    png += chunk(b'IDAT', zlib.compress(pixels, 9)) + chunk(b'IEND', b'')
    (Path(__file__).parent / 'input' / (name + '.png')).write_bytes(png)
