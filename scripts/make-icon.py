"""Draws assets/savestates.ico: a cyan-to-amber diamond (the logo's ◆) on the app's dark blue.

Pure Python (no Pillow). Run `python scripts/make-icon.py` to rebuild the icon; sizes 256, 48, 32, 16.
"""
import struct
import zlib
from pathlib import Path

SIZES = (256, 48, 32, 16)
BACKGROUND, BORDER = (12, 17, 32), (42, 54, 84)  # the app's #0c1120 and #2a3654
CYAN, AMBER = (56, 189, 248), (251, 191, 36)  # #38bdf8 and #fbbf24
OUT = Path(__file__).resolve().parents[1] / "assets" / "savestates.ico"


def sample(x, y):
    """RGBA of the icon at (x, y) in 0..1 coordinates; transparent outside the rounded square."""
    radius, edge, half = 0.22, 0.03, 0.34

    def inside_rounded(inset):
        dx = max(abs(x - 0.5) - (0.5 - inset - radius), 0.0)
        dy = max(abs(y - 0.5) - (0.5 - inset - radius), 0.0)
        return dx * dx + dy * dy <= radius * radius

    if not inside_rounded(0.0):
        return (0, 0, 0, 0)
    if abs(x - 0.5) + abs(y - 0.5) <= half:
        t = min(max((x - 0.5 + y - 0.5) / (2 * half) + 0.5, 0.0), 1.0)  # cyan top-left, amber bottom-right
        return tuple(round(CYAN[i] * (1 - t) + AMBER[i] * t) for i in range(3)) + (255,)
    return (BACKGROUND if inside_rounded(edge) else BORDER) + (255,)


def render(size, grid=8):
    """size x size RGBA rows, each pixel the average of grid x grid samples."""
    rows = []
    for py in range(size):
        row = []
        for px in range(size):
            red = green = blue = alpha = 0
            for sy in range(grid):
                for sx in range(grid):
                    r, g, b, a = sample((px + (sx + 0.5) / grid) / size, (py + (sy + 0.5) / grid) / size)
                    red, green, blue, alpha = red + r * a, green + g * a, blue + b * a, alpha + a
            count = grid * grid
            row.append((0, 0, 0, 0) if alpha == 0 else
                       (round(red / alpha), round(green / alpha), round(blue / alpha), round(alpha / count)))
        rows.append(row)
    return rows


def png(rows):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    raw = b"".join(b"\0" + bytes(c for pixel in row for c in pixel) for row in rows)
    header = struct.pack(">IIBBBBB", len(rows), len(rows), 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def dib(rows):
    """A bottom-up 32-bit BGRA bitmap with its (all zero) AND mask, the classic small-icon format."""
    size = len(rows)
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    pixels = b"".join(bytes(c for r, g, b, a in row for c in (b, g, r, a)) for row in reversed(rows))
    return header + pixels + bytes(((size + 31) // 32 * 4) * size)


def main():
    images = [(size, png(rows) if size == 256 else dib(rows))
              for size in SIZES for rows in [render(size)]]
    out = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    for size, data in images:
        out += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    OUT.parent.mkdir(exist_ok=True)
    OUT.write_bytes(out + b"".join(data for _, data in images))
    print(f"wrote {OUT} ({len(out) + sum(len(d) for _, d in images)} bytes)")


if __name__ == "__main__":
    main()
