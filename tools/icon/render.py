#!/usr/bin/env python3
"""Render the app icon PNGs from app/assets/icon/app-icon.svg.

Usage:
    python3 tools/icon/render.py           # write every PNG
    python3 tools/icon/render.py --check   # re-render and compare with the committed PNGs

Needs only the Python standard library, so it gives the same pixels on macOS
and Linux without rsvg-convert, Inkscape or Pillow. It understands the subset
of SVG the icon uses: <rect> (x, y, width, height, rx, fill, stroke,
stroke-width) and <linearGradient> in objectBoundingBox units, painted in
document order with anti-aliased edges.

Outputs:
  iOS      Assets.xcassets/AppIcon.appiconset/AppIcon-1024.png, 1024x1024 RGB
           without alpha (App Store Connect rejects an icon with alpha).
  Android  mipmap-*/ic_launcher_foreground.png: the artwork without the
           background, inside the 72dp of the 108dp adaptive layer (Android
           masks it; the background is drawable/ic_launcher_background.xml);
           mipmap-*/ic_launcher.png and ic_launcher_round.png: legacy rounded
           square and circle icons.
"""

import math
import pathlib
import struct
import sys
import xml.etree.ElementTree as ET
import zlib

ROOT = pathlib.Path(__file__).resolve().parents[2]
SVG = ROOT / "app/assets/icon/app-icon.svg"
IOS = ROOT / "app/ios/RemoteScreen/Assets.xcassets/AppIcon.appiconset/AppIcon-1024.png"
RES = ROOT / "app/android/app/src/main/res"
NS = "{http://www.w3.org/2000/svg}"
DENSITIES = {"mdpi": 1, "hdpi": 1.5, "xhdpi": 2, "xxhdpi": 3, "xxxhdpi": 4}
# Legacy launcher icons are 48dp; the adaptive foreground layer is 108dp with
# the artwork kept inside its central 72dp.
LEGACY_DP = 48
ADAPTIVE_DP = 108
ADAPTIVE_ART_DP = 72
LEGACY_CORNER = 0.18


def parse_color(value):
    value = value.strip()
    if len(value) == 4:
        value = "#" + "".join(ch * 2 for ch in value[1:])
    if len(value) != 7 or not value.startswith("#"):
        raise ValueError(f"unsupported color {value!r}")
    return tuple(int(value[i:i + 2], 16) / 255 for i in (1, 3, 5))


def parse_svg(path):
    root = ET.parse(path).getroot()
    view = [float(v) for v in root.get("viewBox", "0 0 1024 1024").split()]
    if view[0] != 0 or view[1] != 0 or view[2] != view[3]:
        raise ValueError("the icon must have a square viewBox at 0 0")
    gradients = {}
    for grad in root.iter(NS + "linearGradient"):
        stops = [
            (float(stop.get("offset", "0")), parse_color(stop.get("stop-color", "#000")))
            for stop in grad.iter(NS + "stop")
        ]
        gradients[grad.get("id")] = {
            "x1": float(grad.get("x1", "0")),
            "y1": float(grad.get("y1", "0")),
            "x2": float(grad.get("x2", "1")),
            "y2": float(grad.get("y2", "0")),
            "stops": stops,
        }
    shapes = []
    for element in root:
        tag = element.tag.replace(NS, "")
        if tag == "defs":
            continue
        if tag != "rect":
            raise ValueError(f"unsupported element <{tag}>")
        x = float(element.get("x", "0"))
        y = float(element.get("y", "0"))
        w = float(element.get("width"))
        h = float(element.get("height"))
        shapes.append({
            "id": element.get("id"),
            "x": x, "y": y, "w": w, "h": h,
            "r": min(float(element.get("rx", "0")), w / 2, h / 2),
            "fill": paint(element.get("fill", "#000"), gradients),
            "stroke": paint(element.get("stroke", "none"), gradients),
            "sw": float(element.get("stroke-width", "1")),
        })
    return view[2], shapes


def paint(value, gradients):
    if value == "none":
        return None
    if value.startswith("url(#"):
        return gradients[value[5:-1]]
    return parse_color(value)


