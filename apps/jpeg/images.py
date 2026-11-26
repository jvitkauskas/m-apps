# SPDX-License-Identifier: MIT
"""The jpeg app's pictures, drawn here (nothing is copied), as apps/jpeg/images.rs: python3 apps/jpeg/images.py apps/jpeg
[REFERENCE_DIR]. With a second argument, each JPEG goes there too, to compare a decode with on a computer."""
import io, math, sys
from PIL import Image, ImageDraw, ImageFont

def picture(w, h):
    im = Image.new('RGB', (w, h))
    px = im.load()
    for y in range(h):
        for x in range(w):
            r = int(255*x/(w-1)); g = int(255*y/(h-1)); b = int(128+127*math.sin((x+y)/23))
            px[x, y] = (r, g, b)
    d = ImageDraw.Draw(im)
    d.ellipse((w*0.08, h*0.1, w*0.45, h*0.47), fill=(230, 40, 40), outline=(255, 255, 255), width=3)
    d.ellipse((w*0.55, h*0.12, w*0.92, h*0.49), fill=(40, 200, 60))
    d.rectangle((w*0.1, h*0.58, w*0.9, h*0.7), fill=(20, 20, 200))
    for i in range(8):                      # colour bars
        c = [(255,255,255),(255,255,0),(0,255,255),(0,255,0),(255,0,255),(255,0,0),(0,0,255),(0,0,0)][i]
        d.rectangle((w*i/8, h*0.86, w*(i+1)/8-1, h-1), fill=c)
    for x in range(int(w*0.1), int(w*0.9), 4):  # fine stripes, for high frequencies
        d.line((x, h*0.73, x, h*0.83), fill=(255, 255, 255))
    font = ImageFont.load_default(size=max(14, h//12))
    d.text((w*0.5, h*0.64), 'JPEG HW', fill=(255, 255, 255), anchor='mm', font=font)
    return im

def jpeg(im, **kw):
    b = io.BytesIO()
    im.save(b, 'JPEG', **kw)
    return b.getvalue()

small = picture(240, 240)
images = [
    ('420', jpeg(small, quality=85, subsampling=2)),
    ('422', jpeg(small, quality=85, subsampling=1)),
    ('444', jpeg(small, quality=85, subsampling=0)),
    ('420 OPTIMIZED', jpeg(small, quality=85, subsampling=2, optimize=True)),
    ('480X320', jpeg(picture(480, 320), quality=90, subsampling=2)),
]
out = sys.argv[1]
with open(out + '/images.rs', 'w') as f:
    f.write('// SPDX-License-Identifier: MIT\n//! Made by apps/jpeg/images.py: the pictures the app decodes, as JPEG files.\nuse kit::Aligned;\n\n')
    f.write('/// A picture: its name, for the log and the screen, and its file.\n'
            "pub struct Picture {\n    pub name: &'static str,\n    pub data: &'static [u8],\n}\n\n")
    for i, (name, data) in enumerate(images):
        f.write(f'static IMAGE{i}: Aligned<{len(data)}> = Aligned([\n')
        for at in range(0, len(data), 24):
            f.write('    ' + ', '.join(str(x) for x in data[at:at+24]) + ',\n')
        f.write(']);\n')
    f.write(f'\npub static IMAGES: [Picture; {len(images)}] = [\n')
    for i, (name, data) in enumerate(images):
        f.write(f'    Picture {{ name: "{name}", data: &IMAGE{i}.0 }},\n')
    f.write('];\n')
for i, (name, data) in enumerate(images):
    if len(sys.argv) > 2:
        open(f'{sys.argv[2]}/ref{i}.jpg', 'wb').write(data)
    print(name, len(data))
