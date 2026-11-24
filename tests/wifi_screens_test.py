# SPDX-License-Identifier: MIT
"""Draw the WiFi app's screens on the computer (tests/wifi_screens_host.rs), for both boards, with overflow checks and
debug assertions. With an argument, it keeps the pictures there, as PPM."""
from pathlib import Path
import subprocess
import sys
import tempfile
sys.path.insert(0, str(Path(__file__).resolve().parent))
from host import build  # noqa: E402

with tempfile.TemporaryDirectory() as folder:
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(folder)
    out.mkdir(parents=True, exist_ok=True)
    host = build('wifi_screens_host')
    result = subprocess.run([host, out], capture_output=True, text=True, timeout=120)
    if result.returncode:
        sys.exit(result.stdout+result.stderr)
    assert len(list(out.glob('*.ppm'))) == 42, len(list(out.glob('*.ppm')))
print('wifi screens OK')
