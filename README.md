# m-apps

Programs that run from RAM on the M-VAVE FM-1 and SMK-37, without changing the firmware in flash. This repo is the kit for building them, and the apps, all in Rust.

An app is one file. The same file can be started from an SD card, over MIDI, or with M-Flasher's `run-ram`. The format and the rules are in [m-bootloader's SPEC, §11.8](https://github.com/jvitkauskas/m-bootloader/blob/main/docs/SPEC.md).

**Status:** early. `hello` and `usbdisk` have run on an FM-1 and an SMK-37, and the `picker` launcher on an SMK-37. There are no releases yet, so build from source.

- **`run-ram`:** `make run` below uses this. It needs M-Flasher 0.9.2 or newer.
- **MIDI:** m-bootloader 2.2's MIDI mode and RAM stage can load and run an app, but nothing public sends one yet. M-Flasher's page has no button for it.
- **Firmware slots:** `slots` below keeps whole flash images on the card, and swaps between them with no computer.
- **Sound:** `mp3` below plays MP3 files from the card, and has run on an SMK-37.
- **SD card:** `picker` below is a launcher for the bootloader's `SD CARD` item: the apps on the card as tiles over rising bubbles, after the Wii's Homebrew Channel. It has run on an SMK-37, not yet on an FM-1.

Writing apps with an agent? Start with [AGENTS.md](AGENTS.md), which has the loop, the rules and the traps found so far.

## Build

You need:
- the [pi32v2 LLVM toolchain](https://github.com/jvitkauskas/pi32v2-llvm), release `pi32v2-23.1.6-2`;
- the [pi32v2 Rust compiler](https://github.com/jvitkauskas/pi32v2-rust), release `pi32v2-1.100.0-1`, built on that LLVM, as the linker makes the code from its bitcode;
- `cargo` and a recent `rustc` for the computer, for building Rust's `core` once and for the tests;
- Python 3.

```sh
git clone --recurse-submodules https://github.com/jvitkauskas/m-apps
cd m-apps
make TOOLCHAIN=/path/to/toolchain PI32V2_RUST=/path/to/pi32v2-rust
```

This makes every app for both boards, such as `build/fm1/hello.app` and `build/smk37/hello.app`, and the universal ones in `build/any`. The apps use m-bootloader's drivers, its Rust crate (`mboot`), which comes in as the `m-bootloader` submodule.

- **How it's built:** the kit (`kit/lib.rs`) is a library, built once for each board. Each app is a crate of its own (`apps/<app>/main.rs`). rustc writes LLVM bitcode, and the linker optimises each app as one program with the kit, the drivers and `core`.
- **Size and speed:** apps are built for size (`opt-level=z`). What has to keep time is built for speed, function by function, such as mp3's decoder and WiFi's key.
- **No `core::fmt`:** formatting would cost kilobytes, so the log has its own `log!`, and no app uses `format!` or `{:?}`.

`make release` makes what a release carries, in `build/release`. The on-device store installs from these:
- **`<app>-<board>.tar`:** an app's folder for `/MBOOT/APPS/<APP>/`, such as `mp3-smk37.tar` with `BOOT.APP`, `META.TXT`, `ICON.QOI` and `NOTICE.TXT` (see [License](#license)).
- **`mboot-<board>.tar`:** `MENU.BIN`, `LAUNCH.BIN` and the launcher's `NOTICE.TXT` for `/MBOOT`.
- **`SHA256SUMS`.**

Each tar is plain ustar, made by `tools/app_tar.py`. Every name is 8.3 in capitals, as the kit's card code writes them, and the same files always make the same bytes. CI builds and tests every push, and a `v*` tag publishes a release with these files.

## Run

This starts an app, waits for it to end, prints its log and restarts the unit, from firmware or download mode:

```sh
make run APP=hello BOARD=fm1 TOOLCHAIN=/path/to/toolchain PI32V2_RUST=/path/to/pi32v2-rust M_FLASHER="sudo m-flasher"
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

## Tests

`make test` runs the host tests: the code that needs no hardware (the FAT, the launcher's catalog, screens, `WIFI.TXT`, the JPEG headers), built for the computer with overflow checks and debug assertions on (`tests/<x>_host.rs`, which each `tests/<x>_test.py` builds through `tests/host.py`). There the card is an image in memory (`kit/card_sim.rs`).

`make fuzz` fuzzes the parsers of whatever comes from a card or the radio, with libFuzzer, for `FUZZ_SECONDS` each (10 by default):
- `jpeg`: the JPEG headers;
- `wifi_list`: `WIFI.TXT`, read, changed and written back, which must read as the same list;
- `fat`: the FAT code and the launcher's catalog on any card image;
- `qoi`: the launcher's icons;
- `meta`: `META.TXT`;
- `frames`: beacons, data frames through to ARP, DHCP and UDP, and the station's management responses and WPA2 key messages, in each state it waits in.

Each harness is `tests/fuzz/<name>_fuzz.rs`, built with the pi32v2 Rust compiler (`PI32V2_RUST`, a nightly that has the computer's `std` too) with LLVM's coverage for libFuzzer, and with overflow checks and debug assertions on; a panic aborts, which libFuzzer reports as a crash. It needs that compiler and clang with libFuzzer and lld, which links it (on Debian, `clang`, `libclang-rt-dev` and `lld`; `FUZZ_CC` names a clang that isn't first on PATH); without them, it says so and passes, or fails with `FUZZ_REQUIRE=1`, as in CI. It has no AddressSanitizer: the code fuzzed is safe Rust, whose bounds checks catch a read past a slice, but for the card's image in memory (`kit/card_sim.rs`), which checks each sector against the card's size first. A crash leaves the input that caused it in `build/fuzz/<name>/`; `build/fuzz/<name>/<name> <input>` runs it again, and `tests/fuzz/found/<name>/` keeps the inputs that found a bug, which every run starts from. Parsers built for this need no hardware, so the code they need from an app is kept apart from it: `apps/wifi/frames.rs` (and `wifilist.rs`, `wifi_ip.rs`, `wifi_sta.rs`, `wifi_crypto.rs`, which build on a computer too), and `parse` in `apps/jpeg/jpeg_hw.rs`. A new parser of outside data gets a harness in `tests/fuzz/` and a line in `tests/fuzz_test.py`.

## The log

`log!("x=", x)` adds a line to a ring buffer in RAM, at `0x01C7F000`: its arguments one after another, text and numbers, with `log::hex(x)`, `log::Hex(x, 8)`, `log::Dec(x, 5)` and `log::Char(c)` for hex, widths and characters. The buffer holds about 3.4 KiB, and the oldest lines go first. Each line is written straight to RAM, so it survives whatever happens next.

The route doesn't matter, because every route ends the same way:

| How the app ends | What happens | Getting the log |
|---|---|---|
| Returns, started by `run-ram` | Resets into download mode | `make run` reads it |
| Returns, started over MIDI or from the card | Goes back to what started it | `make log BOARD=fm1` |
| Crashes | Shows `APP CRASHED`, then resets into download mode after 8 seconds. The FM-1's screen goes blank then, and the SMK-37's keeps the message | `make log BOARD=fm1` |
| Hangs | The watchdog resets it into download mode after 32 seconds | `make log BOARD=fm1` |

`make log` asks a running unit to go to download mode, then uses `build/<board>/logread.bin` to read the log. If the unit is already in download mode, it just reads. **Switching the unit off loses the log,** and starting firmware probably does too.

A crash line looks like `CRASH: cause 00000001 near 01c30426, last call from 01c303d0`. Look those up in `build/<board>/<app>.map`. "Near" means the exception is taken a little late: it's been seen one call after the bad instruction. Causes seen so far: 1 for a misaligned read, and 2 for a jump to a bad address or an illegal instruction. A Rust panic, such as a failed bounds check, is the compiler's trap, an illegal instruction: its line says `PANIC: cause 00000002`, near where it was, and the screen `APP STOPPED`. Not everything crashes: a write to unmapped memory carried on silently, and a run of bad instruction bytes ended back in firmware, with the log lost.

**The guard** is what makes this work, and it's on when the app's main starts. It arms the watchdog, sets the download-mode request and installs a crash handler, and undoes all of that if the app returns.

- A hang is 32 seconds without `log!()` or `kit::app::feed()`, so call one of them in anything slower.
- `kit::app::guard(false)` turns it off, for work that must not be cut short, such as writing flash.

`make run APP=crashtest`, `make run APP=panictest` and `make run APP=hangtest` test it. All three should exit with 2.

## Write an app

Add a folder in `apps/`, add its name to `APPS` in the `Makefile`, and write `apps/<name>/main.rs`:

```rust
#![no_std]
kit::app!(main);

fn main(lsb_mhz: u32) -> i32 {
    // Interrupts are off, and you're on the app's own stack.
    0
}
```

- `kit/` holds the entry (`start.S`), the start (`app.rs`) and the linker script. It clears `.bss` every time the app starts. What an app adds to the build, such as its tests or `<name>_OPT`, goes in `apps/<name>/app.mk`.
- `lsb_mhz` is the LSB clock the app was started at. Use it to work out timings, as it differs by route.
- Returning goes back to what started the app, or to download mode if that was the ROM. An app can only return if it stayed within the RAM §11.8 allows.
- The file and its RAM have to end by `0x01C7F000`, as the log takes the top of the slot §11.8 describes.
- `kit::APP_BOARD` is 1 on the FM-1 and 2 on the SMK-37, and each board gets its own build.
- **One file for both boards:** an app in `UNIVERSAL_APPS` is also built in `build/any`, with `APP_BOARD` 0 and board 0 in its trailer, which the launcher and m-bootloader take on either. It asks `kit::app::board()` which board it's on, and uses the answer where it would have used `APP_BOARD`. That reads the unit's own flash, never its pins (m-bootloader's SPEC §8.1): an SMK-37 by its calibration (`trimdat` at `0xC1224`, or its copy `TRIMCOPY` at `0xFF000`), an FM-1 by its flash chip (`0x856014`, `0xB36014`). It's 0 for anything else, and an app should then touch no pin and return. In a per-board build, `app::board()` is `APP_BOARD`. `hello`, `usbdisk`, `slots`, `mp3`, the launcher and its loader are universal, and were run on both boards.
- **The launcher's look** for an app: `kit/canvas.rs` draws a full 240 x 240 picture in RAM with the launcher's colours, cards, Barlow text and bar, and sends only what changed. `kit/pixel.rs` is the player's 90s look, drawn at half size and shown doubled. Both draw on a computer too, which the screen tests use.
- **Keys:** `kit/keys.rs` reads the whole matrix on both boards, polled: the 27 piano keys both have (F3 to G5) and the buttons, where `board_keys()` reads four. REC (SEQ REC) is the way back in every app. `kit/textentry.rs` types on the piano keys.
- Started from `run-ram`, an app that ends shows `THE APP HAS ENDED` before the unit goes back to download mode, as the SMK-37's panel would otherwise keep the app's last screen.
- An app needing a larger stack sets `<name>_STACK := 0x8000` in its `app.mk`. The default is 16 KiB.
- The card driver is `kit/card.rs`, and `kit/fat.rs` reads and writes FAT32 on it, for a unit with a card mod.
- `META.TXT` beside the app's source is what the card's launcher shows for it (see `picker` below), and `ICON.QOI` its icon, if it has one, made from `icon.png` with `tools/icon.py`. `make card` puts them in the app's folder.
- Started by the launcher, `kit::app::folder()` is the app's folder on the card, for its own files: mount the card, then `volume.find(app::folder(), b"SONGS   DAT")`. Otherwise it's 0. The kit puts a small header at offset 4 of every app, after the entry's jump, for this: a second way in that the launcher calls with the folder (`kit::app::AppLaunch`).

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
/MBOOT/APPS/MP3/NOTICE.TXT     its licences, if it has code or a font under another licence
/MBOOT/APPS/MP3/...            the app's own files
```

`make card BOARD=smk37` (or `fm1`) builds all of that in `build/smk37/card/MBOOT`, for these apps: hello, usbdisk, slots, tone, mp3 and unbrick. Copy it to the card's root.

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

- **Keys:** OCT- and OCT+ move, and PLAY starts, on the FM-1. On the SMK-37, it's `<`, `>` and SEQ PLAY. Holding a key repeats it. `Back`, at the end of the list, goes back to what started it, which from `run-ram` means download mode, and so does REC (SEQ REC on the SMK-37) from anywhere in the list.
- **Names:** folders are found by their 8.3 names. The name to show comes from `META.TXT`, and the list is in order of it.
- **Checks:** before loading, it checks the file's trailer and refuses another board's app with `OTHER BOARD`. After loading, it checks the CRC. What's refused shows in the bar at the bottom.
- **Returning:** an app that returns comes back to the list, which is read again, as the app may have changed the card. A non-zero result shows as `MP3 Player returned 7`. It shows the first 100 apps by name.
- **Size:** it refuses a file over `0x4F000` bytes, the most the kit can build, as the log starts where that ends.
- **Speed:** the bubbles redraw only where they move, at about 34 frames a second. On the SMK-37 it runs the panel at LSB/2 rather than the bootloader's LSB/5, which has worked on one unit; the FM-1 keeps LSB/5 until that's been tried there.
- **The look:** `tests/scene_host.rs` draws the screen on a computer, from the same code, for trying changes without a unit.
- **Without keys:** `make PICKER_KEYS='>>s' build/smk37/LAUNCH.BIN` builds a launcher that presses keys itself, one every 1.5 s, on through the apps it starts: `<` and `>` move, `s` starts and `r` is REC. It logs each one, and how long each move took to draw.

### mp3

A player in the look of the 90s desktop players: a main window with segment digits, an analyser, the title scrolling, the bitrate, rate and channels, a position bar and the buttons, over its playlist. It plays the MP3 files in the card's root, one after another. It needs a card mod, and `picker` to start it: `make card` puts it in `/MBOOT/APPS/MP3`.

- **Keys:** OCT- and OCT+ (`<` and `>` on the SMK-37) move through the playlist, and a track moved to starts at once. `<< BACK`, at the end, is there while playing too, and changes nothing until PLAY. Held while playing, they rewind and fast-forward, in 2 s jumps with a moment of sound between. PLAY starts and pauses, and on `<< BACK` it leaves. REC (SEQ REC on the SMK-37) leaves from anywhere, as it means nothing else once the unit has started.
- **The look** is all drawn in code, in `ui.rs`: nothing is a bitmap. It's drawn at 120 × 120 and shown at twice that, as the players' double-size mode did, which keeps it readable on the small screen. The look is shared with other apps through `kit/pixel.rs`, and its fonts are pictures in `kit/pixel_font.py`, which makes `pixel_font.rs`. Only what changed is sent, and `tests/mp3_ui_test.py` draws the screens on a computer.
- **Sound** comes from `kit/audio.rs`, polled, with no interrupts. Decoded sound goes into a ring of about 190 ms, and the half buffers are filled from it between every card read, decode and piece of the screen, so a slow moment doesn't cut the sound. The screen is drawn when the ring is full, a little at a time. The DAC runs at 24 MHz / 544, about 44118 Hz, which plays 44.1 kHz files 0.04% fast. Other rates go through a simple resampler. The log counts the halves the ring couldn't fill.
- **Decoding** is [minimp3](https://github.com/lieff/minimp3). At the CPU's 360 MHz from the bootloader, a 320 kbps frame took about 3.3 ms of its 26 ms. At 96 MHz from `run-ram` it takes about 13 ms, and an FM-1 played 22 kHz mono, 48 kHz and VBR files there with no gaps.
- **The analyser** is a 512-point FFT of each frame, in 14 bands of about two thirds of an octave (`spectrum.rs`).
- **Names** come from `fat_list_named()` in UTF-8. `track_name()` turns them into what the font has, which is ASCII, drawn in capitals, with `?` past it.
- **The length** comes from the frame count in a file's first frame if it has one, and otherwise from its size and bitrate. Seeking guesses where a second starts from the same.

`apps/tone` is a smaller start: two tones, with the real sample rate in its log.

### jpeg

A test of the chip's JPEG decoder (`JL_JPG` at `0x41900`): it decodes five pictures built into it, shows each, and logs how long the decoder took. `apps/jpeg/jpeg_hw.rs` drives it, polled. The register sequence was worked out from how JieLi's AC79 SDK drives it, and the code is written fresh. Its header says what the decoder takes.

- **What it decodes:** baseline JPEG with three components, 4:2:0, 4:2:2 or 4:4:4, a width that's a multiple of 16 (8 for 4:4:4). One row of MCUs per start, into planar Y, Cb and Cr. Not handled: restart markers, a file bigger than RAM, progressive and greyscale (the hardware can't do those).
- **Speed** on the FM-1 from `run-ram` (CPU 96 MHz): 240 x 240 in 1.5 ms (4:2:0), 2.0 ms (4:2:2) and 2.8 ms (4:4:4), and 480 x 320 4:2:0 in 3.7 ms. Turning that into RGB for the screen, one line at a time in software, took about 170 ms (measured with the C), so that's the part to speed up.
- **The pictures** are drawn by `apps/jpeg/images.py`, which writes `images.rs`. With a second argument it also saves the JPEGs, to compare a decode with on a computer: a build with `jpeg_CFG='--cfg jpeg_dump="n"'` leaves picture n's planes in RAM. For 4:4:4 they came within 3 levels of Pillow's.

It's a test for now, not on the card.

### slots

Keeps firmware and its settings on the card, and swaps between them with no computer. It needs a card mod, and `picker` to start it: `make card` puts it in `/MBOOT/APPS/SLOTS`.

A slot is `/MBOOT/SLOTS/FM1/` or `/MBOOT/SLOTS/SMK37/`, then `<its name>.slt`, shared by every unit of that model. It holds the firmware's part of the flash only, what a load writes: `0x4000` up to the key area at `0xFF000`, and on the SMK-37 not its Bluetooth and calibration sectors at `0xC0000`-`0xC1FFF`, as [clean-flash](https://github.com/jvitkauskas/clean-flash)'s firmware scope. The boot area, the key area and the unit's own data are never in a slot, and a load never writes them. Blank sectors are left out: a 512-byte header (`apps/slots/slotfile.rs`) says which sectors follow it, with CRCs of both.

- **The list** has `Save what's on the unit` first, then the slots, marking the one that's `On the unit now`, then `Back`. Choosing a slot offers `Load it onto the unit`, with PLAY (SEQ PLAY) held for 1 s, and `Delete it`, held for 2 s. REC (SEQ REC) goes back from anywhere.
- **Names** are typed on the piano keys when saving (`kit/textentry.rs`, from fm1-radio): F3 to E5 type a to z, G5 deletes, OCT- held types capitals, and OCT+ held digits and symbols a file name may have. A computer sees the same name, as `kit/fat.rs` writes long names. Left empty, it's `Slot 1` and on; a name a slot has already gets a number.
- **Loading** writes only what differs, erasing the sectors the slot leaves blank. It takes about 30 s, and then the unit restarts into that firmware. Before it writes, the slot's CRC and its firmware are checked, as the bootloader checks firmware, with the unit's own boot area.
- **It saves first.** Before writing, what's on the unit goes over the slot it came from, or into a new slot, `Saved`, if it's other firmware, or if the slot being loaded is the one it came from. If a slot already holds it, nothing is saved. If there's no room, it stops with nothing erased. Each unit keeps which slot it came from in `<8 hex digits of its unique ID>.DAT` beside the slots.
- **If the power goes,** switch on again. With no valid firmware, the bootloader's menu opens by itself, showing `APP NOT FOUND`. Choose `SD CARD`, then Slots, and it shows `The last load stopped` with that slot selected. What isn't valid firmware is never saved, so the slot stays good. Load it again.

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

**To leave,** eject the disk on the computer, which is the way to use normally, or press REC (SEQ REC on the SMK-37), which doesn't wait for the computer to finish, so eject first if you can. With no computer attached, it also leaves after 2 minutes. It doesn't leave just because the disk is idle, as a computer can hold writes back for a while.

**The screen** is in the launcher's look and font: an SD card cabled to a laptop, the same on both boards, with dots running along the cable while data moves (blue reading, orange writing), and the speed and totals below. The pictures are drawn by `apps/usbdisk/art.py`. It draws between USB commands, when the card is idle, and only what changed, so it costs about 2% of the speed.

A write has reached the card before the computer is told it succeeded, and data is checked by CRC in both directions. It picks the fastest card clock that reads cleanly, from 24 MHz down to 4, and the app's log says which. From `run-ram`, an FM-1 prototype with long wires settled on 8 MHz, at about 750 KB/s reading and 440 KB/s writing. An SMK-37 settled on 12 MHz, at about 870 KB/s reading and 440 KB/s writing, and about 1,100 KB/s and 820 KB/s from the bootloader. USB 1.1 limits it to around 1 MB/s whatever the clock.

**On macOS:**
- macOS gives up on mounting after 20 s (`ETIMEDOUT`), and it seems to read the whole FAT in that time. That's about 10 s on a 32 GB card with 16 KiB clusters. A larger FAT or a slower clock could fail to mount.
- Spotlight can hold the disk for a while, so Eject may be refused at first. It took 38 s in testing. The app ends once the eject goes through.
- macOS adds its own hidden `.Spotlight-V100` and `.fseventsd` folders to the card.

### unbrick

[M-Unbricker](https://github.com/jvitkauskas/m-unbricker), from the card: it repairs a broken FM-1 or SMK-37 with this one and a USB-C cable, with no computer. It guides you on the screen: move the cable to the broken unit and switch that on, and it puts its ROM in download mode with JieLi's USB_KEY, writes a fresh m-bootloader, erases the rest, and checks it. Either model can repair either.

It's its own repo, here as the `m-unbricker` submodule, and its own Makefile builds it: what it makes is already an app, so `make` copies it to `build/<board>/unbrick.app`, and `make card` puts it in `/MBOOT/APPS/UNBRICK`. To update it, move the submodule. The launcher starts it with the LSB clock, as m-bootloader's MIDI mode does, which it already handles. It doesn't return: at the end it restarts the unit.

Started from the launcher on a unit with a card mod, it has repaired an FM-1 from an SMK-37 Pro and an SMK-37 Pro from an FM-1.

### wifi

Keeps the WiFi networks the unit joins, in `/WIFI.TXT` at the card's root, and tests them. The file is for any firmware or app on the unit, not only these: [docs/WIFI-TXT.md](docs/WIFI-TXT.md) is its format. The app needs a card mod, and `picker` to start it: `make card` puts it in `/MBOOT/APPS/WIFI`. One file runs on both boards.

`WIFI.TXT` is meant to be edited on a computer too:

```
# WiFi networks on this card. use= is the one to join.
use=Home

ssid=Home
password=correct horse

ssid=Cafe
password=
```

One setting a line: the value is everything after the first `=`, spaces and all. `ssid=` starts a network, `password=` is its password (none for an open one), and `use=` names the one to join, otherwise the first. Lines starting with `#`, blank lines and settings it doesn't know are skipped, and CRLF and a byte order mark are fine. Up to 12 networks; names of 1 to 32 bytes, passwords of 8 to 63 characters. When the app saves, it changes only what it means to: comments, blank lines, settings it doesn't know and networks it skipped stay as they were. It writes the file the safe way (`Volume::reserve`, `write`, `swap`), and won't change one over 4 KiB. `apps/wifi/wifilist.rs` reads and writes it, and builds on a computer for its test.

- **The list** has `Add a network`, the networks, marking the one to join, then `Back`. A network's page tests it, makes it the one to join, changes its password, or deletes it (PLAY held 1.5 s). REC goes back.
- **Adding** listens on channels 1 to 13, about 2 s, and lists what it heard, strongest first, with the signal and channel. Choosing one asks for its password, on the piano keys: OCT- held for capitals, OCT+ for digits and symbols, both for the rest of printable ASCII. `Type a name` adds one that wasn't heard, or a hidden one. Then it tests it: joined, it's saved and made the one to use; not, it says why, and PLAY saves it anyway.
- **A test** starts the radio, finds the network, makes the WPA2 key, joins, and asks DHCP for an address. Then it shows the address, signal, channel and time.
- **What it joins:** WPA2-PSK (CCMP, without required management frame protection), which includes most routers set to WPA2/WPA3, and open networks. WPA3-only is listed, as one it can't join.
- **The stack** is from [w0ts/fm1-base](https://github.com/w0ts/fm1-base): the radio's start-up and WiFi path from its felucca-bsp headers, and Optimist's WPA2 station, crypto and IP layer, in `apps/wifi/bsp/` and `apps/wifi/wifi_*.rs`, cut down to what a station and the store need, and with one hook (`apps/wifi/bsp/README.md` says what changed). `net.rs` joins and gets an address, after fm1-base's `device/wifi_own.c`. The radio's buffers are in the RAM below the app (`.dma`), which its DMA reaches.
- **The key** is 4096 rounds of HMAC-SHA-1. The chip's SHA unit does each block (`sha1hw.rs`, driven as JieLi's SDK does) once it's given what the software does on four blocks, and is 8.5 times faster. The passphrase's two HMAC pad blocks are hashed once rather than in every round, which halves the blocks: the key takes about 0.5 s on the FM-1 from `run-ram`.
- **The clock:** started from the card at 360 MHz, it drops to 320 MHz, as the datasheet rates the chip, and goes back before returning. At 360 MHz with the radio on, the store experiment's crypto went wrong at times.
- **Leaving** turns the radio off as it was found (`net_off()`): a launcher that got it back still on showed a black screen.

Tested on both boards from `run-ram`: an open network gave an address in about 3 s, and a WPA2 one in 4 to 8 s with the signal at -50 to -62 dBm. From the card's launcher on the FM-1 (at 360 MHz, so with the switch to 320): a scan, and the way back to the launcher. Joining from there hasn't been tried yet. A name typed for a network not heard is asked for by name (a probe request), as a hidden network answers only that. `make build/any/wifi.app WIFI_TEST=autotest` builds a version that needs no keys: it scans, then tests every network in `WIFI.TXT`, all in its log.

## License

MIT, as m-bootloader is. See [LICENSE](LICENSE). Some parts have their own licences, and each file's header says which:

- `kit/audio.rs` is MPL-2.0, adapted from [felucca-bsp](https://github.com/hugelton/felucca-bsp). MPL applies file by file, so changes to it stay MPL-2.0, and an app using them can have any licence.
- `apps/mp3/minimp3.rs` is a port of [minimp3](https://github.com/lieff/minimp3) to Rust, CC0-1.0 as minimp3 is.
- `kit/keys.rs` is MPL-2.0, from felucca-bsp's key tables (`boards/fm1/fm1_input.h` and `boards/smk37/fm1_input.h`).
- `kit/textentry.rs` is MPL-2.0, adapted from Charles Vestal's [fm1-radio](https://github.com/charlesvestal/fm1-radio) password page.
- `unbrick` is built from the `m-unbricker` submodule (MIT). It holds glyphs rendered from DejaVu Sans (Bitstream Vera licence and public domain), and its own list in `m-unbricker/THIRD-PARTY.md`.
- `apps/wifi`'s radio and WiFi stack are from fm1-base and felucca-bsp: MPL-2.0, "GPL-3.0-only OR MPL-2.0" (taken here as MPL-2.0), and Apache-2.0 for three files from JieLi's SDK. `apps/wifi/net.rs` is MPL-2.0, as it follows fm1-base's `wifi_own.c`. `apps/wifi/bsp/README.md` lists them.
- `kit/font_data.rs` is Barlow Semi Condensed, under the SIL Open Font License 1.1 (`LICENSES/OFL-1.1-Barlow.txt`), as 4-bit alpha made by fm1-base's `tools/gen_font.py`.

The MPL-2.0 and Apache-2.0 texts are in `LICENSES/`, with the font's licence.

**A built app carries its own notice.** An app with any of these in it has a `NOTICE.TXT` beside it on the card and in its release tar, as these licences ask of a copy given to someone: what's under which licence, by whom, a link to the source it was built from, and the font licences in full. `make card` writes it from `apps/<app>/NOTICE.TXT`, with `@SOURCE@` made the commit's (or tag's) address, then each file in `<app>_LICENSES` in the `Makefile`. The launcher's is `apps/picker/NOTICE.TXT`, in `/MBOOT`. Today these have one: `tone` and `mp3` (MPL-2.0 sound output; `mp3` also credits minimp3), `slots` (MPL-2.0 keys and typing, the Barlow font), `usbdisk` and the launcher (the Barlow font), `unbrick` (the DejaVu glyphs), and `wifi` (the MPL-2.0 and Apache-2.0 radio and WiFi stack, keys and typing, the Barlow font; the Apache-2.0 text in full). An app using none of them needs no notice: `hello`, `crashtest`, `hangtest` and `jpeg` are MIT throughout.
