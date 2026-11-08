# m-apps

Programs that run from RAM on the M-VAVE FM-1 and SMK-37, without changing the firmware in flash. This repo is the kit for building them, and the apps.

An app is one file. The same file can be started from an SD card, over MIDI, or with M-Flasher's `run-ram`. The format and the rules are in [m-bootloader's SPEC, §11.8](https://github.com/jvitkauskas/m-bootloader/blob/main/docs/SPEC.md).

**Status:** early. `hello` and `usbdisk` have run on an FM-1 and an SMK-37, and the `picker` launcher on an SMK-37. There are no releases yet, so build from source.

- **`run-ram`:** `make run` below uses this. It needs M-Flasher 0.9.2 or newer.
- **MIDI:** m-bootloader 2.2's MIDI mode and RAM stage can load and run an app, but nothing public sends one yet. M-Flasher's page has no button for it.
- **Firmware slots:** `slots` below keeps whole flash images on the card, and swaps between them with no computer.
- **Sound:** `mp3` below plays MP3 files from the card, and has run on an SMK-37.
- **SD card:** `picker` below is a launcher for the bootloader's `SD CARD` item: the apps on the card as tiles over rising bubbles, after the Wii's Homebrew Channel. It has run on an SMK-37, not yet on an FM-1.

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
- The card driver is `kit/card.h`, for a unit with a card mod.
- `META.TXT` beside the app's source is what the card's launcher shows for it (see `picker` below), and `ICON.QOI` its icon, if it has one, made from `icon.png` with `tools/icon.py`. `make card` puts them in the app's folder.
- Started by the launcher, `app_folder()` is the app's folder on the card, for its own files: mount the card, then `fat_find(app_folder(), "SONGS   DAT", &file, &found)`. Otherwise it's 0. The kit puts a small header at offset 4 of every app, after the entry's jump, for this: a second way in that the launcher calls with the folder (`kit/app_launch.h`).

## Apps

### picker

The launcher for the card: the apps in `/MBOOT/APPS` as tiles over rising bubbles, each with its name, a line about it, its version and an icon, and it starts the one you choose. It needs a card mod, the same as `usbdisk`. It comes in two parts:

- `build/<board>/MENU.BIN` (`apps/menu`), which the bootloader's `SD CARD` item runs, loads the launcher and runs it. Stage 2 only has 32 KiB.
- `build/<board>/LAUNCH.BIN` (`apps/picker`) is the launcher. It runs from `0x01C0A800`, with 48 KiB, which an app that returns leaves alone (m-bootloader's SPEC §11.8), so it's still there when the app ends. Its lists and pictures go in the apps' slot until it loads an app, and it reads the card again after one.

Each app has a folder of its own in `/MBOOT/APPS`, as the Homebrew Channel has them. `BOOT.APP` is the app, built for that board, and the rest is optional:

```
/MBOOT/MENU.BIN
/MBOOT/LAUNCH.BIN
/MBOOT/APPS/MP3/BOOT.APP
/MBOOT/APPS/MP3/ICON.QOI
/MBOOT/APPS/MP3/META.TXT
/MBOOT/APPS/MP3/...            the app's own files
```

`make card BOARD=smk37` (or `fm1`) builds all of that in `build/smk37/card/MBOOT`, for these apps: hello, usbdisk, slots, tone and mp3. Copy it to the card's root.

`META.TXT` is plain text, lines of `key=value`, and every line is optional:

```
name=MP3 Player
about=Plays the MP3 files on the card
version=1.0
```

Without a name, the launcher shows the folder's. Other keys are skipped, as is anything past the first 512 bytes. A folder without `BOOT.APP` isn't listed.

An icon is 44 × 44 pixels, as QOI with transparency, and the launcher rounds its corners. `python3 tools/icon.py picture.png ICON.QOI` makes one from any picture (it needs Pillow). Without one, the tile has the name's first letter on a colour.

To run it without the bootloader's menu, with the card set up:

```sh
sudo m-flasher download-mode
sudo m-flasher run-ram build/smk37/MENU.BIN --addr 0x01C28000
```

- **Keys:** OCT- and OCT+ move, and PLAY starts, on the FM-1. On the SMK-37, it's `<`, `>` and SEQ PLAY. Holding a key repeats it. `Back`, at the end of the list, goes back to what started it, which from `run-ram` means download mode.
- **Names:** folders are found by their 8.3 names. The name to show comes from `META.TXT`, and the list is in order of it.
- **Checks:** before loading, it checks the file's trailer and refuses another board's app with `OTHER BOARD`. After loading, it checks the CRC. What's refused shows in the bar at the bottom.
- **Returning:** an app that returns comes back to the list, which is read again, as the app may have changed the card. A non-zero result shows as `MP3 Player returned 7`. It shows the first 100 apps by name.
- **Size:** it refuses a file over `0x4F000` bytes, the most the kit can build, as the log starts where that ends.
- **Speed:** the bubbles redraw only where they move, at about 34 frames a second. On the SMK-37 it runs the panel at LSB/2 rather than the bootloader's LSB/5, which has worked on one unit; the FM-1 keeps LSB/5 until that's been tried there.
- **The look:** `tests/scene_host.c` draws the screen on a computer, from the same code, for trying changes without a unit.

### mp3

Lists the MP3 files in the card's root, plays the one you choose, and shows the time elapsed and left. It needs a card mod, and `picker` to start it: `make card` puts it in `/MBOOT/APPS/MP3`. It's mainly a reference for playing sound and reading a card at once, so it has no pause, seeking or volume.

- **Sound** comes from `kit/audio.c`, polled, with no interrupts: fill a half buffer each time one comes free. The DAC runs at 24 MHz / 544, about 44118 Hz, which plays 44.1 kHz files 0.04% fast. Other rates go through a simple resampler. If filling falls behind, the hardware repeats old samples, and nothing warns of it, so the app counts missed halves in its log.
- **Decoding** is [minimp3](https://github.com/lieff/minimp3). At the CPU's 360 MHz from the bootloader, a 320 kbps frame took about 3.3 ms of its 26 ms. It also keeps up at 96 MHz from `run-ram`, at about 12.5 ms.
- **Names** come from `fat_list_named()` in UTF-8. `screen_name()` is the one function turning them into what the screen has, which is capitals and a few others, with blanks for what it lacks and `?` past ASCII. To use a better font, start there.
- **The time left** comes from the frame count in a file's first frame if it has one, and otherwise from its size and bitrate.

`apps/tone` is a smaller start: two tones, with the real sample rate in its log.

### slots

Keeps firmware and its settings on the card, and swaps between them with no computer. It needs a card mod, and `picker` to start it: `make card` puts it in `/MBOOT/APPS/SLOTS`.

A slot is `/MBOOT/SLOTS/<8 hex digits>/NAME.BIN`, in a folder named from the unit's unique ID, so slots can't be loaded on the wrong unit by mistake. It's the same 1 MiB file as an M-Flasher backup, so slots can be copied and renamed on a computer. Names are 8.3.

- **The list** marks the slot `ON UNIT`. `SAVE AS NEW SLOT` saves what's there as `SLOT01` and so on. Choosing a slot offers `LOAD` and `DELETE`, each needing a 3 s hold.
- **Loading** writes what a restore does: not the boot area, the key area, or the SMK-37's Bluetooth and calibration sectors. It takes about 30 s, and then the unit restarts into that firmware.
- **It saves first.** Before writing, what's on the unit goes over the slot it came from, or into a new slot if it's other firmware, or if the slot being loaded is the one it came from. If a slot already holds it, nothing is saved. If there's no room, it stops with nothing erased.
- **If the power goes,** switch on again. With no valid firmware, the bootloader's menu opens by itself, showing `APP NOT FOUND`. Choose `SD CARD`, then `SLOTS`, and it shows `LAST LOAD STOPPED` with that slot selected. What isn't valid firmware is never saved, so the slot stays good. Load it again.

The card is written in an order where losing power leaves, at worst, clusters no file uses, which `fsck` can reclaim. The guard stays on while flash is written, unlike what `AGENTS.md` says for other apps, as each sector feeds it. So only a real hang reaches it, and then the unit ends in download mode.

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

A write has reached the card before the computer is told it succeeded, and data is checked by CRC in both directions. It picks the fastest card clock that reads cleanly, from 24 MHz down to 4, and the app's log says which. From `run-ram`, an FM-1 prototype with long wires settled on 8 MHz, at about 750 KB/s reading and 440 KB/s writing. An SMK-37 settled on 12 MHz, at about 870 KB/s reading and 440 KB/s writing, and about 1,100 KB/s and 820 KB/s from the bootloader. USB 1.1 limits it to around 1 MB/s whatever the clock.

**On macOS:**
- macOS gives up on mounting after 20 s (`ETIMEDOUT`), and it seems to read the whole FAT in that time. That's about 10 s on a 32 GB card with 16 KiB clusters. A larger FAT or a slower clock could fail to mount.
- Spotlight can hold the disk for a while, so Eject may be refused at first. It took 38 s in testing. The app ends once the eject goes through.
- macOS adds its own hidden `.Spotlight-V100` and `.fseventsd` folders to the card.

## License

MIT, as m-bootloader is. See [LICENSE](LICENSE). Two parts have their own licences, and each file's header says which:

- `kit/audio.c` and `kit/audio.h` are MPL-2.0, adapted from [felucca-bsp](https://github.com/hugelton/felucca-bsp). MPL applies file by file, so changes to these two files stay MPL-2.0, and an app using them can have any licence.
- `apps/mp3/minimp3.h` is [minimp3](https://github.com/lieff/minimp3), CC0-1.0.
- `apps/picker/font.h` is Barlow Semi Condensed, under the SIL Open Font License 1.1 (`apps/picker/Barlow-OFL.txt`), as 4-bit alpha made by fm1-base's `tools/gen_font.py`.
