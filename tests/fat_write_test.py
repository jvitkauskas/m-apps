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

    # Names, as a computer wrote them.
    make(per_cluster=1)
    run('mkdir', 'N')
    wanted = ['plain.mp3', 'UPPER.MP3', 'Mixed Case Song (Live) & more!.mp3', 'exactly13char.mp3', 'ąčęėįšųūž lietuviškai.mp3', '日本語の歌.mp3']
    for name in wanted:
        put('N', name, name.encode())
    put('N', 'a.txt', b'not listed')
    subprocess.run([MMD, '-i', image, '::/N/folder.mp3'], check=True, env=ENV)
    def names(ext='MP3'):
        lines = run('names', 'N', ext).splitlines()
        return [l.split('\t')[3] for l in lines if l.startswith('name\t')], lines
    got, lines = names()
    assert lines[0] == f'count {len(wanted)} 0', lines
    assert sorted(got) == sorted(wanted), got
    assert [n.upper() for n in got if n.isascii()] == sorted(n.upper() for n in wanted if n.isascii()), got
    # Sizes come with the right file.
    for line in (l for l in lines if l.startswith('name\t')):
        _, _, size, name = line.split('\t')
        assert int(size) == len(name.encode()), line
    # Past what's kept: cut between characters, and past the list's size, the first in order.
    make(per_cluster=1)
    run('mkdir', 'N')
    cut = 'é'*100+'.mp3'
    put('N', cut, b'x')
    got, _ = names()
    assert got == ['é'*35], got
    long_ascii = 'x'*200+'.mp3'
    put('N', long_ascii, b'x')
    got, _ = names()
    assert 'x'*71 in got, got
    make(per_cluster=1)
    run('mkdir', 'N')
    for i in reversed(range(12)):
        put('N', f'song number {i:02}.mp3', b'x')
    got, lines = names()
    assert lines[0] == 'count 8 1' and got == [f'song number {i:02}.mp3' for i in range(8)], lines
    # A short name's case bits, and a deleted long name's pieces that were left behind.
    raw = bytearray(image.read_bytes())
    make(per_cluster=1)
    run('mkdir', 'N')
    put('N', 'long name one.mp3', b'1')
    put('N', 'long name two.mp3', b'2')
    new('N', 'SHORT.MP3', b's')
    raw = bytearray(image.read_bytes())
    reserved, fats, fat_size, root = int.from_bytes(raw[14:16], 'little'), raw[16], int.from_bytes(raw[36:40], 'little'), int.from_bytes(raw[44:48], 'little')
    data_at = (reserved+fats*fat_size)*512
    at = data_at+(root-2)*512
    while raw[at:at+11] != b'N          ':
        at += 32
    folder_at = data_at+(int.from_bytes(raw[at+26:at+28], 'little')-2)*512
    entries = [folder_at+i*32 for i in range(16)]
    short = next(p for p in entries if raw[p:p+11] == b'SHORT   MP3')
    raw[short+12] = 0x18
    # The short entry of the first long name goes, as a tool might leave it: its pieces then belong to nothing.
    first_long = next(p for p in entries if raw[p+11] != 0x0f and raw[p:p+6] == b'LONGNA' and raw[p] != 0xe5)
    raw[first_long] = 0xe5
    # And the second's checksum goes wrong: it falls back to its short name.
    second_long = next(p for p in entries if raw[p+11] != 0x0f and raw[p:p+6] == b'LONGNA' and p != first_long)
    second_short = bytes(raw[second_long:second_long+11]).decode()
    raw[second_long+10] = ord('X')
    raw[second_long+8:second_long+11] = b'MP3'
    raw[second_long+6] = ord('7')
    second_short = bytes(raw[second_long:second_long+11]).decode()
    image.write_bytes(bytes(raw))
    got, _ = names()
    assert sorted(got) == sorted(['short.mp3', second_short[:8].strip()+'.MP3']), got
    # Beyond the 16 bits mtools writes: a surrogate pair, one with half missing, and a name of exactly 13 units with no end mark.
    def entries_for(units, name11, start):
        total = (len(units)+12)//13
        padded = units+[0]+[0xffff]*12 if len(units) % 13 else units
        total = max(1, (len(units)+12)//13)
        sum_ = 0
        for b in name11:
            sum_ = ((sum_ >> 1 | sum_ << 7) & 255)+b & 255
        out = b''
        for piece in range(total, 0, -1):
            part = (padded+[0xffff]*13)[(piece-1)*13:piece*13]
            e = bytearray(32)
            e[0] = piece | (0x40 if piece == total else 0)
            for place, unit in zip((1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30), part):
                e[place:place+2] = unit.to_bytes(2, 'little')
            e[11] = 0x0f
            e[13] = sum_
            out += bytes(e)
        short_entry = bytearray(32)
        short_entry[:11] = name11
        short_entry[11] = 0x20
        short_entry[26:28] = start.to_bytes(2, 'little')
        short_entry[28:32] = (1).to_bytes(4, 'little')
        return out+bytes(short_entry)
    make(per_cluster=1)
    run('mkdir', 'N')
    new('N', 'DATA.MP3', b'd')
    raw = bytearray(image.read_bytes())
    reserved, fats, fat_size, root = int.from_bytes(raw[14:16], 'little'), raw[16], int.from_bytes(raw[36:40], 'little'), int.from_bytes(raw[44:48], 'little')
    data_at = (reserved+fats*fat_size)*512
    at = data_at+(root-2)*512
    while raw[at:at+11] != b'N          ':
        at += 32
    place = data_at+(int.from_bytes(raw[at+26:at+28], 'little')-2)*512
    while raw[place:place+11] != b'DATA    MP3':
        place += 32
    start = int.from_bytes(raw[place+26:place+28], 'little')
    place += 32
    made = entries_for([ord(c) for c in 'emoji ']+[0xd83c, 0xdfb5]+[ord(c) for c in ' song.mp3'], b'EMOJI   MP3', start)
    made += entries_for([ord('a'), 0xd83c, ord('b')]+[ord(c) for c in '.mp3'], b'BROKEN  MP3', start)
    made += entries_for([ord(c) for c in 'thirteen1.mp3'], b'THIRTEENMP3', start)
    made += bytes(entries_for([], b'\x90CODE   MP3', start)[-32:])
    raw[place:place+len(made)] = made
    image.write_bytes(bytes(raw))
    got, _ = names()
    assert sorted(got) == sorted(['DATA.MP3', 'emoji 🎵 song.mp3', 'a\ufffdb.mp3', 'thirteen1.mp3', '?CODE.MP3']), got

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
