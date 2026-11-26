# SPDX-License-Identifier: MIT
"""Draw usbdisk's screens on the computer (tests/usbdisk_look_host.rs), with overflow checks and debug assertions, for
both boards. With an argument, it keeps the pictures there, as PPM."""
from pathlib import Path
import subprocess
import sys
import tempfile
sys.path.insert(0, str(Path(__file__).resolve().parent))
from host import build as host_build  # noqa: E402

with tempfile.TemporaryDirectory() as folder:
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(folder)
    out.mkdir(parents=True, exist_ok=True)
    host = host_build('usbdisk_look_host')
    result = subprocess.run([host, out], capture_output=True, text=True, timeout=120)
    if result.returncode:
        sys.exit(result.stdout+result.stderr)
    shots = sorted(out.glob('*.ppm'))
    assert len(shots) == 14, len(shots)
    # Reading and writing move the packets, so those screens differ from connected and from each other.
    data = {p.stem: p.read_bytes() for p in shots}
    for board in '12':
        assert len({data[f'{board}-2-connected'], data[f'{board}-3-reading'], data[f'{board}-4-writing']}) == 3
print('usbdisk look OK')
