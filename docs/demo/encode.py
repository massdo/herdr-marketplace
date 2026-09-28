#!/usr/bin/env python3
"""Encodes the frames render.mjs saved into the README's looping WebP.

Lossy frames keep the camera moves light. With allow_mixed, libwebp stores a
frame losslessly when that is smaller, as small interface changes usually
are, and a keyframe every 10 to 20 frames clears the faint trails lossy
frames leave behind on a dark background.

    python3 encode.py FRAMES_DIR OUTPUT.webp FPS
"""

import sys
from pathlib import Path

from PIL import Image


def main(frames_dir, output, fps):
    frames = [Image.open(path) for path in sorted(Path(frames_dir).glob("*.png"))]
    frames[0].save(
        output, save_all=True, append_images=frames[1:], duration=round(1000 / fps), loop=0,
        lossless=False, quality=80, method=4, allow_mixed=True, kmin=10, kmax=20,
    )
    print(f"wrote {output} ({Path(output).stat().st_size / 1e6:.1f} MB)")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], float(sys.argv[3]))
