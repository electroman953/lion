#!/usr/bin/env python3
"""Extracts glyphs of the X11 "misc-fixed" fonts, which are in the public domain, into
`src/font_data.rs`: the font that `lion_ui` draws with, the same on every system.

Usage: python3 tools/extract_font.py /usr/share/fonts/X11/misc > src/font_data.rs
"""

import gzip
import struct
import sys

PROPERTIES, METRICS, BITMAPS, ENCODINGS, BDF_ACCELERATORS, ACCELERATORS = 1, 4, 8, 32, 256, 2

# The fonts, by the name of their file, and the name they get in Rust.
FONTS = [("7x13", "FONT_13"), ("9x18", "FONT_18"), ("9x18B", "FONT_18_BOLD"), ("10x20", "FONT_20")]

# The characters kept: Latin, Greek, punctuation, arrows, mathematics, boxes and marks.
RANGES = [
    (0x20, 0x7E), (0xA0, 0x17F), (0x391, 0x3C9), (0x2010, 0x2027), (0x2030, 0x203A),
    (0x20AC, 0x20AC), (0x2190, 0x2195), (0x2200, 0x22FF), (0x2500, 0x257F),
    (0x25A0, 0x25CF), (0x2713, 0x2718),
]


def read_pcf(path):
    data = gzip.open(path).read()
    assert data[:4] == b"\x01fcp", path
    (count,) = struct.unpack_from("<I", data, 4)
    tables = {}
    for i in range(count):
        kind, fmt, size, offset = struct.unpack_from("<IIII", data, 8 + 16 * i)
        tables[kind] = (fmt, offset)
    return data, tables


def fmt_of(data, offset):
    (fmt,) = struct.unpack_from("<I", data, offset)
    return fmt, (">" if fmt & 4 else "<")


def properties(data, tables):
    fmt, offset = tables[PROPERTIES]
    fmt, e = fmt_of(data, offset)
    (n,) = struct.unpack_from(e + "I", data, offset + 4)
    props = []
    pos = offset + 8
    for _ in range(n):
        name, is_string, value = struct.unpack_from(e + "IBI", data, pos)
        props.append((name, is_string, value))
        pos += 9
    pos += (4 - n % 4) % 4 if n % 4 else 0
    (size,) = struct.unpack_from(e + "I", data, pos)
    strings = data[pos + 4 : pos + 4 + size]
    text = lambda at: strings[at : strings.index(b"\0", at)].decode("latin-1")
    return {text(name): (text(value) if is_string else value) for name, is_string, value in props}


def metrics(data, tables):
    fmt, offset = tables[METRICS]
    fmt, e = fmt_of(data, offset)
    result = []
    if fmt & 0x100:
        (n,) = struct.unpack_from(e + "H", data, offset + 4)
        for i in range(n):
            l, r, w, a, d = struct.unpack_from("5B", data, offset + 6 + 5 * i)
            result.append((l - 0x80, r - 0x80, w - 0x80, a - 0x80, d - 0x80))
    else:
        (n,) = struct.unpack_from(e + "I", data, offset + 4)
        for i in range(n):
            l, r, w, a, d, _ = struct.unpack_from(e + "6h", data, offset + 8 + 12 * i)
            result.append((l, r, w, a, d))
    return result


def bitmaps(data, tables):
    fmt, offset = tables[BITMAPS]
    fmt, e = fmt_of(data, offset)
    (n,) = struct.unpack_from(e + "I", data, offset + 4)
    offsets = struct.unpack_from(e + "%dI" % n, data, offset + 8)
    sizes = struct.unpack_from(e + "4I", data, offset + 8 + 4 * n)
    start = offset + 8 + 4 * n + 16
    pad = fmt & 3
    msb_bits = bool(fmt & 8)
    return offsets, data[start : start + sizes[pad]], 1 << pad, msb_bits, e


def encodings(data, tables):
    fmt, offset = tables[ENCODINGS]
    fmt, e = fmt_of(data, offset)
    lo2, hi2, lo1, hi1, default = struct.unpack_from(e + "5H", data, offset + 4)
    width = hi2 - lo2 + 1
    count = width * (hi1 - lo1 + 1)
    indices = struct.unpack_from(e + "%dH" % count, data, offset + 14)
    table = {}
    for i, glyph in enumerate(indices):
        if glyph != 0xFFFF:
            table[((lo1 + i // width) << 8) | (lo2 + i % width)] = glyph
    return table


def accelerators(data, tables):
    fmt, offset = tables.get(BDF_ACCELERATORS, tables.get(ACCELERATORS))
    fmt, e = fmt_of(data, offset)
    ascent, descent = struct.unpack_from(e + "ii", data, offset + 12)
    return ascent, descent


def glyph_rows(index, glyph_metrics, bits, cell_width, ascent, height):
    offsets, blob, pad, msb_bits, _ = bits
    left, right, _, glyph_ascent, glyph_descent = glyph_metrics[index]
    width = right - left
    row_bytes = ((width + 7) // 8 + pad - 1) // pad * pad
    rows = [0] * height
    for y in range(glyph_ascent + glyph_descent):
        row = blob[offsets[index] + y * row_bytes : offsets[index] + (y + 1) * row_bytes]
        cell_y = ascent - glyph_ascent + y
        if not 0 <= cell_y < height:
            continue
        for x in range(width):
            byte = row[x // 8]
            bit = (byte >> (7 - x % 8)) & 1 if msb_bits else (byte >> (x % 8)) & 1
            cell_x = left + x
            if bit and 0 <= cell_x < cell_width:
                rows[cell_y] |= 1 << (cell_width - 1 - cell_x)
    return rows


def main(folder):
    out = [
        "//! Glyphs of the X11 \"misc-fixed\" fonts, in the public domain (\"Public domain font.",
        "//! Share and enjoy.\"), made by `tools/extract_font.py`: do not edit by hand.",
        "//!",
        "//! Each font is a cell width, a height, an ascent, and one line per character: its code",
        "//! point, then each row of its cell in hexadecimal, the leftmost pixel in the highest bit.",
        "",
        "use crate::font::FontData;",
        "",
    ]
    for file, name in FONTS:
        data, tables = read_pcf(f"{folder}/{file}.pcf.gz")
        props = properties(data, tables)
        assert "Public domain" in props.get("COPYRIGHT", ""), (file, props.get("COPYRIGHT"))
        glyph_metrics = metrics(data, tables)
        bits = bitmaps(data, tables)
        table = encodings(data, tables)
        ascent, descent = accelerators(data, tables)
        height = ascent + descent
        cell_width = glyph_metrics[table[ord("M")]][2]
        digits = (cell_width + 3) // 4
        lines = []
        for low, high in RANGES:
            for code in range(low, high + 1):
                if code not in table:
                    continue
                rows = glyph_rows(table[code], glyph_metrics, bits, cell_width, ascent, height)
                lines.append("%04x:%s" % (code, "".join("%0*x" % (digits, row) for row in rows)))
        out.append(f"/// `{file}`: {props.get('FAMILY_NAME', '')} {props.get('WEIGHT_NAME', '')}, {len(lines)} characters.")
        out.append(f"pub const {name}: FontData = FontData {{")
        out.append(f"    width: {cell_width},")
        out.append(f"    height: {height},")
        out.append(f"    ascent: {ascent},")
        out.append("    glyphs: \"\\")
        for line in lines:
            out.append(line + "\\n\\")
        out.append("\",")
        out.append("};")
        out.append("")
    print("\n".join(out))


main(sys.argv[1])
