# Writing m-apps with an agent

Apps run from RAM on the M-VAVE FM-1 and SMK-37. They're C, built by `make`, with no OS and no console. `README.md` covers building and the log. The rules that apply to every app are in [m-bootloader's SPEC §11.8](https://github.com/jvitkauskas/m-bootloader/blob/main/docs/SPEC.md).

## The loop

```sh
make test
make run APP=<name> BOARD=<fm1|smk37> TOOLCHAIN=<dir> M_FLASHER="<how to call m-flasher>"
```

`make run` builds the app, starts it, waits for it to end, prints its log and restarts the unit. It exits with 0 if the app returned 0, 1 if it returned something else, and 2 if it crashed, hung or left no log.

**Log what you need, then read it.** There's no debugger. Add `app_log()` lines, run, and read the result. A line is in RAM as soon as the call returns, so the last line of a crash log is the last one that finished.

To check a change really works, run it. A clean build proves little here, as most bugs in this kind of code only appear on the hardware. Report what you ran, on which board, and what you didn't run.

## Add an app

1. Create `apps/<name>/<name>.c` with `int app_main(unsigned lsb_mhz)`, and `#include "app.h"`.
2. Add `<name>` to `APPS` in the `Makefile`.
3. Set up what you use: `board_init(APP_BOARD == 2)` for keys, `display_init(APP_BOARD)` for the screen, and `flash_init(1, 1)` for flash.
4. Call `board_restore()` before returning, if you called `board_init`.

`apps/hello` is the example to copy. Each board gets its own build, with `APP_BOARD` set to 1 or 2.

## Rules

- **Don't write flash unless that's the job, and the owner has agreed to it.** RAM apps can't hurt a unit by themselves: switching it off and on brings it back. A bad flash write can leave a unit needing a USB_KEY dongle to recover. Never write the boot area, `0x0000`–`0x3FFF`.
- **Turn the guard off around flash writes,** with `app_guard(false)`, so a watchdog reset can't cut one short. Afterwards the app must reset, not return.
- **Stay polled.** Interrupts are off when `app_main` starts, and version 1 of the format is polled. An app that turns them on must reset rather than return.
- **Stay in the allowed RAM if the app returns:**
  - The file, `.bss` and stacks have to end by `0x01C7F000`. The linker checks this.
  - `0x01C16800` to below `0x01C20000` is also free.
  - Writing anywhere else means the app must reset, not return.
  - Never write from `0x01C7F000` up, except through `app_log()`. That's the log, then the retained area.
- **Feed the watchdog.** A hang is 32 s without `app_log()` or `app_feed()`, so call one in anything slower.
- **Return with the clocks as they were,** if you changed them. Whatever started the app sets up flash, the screen and USB again.

## What's available

The kit links these from m-bootloader's `src/`, and `--gc-sections` drops what an app doesn't use. Their headers are in `m-bootloader/include/`.

| Header | What it does |
|---|---|
| `log.h` | `app_log("x=%u", x)`, with `%s %c %d %u %x %%` and widths such as `%08x` |
| `app.h` | `app_feed()`, `app_guard(on)`, and includes the three below |
| `display.h` | `display_text(row, color, text)`, and rectangles and a progress bar. Colours are RGB565 |
| `board.h` | `board_keys()` returns `KEY_PREV`, `KEY_NEXT`, `KEY_SELECT` and `KEY_RECOVERY` bits |
| `platform.h` | `hal_delay_us()`, GPIO, flash, `REG()` and `hal_reset()` |
| `usb_midi.h` | A polled USB-MIDI device on USB0 |

There's no C library beyond `memcpy`, `memmove`, `memset`, `memcmp`, `strlen` and 32-bit division. 64-bit division doesn't link (`__divdi3`).

## Traps

These were all seen on real units unless they say otherwise.

- **The clock differs by route.** LSB is 48 MHz from `run-ram` and 60 MHz from the card and MIDI mode, so work out timings from `lsb_mhz`. The CPU also differs, at 96 MHz from `run-ram` and 360 MHz from the card, according to the SPEC. Check speed on the route the app will use.
- **The screen is 15 rows of 20 characters,** with lowercase shown as capitals. It only has `A-Z 0-9 - . / : < > + ?`, and anything else shows as a space. Check `display_init()`'s result.
- **The screens differ in download mode.** The FM-1's goes blank and the SMK-37's keeps its picture. A message drawn just before a reset vanishes on the FM-1, so wait first.
- **Not every fault is caught.**
  - Caught: misaligned reads and jumps to bad addresses. Causes 1 and 2 so far.
  - Silent: a write to unmapped memory just carries on.
  - Log lost: a run of `0xFFFF` instruction bytes ended back in firmware.
  - A bad jump can corrupt a byte or two of the log's text, so don't read too much into a stray character.
- **Crash addresses are approximate.** The exception is taken a little late: it's been seen one call after the bad read. Look up `near` and `last call from` in `build/<board>/<app>.map`, and start your search just before them.
- **No USB unless the app starts it.** On every route, USB is down when `app_main` starts. M-Flasher's "can't confirm the start" message is normal.
- **USB is fussy.** Receive buffers need 4 bytes more than the packet size, as DMA writes past a full packet. Accesses through `0x11804` need spacing: too few cycles made the controller babble. SPEC §11.6 has more.
- **`.bss` is cleared each time,** but only because the kit does it. `.data` holds what the last run left, unless the file was loaded again.
- **WiFi transmit DMA needs RAM below `0x01C20000`,** according to notes in other firmware projects. We haven't tested that here.
- **The FM-1's SD card shares SPI1 with the screen** (SPEC §11.5). Close one before using the other.
- **SD cards have firmware quirks,** so handle an unexpected answer by restarting the card and retrying. One 32 GB card rejected a read of its last sector when it was reached by sequential reads a few ms apart, and then answered "illegal command" to everything until restarted, which took two attempts. A different card didn't do this. `apps/usbdisk/card.c` handles it.
- **All zeros pass a CRC however badly they're misread,** and most of a card's first sectors are blank. So test a clock or a sampling mode on sectors that hold data. A test that read 63 of 64 sectors "cleanly" had only read blank ones, and the mode it praised was reading wrong data every time.
- **SPI1's clock is LSB/(divider+1),** so `lsb_mhz/4-1` gives 4 MHz, not LSB/4. The card's series resistors limit it: 8 MHz works on a prototype and 12 doesn't, with 430 to 510 ohms on CLK, and sampling on the other clock edge doesn't help. 330 ohms on CLK left that prototype's screen blank.
- **A card keeps its state through a soft reset,** because its power stays on. An app that leaves one stuck can make the bootloader's `SD CARD` item disappear until the unit is switched off and on. It also keeps CRC mode on, which the bootloader can't work with, so turn that off before leaving, as `card_finish()` does.

## What needs a person

- Switching a unit off and on. A hang that the watchdog doesn't end needs this, and it loses the log.
- Pressing keys, moving an SD card, swapping units.
- On macOS, `make run` has to be in Terminal on the Mac, as macOS refuses access over SSH. It may also ask to allow a removable volume.

**One user of a unit at a time.** Check with anyone else who might be using it before you start. `m-flasher find` shows the unit's state. Leave it on its firmware when you finish, which `make run` does by default.

## Testing without anyone at the keys

Over MIDI, a unit can be in the bootloader's MIDI mode or its SD card item with nobody pressing anything, if it has a test build of m-bootloader. [SPEC §11.9](https://github.com/jvitkauskas/m-bootloader/blob/main/docs/SPEC.md) covers building and using one. A requested SD card run ends in download mode, so read what it left in RAM from there. Without a test build, those two routes need a person at the keys, and `make run` still works.

## Seeing the screen with a camera

If a camera is pointed at the unit, you can check the screen yourself. On Linux:

```sh
fuser /dev/video0   # if something has it open, it's in use, so ask first
v4l2-ctl -d /dev/video0 -c auto_exposure=1 -c exposure_time_absolute=60
ffmpeg -y -f v4l2 -input_format mjpeg -video_size 1920x1080 -i /dev/video0 -vf "select=gte(n\,165)" -frames:v 1 shot.jpg
```

- The screen overexposes on automatic, so turn exposure down, then restore whatever you changed. The values differ by camera.
- That `select` skips about 5 s of frames for focus to settle. Earlier frames can be too blurry to read. Start it right after the app starts, or the app may be gone before it fires.
- **For several pictures, lock the focus.** Capture once with a lit screen, then `v4l2-ctl -d /dev/video0 -c focus_automatic_continuous=0`, and later pictures only need `gte(n\,30)`. This gave readable text on every row. Set it back to 1 at the end.
- Say what you actually read in the picture, not what you expected to see.
