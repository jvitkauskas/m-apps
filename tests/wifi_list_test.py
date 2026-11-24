# SPDX-License-Identifier: MIT
"""apps/wifi/wifilist.rs on the computer (tests/wifi_list_host.rs), with overflow checks and debug assertions."""
from pathlib import Path
import subprocess
import sys
sys.path.insert(0, str(Path(__file__).resolve().parent))
from host import build  # noqa: E402

host = build('wifi_list_host')
result = subprocess.run([host], capture_output=True, text=True, timeout=60)
print(result.stdout, end='')
if result.returncode:
    sys.exit(result.stderr or 'wifi list FAILED')
