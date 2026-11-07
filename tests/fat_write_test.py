# SPDX-License-Identifier: MIT
"""Run kit/fat.c's writing on real FAT32 images, checked by fsck and mtools, with the power cut at every write."""
import os
from pathlib import Path
import random
import shutil
import subprocess
import tempfile
ROOT = Path(__file__).resolve().parents[1]
ENV = {**os.environ, 'MTOOLS_SKIP_CHECK': '1'}
TOOLS = [shutil.which(t) or shutil.which(f'/usr/sbin/{t}') or shutil.which(f'/sbin/{t}') for t in ('mkfs.vfat', 'fsck.vfat', 'mcopy', 'mdir', 'mmd')]
if not all(TOOLS):
    print('fat writing SKIPPED: needs mkfs.vfat, fsck.vfat and mtools')
    raise SystemExit(0)
MKFS, FSCK, MCOPY, MDIR, MMD = TOOLS
rng = random.Random(1)


def blob(size):
    return bytes(rng.getrandbits(8) for _ in range(size))


with tempfile.TemporaryDirectory() as folder:
    folder = Path(folder)
    host = folder/'host'
    subprocess.run([os.environ.get('HOSTCC', 'cc'), '-std=c11', '-D_DEFAULT_SOURCE', '-Wall', '-Wextra', '-Werror', '-fsanitize=address,undefined',
                    f'-I{ROOT}/kit', '-idirafter', f'{ROOT}/m-bootloader/include', '-DPLATFORM_SIM',
                    ROOT/'tests/fatw_host.c', ROOT/'kit/fat.c', ROOT/'m-bootloader/src/core.c', '-o', host], check=True)
    image = folder/'card'
    data = folder/'data'

    def make(mib=64, per_cluster=2):
        image.unlink(missing_ok=True)
        subprocess.run([MKFS, '-F', '32', '-s', str(per_cluster), '-n', 'TEST', '-C', image, str(mib*1024)], check=True, capture_output=True, env=ENV)

    def run(*args, cut=-1, expect=0):
        done = subprocess.run([host, image, str(cut), *map(str, args)], capture_output=True, text=True, timeout=60, env={**ENV, 'ASAN_OPTIONS': 'detect_leaks=0'})
        assert done.returncode == expect, (args, cut, done.returncode, done.stdout, done.stderr)
        return done.stdout.strip()

    def put(path, name, content):
        data.write_bytes(content)
        subprocess.run([MCOPY, '-i', image, data, f'::/{path}/{name}'.replace('//', '/')], check=True, capture_output=True, env=ENV)

    def get(path, name):
        out = folder/'out'
        out.unlink(missing_ok=True)
        done = subprocess.run([MCOPY, '-n', '-i', image, f'::/{path}/{name}'.replace('//', '/'), out], capture_output=True, env=ENV)
        return out.read_bytes() if done.returncode == 0 else None

    def fsck(allow_lost=False):
        done = subprocess.run([FSCK, '-n', image], capture_output=True, text=True, env=ENV)
        lines = [l for l in done.stdout.splitlines()[1:] if l.strip() and 'Leaving filesystem unchanged' not in l and ' files, ' not in l and 'Auto-removing' not in l]
        if allow_lost:
            # What a cut may leave: clusters no file uses, and a free count that hasn't caught up.
            lines = [l for l in lines if not any(t in l for t in ('Reclaimed', 'unused clusters', 'Free cluster summary', 'Auto-correcting', 'FATs differ', 'Using first FAT'))]
        assert not lines, lines

    def new(path, name, content):
        data.write_bytes(content)
        return run('new', path, name, data)

    # A folder, a file in it, with what was there left alone, and nothing for fsck to say.
    make()
    run('mkdir', 'MBOOT/SLOTS/UNIT1')
    fsck()
    one = blob(70000)
    new('MBOOT/SLOTS/UNIT1', 'SLOT01.BIN', one)
    fsck()
    assert get('MBOOT/SLOTS/UNIT1', 'SLOT01.BIN') == one
    keep = blob(5000)
    put('', 'KEEP.TXT', keep)
    put('', 'A LONG FILE NAME.TXT', keep)
    subprocess.run([MMD, '-i', image, '::/OLD'], check=True, env=ENV)
    run('mkdir', 'OLD')   # it's there already
    fsck()
    # Sizes around a cluster and a sector, and an empty file.
    for i, size in enumerate((0, 1, 511, 512, 513, 1023, 1024, 1025, 2048, 4096)):
        content = blob(size)
        new('MBOOT/SLOTS/UNIT1', f'S{i}.BIN', content)
        got = get('MBOOT/SLOTS/UNIT1', f'S{i}.BIN')
        assert got == content, (size, got and len(got))
    fsck()
    # Replacing a file.
    two = blob(90000)
    data.write_bytes(two)
    run('swap', 'MBOOT/SLOTS/UNIT1', 'SLOT01.BIN', data)
    assert get('MBOOT/SLOTS/UNIT1', 'SLOT01.BIN') == two
    assert get('', 'KEEP.TXT') == keep
    fsck()
    # Removing, including a name a computer gave long-name pieces to.
    run('remove', 'MBOOT/SLOTS/UNIT1', 'S3.BIN')
    assert get('MBOOT/SLOTS/UNIT1', 'S3.BIN') is None
    fsck()
    listing = subprocess.run([MDIR, '-i', image, '-/', '::/'], capture_output=True, text=True, env=ENV).stdout
    short = next(l.split()[0]+'.'+l.split()[1] for l in listing.splitlines() if 'A LONG FILE NAME' in l)
    run('remove', '', short)
    assert get('', 'A LONG FILE NAME.TXT') is None
    fsck()

    # A directory growing past its cluster, on small clusters.
    make(per_cluster=1)
    run('mkdir', 'D')
    names = [f'F{i:04}.BIN' for i in range(120)]
    bodies = {n: blob(300) for n in names}
    for n in names:
        new('D', n, bodies[n])
    fsck()
    for n in names:
        assert get('D', n) == bodies[n], n
    # A freed entry gets used again.
    run('remove', 'D', 'F0050.BIN')
    new('D', 'AGAIN.BIN', b'again')
    assert get('D', 'AGAIN.BIN') == b'again'
    fsck()

    # A directory whose chain loops, with no free entry: refused, with the FAT left as it is.
    make(per_cluster=1)
    run('mkdir', 'D')
    for i in range(20):
        new('D', f'L{i}.BIN', b'x')
    raw = bytearray(image.read_bytes())
    reserved, fats, fat_size, root = int.from_bytes(raw[14:16], 'little'), raw[16], int.from_bytes(raw[36:40], 'little'), int.from_bytes(raw[44:48], 'little')
    fat_at, data_at = reserved*512, (reserved+fats*fat_size)*512
    entry = lambda c: int.from_bytes(raw[fat_at+c*4:fat_at+c*4+4], 'little')&0x0fffffff
    at = data_at+(root-2)*512
    while raw[at:at+11] != b'D          ':
        at += 32
    first = int.from_bytes(raw[at+26:at+28], 'little') | int.from_bytes(raw[at+20:at+22], 'little') << 16
    second = entry(first)
    assert 2 <= second < 0x0ffffff8 and entry(second) >= 0x0ffffff8, 'the folder should end in its second cluster'
    # Both clusters full, then the second points back at the first.
    for c in (first, second):
        for e in range(16):
            place = data_at+(c-2)*512+e*32
            if raw[place] in (0, 0xe5):
                raw[place:place+32] = b'Z%03d%04d' % (c % 1000, e)+b'TXT '+bytes(20)
    for copy in range(fats):
        place = fat_at+copy*fat_size*512+second*4
        raw[place:place+4] = first.to_bytes(4, 'little')
    image.write_bytes(bytes(raw))
    answer = run('new', 'D', 'NEW.BIN', data, expect=3)
    assert answer == 'error BAD FOLDER ON CARD', answer
    written = bytearray(image.read_bytes())
    for copy in range(fats):
        place = fat_at+copy*fat_size*512
        assert written[place+second*4:place+second*4+4] == first.to_bytes(4, 'little'), 'the chain was changed'
        assert written[place+first*4:place+first*4+4] == raw[place+first*4:place+first*4+4], 'the chain was changed'
    assert written[data_at+(first-2)*512:data_at+(first-1)*512] == raw[data_at+(first-2)*512:data_at+(first-1)*512]

    # Free space in pieces: a file across the gaps.
    make(per_cluster=1)
    run('mkdir', 'D')
    for i in range(40):
        new('D', f'G{i}.BIN', blob(1024))
    for i in range(0, 40, 2):
        run('remove', 'D', f'G{i}.BIN')
    pieces = blob(60000)
    new('D', 'BIG.BIN', pieces)
    assert get('D', 'BIG.BIN') == pieces
    for i in range(1, 40, 2):
        assert len(get('D', f'G{i}.BIN')) == 1024
    fsck()

    # A full card: refused, with nothing changed.
    make(mib=34)
    before = image.read_bytes()
    assert run('reserve', 40*1024*1024, expect=3) == 'error CARD IS FULL'
    assert image.read_bytes() == before

    # The power goes at each write of each operation. A file is whole, as it was or as it now is.
    old, replacement, added = blob(6000), blob(7000), blob(3000)

    def base():
        make(per_cluster=1)
        run('mkdir', 'S')
        new('S', 'A.BIN', old)
        put('', 'KEEP.TXT', keep)
        return image.read_bytes()

    start = base()

    def every_cut(label, *args, check):
        image.write_bytes(start)
        total = int(run(*args).split()[1])
        after = image.read_bytes()
        assert total > 3, total
        for cut in range(total):
            image.write_bytes(start)
            run(*args, cut=cut, expect=42)
            fsck(allow_lost=True)
            assert get('', 'KEEP.TXT') == keep, (label, cut)
            check(cut)
        image.write_bytes(after)
        fsck()
        print(f'  {label}: {total} cuts')

    def still_old(cut):
        assert get('S', 'A.BIN') == old, cut

    data.write_bytes(replacement)
    every_cut('replace', 'swap', 'S', 'A.BIN', data, check=lambda cut: (lambda got: (got in (old, replacement)) or (_ for _ in ()).throw(AssertionError(cut)))(get('S', 'A.BIN')))
    data.write_bytes(added)
    def new_file(cut):
        still_old(cut)
        got = get('S', 'B.BIN')
        assert got is None or got == added, cut
    every_cut('new file', 'new', 'S', 'B.BIN', data, check=new_file)
    every_cut('remove', 'remove', 'S', 'A.BIN', check=lambda cut: (lambda got: (got in (None, old)) or (_ for _ in ()).throw(AssertionError(cut)))(get('S', 'A.BIN')))
    def folder_check(cut):
        still_old(cut)
    every_cut('new folders', 'mkdir', 'S/X/Y', check=folder_check)
print('fat writing OK')
