# SPDX-License-Identifier: MIT
"""Run apps/slots' logic with a real FAT32 image and a simulated flash chip, with the power cut during saves and loads."""
import os
from pathlib import Path
import random
import shutil
import subprocess
import tempfile
import zlib
ROOT = Path(__file__).resolve().parents[1]
ENV = {**os.environ, 'MTOOLS_SKIP_CHECK': '1', 'ASAN_OPTIONS': 'detect_leaks=0'}
TOOLS = [shutil.which(t) or shutil.which(f'/usr/sbin/{t}') or shutil.which(f'/sbin/{t}') for t in ('mkfs.vfat', 'fsck.vfat', 'mcopy')]
if not all(TOOLS):
    print('slots SKIPPED: needs mkfs.vfat, fsck.vfat and mtools')
    raise SystemExit(0)
MKFS, FSCK, MCOPY = TOOLS
SIZE, FIRST, END = 0x100000, 0x4000, 0xff000
KEEP = (0xc0000, 0xc2000)
UID = bytes(range(1, 9))
FOLDER = f'MBOOT/SLOTS/{zlib.crc32(UID):08X}'
rng = random.Random(2)


def firmware(tag, settings=0):
    """A whole image: the first sector marks the firmware, and settings are further in."""
    f = bytearray(rng.randbytes(SIZE))
    f[FIRST:FIRST+0x1000] = (b'GOOD'+tag.encode()).ljust(0x1000, b'.')
    f[0x90000:0x91000] = bytes([settings])*0x1000
    return f


def restorable(image):
    return bytes(image[FIRST:KEEP[0]])+bytes(image[KEEP[1]:END])


