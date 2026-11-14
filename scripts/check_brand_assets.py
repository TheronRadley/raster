#!/usr/bin/env python3
"""Check checked-in Windows icon and installer bitmap structure; no Pillow needed."""
import struct
from pathlib import Path

ASSETS = Path(__file__).resolve().parents[1] / "assets"


def main():
    logo = (ASSETS / "raster-logo.png").read_bytes()
    assert logo.startswith(b"\x89PNG\r\n\x1a\n"), "invalid source PNG"
    icon = (ASSETS / "raster.ico").read_bytes()
    reserved, kind, count = struct.unpack_from("<HHH", icon)
    assert (reserved, kind, count) == (0, 1, 7), "invalid ICO header"
    sizes = set()
    for index in range(count):
        width, height, _, _, _, _, size, offset = struct.unpack_from("<BBBBHHII", icon, 6 + index * 16)
        width, height = width or 256, height or 256
        assert width == height and offset >= 6 + count * 16 and offset + size <= len(icon)
        assert icon[offset:offset + 8] == b"\x89PNG\r\n\x1a\n", "missing icon image"
        png_width, png_height = struct.unpack_from(">II", icon, offset + 16)
        assert (png_width, png_height) == (width, height), "icon dimensions mismatch"
        sizes.add(width)
    assert sizes == {16, 24, 32, 48, 64, 128, 256}, "missing icon sizes"
    for name, dimensions in (("installer-banner.bmp", (164, 314)), ("installer-small.bmp", (55, 55))):
        data = (ASSETS / name).read_bytes()
        assert data[:2] == b"BM" and len(data) >= 54, f"invalid bitmap: {name}"
        assert struct.unpack_from("<ii", data, 18) == dimensions, f"wrong bitmap size: {name}"
    print("Brand assets OK")


if __name__ == "__main__":
    main()
