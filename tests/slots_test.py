# SPDX-License-Identifier: MIT
"""Run apps/slots' logic with a real FAT32 image and a simulated flash chip, with the power cut during saves and loads."""
import os
from pathlib import Path
import random
import shutil
import subprocess
import sys
import tempfile
import zlib
sys.path.insert(0, str(Path(__file__).resolve().parent))
from host import build as host_build  # noqa: E402
ROOT = Path(__file__).resolve().parents[1]
ENV = {**os.environ, 'MTOOLS_SKIP_CHECK': '1', 'ASAN_OPTIONS': 'detect_leaks=0'}
TOOLS = [shutil.which(t) or shutil.which(f'/usr/sbin/{t}') or shutil.which(f'/sbin/{t}') for t in ('mkfs.vfat', 'fsck.vfat', 'mcopy', 'mmd')]
if not all(TOOLS):
    print('slots SKIPPED: needs mkfs.vfat, fsck.vfat and mtools')
    raise SystemExit(0)
MKFS, FSCK, MCOPY, MMD = TOOLS
SIZE, FIRST, END, SECTOR = 0x100000, 0x4000, 0xff000, 0x1000
KEEP = (0xc0000, 0xc2000)
UID = bytes(range(1, 9))
FOLDER = 'MBOOT/SLOTS/SMK37'
rng = random.Random(2)


def firmware(tag, settings=0):
    """A whole image: the first sector marks the firmware, settings are further in, and some of it is blank."""
    f = bytearray(rng.randbytes(SIZE))
    f[FIRST:FIRST+SECTOR] = (b'GOOD'+tag.encode()).ljust(SECTOR, b'.')
    f[0x90000:0x91000] = bytes([settings])*SECTOR
    f[0xa0000:0xb8000] = b'\xff'*0x18000
    f[0xe0000:0xe1000] = b'\xff'*SECTOR
    return f


def in_scope(at):
    return FIRST <= at < END and not KEEP[0] <= at < KEEP[1]


def restorable(image):
    return bytes(image[FIRST:KEEP[0]])+bytes(image[KEEP[1]:END])