with tempfile.TemporaryDirectory() as folder:
    folder = Path(folder)
    host = folder/'host'
    subprocess.run([os.environ.get('HOSTCC', 'cc'), '-std=c11', '-D_DEFAULT_SOURCE', '-O1', '-Wall', '-Wextra', '-Werror', '-fsanitize=address,undefined', '-DAPP_BOARD=2',
                    f'-I{ROOT}/apps/slots', f'-I{ROOT}/kit', '-idirafter', f'{ROOT}/m-bootloader/include', '-DPLATFORM_SIM',
                    ROOT/'tests/slots_host.c', ROOT/'apps/slots/slotfile.c', ROOT/'kit/fat.c', ROOT/'m-bootloader/src/core.c', '-o', host], check=True)
    card, flash = folder/'card', folder/'flash'

    def blank(mib=64):
        card.unlink(missing_ok=True)
        subprocess.run([MKFS, '-F', '32', '-s', '8', '-n', 'TEST', '-C', card, str(mib*1024)], check=True, capture_output=True, env=ENV)

    def run(*args, cut=-1, expect=0):
        done = subprocess.run([host, card, flash, str(cut), *args], capture_output=True, text=True, timeout=120, env=ENV)
        assert done.returncode == expect, (args, cut, done.returncode, done.stdout, done.stderr[:400])
        return done.stdout.strip()

    def facts(line):
        return dict(p.split('=', 1) for p in line.split()[2:])

    def slot(name):
        out = folder/'out'
        out.unlink(missing_ok=True)
        done = subprocess.run([MCOPY, '-n', '-i', card, f'::/{FOLDER}/{name}.BIN', out], capture_output=True, env=ENV)
        return out.read_bytes() if done.returncode == 0 else None

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

    # A slot is the whole flash, the same as a backup.
    line = run('save')
    fsck()
    assert facts(line)['slots'] == 'SLOT01,' and facts(line)['here'].strip() == 'SLOT01', line
    assert slot('SLOT01') == bytes(start)
    assert get_flash() == bytes(start), 'saving writes nothing to flash'

    # A second firmware, then back: only what a load writes changes.
    other = firmware('B')
    other[:FIRST] = start[:FIRST]
    other[END:] = start[END:]
    other[KEEP[0]:KEEP[1]] = start[KEEP[0]:KEEP[1]]
    set_flash(other)
    assert facts(run('save'))['slots'] == 'SLOT01,SLOT02,'
    mine = bytearray(other)
    mine[100] ^= 1              # the boot area
    mine[KEEP[0]+5] ^= 1        # calibration
    mine[0xff800] ^= 1          # the key area
    set_flash(mine)
    line = run('load', 'SLOT01')
    fsck()
    now = get_flash()
    assert restorable(now) == restorable(start)
    assert now[:FIRST] == bytes(mine[:FIRST]) and now[KEEP[0]:KEEP[1]] == bytes(mine[KEEP[0]:KEEP[1]]) and now[END:] == bytes(mine[END:]), 'what a load leaves alone'
    assert facts(line)['here'].strip() == 'SLOT01' and facts(line)['active'].strip() == 'SLOT01' and facts(line)['slots'] == 'SLOT01,SLOT02,', line
    # Loading what's already there writes nothing to flash.
    same = get_flash()
    run('load', 'SLOT01')
    assert get_flash() == same

    # Changed settings go over the slot they came from.
    changed = bytearray(get_flash())
    changed[0x90000:0x91000] = b'\x77'*0x1000
    set_flash(changed)
    line = run('load', 'SLOT02')
    assert facts(line)['slots'] == 'SLOT01,SLOT02,', line
    assert restorable(slot('SLOT01')) == restorable(changed)
    assert restorable(get_flash()) == restorable(other)
    fsck()

    # Other firmware gets its own slot, and doesn't go over the active one.
    third = bytearray(get_flash())
    third[FIRST:FIRST+0x1000] = b'GOODC'.ljust(0x1000, b'.')
    set_flash(third)
    line = run('load', 'SLOT01')
    assert facts(line)['slots'] == 'SLOT01,SLOT02,SLOT03,', line
    assert restorable(slot('SLOT03')) == restorable(third)
    assert restorable(slot('SLOT02')) == restorable(other)
    assert restorable(get_flash()) == restorable(changed)

    # Reloading the active slot with changes keeps the changes in a slot of their own, rather than losing the saved one.
    revert = bytearray(get_flash())
    revert[0x90000:0x91000] = b'\x55'*0x1000
    set_flash(revert)
    line = run('load', 'SLOT01')
    assert facts(line)['slots'] == 'SLOT01,SLOT02,SLOT03,SLOT04,', line
    assert restorable(slot('SLOT04')) == restorable(revert)
    assert restorable(get_flash()) == restorable(changed), 'it goes back to what was saved'
    run('delete', 'SLOT04')
    fsck()

    # What isn't valid firmware is never saved.
    broken = bytearray(get_flash())
    broken[FIRST:FIRST+0x1000] = b'\xff'*0x1000
    set_flash(broken)
    line = run('load', 'SLOT02')
    assert facts(line)['slots'] == 'SLOT01,SLOT02,SLOT03,', line
    assert restorable(slot('SLOT01')) == restorable(changed), 'the slot keeps the good copy'
    assert restorable(get_flash()) == restorable(other)

    # A slot that isn't valid, or the wrong size, is refused before anything changes.
    subprocess.run([MCOPY, '-o', '-i', card, folder/'flash', f'::/{FOLDER}/GOOD.BIN'], check=True, capture_output=True, env=ENV)
    bad = bytearray(get_flash())
    bad[FIRST] ^= 1
    (folder/'bad').write_bytes(bytes(bad))
    subprocess.run([MCOPY, '-o', '-i', card, folder/'bad', f'::/{FOLDER}/BAD.BIN'], check=True, capture_output=True, env=ENV)
    (folder/'short').write_bytes(bytes(10000))
    subprocess.run([MCOPY, '-o', '-i', card, folder/'short', f'::/{FOLDER}/SHORT.BIN'], check=True, capture_output=True, env=ENV)
    keep_flash, keep_card = get_flash(), card.read_bytes()
    for name in ('BAD', 'SHORT'):
        assert run('load', name, expect=3).startswith('error SLOT IS DAMAGED')
        assert get_flash() == keep_flash and card.read_bytes() == keep_card, name
    # A long name given on a computer keeps working.
    run('delete', 'BAD')
    run('delete', 'SHORT')
    run('delete', 'GOOD')
    fsck()

    # No room for what's on the unit: refused, with the flash untouched.
    blank(mib=34)
    set_flash(start)
    run('save')
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
    assert run('load', 'SLOT01', expect=3).startswith('error CARD IS FULL')
    assert get_flash() == keep_flash
    fsck(after_cut=True)

    # The power goes during a load that saves first. Whatever's there afterwards, trying again finishes it, with nothing lost.
    blank()
    set_flash(start)
    run('save')
    settings = bytearray(start)
    settings[0x90000:0x91000] = b'\x33'*0x1000
    set_flash(other)
    run('save')
    run('load', 'SLOT01')
    set_flash(settings)
    base_card, base_flash = card.read_bytes(), get_flash()
    target = restorable(other)
    saved_old = slot('SLOT01')
    total = int(run('load', 'SLOT02').split()[1])
    assert total > 500, total
    # Every change near the swap of the slot and the start and end of the flash writing would be hours, so a spread of them.
    cuts = sorted({*range(0, 12), *range(total-40, total), *rng.sample(range(total), 170)})
    print(f'  {total} changes in a load, cutting at {len(cuts)}')
    for cut in cuts:
        card.write_bytes(base_card)
        flash.write_bytes(base_flash)
        run('load', 'SLOT02', cut=cut, expect=42)
        fsck(after_cut=True)
        # Every slot is whole, whatever it holds.
        first = slot('SLOT01')
        assert first is not None and len(first) == SIZE and restorable(first) in (restorable(saved_old), restorable(settings)), cut
        assert restorable(slot('SLOT02')) == target, cut
        line = run('list')
        now = bytearray(get_flash())
        # Trying again finishes it, and what was on the unit is saved.
        line = run('load', 'SLOT02')
        assert restorable(get_flash()) == target, cut
        assert facts(line)['pending'] == '-' and facts(line)['here'].strip() == 'SLOT02', (cut, line)
        assert restorable(slot('SLOT01')) == restorable(settings), f'what was on the unit is lost, cut at {cut}'
        fsck(after_cut=True)
print('slots OK')
