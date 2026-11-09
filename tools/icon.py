# SPDX-License-Identifier: MIT
"""An app's icon for the card's launcher: any picture Pillow reads, made 44 x 44 and saved as QOI (qoiformat.org) with
its transparency. The launcher rounds its corners. It goes in the app's folder as ICON.QOI, or in apps/<name>/ for
make card."""
import argparse
from pathlib import Path
import struct
import sys

SIZE = 44


def qoi(width, height, rgba):
    """QOI 1.0, written from the specification."""
    out = bytearray(b'qoif'+struct.pack('>II', width, height)+bytes([4, 0]))
    index = [(0, 0, 0, 0)]*64
    previous, run = (0, 0, 0, 255), 0
    pixels = [tuple(rgba[i:i+4]) for i in range(0, len(rgba), 4)]
    for n, px in enumerate(pixels):
        if px == previous:
            run += 1
            if run == 62 or n == len(pixels)-1:
                out.append(0xc0 | (run-1))
                run = 0
            continue
        if run:
            out.append(0xc0 | (run-1))
            run = 0
        h = (px[0]*3+px[1]*5+px[2]*7+px[3]*11) % 64
        if index[h] == px:
            out.append(h)
        else:
            index[h] = px
            if px[3] != previous[3]:
                out += bytes([0xff, *px])
            else:
                dr, dg, db = ((px[i]-previous[i]+128) % 256-128 for i in range(3))
                if -2 <= dr <= 1 and -2 <= dg <= 1 and -2 <= db <= 1:
                    out.append(0x40 | (dr+2) << 4 | (dg+2) << 2 | (db+2))
                elif -32 <= dg <= 31 and -8 <= dr-dg <= 7 and -8 <= db-dg <= 7:
                    out += bytes([0x80 | (dg+32), (dr-dg+8) << 4 | (db-dg+8)])
                else:
                    out += bytes([0xfe, *px[:3]])
        previous = px
    return bytes(out+bytes(7)+b'\x01')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('picture', type=Path)
    parser.add_argument('icon', type=Path, help="ICON.QOI")
    args = parser.parse_args()
    try:
        from PIL import Image
    except ImportError:
        sys.exit('icon.py needs Pillow: pip install pillow')
    picture = Image.open(args.picture).convert('RGBA').resize((SIZE, SIZE), Image.LANCZOS)
    args.icon.write_bytes(qoi(SIZE, SIZE, picture.tobytes()))


if __name__ == '__main__':
    main()
