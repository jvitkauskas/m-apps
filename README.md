# m-apps

Programs that run from RAM on the M-VAVE FM-1 and SMK-37, without changing the firmware in flash. This repo is the kit for building them, and the apps.

An app is one file. The same file can be started from an SD card, over MIDI, or with M-Flasher's `run-ram`. The format and the rules are in [m-bootloader's SPEC, §11.8](https://github.com/jvitkauskas/m-bootloader/blob/main/docs/SPEC.md).

**Status:** early. `hello` has run from `run-ram` on an SMK-37. Starting apps from the card and over MIDI isn't finished, and `run-ram` only accepts this address in an M-Flasher build that isn't released yet.

## Build

You need the [pi32v2 LLVM toolchain](https://github.com/jvitkauskas/pi32v2-llvm) (see m-bootloader's README for the release) and Python 3.

```sh
git clone --recurse-submodules https://github.com/jvitkauskas/m-apps
cd m-apps
make TOOLCHAIN=/path/to/toolchain
```

This makes `build/fm1/hello.app` and `build/smk37/hello.app`. The apps use m-bootloader's drivers, which come in as the `m-bootloader` submodule.

## Run

With the unit in download mode:

```sh
m-flasher run-ram build/smk37/hello.app --addr 0x01C30000
```

M-Flasher says it can't confirm the start, which is normal here. `hello` shows its screen, and after a key press or 10 seconds the unit restarts into its firmware.

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
- Returning goes back to what started the app, or resets if that was the ROM. An app can only return if it stayed within the RAM §11.8 allows.
- `APP_BOARD` is 1 on the FM-1 and 2 on the SMK-37, and each board gets its own build.
- An app needing a larger stack sets `<name>_STACK := 0x8000` in the `Makefile`. The default is 16 KiB.

## License

MIT, as m-bootloader is. See [LICENSE](LICENSE).
