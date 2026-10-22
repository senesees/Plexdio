#!/usr/bin/env python3
"""Generate assets/icon.ico and assets/icon.png (no third-party deps).

The icon is an amber disc with a dark "play" chevron, rendered with 4x4
supersampling so edges are anti-aliased.
"""
import math
import struct
import sys
import zlib
from pathlib import Path

AMBER = (229, 160, 13)
DARK = (24, 24, 24)


def inside_triangle(px, py, a, b, c):
    def sign(p1, p2, p3):
        return (p1[0] - p3[0]) * (p2[1] - p3[1]) - (p2[0] - p3[0]) * (p1[1] - p3[1])

    d1 = sign((px, py), a, b)
    d2 = sign((px, py), b, c)
    d3 = sign((px, py), c, a)
    neg = d1 < 0 or d2 < 0 or d3 < 0
    pos = d1 > 0 or d2 > 0 or d3 > 0
    return not (neg and pos)


def render(size):
    ss = 4
    px = []
    tri = ((0.40, 0.30), (0.40, 0.70), (0.72, 0.50))
    for y in range(size):
        row = []
        for x in range(size):
            r = g = b = a = 0.0
            for sy in range(ss):
                for sx in range(ss):
                    u = (x + (sx + 0.5) / ss) / size
                    v = (y + (sy + 0.5) / ss) / size
                    d = math.hypot(u - 0.5, v - 0.5)
                    if d <= 0.47:
                        col = DARK if inside_triangle(u, v, *tri) else AMBER
                        r += col[0]
                        g += col[1]
                        b += col[2]
                        a += 255
            n = ss * ss
            row.append((int(r / max(a / 255, 1e-9)) if a else 0,
                        int(g / max(a / 255, 1e-9)) if a else 0,
                        int(b / max(a / 255, 1e-9)) if a else 0,
                        int(a / n)))
        px.append(row)
    return px


def png_bytes(size, px):
    raw = b"".join(b"\x00" + b"".join(bytes(p) for p in row) for row in px)

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def main(out_dir):
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    sizes = [16, 24, 32, 48, 64, 256]
    images = [png_bytes(s, render(s)) for s in sizes]
    header = struct.pack("<HHH", 0, 1, len(sizes))
    offset = 6 + 16 * len(sizes)
    entries = b""
    for s, data in zip(sizes, images):
        entries += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    (out / "icon.ico").write_bytes(header + entries + b"".join(images))
    (out / "icon.png").write_bytes(images[sizes.index(64)])
    # Raw 64x64 RGBA for the window/tray icon (avoids a PNG decoder in the app).
    (out / "icon64.rgba").write_bytes(b"".join(bytes(p) for row in render(64) for p in row))
    print("wrote", out / "icon.ico", out / "icon.png", out / "icon64.rgba")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "assets")
