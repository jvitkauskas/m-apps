# SPDX-License-Identifier: MIT
"""art.rs: usbdisk's pictures, drawn here with Pillow at four times their size and scaled down, so their edges are
smooth. `python3 apps/usbdisk/art.py`. Each sprite is RGB565 with 4-bit alpha."""
from pathlib import Path
import sys
from PIL import Image, ImageDraw

HERE = Path(__file__).parent
S = 4                       # drawn at this many times the size

INK = (46, 58, 74)
INK_LIGHT = (70, 86, 106)
WHITE = (255, 255, 255)
BLUE = (56, 136, 228)
BLUE_LIGHT = (120, 180, 240)
SILVER = (196, 206, 218)
SILVER_DARK = (150, 162, 176)
SCREEN = (236, 243, 251)
GREEN = (52, 176, 104)
RED = (214, 72, 64)


def canvas(w, h):
    return Image.new('RGBA', (w*S, h*S), (0, 0, 0, 0))


def box(d, x0, y0, x1, y1, r, fill):
    d.rounded_rectangle((x0*S, y0*S, x1*S-1, y1*S-1), radius=r*S, fill=fill)


def sd_card():
    """A card face on: a dark body with its corner cut, grip lines, and a white label with a blue band."""
    w, h = 50, 64
    im = canvas(w, h)
    d = ImageDraw.Draw(im)
    cut = 13
    body = [(4, 0), (w-cut, 0), (w, cut), (w, h-4), (w-4, h), (4, h), (0, h-4), (0, 4)]
    d.polygon([(x*S, y*S) for x, y in body], fill=INK)
    for cx, cy in ((4, 4), (4, h-4), (w-4, h-4)):
        d.ellipse(((cx-4)*S, (cy-4)*S, (cx+4)*S, (cy+4)*S), fill=INK)
    for i in range(6):
        x = 9+i*5
        box(d, x, 4, x+2, 13, 1, INK_LIGHT)
    box(d, 6, 24, w-6, h-6, 3, WHITE)
    box(d, 6, 24, w-6, 33, 3, BLUE)
    d.rectangle((6*S, 30*S, (w-6)*S-1, 33*S-1), fill=BLUE)
    box(d, 11, 40, w-11, 43, 1, SILVER)
    box(d, 11, 47, w-18, 50, 1, SILVER)
    return im


def laptop():
    """A laptop, open: a dark lid with a light screen, on a silver base."""
    w, h = 100, 66
    im = canvas(w, h)
    d = ImageDraw.Draw(im)
    box(d, 10, 0, 90, 56, 5, INK)
    box(d, 14, 4, 86, 52, 2, SCREEN)
    box(d, 0, 56, 100, 64, 3, SILVER)
    d.rectangle((0, 56*S, 100*S-1, 59*S), fill=SILVER)
    box(d, 40, 56, 60, 58, 1, SILVER_DARK)
    box(d, 4, 63, 96, 66, 2, SILVER_DARK)
    return im


def folder(colour, tab):
    w, h = 30, 24
    im = canvas(w, h)
    d = ImageDraw.Draw(im)
    box(d, 0, 0, 13, 6, 2, tab)
    box(d, 0, 3, 30, 24, 3, colour)
    box(d, 0, 7, 30, 24, 3, tab)
    box(d, 0, 9, 30, 24, 3, colour)
    return im


def check():
    w, h = 26, 26
    im = canvas(w, h)
    d = ImageDraw.Draw(im)
    d.ellipse((0, 0, w*S-1, h*S-1), fill=GREEN)
    d.line([(7*S, 13*S), (11*S, 17*S), (19*S, 9*S)], fill=WHITE, width=3*S, joint='curve')
    return im


def cross():
    w, h = 26, 26
    im = canvas(w, h)
    d = ImageDraw.Draw(im)
    d.ellipse((0, 0, w*S-1, h*S-1), fill=RED)
    d.line([(8*S, 8*S), (18*S, 18*S)], fill=WHITE, width=3*S)
    d.line([(18*S, 8*S), (8*S, 18*S)], fill=WHITE, width=3*S)
    return im


SPRITES = (('sd_card', sd_card), ('laptop', laptop), ('folder', lambda: folder(BLUE, BLUE_LIGHT)),
           ('folder_dim', lambda: folder(SILVER, SILVER_DARK)), ('check', check), ('cross', cross))


def small(im):
    return im.resize((im.width//S, im.height//S), Image.LANCZOS)


def emit(name, im):
    im = small(im)
    w, h = im.size
    colours, alphas = [], []
    for r, g, b, a in im.getdata():
        colours.append((r >> 3) << 11 | (g >> 2) << 5 | b >> 3)
        alphas.append((a+8)//17)
    # alpha two pixels to a byte, high nibble first
    packed = [alphas[i] << 4 | (alphas[i+1] if i+1 < len(alphas) else 0) for i in range(0, len(alphas), 2)]

    def rows(vals, n, fmt):
        return ',\n    '.join(', '.join(fmt.format(v) for v in vals[i:i+n]) for i in range(0, len(vals), n))
    upper = name.upper()
    return [f'pub const {upper}_W: i32 = {w};', f'pub const {upper}_H: i32 = {h};',
            f'pub static {upper}_RGB: [u16; {w*h}] = [\n    {rows(colours, w, "0x{:04x}")},\n];',
            f'pub static {upper}_ALPHA: [u8; {len(packed)}] = [\n    {rows(packed, 24, "0x{:02x}")},\n];']


def main():
    out = ['// SPDX-License-Identifier: MIT', '//! Made by art.py.']
    for name, make in SPRITES:
        out += emit(name, make())
    out += ["// Where the laptop's screen is in it, and where the cable meets each picture, from its top.",
            'pub const LAPTOP_SCREEN_X: i32 = 14;', 'pub const LAPTOP_SCREEN_Y: i32 = 4;', 'pub const LAPTOP_SCREEN_W: i32 = 72;',
            'pub const LAPTOP_SCREEN_H: i32 = 48;', 'pub const LAPTOP_PORT_Y: i32 = 60;', 'pub const SD_CARD_PORT_Y: i32 = 56;']
    (HERE/'art.rs').write_text('\n'.join(out)+'\n')
    if len(sys.argv) > 1:                      # a preview of them all, for looking at
        sheet = Image.new('RGBA', (270, 76), (240, 244, 250, 255))
        x = 4
        for _, make in SPRITES:
            im = small(make())
            sheet.alpha_composite(im, (x, 4))
            x += im.width+6
        sheet.resize((sheet.width*3, sheet.height*3), Image.NEAREST).save(sys.argv[1])


if __name__ == '__main__':
    main()
