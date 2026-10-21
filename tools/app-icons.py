#!/usr/bin/env python3
"""Bundle the map's app icons for the display server (SHELL.md: an app is its icon).

Each app's icon comes from its own codebase, in whatever format that codebase keeps it: PNG, Android WebP, or VSF (decoded first by tools/vsficon, since only fluor reads VSF). Every one is brought to the same square size and cut to the same circle here, once, so the display server only ever blits one raw format and decodes nothing.

Each icon also gets its grey, for the map at rest (colour there means only "changed"). The grey is the icon's luminance: decoded to linear light, weighted by the Rec. 709 primaries (sRGB's own, so the weights fit the source), and encoded again. Weighting the encoded values instead — the video shortcut — puts saturated colours far too dark: pure blue becomes grey 18 where its lightness is 77.

Output: target/icons/icons.bin = b'ICN2', count (u8), size (u16 LE), then per icon size x size RGBA (straight alpha) followed by size x size grey, one byte each. The order is the map's (payloads/map): icon n is the n-th app, and 0 means no icon.
Also writes target/icons/sheet.png, the colours over the greys, for checking by eye.
"""
import struct, sys
from pathlib import Path
from PIL import Image, ImageChops, ImageDraw

CODE = Path("/mnt/Harbor/Code")
REPO = Path(__file__).resolve().parent.parent
OUT = REPO / "target" / "icons"
SIZE = 128

# The map's apps, in its order, and where each keeps its icon.
APPS = [
    ("Photon", CODE / "photon/assets/icon-128.png"),
    ("Mahere", CODE / "mahere/installers/icon-256.png"),
    ("Lumis", CODE / "lumis/app/src/main/res/mipmap-xxxhdpi/ic_launcher_round.webp"),
    ("Opsin", OUT / "opsin.rgba"),  # tools/vsficon opsin/assets/opsin_orb.vsf
]

def load(path: Path) -> Image.Image:
    if path.suffix == ".rgba":
        raw = path.read_bytes()
        w, h = struct.unpack_from("<II", raw, 0)
        return Image.frombytes("RGBA", (w, h), raw[8:8 + w * h * 4])
    return Image.open(path).convert("RGBA")

def srgb_to_linear(v: float) -> float:
    return v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4

def linear_to_srgb(y: float) -> float:
    return y * 12.92 if y <= 0.0031308 else 1.055 * y ** (1 / 2.4) - 0.055

LIN = [srgb_to_linear(i / 255) for i in range(256)]

def grey(img: Image.Image) -> Image.Image:
    """The icon's luminance as an encoded grey."""
    out = bytearray()
    for r, g, b, _ in img.getdata():
        y = 0.2126 * LIN[r] + 0.7152 * LIN[g] + 0.0722 * LIN[b]
        out.append(round(255 * linear_to_srgb(y)))
    return Image.frombytes("L", img.size, bytes(out))

def circle(img: Image.Image) -> Image.Image:
    """Square it from the centre, scale to SIZE, and cut the circle with an antialiased edge (drawn at four times the size and reduced)."""
    w, h = img.size
    s = min(w, h)
    img = img.crop(((w - s) // 2, (h - s) // 2, (w - s) // 2 + s, (h - s) // 2 + s)).resize((SIZE, SIZE), Image.LANCZOS)
    big = Image.new("L", (SIZE * 4, SIZE * 4), 0)
    ImageDraw.Draw(big).ellipse((0, 0, SIZE * 4 - 1, SIZE * 4 - 1), fill=255)
    mask = big.resize((SIZE, SIZE), Image.LANCZOS)
    r, g, b, a = img.split()
    # The icon's own alpha times the circle's: an icon that is already round keeps its own edge, a square one gets the circle's.
    return Image.merge("RGBA", (r, g, b, ImageChops.multiply(a, mask)))

OUT.mkdir(parents=True, exist_ok=True)
icons = []
for name, path in APPS:
    if not path.exists():
        sys.exit(f"app-icons: no icon for {name} at {path}")
    icons.append(circle(load(path)))
    print(f"  {name}: {path} -> {SIZE} x {SIZE}")
greys = [grey(i) for i in icons]
blob = b"ICN2" + struct.pack("<BH", len(icons), SIZE) + b"".join(i.tobytes() + g.tobytes() for i, g in zip(icons, greys))
(OUT / "icons.bin").write_bytes(blob)
sheet = Image.new("RGBA", (SIZE * len(icons) + 8 * (len(icons) + 1), 2 * SIZE + 24), (0, 0, 0, 255))
for k, (i, g) in enumerate(zip(icons, greys)):
    sheet.alpha_composite(i, (8 + k * (SIZE + 8), 8))
    sheet.alpha_composite(Image.merge("RGBA", (g, g, g, i.getchannel("A"))), (8 + k * (SIZE + 8), SIZE + 16))
sheet.save(OUT / "sheet.png")
print(f"app-icons: {len(icons)} icons, {len(blob)} bytes -> {OUT / 'icons.bin'}")