def paint_at(spec, shape, px, py):
    if isinstance(spec, tuple):
        return spec
    u = (px - shape["x"]) / shape["w"]
    v = (py - shape["y"]) / shape["h"]
    dx = spec["x2"] - spec["x1"]
    dy = spec["y2"] - spec["y1"]
    t = ((u - spec["x1"]) * dx + (v - spec["y1"]) * dy) / (dx * dx + dy * dy)
    stops = spec["stops"]
    if t <= stops[0][0]:
        return stops[0][1]
    for (o0, c0), (o1, c1) in zip(stops, stops[1:]):
        if t <= o1:
            f = 0 if o1 == o0 else (t - o0) / (o1 - o0)
            return tuple(a + (b - a) * f for a, b in zip(c0, c1))
    return stops[-1][1]


def round_rect_distance(px, py, cx, cy, hx, hy, r):
    qx = abs(px - cx) - hx + r
    qy = abs(py - cy) - hy + r
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    return outside + min(max(qx, qy), 0.0) - r


def clamp01(value):
    return 0.0 if value <= 0 else 1.0 if value >= 1 else value


def render(view, shapes, size, scale=1.0, skip=()):
    """RGBA floats, premultiplied. `scale` shrinks the artwork about the centre."""
    k = size / view * scale
    offset = (size - view * k) / 2
    buf = [0.0] * (size * size * 4)
    for shape in shapes:
        if shape["id"] in skip:
            continue
        cx = shape["x"] + shape["w"] / 2
        cy = shape["y"] + shape["h"] / 2
        hx, hy, r = shape["w"] / 2, shape["h"] / 2, shape["r"]
        half = shape["sw"] / 2 if shape["stroke"] is not None else 0.0
        reach = half + 1.0 / k
        x0 = max(0, int((shape["x"] - reach) * k + offset))
        x1 = min(size, int(math.ceil((shape["x"] + shape["w"] + reach) * k + offset)))
        y0 = max(0, int((shape["y"] - reach) * k + offset))
        y1 = min(size, int(math.ceil((shape["y"] + shape["h"] + reach) * k + offset)))
        layers = []
        if shape["fill"] is not None:
            layers.append(("fill", shape["fill"]))
        if shape["stroke"] is not None:
            layers.append(("stroke", shape["stroke"]))
        for kind, spec in layers:
            for j in range(y0, y1):
                py = (j + 0.5 - offset) / k
                row = j * size
                for i in range(x0, x1):
                    px = (i + 0.5 - offset) / k
                    d = round_rect_distance(px, py, cx, cy, hx, hy, r)
                    if kind == "fill":
                        a = clamp01(0.5 - d * k)
                    else:
                        a = clamp01(0.5 - (abs(d) - half) * k)
                    if a == 0.0:
                        continue
                    cr, cg, cb = paint_at(spec, shape, px, py)
                    at = (row + i) * 4
                    keep = 1.0 - a
                    buf[at] = cr * a + buf[at] * keep
                    buf[at + 1] = cg * a + buf[at + 1] * keep
                    buf[at + 2] = cb * a + buf[at + 2] * keep
                    buf[at + 3] = a + buf[at + 3] * keep
    return buf


def mask(buf, size, corner):
    """Clips to a rounded square; corner 0.5 is a circle."""
    half = size / 2
    r = corner * size
    for j in range(size):
        for i in range(size):
            d = round_rect_distance(i + 0.5, j + 0.5, half, half, half, half, r)
            a = clamp01(0.5 - d)
            if a < 1.0:
                at = (j * size + i) * 4
                for c in range(4):
                    buf[at + c] *= a
    return buf


def to_bytes(buf, size, alpha):
    rows = bytearray()
    for j in range(size):
        rows.append(0)
        for i in range(size):
            at = (j * size + i) * 4
            a = buf[at + 3]
            if alpha:
                rgb = [buf[at + c] / a if a > 0 else 0.0 for c in range(3)]
                rows.extend(round(clamp01(v) * 255) for v in rgb)
                rows.append(round(clamp01(a) * 255))
            else:
                # Composited over black; the icon background is opaque anyway.
                rows.extend(round(clamp01(buf[at + c]) * 255) for c in range(3))
    return bytes(rows)


