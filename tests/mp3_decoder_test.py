# SPDX-License-Identifier: MIT
"""The player's decoder (apps/mp3/minimp3.rs, a port of minimp3) against the C minimp3 it was ported from, sample for
sample: MP3s made with ffmpeg, mono and stereo, MPEG 1, 2 and 2.5, CBR, VBR and ABR, with and without ID3 tags, each
decoded by both, fed as the player feeds its decoder (tests/mp3_decoder_host.rs). Then the same files broken at random,
which must not stop the Rust, and must decode as the C does.

The C is minimp3.h as the C player had it, from git (commit C_PLAYER, main before the port to Rust), or the file
$MINIMP3_H, built as the player built it (MP3 only, no SIMD). Without it, ffmpeg (with libmp3lame) or a C compiler, the
test is skipped."""
import os
from pathlib import Path
import random
import shutil
import subprocess
import sys
import tempfile
sys.path.insert(0, str(Path(__file__).resolve().parent))
from host import build as host_build  # noqa: E402
ROOT = Path(__file__).resolve().parents[1]
BROKEN = 150
# The last commit with the C player.
C_PLAYER = '748bf0036b784ac38aa4c765c882fd7955e51332'

# The C side: as apps/mp3/decoder.c configured minimp3, and fed as apps/mp3/mp3.c fed it. REF_ZERO clears minimp3's
# scratch for each frame, as the Rust's is, for broken files, where minimp3 can read some of it before writing it.
REFERENCE = r'''
#define MINIMP3_IMPLEMENTATION
#define MINIMP3_ONLY_MP3
#define MINIMP3_NO_SIMD
#include "minimp3.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define CHUNK 4096u
#define INPUT (16u*1024u)
static mp3dec_t decoder;
static uint8_t input[INPUT];
static int16_t pcm[MINIMP3_MAX_SAMPLES_PER_FRAME];
int main(int argc, char **argv) {
    FILE *f = fopen(argv[1], "rb"), *o = fopen(argv[2], "wb");
    if (!f || !o)
        return 2;
    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    fseek(f, 0, SEEK_SET);
    uint8_t *file = malloc(size ? size : 1);
    if (fread(file, 1, size, f) != (size_t)size)
        return 2;
    uint32_t have = 0, offset = 0;
    mp3dec_init(&decoder);
    for (;;) {
        while (have < INPUT-CHUNK && offset < size) {
            uint32_t n = size-offset < CHUNK ? size-offset : CHUNK;
            memcpy(input+have, file+offset, n);
            have += n;
            offset += n;
        }
        mp3dec_frame_info_t info = {0};
        int decoded = mp3dec_decode_frame(&decoder, input, (int)have, pcm, &info);
        if (!info.frame_bytes)
            break;
        have -= (uint32_t)info.frame_bytes;
        memmove(input, input+info.frame_bytes, have);
        fwrite(pcm, 2, (size_t)decoded*info.channels, o);
    }
    fclose(o);
    return 0;
}
'''

# name: ffmpeg's arguments after the input, the source (s: 20 s of tones, sweeps, noise and clicks; l: loud enough to
# clip), and whether it needs libshine.
FILES = {
    's128_id3': ('s', ['-c:a', 'libmp3lame', '-b:a', '128k', '-metadata', 'title=Stereo 128', '-id3v2_version', '3']),
    's320_bare': ('s', ['-c:a', 'libmp3lame', '-b:a', '320k', '-id3v2_version', '0', '-write_xing', '0']),
    'm64_id3': ('s', ['-ac', '1', '-c:a', 'libmp3lame', '-b:a', '64k', '-metadata', 'artist=Mono']),
    's48_vbr': ('s', ['-ar', '48000', '-c:a', 'libmp3lame', '-q:a', '2', '-id3v2_version', '0']),
    'm22_mpeg2': ('s', ['-ac', '1', '-ar', '22050', '-c:a', 'libmp3lame', '-b:a', '32k']),
    'm16_mpeg2': ('s', ['-ac', '1', '-ar', '16000', '-c:a', 'libmp3lame', '-b:a', '24k']),
    's8_mpeg25': ('s', ['-ar', '8000', '-c:a', 'libmp3lame', '-b:a', '16k', '-id3v2_version', '0']),
    's24_abr': ('s', ['-ar', '24000', '-c:a', 'libmp3lame', '-abr', '1', '-b:a', '64k']),
    's32_lr': ('s', ['-ar', '32000', '-c:a', 'libmp3lame', '-b:a', '96k', '-joint_stereo', '0']),
    's160_id3v1': ('s', ['-c:a', 'libmp3lame', '-b:a', '160k', '-write_id3v1', '1', '-metadata', 'title=Both tags']),
    'loud_clip': ('l', ['-c:a', 'libmp3lame', '-b:a', '192k']),
    # No lowpass, so the top band has sound, and scfsi.
    'shine128': ('s', ['-c:a', 'libshine', '-b:a', '128k']),
}
SOURCES = {
    's': "aevalsrc=exprs='0.3*sin(2*PI*220*t)*(1+0.5*sin(2*PI*0.5*t))+0.2*sin(2*PI*(440+200*sin(2*PI*0.2*t))*t)"
         "+0.5*lt(mod(t\\,0.37)\\,0.004)*(random(0)*2-1)+0.05*(random(1)*2-1)|0.25*sin(2*PI*330*t)"
         "+0.2*sin(2*PI*(880+300*sin(2*PI*0.3*t))*t)+0.5*lt(mod(t+0.1\\,0.29)\\,0.003)*(random(2)*2-1)"
         "+0.04*(random(3)*2-1)':s=44100:d=12",
    'l': "aevalsrc=exprs='1.6*sin(2*PI*100*t)+0.6*sin(2*PI*3000*t)|1.4*sin(2*PI*150*t)':s=44100:d=6",
}


