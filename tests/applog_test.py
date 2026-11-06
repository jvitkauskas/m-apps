# SPDX-License-Identifier: MIT
"""Run kit/log.c on the computer, and check tools/applog.py against what it writes."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT/'tools'))
from applog import CAPACITY, CRASHED, RETURNED, RUNNING, SIZE, decode, describe

TRAILER = b'APP1\x12\x34\x01\x01'
with tempfile.TemporaryDirectory() as folder:
    folder = Path(folder)
    subprocess.run([os.environ.get('HOSTCC', 'cc'), '-std=c11', '-Wall', '-Wextra', '-Werror', '-Wno-format',
                    '-fsanitize=address,undefined', ROOT/'tests/log_host.c', '-o', folder/'host'], check=True)
    subprocess.run([folder/'host', folder/'a', folder/'b', folder/'c', folder/'d'], check=True)
    running, returned, wrapped, crashed = (decode((folder/name).read_bytes(), b'x'+TRAILER) for name in 'abcd')

assert running['same_app'] and running['state'] == RUNNING and running['lost'] == 0
assert running['text'] == 'plain\ntext c -42 42 abc 00000abc     7 % %q\n-2147483648\n', running['text']
assert 'never returned' in describe(running)
assert returned['state'] == RETURNED and returned['result'] == -3
assert describe(returned).endswith('--- returned -3')
# The ring wrapped: the newest line is there, whole lines only, and the count of what went is right.
assert wrapped['lost'] > 0 and wrapped['text'].endswith('line 399\n')
lines = wrapped['text'].splitlines()
assert all(line.startswith('line ') for line in lines), lines[0]
assert [int(line[5:]) for line in lines] == list(range(int(lines[0][5:]), 400))
assert len(wrapped['text']) <= CAPACITY
assert 'earlier bytes overwritten' in describe(wrapped)

assert crashed['state'] == CRASHED and describe(crashed).endswith('--- crashed')

assert not decode(bytes(SIZE))['found']
assert not decode(b'ALOG')['found']
assert describe(decode(bytes(SIZE))) == 'no log in RAM'
other = decode(b'ALOG'+(1).to_bytes(4, 'little')+bytes(SIZE-8), TRAILER)
assert other['same_app'] is False and 'another app file' in describe(other)
print('app log OK')
