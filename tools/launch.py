# SPDX-License-Identifier: MIT
"""Add the trailer to the card launcher (apps/picker), making /MBOOT/LAUNCH.BIN, which MENU.BIN (apps/menu) loads to
0x01C0A800: LNC1, the CRC16 of what's before it, the board and the format, as an app's trailer has them."""
import argparse
from pathlib import Path
import struct
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'m-bootloader/tools'))
from bankcb import crc16

LARGEST = 0xc000
BOARDS = {'fm1': 1, 'smk37': 2}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('board', choices=BOARDS)
    parser.add_argument('input', type=Path, help='the flat binary, linked for 0x01C0A800')
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    code = args.input.read_bytes()
    if not 16 <= len(code)+8 <= LARGEST:
        parser.exit(1, f'{args.input}: {len(code)+8} bytes with the trailer, not 16 to {LARGEST}\n')
    args.output.write_bytes(code+b'LNC1'+struct.pack('<HBB', crc16(code), BOARDS[args.board], 1))


if __name__ == '__main__':
    main()
