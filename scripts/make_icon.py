"""Generates res/mcommander.ico and res/mcommander-256.png (requires Pillow).

The icon shows two file panels on a blue tile, with a highlighted cursor row
and a folder tab - MCommander's own artwork.
"""
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
S = 1024  # draw large, then downsample for crisp edges


def draw() -> Image.Image:
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    # Tile with a vertical gradient.
    tile = Image.new("RGBA", (S, S))
    td = ImageDraw.Draw(tile)
    top, bottom = (0x2F, 0x86, 0xE6), (0x13, 0x4C, 0xA8)
    for y in range(S):
        t = y / (S - 1)
        td.line([(0, y), (S, y)], fill=tuple(int(a + (b - a) * t) for a, b in zip(top, bottom)) + (255,))
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle([40, 40, S - 40, S - 40], radius=190, fill=255)
    img.paste(tile, (0, 0), mask)

    # Two panels.
    pw, ph, gap = 400, 600, 44
    x0 = (S - (2 * pw + gap)) // 2
    y0 = 270
    for i in range(2):
        x = x0 + i * (pw + gap)
        d.rounded_rectangle([x, y0, x + pw, y0 + ph], radius=36, fill=(255, 255, 255, 255))
        # Header bar of each panel.
        hdr = (0x99, 0xB4, 0xD1, 255) if i == 0 else (0xBF, 0xCD, 0xDB, 255)
        d.rounded_rectangle([x, y0, x + pw, y0 + 90], radius=36, fill=hdr)
        d.rectangle([x, y0 + 50, x + pw, y0 + 90], fill=hdr)
        # Rows.
        for r in range(5):
            y = y0 + 130 + r * 90
            if i == 0 and r == 1:
                d.rounded_rectangle([x + 18, y - 22, x + pw - 18, y + 46], radius=14, fill=(0xCC, 0xE8, 0xFF, 255),
                                    outline=(0x1E, 0x1E, 0x1E, 255), width=8)
            d.rounded_rectangle([x + 40, y - 4, x + 92, y + 30], radius=6, fill=(0xF4, 0xC4, 0x3A, 255))
            d.rounded_rectangle([x + 116, y + 4, x + pw - 50 - (r % 3) * 50, y + 22], radius=9,
                                fill=(0x9A, 0xA4, 0xB0, 255))
    # Folder tab on top, hinting at "commander of files".
    d.rounded_rectangle([x0, 150, x0 + 300, 250], radius=28, fill=(0xF4, 0xC4, 0x3A, 255))
    d.rounded_rectangle([x0 + 230, 190, x0 + 2 * pw + gap, 250], radius=24, fill=(0xF4, 0xC4, 0x3A, 255))
    return img


def main() -> None:
    big = draw()
    res = ROOT / "res"
    res.mkdir(exist_ok=True)
    png = big.resize((256, 256), Image.LANCZOS)
    png.save(res / "mcommander-256.png")
    sizes = [(16, 16), (20, 20), (24, 24), (32, 32), (40, 40), (48, 48), (64, 64), (128, 128), (256, 256)]
    big.resize((256, 256), Image.LANCZOS).save(res / "mcommander.ico", sizes=sizes)
    print("wrote", res / "mcommander.ico", "and", res / "mcommander-256.png")


if __name__ == "__main__":
    main()
