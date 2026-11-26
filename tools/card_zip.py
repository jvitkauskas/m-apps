# SPDX-License-Identifier: MIT
"""A folder as a zip, to unpack onto a card's root: its files sorted, every name 8.3 in capitals as the kit's card
code writes them (app_tar.py's check), dated 1980-01-01, so the same files always make the same bytes.

  card_zip.py OUT.zip FOLDER"""
import argparse
import io
from pathlib import Path
import sys
import zipfile

from app_tar import entries

DATE = (1980, 1, 1, 0, 0, 0)


def make(folder):
    """The zip's bytes; or ValueError, from app_tar.entries."""
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, 'w') as z:
        for name, path in entries(folder):
            if path.is_dir():
                info = zipfile.ZipInfo(name + '/', DATE)
                info.external_attr = (0o40755 << 16) | 0x10
                z.writestr(info, b'')
            else:
                info = zipfile.ZipInfo(name, DATE)
                info.external_attr = 0o100644 << 16
                info.compress_type = zipfile.ZIP_DEFLATED
                z.writestr(info, path.read_bytes(), compresslevel=9)
    return buf.getvalue()


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('out', type=Path)
    p.add_argument('folder', type=Path)
    a = p.parse_args()
    try:
        data = make(a.folder)
    except ValueError as e:
        sys.exit(f'card_zip.py: {a.folder}: {e}')
    a.out.write_bytes(data)


if __name__ == '__main__':
    main()
