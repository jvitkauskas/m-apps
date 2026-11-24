# SPDX-License-Identifier: MIT
"""Draw slots' screens on the computer (tests/slots_screens_host.rs), with overflow checks and debug assertions, for
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
    host = host_build('slots_screens_host')
    result = subprocess.run([host, out], capture_output=True, text=True, timeout=120)
    if result.returncode:
        sys.exit(result.stdout+result.stderr)
    assert len(list(out.glob('*.ppm'))) == 18
print('slots screens OK')
