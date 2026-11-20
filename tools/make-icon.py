"""Deskboxes app icon (Fluent style): two boxes, the front one with a title band and icons.

Draws every size from vector shapes (supersampled), simpler at small sizes, and writes
res/deskboxes.ico (16..256 px) plus res/deskboxes-256.png. Run: python tools/make-icon.py
"""
import os
from PIL import Image, ImageDraw, ImageFilter

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "res")


def lerp(a, b, t):
    return tuple(int(round(a[i] + (b[i] - a[i]) * t)) for i in range(len(a)))


def gradient(size, top, bottom):
    """Vertical gradient image (RGBA)."""
    w, h = size
    g = Image.new("RGBA", (1, h))
    for y in range(h):
        g.putpixel((0, y), lerp(top, bottom, y / max(1, h - 1)))
    return g.resize((w, h))


def rounded(size, box, r):
    m = Image.new("L", size, 0)
    ImageDraw.Draw(m).rounded_rectangle(box, r, fill=255)
    return m


def draw(px):
    ss = 8 if px <= 48 else 4
    S = px * ss
    u = S / 256.0  # design units: 256 grid
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    small = px <= 24

    # Back box (up and to the right), lighter.
    bb = (int(92 * u), int(28 * u), int(236 * u), int(160 * u))
    br = int(26 * u)
    back = gradient((S, S), (148, 214, 255, 255), (86, 170, 240, 255))
    img.paste(back, (0, 0), rounded((S, S), bb, br))

    # Front box shadow.
    fb = (int(20 * u), int(76 * u), int(196 * u), int(232 * u))
    fr = int(28 * u)
    sh = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(sh).rounded_rectangle((fb[0], fb[1] + int(6 * u), fb[2], fb[3] + int(6 * u)), fr, fill=(10, 40, 90, 110))
    sh = sh.filter(ImageFilter.GaussianBlur(10 * u))
    img.alpha_composite(sh)

    # Front box: body gradient, darker title band.
    mask = rounded((S, S), fb, fr)
    body = gradient((S, S), (52, 146, 245, 255), (18, 96, 214, 255))
    img.paste(body, (0, 0), mask)
    band_h = int(46 * u)
    band = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(band).rectangle((fb[0], fb[1], fb[2], fb[1] + band_h), fill=(8, 60, 160, 255))
    bm = Image.new("L", (S, S), 0)
    bm.paste(mask, (0, 0))
    band.putalpha(Image.composite(band.getchannel("A"), Image.new("L", (S, S), 0), mask))
    img.alpha_composite(band)
    # A subtle highlight along the top edge (Fluent depth).
    hl = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(hl).rounded_rectangle(fb, fr, outline=(255, 255, 255, 70), width=max(1, int(3 * u)))
    img.alpha_composite(hl)

    # Title text line in the band.
    if not small:
        ImageDraw.Draw(img).rounded_rectangle(
            (fb[0] + int(26 * u), fb[1] + int(19 * u), fb[0] + int(96 * u), fb[1] + int(29 * u)), int(5 * u), fill=(255, 255, 255, 200)
        )
    # Icons in the body: 2 x 2 tiles (one at the smallest sizes).
    tiles = [(0, 0), (1, 0), (0, 1), (1, 1)] if not small else [(0, 0), (1, 0)]
    tw = 40 if not small else 58
    gap = 14 if not small else 20
    rows = 2 if not small else 1
    x0 = fb[0] + (fb[2] - fb[0] - (2 * tw + gap) * u) / 2
    # Centered in the body (below the band), the same gap across and down.
    body_h = fb[3] - fb[1] - band_h
    y0 = fb[1] + band_h + (body_h - (rows * tw + (rows - 1) * gap) * u) / 2
    colors = [(255, 255, 255, 240), (205, 236, 255, 235), (205, 236, 255, 235), (255, 255, 255, 240)]
    for i, (cx, cy) in enumerate(tiles):
        x = x0 + cx * (tw + gap) * u
        y = y0 + cy * (tw + gap) * u
        ImageDraw.Draw(img).rounded_rectangle((int(x), int(y), int(x + tw * u), int(y + tw * u)), int(12 * u), fill=colors[i])
    return img.resize((px, px), Image.LANCZOS)


def main():
    os.makedirs(OUT, exist_ok=True)
    sizes = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]
    imgs = [draw(s) for s in sizes]
    imgs[-1].save(os.path.join(OUT, "deskboxes-256.png"))
    imgs[-1].save(os.path.join(OUT, "deskboxes.ico"), sizes=[(s, s) for s in sizes], append_images=imgs[:-1])
    # Preview strip for checking by eye.
    strip = Image.new("RGBA", (sum(sizes) + 10 * len(sizes), 256), (40, 40, 40, 255))
    x = 0
    for s, im in zip(sizes, imgs):
        strip.alpha_composite(im, (x, 256 - s))
        x += s + 10
    strip.save(os.path.join(OUT, "preview.png"))
    print("wrote", OUT)


if __name__ == "__main__":
    main()