def skip(why):
    print(f'mp3 decoder SKIPPED: {why}')
    raise SystemExit(0)


def first_difference(a, b):
    return next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))


def c_source():
    if os.environ.get('MINIMP3_H'):
        return Path(os.environ['MINIMP3_H']).read_text()
    done = subprocess.run(['git', '-C', ROOT, 'show', f'{C_PLAYER}:apps/mp3/minimp3.h'], capture_output=True, text=True)
    return done.stdout if done.returncode == 0 else None


ffmpeg, cc = shutil.which('ffmpeg'), shutil.which(os.environ.get('HOSTCC', 'cc'))
if not ffmpeg or not cc:
    skip('needs ffmpeg and a C compiler')
encoders = subprocess.run([ffmpeg, '-hide_banner', '-encoders'], capture_output=True, text=True).stdout
if 'libmp3lame' not in encoders:
    skip("needs ffmpeg's libmp3lame")
header = c_source()
if not header:
    skip(f'needs the C minimp3: git commit {C_PLAYER[:7]}, or MINIMP3_H')

with tempfile.TemporaryDirectory() as folder:
    folder = Path(folder)
    (folder/'minimp3.h').write_text(header.replace('    mp3dec_scratch_t scratch;\n',
                                                   '    mp3dec_scratch_t scratch;\n#ifdef REF_ZERO\n'
                                                   '    memset(&scratch, 0, sizeof scratch);\n#endif\n'))
    (folder/'reference.c').write_text(REFERENCE)
    for name, flags in (('reference', []), ('reference_zero', ['-DREF_ZERO'])):
        subprocess.run([cc, '-std=c99', '-O2', '-w', *flags, folder/'reference.c', '-o', folder/name], check=True)
    rust = host_build('mp3_decoder_host')

    def decode(mp3, reference):
        c, r = folder/'c.pcm', folder/'r.pcm'
        subprocess.run([folder/reference, mp3, c], check=True, timeout=60)
        done = subprocess.run([rust, mp3, r], capture_output=True, text=True, timeout=60)
        assert done.returncode == 0, (mp3, done.stdout, done.stderr)
        return c.read_bytes(), r.read_bytes()

    def make(source, args, out):
        subprocess.run([ffmpeg, '-hide_banner', '-loglevel', 'error', '-y', '-f', 'lavfi', '-i', SOURCES[source],
                        *args, out], check=True)

    made = []
    for name, (source, args) in FILES.items():
        if 'libshine' in args and 'libshine' not in encoders:
            print(f'{name}: skipped, no libshine')
            continue
        mp3 = folder/f'{name}.mp3'
        make(source, args, mp3)
        c, r = decode(mp3, 'reference')
        assert len(c) > 0, name
        assert c == r, f'{name}: the samples differ from the C, first at byte {first_difference(c, r)}'
        print(f'{name}: {len(c)//2} samples, the same')
        made.append(mp3.read_bytes())
    # A tag bigger than what the player reads at a time: a cover picture.
    (folder/'cover.png').write_bytes(subprocess.run(
        [ffmpeg, '-hide_banner', '-loglevel', 'error', '-f', 'lavfi', '-i', 'color=c=red:s=300x300,noise=alls=100:allf=t',
         '-frames:v', '1', '-f', 'image2pipe', '-c:v', 'png', '-'], capture_output=True, check=True).stdout)
    mp3 = folder/'bigtag.mp3'
    subprocess.run([ffmpeg, '-hide_banner', '-loglevel', 'error', '-y', '-f', 'lavfi', '-i', SOURCES['s'], '-i',
                    folder/'cover.png', '-map', '0', '-map', '1', '-c:a', 'libmp3lame', '-b:a', '128k', '-c:v', 'png',
                    '-disposition:v', 'attached_pic', '-id3v2_version', '3', mp3], check=True)
    assert mp3.stat().st_size > 100_000
    c, r = decode(mp3, 'reference')
    assert len(c) > 0 and c == r, 'bigtag: the samples differ from the C'
    print(f'bigtag: {len(c)//2} samples after a {mp3.stat().st_size//1024} KB file, the same')

    # Broken files: bits flipped, bytes changed, and headers changed (layer, CRC, mode extension, free format).
    rng = random.Random(1)
    for k in range(BROKEN):
        d = bytearray(rng.choice(made))
        start = rng.randrange(0, max(1, len(d)-60000))
        d = d[start:start+rng.choice([3000, 20000, 60000])]
        kind = rng.random()
        for _ in range(rng.randint(1, 40)):
            i = rng.randrange(len(d))
            if kind < 0.3:
                d[i] ^= 1 << rng.randrange(8)
            elif kind < 0.6:
                d[i] = rng.randrange(256)
            else:
                j = d.find(b'\xff', i)
                if 0 <= j < len(d)-3:
                    d[j+rng.choice([1, 2, 3])] ^= 1 << rng.randrange(8)
        mp3 = folder/'broken.mp3'
        mp3.write_bytes(d)
        c, r = decode(mp3, 'reference_zero')
        assert c == r, f'broken file {k} decodes differently from the C'
    print(f'{BROKEN} broken files: the same')
print('mp3 decoder OK')
