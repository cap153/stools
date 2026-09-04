#!/usr/bin/env python3
"""Rasterize `assets/icon.svg` into the bitmaps stools ships.

Outputs (regenerate after editing the SVG):

  assets/icon.png  64x64 RGBA — embedded with include_bytes!() as the Windows
                   tray icon (see `load_tray_icon` in src/platform/windows.rs).
  assets/icon.ico  multi-size Windows icon (16/24/32/48/256) for the .exe.

Why 64 for the tray bitmap: Windows asks the shell for a single bitmap and
scales it to whatever the taskbar needs. 64 divides cleanly to 32 (200% DPI)
and 16 (100% DPI), so both land on whole-pixel steps rather than a
fractional resample that would smear the thin purple border.

Backends are probed in order, so this script runs unchanged on a Linux box
(rsvg-convert + ImageMagick) and on Windows (cairosvg + Pillow wheels):

  SVG -> raster : cairosvg | rsvg-convert | magick
  resize / ICO  : Pillow    | magick

Usage:  python3 tools/build_icon.py
"""

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SVG = ROOT / "assets" / "icon.svg"
PNG = ROOT / "assets" / "icon.png"
ICO = ROOT / "assets" / "icon.ico"

# Oversample the vector well above the largest output, then Lanczos down:
# a 512 render keeps the rounded corners and the 14px border smooth at 64.
RASTER = 512
TRAY = 64
ICO_SIZES = (16, 24, 32, 48, 256)


def have(cmd: str) -> bool:
    return shutil.which(cmd) is not None


def run(argv: list[str]) -> None:
    subprocess.run(argv, check=True)


def die(msg: str) -> "NoReturn":  # type: ignore[valid-type]
    sys.exit(f"build_icon: {msg}")


def rasterize_svg(dest: Path) -> str:
    """Render the SVG to a RASTER x RASTER PNG with a transparent background."""
    try:
        import cairosvg  # type: ignore

        cairosvg.svg2png(
            url=str(SVG),
            write_to=str(dest),
            output_width=RASTER,
            output_height=RASTER,
        )
        return "cairosvg"
    except ImportError:
        pass

    if have("rsvg-convert"):
        run(
            [
                "rsvg-convert",
                "-w", str(RASTER),
                "-h", str(RASTER),
                "-o", str(dest),
                str(SVG),
            ]
        )
        return "rsvg-convert"

    if have("magick"):
        # `-background none` keeps the corners transparent instead of white.
        run(
            [
                "magick",
                "-background", "none",
                str(SVG),
                "-resize", f"{RASTER}x{RASTER}",
                str(dest),
            ]
        )
        return "magick"

    die(
        "no SVG rasterizer found — install one of: "
        "`pip install cairosvg`, rsvg-convert (librsvg), ImageMagick"
    )


def write_outputs(raster: Path) -> str:
    """Downsample to the tray PNG and pack the multi-size ICO."""
    try:
        from PIL import Image  # type: ignore
    except ImportError:
        pass
    else:
        img = Image.open(raster).convert("RGBA")
        img.resize((TRAY, TRAY), Image.LANCZOS).save(PNG)
        img.save(ICO, format="ICO", sizes=[(s, s) for s in ICO_SIZES])
        return "Pillow"

    if have("magick"):
        run(["magick", str(raster), "-filter", "Lanczos", "-resize", f"{TRAY}x{TRAY}", str(PNG)])
        run(
            [
                "magick",
                str(raster),
                "-define",
                f"icon:auto-resize={','.join(str(s) for s in ICO_SIZES)}",
                str(ICO),
            ]
        )
        return "ImageMagick"

    die("no image backend found — install `pip install pillow` or ImageMagick")


def main() -> None:
    if not SVG.exists():
        die(f"missing {SVG}")

    ROOT.joinpath("assets").mkdir(exist_ok=True)

    with tempfile.TemporaryDirectory() as tmp:
        raster = Path(tmp) / "icon-raster.png"
        svg_backend = rasterize_svg(raster)
        image_backend = write_outputs(raster)

    print(f"built {PNG.relative_to(ROOT)} ({TRAY}x{TRAY}) via {svg_backend} + {image_backend}")
    print(f"built {ICO.relative_to(ROOT)} (sizes: {', '.join(map(str, ICO_SIZES))})")


if __name__ == "__main__":
    main()
