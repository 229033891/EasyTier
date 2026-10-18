#!/usr/bin/env python3
"""Regenerate the GUI tray icons (4 states x 7 sizes) from one alpha silhouette.

Why this exists: the tray logo is a flat single-colour glyph, so a state is just
a colour. The original icons shipped as hand-made .ico files, which drifted --
`tray-running.ico` ended up with a single 256px entry while its siblings had
7 sizes, so Windows rescaled it and the green icon did not match the others.

The silhouette is taken from an existing icon (all states share it pixel for
pixel), recoloured by replacing RGB and keeping the alpha channel, then written
as a multi-size ICO with PNG payloads -- the same container layout the original
icons used, which the Windows shell and the `image` crate both decode.

Usage:
    python script/gen-tray-icons.py            # rewrite src-tauri/icons/tray-*.ico
    python script/gen-tray-icons.py --check    # report only, exit 1 if stale
"""

from __future__ import annotations

import argparse
import io
import os
import struct
import sys

from PIL import Image

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ICON_DIR = os.path.join(REPO_ROOT, "easytier-gui", "src-tauri", "icons")
SILHOUETTE = os.path.join(ICON_DIR, "tray-idle.ico")

SIZES = (16, 24, 32, 48, 64, 128, 256)

# Keep in sync with the palette in docs (grey idle / yellow connecting /
# green running / red needs attention).
STATES = {
    "tray-idle.ico": "#9AA0A6",
    "tray-connecting.ico": "#FFCF56",
    "tray-running.ico": "#299C46",
    "tray-error.ico": "#D64545",
}


def load_silhouette(path: str) -> Image.Image:
    """Largest frame of `path`, RGBA."""
    image = Image.open(path)
    image.size = (256, 256)
    return image.convert("RGBA")


def recolor(silhouette: Image.Image, hex_color: str) -> Image.Image:
    r, g, b = (int(hex_color[i : i + 2], 16) for i in (1, 3, 5))
    solid = Image.new("RGBA", silhouette.size, (r, g, b, 255))
    solid.putalpha(silhouette.split()[-1])
    return solid


def encode_ico(image: Image.Image, sizes=SIZES) -> bytes:
    entries: list[tuple[int, bytes]] = []
    for size in sizes:
        buf = io.BytesIO()
        image.resize((size, size), Image.LANCZOS).save(buf, format="PNG")
        entries.append((size, buf.getvalue()))

    header = struct.pack("<HHH", 0, 1, len(entries))
    offset = 6 + 16 * len(entries)
    directory = b""
    payload = b""
    for size, data in entries:
        dimension = 0 if size == 256 else size
        directory += struct.pack(
            "<BBBBHHII", dimension, dimension, 0, 0, 1, 32, len(data), offset
        )
        payload += data
        offset += len(data)
    return header + directory + payload


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="report stale icons only")
    args = parser.parse_args()

    silhouette = load_silhouette(SILHOUETTE)
    stale = 0
    for name, color in STATES.items():
        target = os.path.join(ICON_DIR, name)
        data = encode_ico(recolor(silhouette, color))
        current = open(target, "rb").read() if os.path.exists(target) else b""
        if current == data:
            print(f"  ok    {name}  {color}  {len(data)} bytes")
            continue
        stale += 1
        if args.check:
            print(f"  stale {name}  {color}  ({len(current)} -> {len(data)} bytes)")
            continue
        with open(target, "wb") as handle:
            handle.write(data)
        print(f"  wrote {name}  {color}  {len(data)} bytes")

    if args.check and stale:
        print(f"{stale} tray icon(s) stale; rerun without --check", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
