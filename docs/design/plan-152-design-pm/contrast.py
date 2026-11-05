#!/usr/bin/env python3
"""Contrast check for the plan-152 design-pm boards (standard library only).

Reads contrast.csv beside this file (png, element, fg_x, fg_y, bg_x, bg_y),
samples the two pixels in each PNG and prints the WCAG 2.2 contrast ratio
of every row. Exits 1 when a ratio is below 4.5:1.

    python3 docs/design/plan-152-design-pm/contrast.py            # every row
    python3 docs/design/plan-152-design-pm/contrast.py card-ios-light.png
    python3 docs/design/plan-152-design-pm/contrast.py --point card-ios-light.png 120 300 140 300

The --point form measures any extra pair a reviewer picks (fg x y, bg x y).
"""
import csv
import pathlib
import struct
import sys
import zlib

HERE = pathlib.Path(__file__).resolve().parent
MINIMUM = 4.5


def read_png(path):
    """Return (width, height, rows) where rows[y][x] is an (r, g, b) tuple."""
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path.name}: not a PNG")
    pos, idat, width = 8, [], None
    while pos < len(data):
        length, kind = struct.unpack(">I4s", data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            width, height, depth, ctype, _, _, interlace = struct.unpack(">IIBBBBB", body)
            if depth != 8 or ctype not in (2, 6) or interlace:
                raise ValueError(f"{path.name}: needs 8-bit RGB or RGBA, not interlaced")
            bpp = 3 if ctype == 2 else 4
        elif kind == b"IDAT":
            idat.append(body)
        elif kind == b"IEND":
            break
    raw = zlib.decompress(b"".join(idat))
    stride = width * bpp
    rows, prev, i = [], bytearray(stride), 0
    for _ in range(height):
        ftype, line = raw[i], bytearray(raw[i + 1:i + 1 + stride])
        i += 1 + stride
        for x in range(stride):
            a = line[x - bpp] if x >= bpp else 0
            b = prev[x]
            c = prev[x - bpp] if x >= bpp else 0
            if ftype == 1:
                line[x] = (line[x] + a) & 255
            elif ftype == 2:
                line[x] = (line[x] + b) & 255
            elif ftype == 3:
                line[x] = (line[x] + (a + b) // 2) & 255
            elif ftype == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pred = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
                line[x] = (line[x] + pred) & 255
        rows.append([tuple(line[x * bpp:x * bpp + 3]) for x in range(width)])
        prev = line
    return width, height, rows


def luminance(rgb):
    def chan(v):
        v /= 255
        return v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4
    r, g, b = (chan(v) for v in rgb)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def ratio(fg, bg):
    hi, lo = sorted((luminance(fg), luminance(bg)), reverse=True)
    return (hi + 0.05) / (lo + 0.05)


def hexof(rgb):
    return "#%02x%02x%02x" % rgb


_cache = {}


def pixel(png, x, y):
    if png not in _cache:
        _cache[png] = read_png(HERE / png)
    width, height, rows = _cache[png]
    if not (0 <= x < width and 0 <= y < height):
        raise ValueError(f"{png}: point {x},{y} is outside {width}x{height}")
    return rows[y][x]


def main(argv):
    if argv[:1] == ["--point"]:
        png, fx, fy, bx, by = argv[1], *map(int, argv[2:6])
        fg, bg = pixel(png, fx, fy), pixel(png, bx, by)
        r = ratio(fg, bg)
        print(f"{png}\tpoint\t{hexof(fg)}\t{hexof(bg)}\t{r:.2f}\t{'pass' if r >= MINIMUM else 'FAIL'}")
        return 0 if r >= MINIMUM else 1
    only = set(argv)
    failed = total = 0
    lowest = None
    with open(HERE / "contrast.csv", newline="") as fh:
        for row in csv.DictReader(fh):
            if only and row["png"] not in only:
                continue
            fg = pixel(row["png"], int(row["fg_x"]), int(row["fg_y"]))
            bg = pixel(row["png"], int(row["bg_x"]), int(row["bg_y"]))
            r = ratio(fg, bg)
            total += 1
            ok = r >= MINIMUM
            failed += not ok
            lowest = r if lowest is None else min(lowest, r)
            print(f"{row['png']}\t{row['element']}\t{hexof(fg)}\t{hexof(bg)}\t{r:.2f}\t{'pass' if ok else 'FAIL'}")
    print(f"# {total} pairs, {failed} below {MINIMUM}:1, lowest {lowest:.2f}:1" if total else "# no rows")
    return 1 if failed or not total else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
