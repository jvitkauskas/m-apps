# SPDX-License-Identifier: MIT
SHELL := /bin/bash
.DEFAULT_GOAL := all
.DELETE_ON_ERROR:
# The pi32v2 LLVM toolchain, unpacked in toolchain/ or at TOOLCHAIN=<directory> (see README).
TOOLCHAIN ?= toolchain
CLANG := $(TOOLCHAIN)/bin/clang
LINK := $(TOOLCHAIN)/bin/ld.lld
OBJCOPY := $(TOOLCHAIN)/bin/llvm-objcopy
AR := $(TOOLCHAIN)/bin/llvm-ar
# The pi32v2 Rust compiler (see README), unpacked in pi32v2-rust/ or at PI32V2_RUST=<directory>. It has to be built on
# the toolchain's LLVM: the linker generates the code from its bitcode.
PI32V2_RUST ?= pi32v2-rust
RUSTC := $(PI32V2_RUST)/bin/rustc
# Any recent rustc, for the tests on this computer, and cargo, to build `core` once from the compiler's sources.
HOST_RUSTC ?= rustc
CARGO ?= cargo
MB := m-bootloader
# The apps are Rust: the kit (kit/lib.rs) is a library, the drivers are m-bootloader's crate (m-bootloader/rust), and
# each app is a crate of its own (apps/<app>/main.rs). rustc writes bitcode, and the linker optimises each app as one
# program with the kit, the drivers and `core`, as it did the C.
APPS := hello crashtest hangtest panictest usbdisk slots tone mp3 jpeg wifi
# Built once more as universal apps, in build/any.
UNIVERSAL_APPS := hello usbdisk slots mp3 wifi
# Rust's optimisation level: z, the smallest, with -inline-minsize-hint-threshold (pi32v2-llvm 23.1.6-2), as
# m-bootloader has it. An app can have its own: <name>_OPT := 2.
RUST_OPT ?= z
ifeq ($(RUST_OPT),z)
LINK += -mllvm -inline-minsize-hint-threshold=50
endif
# A panic (-C panic=immediate-abort) is the compiler's trap: btbclr, which the CPU takes as an illegal instruction,
# so it ends in the guard's crash handler (kit/app.rs) with the log saying where.
RUST_LIB := lib/rustlib/pi32v2-unknown-none/lib
rust_flags = --target pi32v2-unknown-none --edition 2024 -C opt-level=$(1) -C codegen-units=1 -C debuginfo=0 \
	-C symbol-mangling-version=v0 -Zunstable-options -C panic=immediate-abort $(if $(filter z,$(1)),-C llvm-args=-inline-minsize-hint-threshold=50) \
	--remap-path-prefix=$(CURDIR)=. --sysroot build/rust/sysroot -Clinker-plugin-lto -Clto=fat
KIT_SRC := $(wildcard kit/*.rs)
MB_SRC := $(wildcard $(MB)/rust/src/*.rs $(MB)/rust/src/*/*.rs)
# A stack for each app. An app can have its own: <name>_STACK := 0x8000
APP_STACK_DEFAULT := 0x4000
# The card's MENU.BIN loads the launcher, in the 32 KiB stage 2 has (m-bootloader's docs/SPEC.md §11.5). The launcher
# runs from 0x01C0A800, with the 48 KiB below 0x01C16800, which an app that returns leaves alone (§11.8).
menu_ORIGIN := 0x1c28000
menu_LENGTH := 0x8000
menu_STACK := 0x1000
picker_ORIGIN := 0x1c0a800
picker_LENGTH := 0xc000
picker_STACK := 0x2000
# WIFI_TEST=autotest builds the WiFi app that needs no keys. What it's built with is remembered, so changing it
# builds the app again.
WIFI_TEST ?=
build/wifi-test.txt: FORCE
	@mkdir -p build
	@echo '$(WIFI_TEST)' | cmp -s - $@ || echo '$(WIFI_TEST)' > $@
wifi_CFG := $(if $(WIFI_TEST),--cfg wifi_autotest)
wifi_DEPS := build/wifi-test.txt

