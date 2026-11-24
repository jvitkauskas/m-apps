# SPDX-License-Identifier: MIT
"""apps/jpeg's header parser on the computer (tests/jpeg_parse_host.rs), with overflow checks and debug assertions."""
from pathlib import Path
import subprocess
import sys
sys.path.insert(0, str(Path(__file__).resolve().parent))
from host import build  # noqa: E402

host = build('jpeg_parse_host')
result = subprocess.run([host], capture_output=True, text=True, timeout=60)
print(result.stdout, end='')
if result.returncode:
    sys.exit(result.stderr)
