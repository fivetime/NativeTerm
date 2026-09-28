#!/usr/bin/env python3
"""The pictures of crates/native-term-app/assets/os, from their sources.

    render-os-logos.py <gilbarbara/logos clone> <font-logos clone> <folder to write into>

Needs inkscape (the SVGs are drawn by it, 384 pixels at their longer
side) and Pillow (they are brought to 96 x 96 with the logo in the middle,
what is around it transparent). NativeTerm brings them to the size a row
shows them at when it runs.

A logo that comes in its own colours is kept as it is. One that is a
shape only (font-logos) is written white, and NativeTerm gives it a
colour when it draws it.
"""
import os
import subprocess
import sys
import tempfile

from PIL import Image

SIDE = 96
# name here: (which source, its file, a shape only)
LOGOS = {
    "ubuntu": ("gilbarbara", "logos/ubuntu.svg", False),
    "debian": ("gilbarbara", "logos/debian.svg", False),
    "freebsd": ("gilbarbara", "logos/freebsd.svg", False),
    "raspbian": ("gilbarbara", "logos/raspberry-pi.svg", False),
    "windows": ("gilbarbara", "logos/microsoft-windows-icon.svg", False),
    "deepin": ("font-logos", "vectors/deepin.svg", True),
    "kali": ("font-logos", "vectors/kali-linux.svg", True),
}


def main():
    gilbarbara, font_logos, out = sys.argv[1:4]
    roots = {"gilbarbara": gilbarbara, "font-logos": font_logos}
    os.makedirs(out, exist_ok=True)
    for name, (source, path, shape) in sorted(LOGOS.items()):
        svg = os.path.join(roots[source], path)
        with tempfile.TemporaryDirectory() as tmp:
            big = os.path.join(tmp, "big.png")
            probe = subprocess.run(
                ["inkscape", svg, "--query-width", "--query-height", "--export-area-drawing"],
                capture_output=True,
                text=True,
                check=True,
            ).stdout.split()
            wide = float(probe[0]) >= float(probe[1])
            side = ["--export-width=384"] if wide else ["--export-height=384"]
            subprocess.run(
                ["inkscape", svg, "--export-area-drawing", "--export-type=png", "--export-filename=" + big] + side,
                capture_output=True,
                check=True,
            )
            image = Image.open(big).convert("RGBA")
        image.thumbnail((SIDE, SIDE), Image.LANCZOS)
        if shape:
            white = Image.new("RGBA", image.size, (255, 255, 255, 0))
            white.putalpha(image.getchannel("A"))
            image = white
        page = Image.new("RGBA", (SIDE, SIDE), (0, 0, 0, 0))
        page.paste(image, ((SIDE - image.width) // 2, (SIDE - image.height) // 2))
        page.save(os.path.join(out, name + ".png"), optimize=True)
        print("%-10s %-11s %s -> %dx%d" % (name, source, path, image.width, image.height))


main()
