#!/usr/bin/env python3
"""Make the probe's test textures: one solid colour per bake channel the test outfit paints, as the
raw JPEG 2000 codestreams (.j2k) Second Life and OpenSim store textures as.

Needs Pillow built with OpenJPEG. Run from this folder: python3 make-textures.py
"""
from pathlib import Path

from PIL import Image

SIZE = (128, 128)
COLOURS = {
    "aux1": (255, 0, 0),
    "aux2": (0, 255, 0),
    "aux3": (0, 0, 255),
    "leftarm": (255, 255, 0),
    "leftleg": (255, 0, 255),
    "skirt": (0, 255, 255),
}

out = Path(__file__).resolve().parent / "textures"
out.mkdir(exist_ok=True)
for name, colour in COLOURS.items():
    # no_jp2: the bare codestream, not the JP2 file format around it
    Image.new("RGB", SIZE, colour).save(out / f"{name}.j2k", "JPEG2000", no_jp2=True, irreversible=False)
    print(name, (out / f"{name}.j2k").stat().st_size, "bytes")