def parse(data):
    """A slot file as the flash it describes: what it doesn't hold is blank."""
    assert data[:4] == b'SLOT' and data[4] == 1 and data[5] == 2, data[:8]
    assert int.from_bytes(data[508:512], 'little') == zlib.crc32(data[:508])
    stored = int.from_bytes(data[16:48], 'little')
    count = int.from_bytes(data[8:12], 'little')
    assert len(data) == 512+count*SECTOR and bin(stored).count('1') == count
    assert int.from_bytes(data[112:116], 'little') == zlib.crc32(data[512:])
    image = bytearray(b'\xff'*SIZE)
    i = 0
    for n in range(SIZE//SECTOR):
        if stored >> n & 1:
            assert in_scope(n*SECTOR), hex(n*SECTOR)
            image[n*SECTOR:(n+1)*SECTOR] = data[512+i*SECTOR:512+(i+1)*SECTOR]
            i += 1
    return image, count


with tempfile.TemporaryDirectory() as folder:
    folder = Path(folder)
    host = host_build('slots_host')
    card, flash = folder/'card', folder/'flash'

    def blank(mib=64):
        card.unlink(missing_ok=True)
        subprocess.run([MKFS, '-F', '32', '-s', '8', '-n', 'TEST', '-C', card, str(mib*1024)], check=True, capture_output=True, env=ENV)

    def run(*args, cut=-1, expect=0):
        done = subprocess.run([host, card, flash, str(cut), *args], capture_output=True, text=True, timeout=120, env=ENV)
        assert done.returncode == expect, (args, cut, done.returncode, done.stdout, done.stderr[:400])
        return done.stdout.strip()

    def facts(line):
        return dict(p.split('=', 1) for p in line.split('\t')[1:])

    def raw(path):
        out = folder/'out'
        out.unlink(missing_ok=True)
        done = subprocess.run([MCOPY, '-n', '-i', card, f'::/{path}', out], capture_output=True, env=ENV)
        return out.read_bytes() if done.returncode == 0 else None

    def slot(name):
        data = raw(f'{FOLDER}/{name}.slt')
        return None if data is None else parse(data)[0]

    def fsck(after_cut=False):
        done = subprocess.run([FSCK, '-n', card], capture_output=True, text=True, env=ENV)
        lines = [l for l in done.stdout.splitlines()[1:] if l.strip() and 'Leaving filesystem unchanged' not in l and ' files, ' not in l]
        if after_cut:
            lines = [l for l in lines if not any(t in l for t in ('Reclaimed', 'unused clusters', 'Free cluster summary', 'Auto-correcting', 'FATs differ', 'Using first FAT'))]
        assert not lines, lines

    def set_flash(image):
        flash.write_bytes(bytes(image))

    def get_flash():
        return flash.read_bytes()

    # Nothing's made on the card until there's something to save.
    blank()
    start = firmware('A')
    set_flash(start)
    before = card.read_bytes()
    line = run('list')
    assert facts(line) == {'here': '-', 'pending': '-', 'active': '-', 'slots': ''}, line
    assert card.read_bytes() == before

    # A slot is the firmware's part of the flash, without its blank sectors, under the name it was given.
    line = run('save', 'Felucca 1.8')
    fsck()
    assert facts(line)['slots'] == 'Felucca 1.8|' and facts(line)['here'] == 'Felucca 1.8', line
    image, count = parse(raw(f'{FOLDER}/Felucca 1.8.slt'))
    assert restorable(image) == restorable(start)
    assert count == len([a for a in range(FIRST, END, SECTOR) if in_scope(a) and start[a:a+SECTOR] != b'\xff'*SECTOR]), count
    assert image[:FIRST] == b'\xff'*FIRST and image[END:] == b'\xff'*(SIZE-END) and image[KEEP[0]:KEEP[1]] == b'\xff'*0x2000, \
        'no unit data in a slot'
    assert raw(f'{FOLDER}/SLOT0001.SLT') is not None
    assert get_flash() == bytes(start), 'saving writes nothing to flash'
    # Names: refused when a computer couldn't have them, numbered when taken.
    for bad in ('a/b', 'a:b', 'ends in a dot.', ' starts with a space', 'x'*41, ''):
        assert run('save', bad, expect=3).startswith('error NAME NOT ALLOWED'), bad
    line = run('save', 'felucca 1.8')
    assert facts(line)['slots'] == 'Felucca 1.8|felucca 1.8 2|', line
    run('delete', 'felucca 1.8 2')
    fsck()

    # A second firmware, then back: only what a load writes changes.
    other = firmware('B')
    other[:FIRST] = start[:FIRST]
    other[END:] = start[END:]
    other[KEEP[0]:KEEP[1]] = start[KEEP[0]:KEEP[1]]
    set_flash(other)
    assert facts(run('save', 'Other'))['slots'] == 'Felucca 1.8|Other|'
    mine = bytearray(other)
    mine[100] ^= 1              # the boot area
    mine[KEEP[0]+5] ^= 1        # calibration
    mine[0xff800] ^= 1          # the key area
    set_flash(mine)
    line = run('load', 'Felucca 1.8')
    fsck()
    now = get_flash()
    assert restorable(now) == restorable(start)
    assert now[:FIRST] == bytes(mine[:FIRST]) and now[KEEP[0]:KEEP[1]] == bytes(mine[KEEP[0]:KEEP[1]]) and now[END:] == bytes(mine[END:]), 'what a load leaves alone'
    assert facts(line)['here'] == 'Felucca 1.8' and facts(line)['active'] == 'Felucca 1.8' and facts(line)['slots'] == 'Felucca 1.8|Other|', line
    # Loading what's already there writes nothing to flash.
    same = get_flash()
    run('load', 'Felucca 1.8')
    assert get_flash() == same
    # A sector the slot doesn't hold is erased.
    dirty = bytearray(get_flash())
    dirty[0xa5000:0xa6000] = b'\x12'*SECTOR
    dirty[0xe0000] = 0
    set_flash(dirty)
    run('load', 'Other')
    assert restorable(get_flash()) == restorable(other), 'sectors the slot leaves blank are erased'
    # What was on the unit went over its slot first, as changed settings do.
    run('load', 'Felucca 1.8')
    assert restorable(get_flash()) == restorable(dirty)

    # Changed settings go over the slot they came from, which keeps its name.
    changed = bytearray(get_flash())
    changed[0x90000:0x91000] = b'\x77'*SECTOR
    set_flash(changed)
    line = run('load', 'Other')
    assert facts(line)['slots'] == 'Felucca 1.8|Other|', line
    assert restorable(slot('Felucca 1.8')) == restorable(changed)
    assert restorable(get_flash()) == restorable(other)
    fsck()

    # Other firmware gets its own slot, and doesn't go over the active one.
    third = bytearray(get_flash())
    third[FIRST:FIRST+SECTOR] = b'GOODC'.ljust(SECTOR, b'.')
    set_flash(third)
    line = run('load', 'Felucca 1.8')
    assert facts(line)['slots'] == 'Felucca 1.8|Other|Saved|', line
    assert restorable(slot('Saved')) == restorable(third)
    assert restorable(slot('Other')) == restorable(other)
    assert restorable(get_flash()) == restorable(changed)

    # Reloading the active slot with changes keeps the changes in a slot of their own, rather than losing the saved one.
    revert = bytearray(get_flash())
    revert[0x90000:0x91000] = b'\x55'*SECTOR
    set_flash(revert)
    line = run('load', 'Felucca 1.8')
    assert facts(line)['slots'] == 'Felucca 1.8|Other|Saved|Saved 2|', line
    assert restorable(slot('Saved 2')) == restorable(revert)
    assert restorable(get_flash()) == restorable(changed), 'it goes back to what was saved'
    run('delete', 'Saved 2')
    fsck()

    # What isn't valid firmware is never saved.
    broken = bytearray(get_flash())
    broken[FIRST:FIRST+SECTOR] = b'\xff'*SECTOR
    set_flash(broken)
    line = run('load', 'Other')
    assert facts(line)['slots'] == 'Felucca 1.8|Other|Saved|', line
    assert restorable(slot('Felucca 1.8')) == restorable(changed), 'the slot keeps the good copy'
    assert restorable(get_flash()) == restorable(other)

    # A damaged slot is refused before anything changes: a byte of its data, its header, or cut short.
    good = raw(f'{FOLDER}/Other.slt')
    for label, data in (('data', good[:9000]+bytes([good[9000] ^ 1])+good[9001:]), ('header', good[:20]+bytes([good[20] ^ 1])+good[21:]),
                        ('short', good[:len(good)-SECTOR])):
        (folder/'bad').write_bytes(data)
        subprocess.run([MCOPY, '-o', '-i', card, folder/'bad', f'::/{FOLDER}/Bad {label}.slt'], check=True, capture_output=True, env=ENV)
    keep_flash, keep_card = get_flash(), card.read_bytes()
    for label in ('data', 'header', 'short'):
        assert run('load', f'Bad {label}', expect=3).startswith('error SLOT IS DAMAGED'), label
        assert get_flash() == keep_flash and card.read_bytes() == keep_card, label
    for label in ('data', 'header', 'short'):
        run('delete', f'Bad {label}')
    fsck()

    # No room for what's on the unit: refused, with the flash untouched.
    blank(mib=34)
    set_flash(start)
    run('save', 'First')
    filler = folder/'filler'
    free = 0
    for line in subprocess.run([FSCK, '-n', '-v', card], capture_output=True, text=True, env=ENV).stdout.splitlines():
        if ' files, ' in line:
            used, total = line.split(' files, ')[1].split()[0].split('/')
            free = (int(total)-int(used))*8*512
    filler.write_bytes(bytes(free-4096*2))
    subprocess.run([MCOPY, '-i', card, filler, '::/FILLER.BIN'], check=True, capture_output=True, env=ENV)
    set_flash(other)
    keep_flash = get_flash()
    assert run('load', 'First', expect=3).startswith('error CARD IS FULL')
    assert get_flash() == keep_flash
    fsck(after_cut=True)

    # The power goes during a load that saves first. Whatever's there afterwards, trying again finishes it, with nothing lost.
    blank()
    set_flash(start)
    run('save', 'One')
    settings = bytearray(start)
    settings[0x90000:0x91000] = b'\x33'*SECTOR
    set_flash(other)
    run('save', 'Two')
    run('load', 'One')
    set_flash(settings)
    base_card, base_flash = card.read_bytes(), get_flash()
    target = restorable(other)
    saved_old = restorable(slot('One'))
    total = int(run('load', 'Two').split('\t')[0].split()[1])
    assert total > 300, total
    # Every change near the swap of the slot and the start and end of the flash writing would be hours, so a spread of them.
    cuts = sorted({*range(0, 12), *range(total-40, total), *rng.sample(range(total), 170)})
    print(f'  {total} changes in a load, cutting at {len(cuts)}')
    for cut in cuts:
        card.write_bytes(base_card)
        flash.write_bytes(base_flash)
        run('load', 'Two', cut=cut, expect=42)
        fsck(after_cut=True)
        # Every slot is whole, whatever it holds.
        first = slot('One')
        assert first is not None and restorable(first) in (saved_old, restorable(settings)), cut
        assert restorable(slot('Two')) == target, cut
        run('list')
        # Trying again finishes it, and what was on the unit is saved.
        line = run('load', 'Two')
        assert restorable(get_flash()) == target, cut
        assert facts(line)['pending'] == '-' and facts(line)['here'] == 'Two', (cut, line)
        assert restorable(slot('One')) == restorable(settings), f'what was on the unit is lost, cut at {cut}'
        fsck(after_cut=True)
print('slots OK')
