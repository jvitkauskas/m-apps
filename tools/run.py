# SPDX-License-Identifier: MIT
"""Start an app with M-Flasher's run-ram, wait for it to end, and print its log. Without an app, print the log already
in RAM, however the app was started. Exit status: 0 the app returned 0, 1 it returned something else, 2 no result."""
import argparse
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import time
from applog import ADDRESS, RETURNED, SIZE, decode, describe

APP_ADDRESS, READER_ADDRESS = 0x01c30000, 0x01c28000


class Flasher:
    def __init__(self, command):
        self.command = shlex.split(command)

    def run(self, *args):
        return subprocess.run([*self.command, *args], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                              errors='replace', check=False)

    def state(self):
        """'download', 'running', or None with no unit."""
        lines = [line for line in self.run('find').stdout.splitlines() if line.strip()]
        if any(not line.startswith('midi') and '\t' in line for line in lines):
            return 'download'
        return 'running' if any(line.startswith('midi') for line in lines) else None

    def wait_for(self, state, seconds):
        end = time.monotonic()+seconds
        while time.monotonic() < end:
            if self.state() == state:
                time.sleep(1)
                return True
            time.sleep(0.5)
        return False

    def require(self, what, *args):
        done = self.run(*args)
        if done.returncode:
            sys.exit(f'{what} failed:\n{done.stdout}')
        return done.stdout


def to_download_mode(flasher):
    state = flasher.state()
    if state == 'running':
        flasher.require('download-mode', 'download-mode')
    elif state is None:
        sys.exit('no unit found')
    if not flasher.wait_for('download', 20):
        sys.exit('the unit did not reach download mode')


def read_log(flasher, reader):
    with tempfile.TemporaryDirectory() as folder:
        out = Path(folder)/'log.bin'
        flasher.require('reading the log', 'run-ram', str(reader), '--addr', f'0x{READER_ADDRESS:08X}',
                        '--read', f'0x{ADDRESS:08X}:{SIZE}', '-o', str(out))
        return out.read_bytes()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--flasher', default='m-flasher', help='how to call M-Flasher, such as "sudo m-flasher"')
    parser.add_argument('--reader', type=Path, required=True, help='build/<board>/logread.bin')
    parser.add_argument('--timeout', type=float, default=60, help='seconds to wait for the app to end')
    parser.add_argument('--stay', action='store_true', help='stay in download mode, rather than restarting at the end')
    parser.add_argument('app', type=Path, nargs='?', help='the .app file; without it, only read the log')
    args = parser.parse_args()
    flasher = Flasher(args.flasher)
    app = args.app.read_bytes() if args.app else None
    to_download_mode(flasher)
    if args.app:
        print(f'starting {args.app}', flush=True)
        flasher.require('run-ram', 'run-ram', str(args.app), '--addr', f'0x{APP_ADDRESS:08X}')
        # The ROM's USB goes away first. Give it a moment, so the old download mode isn't taken for the new.
        time.sleep(1)
        if not flasher.wait_for('download', args.timeout):
            state = flasher.state()
            if state == 'running':
                print('The unit went back to its firmware, which overwrites RAM, so the log is lost.')
            else:
                print(f'The app has not ended after {args.timeout:.0f} s. Its log needs download mode, and switching the '
                      'unit off loses it.')
            return 2
    log = decode(read_log(flasher, args.reader), app)
    print(describe(log), flush=True)
    if not args.stay:
        flasher.require('restart', 'restart')
    if not log['found'] or log['same_app'] is False or log['state'] != RETURNED:
        return 2
    return 0 if log['result'] == 0 else 1


if __name__ == '__main__':
    sys.exit(main())
