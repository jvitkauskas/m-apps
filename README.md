# m-apps

Programs that run from RAM on the M-VAVE FM-1 and SMK-37, without changing the firmware in flash. This repo is the kit for building them, and the apps.

An app is one file. The same file can be started from an SD card, over MIDI, or with M-Flasher's `run-ram`. The format and the rules are in [m-bootloader's SPEC, §11.8](https://github.com/jvitkauskas/m-bootloader/blob/main/docs/SPEC.md).

**Status:** early. `hello` has run on an FM-1 and an SMK-37, from `run-ram` and over MIDI.

- **`run-ram`:** `make run` below uses this. The address needs an M-Flasher newer than 0.9.1, which isn't released yet.
- **MIDI:** m-bootloader 2.2's MIDI mode and RAM stage can load and run an app, but nothing public sends one yet. M-Flasher's page has no button for it.
- **SD card:** not available yet. It needs a picker on the card, which comes later.

Writing apps with an agent? Start with [AGENTS.md](AGENTS.md), which has the loop, the rules and the traps found so far.

## Build

You need the [pi32v2 LLVM toolchain](https://github.com/jvitkauskas/pi32v2-llvm) (see m-bootloader's README for the release) and Python 3.

```sh
git clone --recurse-submodules https://github.com/jvitkauskas/m-apps
cd m-apps
make TOOLCHAIN=/path/to/toolchain
```

This makes `build/fm1/hello.app` and `build/smk37/hello.app`. The apps use m-bootloader's drivers, which come in as the `m-bootloader` submodule.

## Run

This starts an app, waits for it to end, prints its log and restarts the unit, from firmware or download mode:

```sh
make run APP=hello BOARD=fm1 TOOLCHAIN=/path/to/toolchain M_FLASHER="sudo m-flasher"
```

`hello` shows its screen for 10 seconds or until a key press, then this prints:

```
started by the ROM, LSB 48 MHz
hello on board 1
screen found
no key in 10000 ms
app_main returned 0
--- returned 0
```

The exit status is 0 if the app returned 0, 1 if it returned something else, and 2 if it crashed, hung or left no log. `RUN_FLAGS=--stay` stays in download mode at the end, and `RUN_FLAGS="--timeout 90"` waits longer than the default 60 seconds. On macOS, this has to run in Terminal on the Mac, as macOS refuses access over SSH.

## The log

`app_log("x=%u", x)` adds a line to a ring buffer in RAM, at `0x01C7F000`. It knows `%s %c %d %u %x %%` and widths like `%08x`. The buffer holds about 3.4 KiB, and the oldest lines go first. Each line is written straight to RAM, so it survives whatever happens next.

The route doesn't matter, because every route ends the same way:

| How the app ends | What happens | Getting the log |
|---|---|---|
| Returns, started by `run-ram` | Resets into download mode | `make run` reads it |
| Returns, started over MIDI or from the card | Goes back to what started it | `make log BOARD=fm1` |
| Crashes | Shows `APP CRASHED`, then resets into download mode after 8 seconds. The FM-1's screen goes blank then, and the SMK-37's keeps the message | `make log BOARD=fm1` |
| Hangs | The watchdog resets it into download mode after 32 seconds | `make log BOARD=fm1` |

`make log` asks a running unit to go to download mode, then uses `build/<board>/logread.bin` to read the log. If the unit is already in download mode, it just reads. **Switching the unit off loses the log,** and starting firmware probably does too.

A crash line looks like `CRASH: cause 00000001 near 01c30426, last call from 01c303d0`. Look those up in `build/<board>/<app>.map`. "Near" means the exception is taken a little late: it's been seen one call after the bad instruction. Causes seen so far: 1 for a misaligned read, and 2 for a jump to a bad address. Not everything crashes: a write to unmapped memory carried on silently, and a run of bad instruction bytes ended back in firmware, with the log lost.

**The guard** is what makes this work, and it's on when `app_main` starts. It arms the watchdog, sets the download-mode request and installs a crash handler, and undoes all of that if the app returns.

- A hang is 32 seconds without `app_log()` or `app_feed()`, so call one of them in anything slower.
- `app_guard(false)` turns it off, for work that must not be cut short, such as writing flash.

`make run APP=crashtest` and `make run APP=hangtest` test it. Both should exit with 2.

## Write an app

Add a folder in `apps/`, add its name to `APPS` in the `Makefile`, and write `app_main`:

```c
#include "app.h"

int app_main(unsigned lsb_mhz) {
    /* Interrupts are off, and you're on the app's own stack. */
    return 0;
}
```

- `kit/` holds the entry, the C start and the linker script. It clears `.bss` every time the app starts.
- `lsb_mhz` is the LSB clock the app was started at. Use it to work out timings, as it differs by route.
- Returning goes back to what started the app, or to download mode if that was the ROM. An app can only return if it stayed within the RAM §11.8 allows.
- The file and its RAM have to end by `0x01C7F000`, as the log takes the top of the slot §11.8 describes.
- `APP_BOARD` is 1 on the FM-1 and 2 on the SMK-37, and each board gets its own build.
- An app needing a larger stack sets `<name>_STACK := 0x8000` in the `Makefile`. The default is 16 KiB.

## Apps

### usbdisk

Shows the unit's SD card to the computer as a USB disk, so files can be copied without taking the card out. It needs a card mod, and has run on an FM-1 and an SMK-37.

| Card pin | FM-1 | SMK-37 |
|---|---|---|
| CLK | PC9 | PB6 |
| DI | PC10 | PB7 |
| DO | PC8, through 220 ohms | PB5 |
| CS | NOT PC7 | NOT PC8 |

DO shares the panel's D/C, and CS is the inverse of the panel's CS.

```sh
make run APP=usbdisk BOARD=smk37 RUN_FLAGS="--timeout 3600" M_FLASHER="sudo m-flasher"
```

**To leave,** eject the disk on the computer, which is the way to use normally, or press PLAY (SEQ PLAY on the SMK-37), which doesn't wait for the computer to finish, so eject first if you can. With no computer attached, it also leaves after 2 minutes. It doesn't leave just because the disk is idle, as a computer can hold writes back for a while.

A write has reached the card before the computer is told it succeeded, and data is checked by CRC in both directions. It picks the fastest card clock that reads cleanly, from 24 MHz down to 4. From `run-ram`, an FM-1 prototype with long wires settled on 8 MHz, at about 750 KB/s reading and 440 KB/s writing. An SMK-37 settled on 12 MHz, at about 870 KB/s reading and 440 KB/s writing. USB 1.1 limits it to around 1 MB/s whatever the clock.

**On macOS:**
- macOS gives up on mounting after 20 s (`ETIMEDOUT`), and it seems to read the whole FAT in that time. That's about 10 s on a 32 GB card with 16 KiB clusters. A larger FAT or a slower clock could fail to mount.
- Spotlight can hold the disk for a while, so Eject may be refused at first. It took 38 s in testing. The app ends once the eject goes through.
- macOS adds its own hidden `.Spotlight-V100` and `.fseventsd` folders to the card.

## License

MIT, as m-bootloader is. See [LICENSE](LICENSE).
