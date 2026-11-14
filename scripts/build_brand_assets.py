#!/usr/bin/env python3
"""Regenerate Windows artwork from the original Raster logo (requires Pillow)."""
from pathlib import Path

from PIL import Image, ImageOps

ASSETS = Path(__file__).resolve().parents[1] / "assets"


def main():
    original = Image.open(ASSETS / "raster-logo.png").convert("RGB")
    # Keep the entire height and its glow; remove only excess horizontal black
    # padding. The original artwork remains untouched in raster-logo.png.
    side = min(original.size)
    left = (original.width - side) // 2
    top = (original.height - side) // 2
    square = original.crop((left, top, left + side, top + side))
    square.save(
        ASSETS / "raster.ico",
        sizes=[(n, n) for n in (16, 24, 32, 48, 64, 128, 256)],
    )
    for name, size in (("installer-banner.bmp", (164, 314)), ("installer-small.bmp", (55, 55))):
        canvas = Image.new("RGB", size, "black")
        image = ImageOps.contain(square, size, Image.Resampling.LANCZOS)
        canvas.paste(image, ((size[0] - image.width) // 2, (size[1] - image.height) // 2))
        canvas.save(ASSETS / name)


if __name__ == "__main__":
    main()