all: $(foreach a,$(APPS) unbrick,build/fm1/$(a).app build/smk37/$(a).app) $(foreach a,$(UNIVERSAL_APPS) unbrick,build/any/$(a).app) build/any/MENU.BIN build/any/LAUNCH.BIN build/fm1/logread.bin build/smk37/logread.bin build/fm1/MENU.BIN build/smk37/MENU.BIN build/fm1/LAUNCH.BIN build/smk37/LAUNCH.BIN

clean:
	rm -rf build

# What changes the Rust without changing a file: another compiler or linker builds it all again.
build/rust/options: FORCE
	@mkdir -p $(@D)
	@{ echo 'RUST_OPT=$(RUST_OPT)'; $(RUSTC) -vV; $(LINK) --version; } > $@.new 2>&1; \
		cmp -s $@.new $@ && rm $@.new || mv $@.new $@
# Rust's core library for pi32v2, built once from the compiler's sources, as bitcode of the kind the rest is
# (-Clto=fat), as m-bootloader builds it.
build/rust/sysroot.stamp: kit/sysroot/Cargo.toml kit/sysroot/src/lib.rs Makefile build/rust/options
	@mkdir -p build/rust/sysroot/$(RUST_LIB)
	$(CARGO) clean --quiet --manifest-path kit/sysroot/Cargo.toml --target-dir build/rust/target
	CARGO_TARGET_DIR=build/rust/target CARGO_PROFILE_RELEASE_OPT_LEVEL=$(RUST_OPT) RUSTC_BOOTSTRAP=1 RUSTC=$(RUSTC) RUSTFLAGS="-Csymbol-mangling-version=v0 -Zunstable-options -Cpanic=immediate-abort $(if $(filter z,$(RUST_OPT)),-C llvm-args=-inline-minsize-hint-threshold=50) --remap-path-prefix=$(CURDIR)=. -Clto=fat" \
		$(CARGO) build --quiet --release --manifest-path kit/sysroot/Cargo.toml --target pi32v2-unknown-none -Zbuild-std=core
	@# (wherever this cargo keeps them: under deps/ up to 1.99, under build/<crate>/ from 1.100)
	for f in core compiler_builtins; do \
		set -- $$(find build/rust/target/pi32v2-unknown-none/release -name "lib$$f-*.rlib"); \
		[ $$# = 1 ] && cp "$$1" build/rust/sysroot/$(RUST_LIB)/lib$$f.rlib || exit 1; done
	touch $@
# A library's one object, for the linker.
rlib_object = $(AR) p $(1) $$($(AR) t $(1) | grep '\.o$$') > $(2)
build/rust/core.o: build/rust/sysroot.stamp
	$(call rlib_object,build/rust/sysroot/$(RUST_LIB)/libcore.rlib,$@)
build/rust/compiler_builtins.o: build/rust/sysroot.stamp
	$(call rlib_object,build/rust/sysroot/$(RUST_LIB)/libcompiler_builtins.rlib,$@)
RUNTIME := build/rust/core.o build/rust/compiler_builtins.o

# make test: the tests on this computer (tests/), with the kit and the apps built for it: each test's program is
# tests/<name>.rs, built as build/host/<name> (tests/host.py), with overflow checks and debug assertions.
HOST_FLAGS := --edition 2024 -C opt-level=1 -C debuginfo=1 -C overflow-checks=on -C debug-assertions=on
build/host/libmboot.rlib: $(MB_SRC) Makefile
	@mkdir -p $(@D)
	$(HOST_RUSTC) $(HOST_FLAGS) --crate-name mboot --crate-type rlib --cfg panel_test $(MB)/rust/src/lib.rs -o $@
build/host/libkit.rlib: $(KIT_SRC) build/host/libmboot.rlib Makefile
	$(HOST_RUSTC) $(HOST_FLAGS) --crate-name kit --crate-type rlib --cfg 'board="fm1"' --extern mboot=build/host/libmboot.rlib -L build/host kit/lib.rs -o $@
build/host/%: tests/%.rs build/host/libkit.rlib $(wildcard apps/*/*.rs) Makefile
	$(HOST_RUSTC) $(HOST_FLAGS) --extern kit=build/host/libkit.rlib -L build/host $< -o $@
test:
	python3 tests/applog_test.py
	python3 tests/app_tar_test.py
	python3 tests/fat_write_test.py
	$(foreach t,$(APP_TESTS),python3 $(t) &&) true
# What an app adds to the build: apps/<app>/app.mk, with its tests in APP_TESTS, and anything its build makes.
APP_TESTS :=
-include $(wildcard apps/*/app.mk)

# make fuzz: the parsers of what comes from a card or the radio, fuzzed for FUZZ_SECONDS each (tests/fuzz_test.py):
# tests/fuzz/*_fuzz.rs, built with the pi32v2 Rust compiler (PI32V2_RUST) with coverage, and linked with libFuzzer by
# the computer's clang. Needs clang with libFuzzer (Debian: clang, libclang-rt-dev); without it, it says so and passes.
FUZZ_SECONDS ?= 10
# The computer's clang, not the pi32v2 one: FUZZ_CC=/usr/bin/clang when the toolchain's is first on PATH.
FUZZ_CC ?= clang
# FUZZ_REQUIRE=1: fail rather than skip without libFuzzer or the Rust compiler, as CI does.
FUZZ_REQUIRE ?=
fuzz:
	FUZZ_CC=$(FUZZ_CC) FUZZ_REQUIRE=$(FUZZ_REQUIRE) PI32V2_RUST=$(PI32V2_RUST) python3 tests/fuzz_test.py $(FUZZ_SECONDS)

# make run APP=hello BOARD=fm1: start an app with run-ram, wait for it to end, and print its log.
# make log BOARD=fm1: print the log an app left, however it was started.
# M_FLASHER is how to call M-Flasher, such as "sudo m-flasher".
M_FLASHER ?= m-flasher
RUN_FLAGS ?=
run: build/$(BOARD)/$(APP).app build/$(BOARD)/logread.bin
	python3 tools/run.py --flasher "$(M_FLASHER)" --reader build/$(BOARD)/logread.bin $(RUN_FLAGS) build/$(BOARD)/$(APP).app
log: build/$(BOARD)/logread.bin
	python3 tools/run.py --flasher "$(M_FLASHER)" --reader build/$(BOARD)/logread.bin $(RUN_FLAGS)

# board, its --cfg names for the drivers, its number
define BOARD_template
# The drivers. panel_test gives display::pixels(), which the RAM apps draw with.
build/$(1)/rust/libmboot.rlib: $(MB_SRC) build/rust/sysroot.stamp Makefile
	@mkdir -p $$(@D)
	$(RUSTC) $$(call rust_flags,$(RUST_OPT)) --crate-name mboot --crate-type rlib --cfg panel_test $(2) $(MB)/rust/src/lib.rs -o $$@
build/$(1)/rust/libkit.rlib: $(KIT_SRC) build/$(1)/rust/libmboot.rlib Makefile
	$(RUSTC) $$(call rust_flags,$(RUST_OPT)) --crate-name kit --crate-type rlib --cfg 'board="$(1)"' --extern mboot=build/$(1)/rust/libmboot.rlib -L build/$(1)/rust kit/lib.rs -o $$@
build/$(1)/rust/%.o: build/$(1)/rust/lib%.rlib
	$$(call rlib_object,$$<,$$@)
build/$(1)/start.o: kit/start.S Makefile
	@mkdir -p $$(@D)
	$(CLANG) -target pi32v2 -mcpu=r3 -c $$< -o $$@
# What reads the log back: a program for the ROM's run-ram, in the flash loader's second slot.
build/$(1)/logread.ld: $(MB)/tools/target.ld.S Makefile
	@mkdir -p $$(@D)
	$(CLANG) -E -P -x c -DENTRY_SYMBOL=logread_entry -DRAM_ORIGIN=0x1c28000 -DRAM_LENGTH=0x8000 -DLOADER_BUILD $$< -o $$@
build/$(1)/logread.o: kit/logread/main.rs build/$(1)/rust/libmboot.rlib Makefile
	$(RUSTC) $$(call rust_flags,$(RUST_OPT)) --crate-name logread --crate-type lib --emit=obj --extern mboot=build/$(1)/rust/libmboot.rlib -L build/$(1)/rust $$< -o $$@
build/$(1)/logread.elf: build/$(1)/logread.o build/$(1)/rust/mboot.o $(RUNTIME) build/$(1)/logread.ld
	$(LINK) --plugin-opt=O3 --relax --gc-sections -T build/$(1)/logread.ld -o $$@ $$(filter %.o,$$^)
build/$(1)/logread.bin: build/$(1)/logread.elf
	$(OBJCOPY) -O binary -j .text $$< $$@
endef
$(eval $(call BOARD_template,fm1,,1))
$(eval $(call BOARD_template,smk37,--cfg smk,2))
# Universal apps: one file for both boards, board 0 in its trailer, finding out which it's on with app::board().
$(eval $(call BOARD_template,any,,0))

# app, board
define APP_template
# An app's app.mk sets its opt level, cfgs and stack, so a change there builds it again.
build/$(2)/$(1)/$(1).o: $$(wildcard apps/$(1)/*.rs apps/$(1)/*/*.rs apps/$(1)/app.mk) build/$(2)/rust/libkit.rlib $$($(1)_DEPS) Makefile
	@mkdir -p $$(@D)
	$(RUSTC) $$(call rust_flags,$$(or $$($(1)_OPT),$(RUST_OPT))) --crate-name $(1) --crate-type lib --emit=obj $$($(1)_CFG) \
		--extern kit=build/$(2)/rust/libkit.rlib -L build/$(2)/rust apps/$(1)/main.rs -o $$@
build/$(2)/$(1)/app.ld: kit/app.ld.S $$(wildcard apps/$(1)/app.mk) Makefile
	@mkdir -p $$(@D)
	$(CLANG) -E -P -x c -DAPP_STACK=$$(or $$($(1)_STACK),$(APP_STACK_DEFAULT)) -DAPP_ORIGIN=$$(or $$($(1)_ORIGIN),0x1c30000) -DAPP_LENGTH=$$(or $$($(1)_LENGTH),0x4f000) -DAPP_LOW=$$(or $$($(1)_LOW),0x1c16800) $$< -o $$@
build/$(2)/$(1).elf: build/$(2)/start.o build/$(2)/$(1)/$(1).o build/$(2)/rust/kit.o build/$(2)/rust/mboot.o $(RUNTIME) build/$(2)/$(1)/app.ld
	$(LINK) --plugin-opt=O3 --relax --gc-sections -T build/$(2)/$(1)/app.ld -Map build/$(2)/$(1).map -o $$@ $$(filter %.o,$$^)
build/$(2)/$(1).app: build/$(2)/$(1).elf $(MB)/tools/stage2.py
	$(OBJCOPY) -O binary -j .text $$< build/$(2)/$(1).raw
	python3 $(MB)/tools/stage2.py --app $(2) build/$(2)/$(1).raw $$@
endef
$(foreach a,$(APPS) menu picker,$(eval $(call APP_template,$(a),fm1)))
$(foreach a,$(APPS) menu picker,$(eval $(call APP_template,$(a),smk37)))
$(foreach a,$(UNIVERSAL_APPS) menu picker,$(eval $(call APP_template,$(a),any)))

build/%/MENU.BIN: build/%/menu.elf $(MB)/tools/stage2.py
	$(OBJCOPY) -O binary -j .text $< build/$*/menu.raw
	python3 $(MB)/tools/stage2.py build/$*/menu.raw $@
build/%/LAUNCH.BIN: build/%/picker.elf tools/launch.py
	$(OBJCOPY) -O binary -j .text $< build/$*/picker.raw
	python3 tools/launch.py $* build/$*/picker.raw $@

# M-Unbricker is its own repo, here as the m-unbricker submodule, and its own Makefile builds it: what it makes is
# already an app. It's Rust too, on its own m-bootloader submodule (so check out submodules recursively), with this
# toolchain and Rust compiler. It's on the card as /MBOOT/APPS/UNBRICK, with apps/unbrick's META.TXT and ICON.QOI.
# One make of it builds all three boards: three at once (make -j) would share its build/rust/ and race.
build/unbrick.stamp: FORCE
	$(MAKE) -C m-unbricker TOOLCHAIN=$(abspath $(TOOLCHAIN)) PI32V2_RUST=$(abspath $(PI32V2_RUST)) \
		build/m-unbricker-fm1.bin build/m-unbricker-smk37.bin build/m-unbricker-any.bin
	@mkdir -p $(@D) && touch $@
build/%/unbrick.app: build/unbrick.stamp
	@mkdir -p $(@D)
	cp m-unbricker/build/m-unbricker-$*.bin $@
FORCE:
.PHONY: FORCE

# What an app with code or a font under another licence carries beside it, as NOTICE.TXT: apps/<app>/NOTICE.TXT (the
# launcher's is apps/picker's) with @SOURCE@ made where its source is, then each licence in <app>_LICENSES.
SOURCE := https://github.com/jvitkauskas/m-apps/tree/$(shell git describe --tags --exact-match 2>/dev/null || git rev-parse HEAD 2>/dev/null || echo main)
slots_LICENSES := LICENSES/OFL-1.1-Barlow.txt
usbdisk_LICENSES := LICENSES/OFL-1.1-Barlow.txt
picker_LICENSES := LICENSES/OFL-1.1-Barlow.txt
unbrick_LICENSES := m-unbricker/web/LICENSES/DejaVu-fonts.txt
wifi_LICENSES := LICENSES/Apache-2.0.txt LICENSES/OFL-1.1-Barlow.txt
notice = sed 's|@SOURCE@|$(SOURCE)|' apps/$(1)/NOTICE.TXT > $(2); for l in $($(1)_LICENSES); do printf '\n' >> $(2); cat $$l >> $(2); done

# make card BOARD=smk37: build/smk37/card/MBOOT, the launcher and these apps in their folders, to copy to a card.
CARD_APPS := hello usbdisk slots tone mp3 unbrick wifi
card: build/$(BOARD)/MENU.BIN build/$(BOARD)/LAUNCH.BIN $(foreach a,$(CARD_APPS),build/$(BOARD)/$(a).app)
	rm -rf build/$(BOARD)/card
	mkdir -p build/$(BOARD)/card/MBOOT/APPS
	cp build/$(BOARD)/MENU.BIN build/$(BOARD)/LAUNCH.BIN build/$(BOARD)/card/MBOOT/
	$(call notice,picker,build/$(BOARD)/card/MBOOT/NOTICE.TXT)
	$(foreach a,$(CARD_APPS),d=build/$(BOARD)/card/MBOOT/APPS/$$(echo $(a) | tr a-z A-Z); mkdir -p $$d; \
		cp build/$(BOARD)/$(a).app $$d/BOOT.APP; cp apps/$(a)/META.TXT $$d/; \
		if [ -f apps/$(a)/ICON.QOI ]; then cp apps/$(a)/ICON.QOI $$d/; fi; \
		if [ -f apps/$(a)/NOTICE.TXT ]; then $(call notice,$(a),$$d/NOTICE.TXT); fi;)

# make release: build/release, what a release carries for the store: <app>-<board>.tar, an app's folder for
# /MBOOT/APPS/<APP>/, and mboot-<board>.tar, MENU.BIN, LAUNCH.BIN and the launcher's NOTICE.TXT for /MBOOT, each a
# plain tar (tools/app_tar.py), and SHA256SUMS.
BOARDS := fm1 smk37
release:
	rm -rf build/release
	$(foreach b,$(BOARDS),$(MAKE) card BOARD=$(b) &&) true
	mkdir -p build/release
	set -e; for b in $(BOARDS); do \
		for a in $(CARD_APPS); do \
			python3 tools/app_tar.py build/release/$$a-$$b.tar build/$$b/card/MBOOT/APPS/$$(echo $$a | tr a-z A-Z); \
		done; \
		rm -rf build/$$b/mboot; mkdir -p build/$$b/mboot; cp build/$$b/MENU.BIN build/$$b/LAUNCH.BIN build/$$b/card/MBOOT/NOTICE.TXT build/$$b/mboot/; \
		python3 tools/app_tar.py build/release/mboot-$$b.tar build/$$b/mboot; \
	done
	cd build/release && ls *.tar | LC_ALL=C sort | xargs sha256sum > SHA256SUMS

.PHONY: all clean test fuzz run log card release
