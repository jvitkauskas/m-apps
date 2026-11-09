# SPDX-License-Identifier: MIT
"""A folder as a plain tar, for the store to unpack onto a card: ustar, its files sorted, every name 8.3 in capitals as
the kit's card code writes them, owned by nobody, dated 0, and nothing after the two zero blocks that end it, so the
same files always make the same bytes.

  app_tar.py OUT.tar FOLDER"""
import argparse
from pathlib import Path
import re
import sys
import tarfile

PART = re.compile(r'[A-Z0-9_-]{1,8}(\.[A-Z0-9_-]{1,3})?')
BLOCK = 512


def entries(folder):
    """(name in the tar, path) for each file and folder under folder, sorted; or ValueError for a name a card can't
    hold."""
    out = []
    for path in sorted(folder.rglob('*')):
        name = path.relative_to(folder).as_posix()
        for part in name.split('/'):
            if not PART.fullmatch(part):
                raise ValueError(f'{name}: not an 8.3 name in capitals')
        if path.is_symlink() or not (path.is_file() or path.is_dir()):
            raise ValueError(f'{name}: only files and folders')
        out.append((name, path))
    return out


def make(folder):
    out = bytearray()
    for name, path in entries(folder):
        info = tarfile.TarInfo(name+'/' if path.is_dir() else name)
        info.mtime, info.uid, info.gid, info.uname, info.gname = 0, 0, 0, '', ''
        if path.is_dir():
            info.type, info.mode = tarfile.DIRTYPE, 0o755
            out += info.tobuf(tarfile.USTAR_FORMAT)
            continue
        data = path.read_bytes()
        info.size, info.mode = len(data), 0o644
        out += info.tobuf(tarfile.USTAR_FORMAT) + data + bytes(-len(data) % BLOCK)
    return bytes(out + bytes(2*BLOCK))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('output', type=Path)
    parser.add_argument('folder', type=Path)
    args = parser.parse_args()
    try:
        data = make(args.folder)
    except ValueError as e:
        sys.exit(f'app_tar.py: {args.folder}: {e}')
    args.output.write_bytes(data)


if __name__ == '__main__':
    main()
