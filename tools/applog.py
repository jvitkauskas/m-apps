# SPDX-License-Identifier: MIT
"""Decode an app's log read from RAM: the layout is in kit/log.h."""
import argparse
from pathlib import Path
import struct

ADDRESS, SIZE, HEADER = 0x01c7f000, 0xd80, 32
CAPACITY = SIZE-HEADER
MAGIC, FORMAT = b'ALOG', 1
RUNNING, RETURNED, CRASHED = 1, 2, 3


def decode(raw, app=None):
    """What the log says: found, whether it's from this app file, state, result, lost bytes, text."""
    if len(raw) < SIZE or raw[:4] != MAGIC:
        return {'found': False}
    fmt, total, state, result = struct.unpack_from('<I', raw, 4)[0], *struct.unpack_from('<IIi', raw, 16)
    if fmt != FORMAT:
        return {'found': False, 'reason': f'log format {fmt}'}
    ring = raw[HEADER:SIZE]
    lost = max(0, total-CAPACITY)
    if lost:
        at = total % CAPACITY
        text = ring[at:]+ring[:at]
        # The oldest line lost its start, so it goes too.
        text = text[text.find(b'\n')+1:]
    else:
        text = ring[:total]
    return {
        'found': True,
        'same_app': None if app is None else raw[8:16] == app[-8:],
        'state': state,
        'result': result,
        'lost': lost,
        'text': text.decode('utf-8', 'replace'),
    }


def describe(log):
    """The log as a person or an agent reads it, ending with how the app ended."""
    if not log['found']:
        return 'no log in RAM'+(f": {log['reason']}" if 'reason' in log else '')
    out = []
    if log['same_app'] is False:
        out.append('WARNING: this log is from another app file')
    if log['lost']:
        out.append(f"... {log['lost']} earlier bytes overwritten")
    out.append(log['text'].rstrip('\n'))
    if log['state'] == RETURNED:
        out.append(f"--- returned {log['result']}")
    elif log['state'] == CRASHED:
        out.append('--- crashed')
    else:
        out.append('--- never returned: it hung, or something reset the unit')
    return '\n'.join(out)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('log', type=Path, help=f'{SIZE} bytes read from 0x{ADDRESS:08X}')
    parser.add_argument('--app', type=Path, help='the app file, to check that the log is from it')
    args = parser.parse_args()
    print(describe(decode(args.log.read_bytes(), args.app.read_bytes() if args.app else None)))


if __name__ == '__main__':
    main()
