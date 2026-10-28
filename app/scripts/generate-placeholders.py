#!/usr/bin/env python3
"""Reproducible temporary RS artwork, using only the Python standard library."""
from pathlib import Path
import struct
import zlib

assets = Path(__file__).resolve().parents[1] / 'ios/RemoteScreen/Placeholders'
assets.mkdir(parents=True, exist_ok=True)
glyphs = (
    ('11110', '10001', '10001', '11110', '10100', '10010', '10001'),
    ('01111', '10000', '10000', '01110', '00001', '00001', '11110'),
)


def png(width, height=None, scale=None):
    height = height or width
    scale = scale or max(1, width // 18)
    left, top = (width - 11 * scale) // 2, (height - 7 * scale) // 2
    rows = []
    for y in range(height):
        row = bytearray([0])
        for x in range(width):
            ink = False
            for index, glyph in enumerate(glyphs):
                gx, gy = (x - left - index * 6 * scale) // scale, (y - top) // scale
                if 0 <= gx < 5 and 0 <= gy < 7 and glyph[gy][gx] == '1':
                    ink = True
            row.extend((236, 240, 242) if ink else (13, 17, 20))
        rows.append(row)

    def chunk(kind, value):
        return (struct.pack('>I', len(value)) + kind + value +
                struct.pack('>I', zlib.crc32(kind + value) & 0xffffffff))

    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0)) +
            chunk(b'IDAT', zlib.compress(b''.join(rows), 9)) + chunk(b'IEND', b''))


# Plain PNG resources work in the sandbox without actool/CoreSimulator.
# UIKit resolves the scale and device suffixes from CFBundleIconFiles.
for name, size in [('AppIcon60x60@2x.png', 120), ('AppIcon60x60@3x.png', 180),
                   ('AppIcon76x76~ipad.png', 76), ('AppIcon76x76@2x~ipad.png', 152),
                   ('AppIcon83.5x83.5@2x~ipad.png', 167), ('AppIcon-1024.png', 1024)]:
    (assets / name).write_bytes(png(size))
(assets / 'LaunchMark@3x.png').write_bytes(png(1290, 2796, scale=24))
