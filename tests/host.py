# SPDX-License-Identifier: MIT
"""The tests' programs for this computer: tests/<name>.rs, built with the kit for this computer by the Makefile, with
overflow checks and debug assertions on."""
from pathlib import Path
import subprocess
ROOT = Path(__file__).resolve().parents[1]


def build(name):
    subprocess.run(['make', '-s', '-C', str(ROOT), f'build/host/{name}'], check=True)
    return ROOT/'build/host'/name