def png_bytes(size, alpha, raw):
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", size, size, 8, 6 if alpha else 2, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
            + chunk(b"sRGB", b"\x00")
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def read_png(path):
    """(width, height, channels, pixel bytes) of an 8-bit RGB/RGBA PNG."""
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path}: not a PNG")
    pos, idat, header = 8, b"", None
    while pos < len(data):
        length, kind = struct.unpack(">I4s", data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + length]
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat += body
        pos += 12 + length
    width, height, depth, color, _, _, interlace = header
    if depth != 8 or color not in (2, 6) or interlace != 0:
        raise ValueError(f"{path}: expected 8-bit RGB or RGBA, got depth {depth} color {color}")
    channels = 3 if color == 2 else 4
    raw = zlib.decompress(idat)
    stride = width * channels
    out = bytearray()
    prev = bytearray(stride)
    pos = 0
    for _ in range(height):
        kind = raw[pos]
        line = bytearray(raw[pos + 1:pos + 1 + stride])
        pos += 1 + stride
        for i in range(stride):
            left = line[i - channels] if i >= channels else 0
            up = prev[i]
            corner = prev[i - channels] if i >= channels else 0
            if kind == 1:
                line[i] = (line[i] + left) & 255
            elif kind == 2:
                line[i] = (line[i] + up) & 255
            elif kind == 3:
                line[i] = (line[i] + (left + up) // 2) & 255
            elif kind == 4:
                p = left + up - corner
                pa, pb, pc = abs(p - left), abs(p - up), abs(p - corner)
                pred = left if pa <= pb and pa <= pc else up if pb <= pc else corner
                line[i] = (line[i] + pred) & 255
        out.extend(line)
        prev = line
    return width, height, channels, bytes(out)


def outputs(view, shapes):
    """(path, size, alpha, render callback) for every PNG."""
    jobs = [(IOS, 1024, False, lambda: render(view, shapes, 1024))]
    for name, density in DENSITIES.items():
        legacy = round(LEGACY_DP * density)
        adaptive = round(ADAPTIVE_DP * density)
        folder = RES / f"mipmap-{name}"
        jobs.append((folder / "ic_launcher.png", legacy, True,
                     lambda s=legacy: mask(render(view, shapes, s), s, LEGACY_CORNER)))
        jobs.append((folder / "ic_launcher_round.png", legacy, True,
                     lambda s=legacy: mask(render(view, shapes, s), s, 0.5)))
        jobs.append((folder / "ic_launcher_foreground.png", adaptive, True,
                     lambda s=adaptive: render(view, shapes, s, ADAPTIVE_ART_DP / ADAPTIVE_DP,
                                               skip=("background",))))
    return jobs


def main(argv):
    check = argv[1:] == ["--check"]
    if argv[1:] not in ([], ["--check"]):
        print("usage: tools/icon/render.py [--check]", file=sys.stderr)
        return 2
    view, shapes = parse_svg(SVG)
    if not any(shape["id"] == "background" for shape in shapes):
        raise ValueError('the icon needs a <rect id="background"> for the adaptive layers')
    failures = 0
    for path, size, alpha, draw in outputs(view, shapes):
        raw = to_bytes(draw(), size, alpha)
        rel = path.relative_to(ROOT)
        if not check:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(png_bytes(size, alpha, raw))
            print(f"wrote {rel} {size}x{size} {'RGBA' if alpha else 'RGB'}")
            continue
        try:
            width, height, channels, pixels = read_png(path)
        except (OSError, ValueError) as error:
            print(f"FAIL {rel}: {error}")
            failures += 1
            continue
        expected = (size, size, 4 if alpha else 3)
        if (width, height, channels) != expected:
            print(f"FAIL {rel}: {width}x{height}x{channels}, expected {expected[0]}x{expected[1]}x{expected[2]}")
            failures += 1
            continue
        stride = size * channels
        unfiltered = b"".join(raw[j * (stride + 1) + 1:(j + 1) * (stride + 1)] for j in range(size))
        diff = max((abs(a - b) for a, b in zip(unfiltered, pixels)), default=0)
        if diff > 1:
            print(f"FAIL {rel}: pixels differ from the SVG by up to {diff}")
            failures += 1
        else:
            print(f"ok {rel} {size}x{size} {'RGBA' if alpha else 'RGB, no alpha'}")
    failures += check_background(shapes)
    return 1 if failures else 0


def check_background(shapes):
    """The adaptive background drawable repeats the SVG background gradient."""
    path = RES / "drawable/ic_launcher_background.xml"
    android = "{http://schemas.android.com/apk/res/android}"
    gradient = ET.parse(path).getroot().find("gradient")
    stops = next(shape for shape in shapes if shape["id"] == "background")["fill"]["stops"]
    want = [stops[0][1], stops[-1][1]]
    got = [parse_color(gradient.get(android + "startColor")),
           parse_color(gradient.get(android + "endColor"))]
    rel = path.relative_to(ROOT)
    if gradient.get(android + "angle") != "270" or got != want:
        print(f"FAIL {rel}: expected a top-to-bottom gradient matching the SVG background")
        return 1
    print(f"ok {rel}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
