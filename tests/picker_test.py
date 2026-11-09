# SPDX-License-Identifier: MIT
"""Run apps/picker's FAT and loader code on the computer, on generated card images, good and malformed."""
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT/'m-bootloader/tools'))
from bankcb import crc16

SECTOR, ENTRY = 512, 32
RESERVED, FAT_SECTORS, FATS = 32, 64, 2
END = 0x0fffffff
GUARD = 64
LOAD_BUFFER = 0x4fd80+GUARD


def app(code, board=2, fmt=1):
    return code+b'APP1'+struct.pack('<HBB', crc16(code), board, fmt)


def kit_app(body=b'b'*200, entry=0x01c30070, magic=b'MAPP'):
    """An app as the kit builds it: the entry's jump, the header (kit/app_launch.h), then the rest."""
    return app(b'\xc0\xea\x3c\x00'+magic+bytes([1, 0, 0, 0])+struct.pack('<I', entry)+body)


class Card:
    """A FAT32 volume. Clusters are handed out in the order asked for, so files can be fragmented."""

    def __init__(self, per_cluster=1, clusters=4000, base=0):
        self.per_cluster, self.base = per_cluster, base
        self.clusters = clusters
        self.total = RESERVED+FATS*FAT_SECTORS+clusters*per_cluster
        self.fat = {0: 0x0ffffff8, 1: END}
        self.data = {}
        self.free = 3
        self.root = 2
        self.fat[2] = END
        self.folders = {2: []}

    def allocate(self, count, step=1):
        out = [self.free+i*step for i in range(count)]
        self.free += count*step
        return out

    def chain(self, clusters, end=END):
        for here, following in zip(clusters, clusters[1:]+[end]):
            self.fat[here] = following

    def write_file(self, folder, name, ext, content, step=1, size=None, attr=0x20):
        bytes_per = self.per_cluster*SECTOR
        clusters = self.allocate(max(1, (len(content)+bytes_per-1)//bytes_per), step) if content else [0]
        if content:
            self.chain(clusters)
            for i, c in enumerate(clusters):
                self.data[c] = content[i*bytes_per:(i+1)*bytes_per]
        self.add_entry(folder, name, ext, attr, clusters[0], len(content) if size is None else size)
        return clusters

    def add_entry(self, folder, name, ext, attr, start, size):
        entry = name.ljust(8).encode()+ext.ljust(3).encode()+bytes([attr])+bytes(8)
        entry += struct.pack('<HHHHI', start >> 16, 0, 0, start & 0xffff, size)
        self.folders[folder].append(entry)

    def add_raw(self, folder, entry):
        self.folders[folder].append(entry)

    def make_folder(self, parent, name, clusters_needed=1):
        clusters = self.allocate(clusters_needed)
        self.chain(clusters)
        self.folders[clusters[0]] = []
        self.add_entry(parent, name, '', 0x10, clusters[0], 0)
        return clusters[0]

    def walk_chain(self, first):
        out, c, seen = [], first, set()
        while 2 <= c < 2+self.clusters and c not in seen:
            out.append(c)
            seen.add(c)
            c = self.fat.get(c, END)
        return out

    def build(self, mbr=True):
        data_at = RESERVED+FATS*FAT_SECTORS
        volume = bytearray((self.total)*SECTOR)
        boot = bytearray(SECTOR)
        boot[11:13] = struct.pack('<H', SECTOR)
        boot[13] = self.per_cluster
        boot[14:16] = struct.pack('<H', RESERVED)
        boot[16] = FATS
        boot[32:36] = struct.pack('<I', self.total)
        boot[36:40] = struct.pack('<I', FAT_SECTORS)
        boot[44:48] = struct.pack('<I', self.root)
        boot[510:512] = b'\x55\xaa'
        volume[:SECTOR] = boot
        bytes_per = self.per_cluster*SECTOR
        for folder, entries in self.folders.items():
            body = b''.join(entries)
            for i, c in enumerate(self.walk_chain(folder)):
                self.data[c] = body[i*bytes_per:(i+1)*bytes_per]
        for c, content in self.data.items():
            at = (data_at+(c-2)*self.per_cluster)*SECTOR
            volume[at:at+len(content)] = content
        for copy in range(FATS):
            at = (RESERVED+copy*FAT_SECTORS)*SECTOR
            for c, value in self.fat.items():
                volume[at+c*4:at+c*4+4] = struct.pack('<I', value)
        if not mbr:
            return bytes(volume)
        first = bytearray(SECTOR)
        first[450] = 0x0c
        first[454:458] = struct.pack('<I', self.base)
        first[510:512] = b'\x55\xaa'
        return bytes(first)+bytes((self.base-1)*SECTOR)+bytes(volume)


def standard(**args):
    card = Card(**args)
    mboot = card.make_folder(card.root, 'MBOOT')
    apps = card.make_folder(mboot, 'APPS')
    return card, apps


with tempfile.TemporaryDirectory() as folder:
    folder = Path(folder)
    host = folder/'host'
    subprocess.run([os.environ.get('HOSTCC', 'cc'), '-std=c11', '-Wall', '-Wextra', '-Werror', '-fsanitize=address,undefined',
                    f'-I{ROOT}/apps/picker', f'-I{ROOT}/kit', '-idirafter', f'{ROOT}/m-bootloader/include', '-DPLATFORM_SIM',
                    ROOT/'tests/fat_host.c', ROOT/'kit/fat.c', ROOT/'kit/load.c', ROOT/'apps/picker/catalog.c',
                    ROOT/'m-bootloader/src/core.c',
                    '-o', host], check=True)

    def run(image, name='-', board=2, fail='-'):
        (folder/'card').write_bytes(image)
        out = subprocess.run([host, folder/'card', str(board), name, folder/'loaded', str(fail)], capture_output=True, text=True,
                             check=True, timeout=30).stdout.splitlines()
        return out

    def listing(lines):
        return [line.split()[1:] for line in lines if line.startswith('app ')]

    def loaded(content):
        """What's in the buffer: the file's bytes exactly, and nothing written past them."""
        raw = (folder/'loaded').read_bytes()
        assert len(raw) == LOAD_BUFFER
        assert raw[:len(content)] == content, 'the file came out different'
        assert set(raw[len(content):]) == {0xa5}, 'something was written past the file'

    # A normal card: only .APP files, in order, and with long-name pieces, deleted files and folders ignored.
    card, apps = standard(base=2048)
    card.write_file(apps, 'ZED', 'APP', app(b'z'*100))
    card.add_raw(apps, b'\xe5ONE    APP'+bytes([0x20])+bytes(20))
    card.add_raw(apps, b'\x41l\x00o\x00n\x00g\x00 \x00'+bytes([0x0f])+bytes(20))
    card.write_file(apps, 'ALPHA', 'APP', app(b'a'*1000))
    card.write_file(apps, 'README', 'TXT', b'text')
    card.add_entry(apps, 'SUB', 'APP', 0x10, 5, 0)
    card.add_entry(apps, 'LABEL', 'APP', 0x08, 0, 0)
    card.write_file(apps, 'MID', 'APP', app(b'm'*5000))
    lines = run(card.build())
    assert lines[0] == 'list ok 3 0', lines
    assert listing(lines) == [['ALPHA', '1008'], ['MID', '5008'], ['ZED', '108']], lines

    # No partition table, and a larger cluster.
    card, apps = standard(per_cluster=8)
    code = app(bytes(range(256))*20)
    card.write_file(apps, 'A', 'APP', code)
    assert run(card.build(mbr=False), 'A')[-1] == 'load ok'
    loaded(code)

    # What isn't FAT32, or isn't there.
    assert run(bytes(SECTOR*100))[0].startswith('list CARD IS NOT FAT32')
    first = bytearray(SECTOR*100)
    first[510:512] = b'\x55\xaa'
    first[454:458] = struct.pack('<I', 0xfffffff0)
    assert run(bytes(first))[0].startswith('list CARD IS NOT FAT32')
    fat16 = bytearray(Card().build(mbr=False))
    fat16[22:24] = b'\x10\x00'
    assert run(bytes(fat16))[0].startswith('list CARD IS NOT FAT32')
    shrunk = Card().build(mbr=False)
    assert run(shrunk[:SECTOR*200])[0].startswith('list CARD IS NOT FAT32')
    assert run(Card().build(mbr=False))[0].startswith('list NO /MBOOT/APPS')
    card = Card()
    card.make_folder(card.root, 'MBOOT')
    assert run(card.build(mbr=False))[0].startswith('list NO /MBOOT/APPS')
    card, apps = standard()
    assert run(card.build(mbr=False))[0] == 'list ok 0 0'

    # A folder across clusters, with more files than fit: the first ones by name stay.
    card = Card()
    mboot = card.make_folder(card.root, 'MBOOT')
    apps = card.make_folder(mboot, 'APPS', clusters_needed=20)
    for i in reversed(range(150)):
        card.write_file(apps, f'F{i:03}', 'APP', app(b'x'*20))
    lines = run(card.build(mbr=False))
    assert lines[0] == 'list ok 100 1', lines[0]
    assert [name for name, _ in listing(lines)] == [f'F{i:03}' for i in range(100)]

    # A directory whose chain loops ends with an error, and doesn't run forever.
    card = Card()
    mboot = card.make_folder(card.root, 'MBOOT')
    apps = card.make_folder(mboot, 'APPS', clusters_needed=2)
    second = card.fat[apps]
    for i in range(32):
        card.write_file(apps, f'L{i}', 'APP', app(b'y'*20))
    image = bytearray(card.build(mbr=False))
    for copy in range(FATS):
        at = (RESERVED+copy*FAT_SECTORS)*SECTOR+second*4
        image[at:at+4] = struct.pack('<I', apps)
    # Fill both sectors so no zero entry ends the listing first.
    data_at = RESERVED+FATS*FAT_SECTORS
    for c in (apps, second):
        at = (data_at+(c-2))*SECTOR
        for i in range(16):
            image[at+i*ENTRY:at+(i+1)*ENTRY] = b'N       TXT '+bytes(20)
    assert run(bytes(image))[0].startswith('list BAD FOLDER ON CARD')
    # A folder pointing outside the volume.
    card = Card()
    mboot = card.make_folder(card.root, 'MBOOT')
    card.add_entry(mboot, 'APPS', '', 0x10, 0x0fffff00, 0)
    assert run(card.build(mbr=False))[0].startswith('list BAD FOLDER ON CARD')

    # Fragmented files, and sizes either side of a sector, with exactly the file's bytes written. The trailer can fall across two.
    for size in (16, 505, 508, 510, 512, 513, 516, 1020, 1024, 4097, 70001):
        code = app(bytes((i*7) & 255 for i in range(size-8)))
        card, apps = standard()
        card.write_file(apps, 'FRAG', 'APP', code, step=3)
        assert run(card.build(mbr=False), 'FRAG')[-1] == 'load ok', size
        loaded(code)

    # What gets refused, with nothing run.
    def refused(content, expected, board=2, **args):
        card, apps = standard()
        card.write_file(apps, 'R', 'APP', content, **args)
        result = run(card.build(mbr=False), 'R', board)[-1]
        assert result == f'load {expected}', (result, expected)

    good = app(b'g'*600)
    refused(app(b'g'*600, board=1), 'OTHER BOARD')
    refused(app(b'g'*600, board=2), 'OTHER BOARD', board=1)
    card, apps = standard()
    card.write_file(apps, 'ANY', 'APP', app(b'g'*600, board=0))
    assert run(card.build(mbr=False), 'ANY')[-1] == 'load ok'
    refused(app(b'g'*600, fmt=2), 'NEWER FILE FORMAT')
    refused(b'g'*600+b'STG2'+struct.pack('<HBB', crc16(b'g'*600), 0, 0), 'NOT AN APP FILE')
    refused(b'short', 'NOT AN APP FILE')
    refused(good[:100]+b'!'+good[101:], 'APP FILE DAMAGED')
    refused(app(b'g'*0x50000), 'APP TOO BIG')
    # The largest that loads ends where the log starts.
    refused(app(b'g'*(0x4f000-8+1)), 'APP TOO BIG')
    code = app(bytes((i*3) & 255 for i in range(0x4f000-8)))
    card, apps = standard()
    card.write_file(apps, 'MOST', 'APP', code)
    assert run(card.build(mbr=False), 'MOST')[-1] == 'load ok'
    loaded(code)
    # A file that claims more than its chain holds.
    refused(good, 'CARD READ ERROR', size=len(good)+SECTOR*4)
    refused(good, 'APP TOO BIG', size=0x7fffffff)
    # A chain that leaves the volume, and one that loops: the size limits how far it goes.
    card, apps = standard()
    clusters = card.write_file(apps, 'OUT', 'APP', app(b'o'*3000))
    card.fat[clusters[1]] = 0x0ffffff0
    assert run(card.build(mbr=False), 'OUT')[-1] == 'load CARD READ ERROR'
    card, apps = standard()
    clusters = card.write_file(apps, 'LOOP', 'APP', app(b'o'*3000))
    card.fat[clusters[3]] = clusters[0]
    assert run(card.build(mbr=False), 'LOOP')[-1] == 'load NOT AN APP FILE'

    # A sector that can't be read.
    card, apps = standard()
    clusters = card.write_file(apps, 'BAD', 'APP', app(b'b'*3000))
    image = card.build(mbr=False)
    data_at = RESERVED+FATS*FAT_SECTORS
    assert run(image, 'BAD', fail=data_at+clusters[2]-2)[-1] == 'load CARD READ ERROR'
    assert run(image, fail=0)[0].startswith('list CARD READ ERROR')
    assert run(image, 'BAD', fail=RESERVED)[-1] == 'load CARD READ ERROR'
    # The launcher's list (apps/picker/catalog.c): a folder each, with BOOT.APP, by the name shown.
    def catalog(image):
        lines = run(image, '@catalog')
        return lines[0], [line[4:].split('|') for line in lines[1:] if line.startswith('app ')]

    card, apps = standard()
    zed = card.make_folder(apps, 'ZED')
    card.write_file(zed, 'BOOT', 'APP', kit_app())
    card.write_file(zed, 'META', 'TXT', b'name=zed\nversion=2.0\n')
    mp3 = card.make_folder(apps, 'MP3')
    card.write_file(mp3, 'BOOT', 'APP', kit_app(b'm'*300))
    card.write_file(mp3, 'ICON', 'QOI', b'i'*500)
    card.write_file(mp3, 'META', 'TXT', b'name=MP3 Player\r\nabout = Plays the MP3 files on the card \r\n# a note\nauthor=x\n')
    card.write_file(mp3, 'SONGS', 'DAT', b'd'*100)
    empty = card.make_folder(apps, 'EMPTY')
    card.write_file(empty, 'README', 'TXT', b'no app here')
    bare = card.make_folder(apps, 'BARE')
    card.write_file(bare, 'BOOT', 'APP', app(b'x'*100))
    # An app on its own, as before folders, isn't one.
    card.write_file(apps, 'LONE', 'APP', kit_app())
    head, listed = catalog(card.build(mbr=False))
    assert head == 'catalog ok 3 0', head
    assert listed == [
        ['BARE    ', '108', '0', 'BARE', '', '', '0'],
        ['MP3     ', str(len(kit_app(b'm'*300))), '500', 'MP3 Player', 'Plays the MP3 files on the card', '', '1c30070'],
        ['ZED     ', str(len(kit_app())), '0', 'zed', '', '2.0', '1c30070'],
    ], listed

    # META.TXT: long values cut, characters the screen hasn't got as one ?, empty values and odd lines ignored, at most
    # 512 bytes read. A second entry outside the app, or a header that isn't the kit's, isn't used.
    card, apps = standard()
    odd = card.make_folder(apps, 'ODD')
    card.write_file(odd, 'BOOT', 'APP', kit_app(entry=0x01c00000))
    card.write_file(odd, 'META', 'TXT', ('name=Café ♫ '+'x'*40+'\nabout=\nversion\n=nothing\nversion=3.1\n').encode()
                    + b'\n'*600+b'name=too late\n')
    other = card.make_folder(apps, 'OTHER')
    card.write_file(other, 'BOOT', 'APP', kit_app(magic=b'INFO'))
    head, listed = catalog(card.build(mbr=False))
    assert listed == [['ODD     ', str(len(kit_app())), '0', 'Caf? ? '+'x'*17, '', '3.1', '0'],
                      ['OTHER   ', str(len(kit_app())), '0', 'OTHER', '', '', '0']], listed

    # Folders without apps, and none at all: nothing to start.
    card, apps = standard()
    card.make_folder(apps, 'NOTHING')
    assert catalog(card.build(mbr=False))[0] == 'catalog NO APPS IN /MBOOT/APPS 0 0'
    card, apps = standard()
    assert catalog(card.build(mbr=False))[0] == 'catalog NO APPS IN /MBOOT/APPS 0 0'
print('picker OK')
