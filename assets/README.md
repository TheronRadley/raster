# Raster branding

`raster-logo.png` is the original supplied artwork. The Windows icon and Inno
Setup bitmap assets are derived from it, without changing the logo or removing
its glow. Their square crop removes only the excess horizontal black padding;
the complete image height is retained. The installer and app share the same
icon through `packaging/windows/raster.iss` and `build.rs`.

To regenerate the derivatives, install Pillow in a development environment and
run `python scripts/build_brand_assets.py` from the repository root. Pillow is
not needed to build or run Raster; the generated assets are checked in.
