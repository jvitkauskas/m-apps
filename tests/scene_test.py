# SPDX-License-Identifier: MIT
"""Draw the launcher's screen on the computer (tests/scene_host.c), with the sanitizers: icons through tools/icon.py's
encoder and apps/picker/qoi.c, the selection moving, and the panel for nothing to start."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT/'tools'))
from icon import SIZE, qoi

FRAME = 240*240*2


def icon(seed):
    """An icon with every kind of QOI step in it: runs, small and large changes, and transparency."""
    out = bytearray()
    for y in range(SIZE):
        for x in range(SIZE):
            if x < 8:
                out += bytes([200, 40, 40, 255])
            elif y < 8:
                out += bytes([(x*seed) & 255, (y*3) & 255, (x+y) & 255, 255])
            else:
                out += bytes([(x*7+seed) & 255, (y*5) & 255, 120, 0 if (x+y) % 11 == 0 else 255])
    return bytes(out)


with tempfile.TemporaryDirectory() as folder:
    folder = Path(folder)
    host = folder/'host'
    subprocess.run([os.environ.get('HOSTCC', 'cc'), '-std=c11', '-g', '-Wall', '-Wextra', '-Werror',
                    '-fsanitize=address,undefined', f'-I{ROOT}/apps/picker', ROOT/'tests/scene_host.c',
                    ROOT/'apps/picker/scene.c', ROOT/'apps/picker/qoi.c', '-o', host], check=True)
    icons = []
    for seed in (3, 5):
        path = folder/f'{seed}.qoi'
        path.write_bytes(qoi(SIZE, SIZE, icon(seed)))
        icons.append(path)
    frames = 121
    subprocess.run([host, folder/'out', str(frames), *icons], check=True, timeout=120)
    data = (folder/'out').read_bytes()
    assert len(data) == (frames+1)*FRAME, len(data)
    shots = [data[i*FRAME:(i+1)*FRAME] for i in range(frames+1)]
    # The bubbles move every frame; the tiles change when the selection does.
    assert all(shots[i] != shots[i+1] for i in range(frames-1))
    # A broken icon is refused, not drawn.
    bad = folder/'bad.qoi'
    bad.write_bytes(qoi(SIZE, SIZE, icon(3))[:200])
    assert subprocess.run([host, folder/'out', '1', bad], timeout=60).returncode == 3
    big = folder/'big.qoi'
    big.write_bytes(qoi(SIZE+1, SIZE, bytes(4*(SIZE+1)*SIZE)))
    assert subprocess.run([host, folder/'out', '1', big], timeout=60).returncode == 3
print('scene OK')
