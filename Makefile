# SPDX-License-Identifier: MIT
SHELL := /bin/bash
.DEFAULT_GOAL := all
.DELETE_ON_ERROR:
# The pi32v2 LLVM toolchain, unpacked in toolchain/ or at TOOLCHAIN=<directory> (see m-bootloader's README).
TOOLCHAIN ?= toolchain
CLANG := $(TOOLCHAIN)/bin/clang
LINK := $(TOOLCHAIN)/bin/ld.lld
OBJCOPY := $(TOOLCHAIN)/bin/llvm-objcopy
MB := m-bootloader
CFLAGS := -target pi32v2 -mcpu=r3 -std=c99 -Oz -ffreestanding -fno-builtin -fno-common -ffunction-sections -fdata-sections \
	-Wall -Wextra -Werror -flto -DRAM_STAGE -Ikit -I$(MB)/include
# The drivers apps get from m-bootloader. --gc-sections drops what an app doesn't use.
DRIVERS := core platform display board runtime usb_midi
APPS := hello crashtest hangtest usbdisk slots tone mp3
# A stack for each app's C code. An app can override it: <name>_STACK := 0x8000
APP_STACK_DEFAULT := 0x4000
# The decoder keeps about 16 KiB on the stack, and speed matters more than size for it.
mp3_STACK := 0x8000
mp3_CFLAGS := -O2 -Iapps/mp3
# The card's MENU.BIN loads the launcher, in the 32 KiB stage 2 has (m-bootloader's docs/SPEC.md §11.5). The launcher
# runs from 0x01C0A800, with the 48 KiB below 0x01C16800, which an app that returns leaves alone (§11.8).
menu_ORIGIN := 0x1c28000
menu_LENGTH := 0x8000
menu_STACK := 0x1000
picker_ORIGIN := 0x1c0a800
picker_LENGTH := 0xc000
picker_STACK := 0x2000

all: $(foreach a,$(APPS),build/fm1/$(a).app build/smk37/$(a).app) build/fm1/logread.bin build/smk37/logread.bin build/fm1/MENU.BIN build/smk37/MENU.BIN build/fm1/LAUNCH.BIN build/smk37/LAUNCH.BIN

clean:
	rm -rf build

test:
	python3 tests/applog_test.py
	python3 tests/picker_test.py
	python3 tests/scene_test.py
	python3 tests/fat_write_test.py
	python3 tests/slots_test.py
	python3 tests/app_tar_test.py

# make run APP=hello BOARD=fm1: start an app with run-ram, wait for it to end, and print its log.
# make log BOARD=fm1: print the log an app left, however it was started.
# M_FLASHER is how to call M-Flasher, such as "sudo m-flasher".
M_FLASHER ?= m-flasher
RUN_FLAGS ?=
run: build/$(BOARD)/$(APP).app build/$(BOARD)/logread.bin
	python3 tools/run.py --flasher "$(M_FLASHER)" --reader build/$(BOARD)/logread.bin $(RUN_FLAGS) build/$(BOARD)/$(APP).app
log: build/$(BOARD)/logread.bin
	python3 tools/run.py --flasher "$(M_FLASHER)" --reader build/$(BOARD)/logread.bin $(RUN_FLAGS)

# board, its -D flag, its number
define BOARD_template
build/$(1)/drv/%.o: $(MB)/src/%.c Makefile
	@mkdir -p $$(@D)
	$(CLANG) $(CFLAGS) $(2) -DAPP_BOARD=$(3) -c $$< -o $$@
build/$(1)/kit/%.o: kit/%.c kit/app.h kit/app_launch.h kit/audio.h kit/card.h kit/fat.h kit/load.h kit/log.h Makefile
	@mkdir -p $$(@D)
	$(CLANG) $(CFLAGS) $(2) -DAPP_BOARD=$(3) -c $$< -o $$@
build/$(1)/kit/start.o: kit/start.S Makefile
	@mkdir -p $$(@D)
	$(CLANG) -target pi32v2 -mcpu=r3 -c $$< -o $$@
# What reads the log back: a program for the ROM's run-ram, in the flash loader's second slot.
build/$(1)/logread.ld: $(MB)/tools/target.ld.S Makefile
	@mkdir -p $$(@D)
	$(CLANG) -E -P -x c -DENTRY_SYMBOL=logread_entry -DRAM_ORIGIN=0x1c28000 -DRAM_LENGTH=0x8000 -DLOADER_BUILD $$< -o $$@
build/$(1)/logread.elf: build/$(1)/kit/logread.o build/$(1)/drv/platform.o build/$(1)/drv/runtime.o build/$(1)/logread.ld
	$(LINK) --plugin-opt=O3 --relax --gc-sections -T build/$(1)/logread.ld -o $$@ $$(filter %.o,$$^)
build/$(1)/logread.bin: build/$(1)/logread.elf
	$(OBJCOPY) -O binary -j .text $$< $$@
endef
$(eval $(call BOARD_template,fm1,,1))
$(eval $(call BOARD_template,smk37,-DBOARD_SMK,2))

# app, board, its -D flag, its number, its name for the trailer
define APP_template
build/$(2)/$(1)/%.o: apps/$(1)/%.c $$(wildcard apps/$(1)/*.h) kit/app.h kit/app_launch.h kit/log.h Makefile
	@mkdir -p $$(@D)
	$(CLANG) $(CFLAGS) $(3) -DAPP_BOARD=$(4) $$($(1)_CFLAGS) -c $$< -o $$@
build/$(2)/$(1)/app.ld: kit/app.ld.S Makefile
	@mkdir -p $$(@D)
	$(CLANG) -E -P -x c -DAPP_STACK=$$(or $$($(1)_STACK),$(APP_STACK_DEFAULT)) -DAPP_ORIGIN=$$(or $$($(1)_ORIGIN),0x1c30000) -DAPP_LENGTH=$$(or $$($(1)_LENGTH),0x4f000) -DAPP_LOW=$$(or $$($(1)_LOW),0x1c16800) $$< -o $$@
build/$(2)/$(1).elf: $$(patsubst apps/$(1)/%.c,build/$(2)/$(1)/%.o,$$(wildcard apps/$(1)/*.c)) build/$(2)/kit/app.o build/$(2)/kit/audio.o build/$(2)/kit/card.o build/$(2)/kit/fat.o build/$(2)/kit/load.o build/$(2)/kit/log.o build/$(2)/kit/start.o $$(addprefix build/$(2)/drv/,$$(addsuffix .o,$(DRIVERS))) build/$(2)/$(1)/app.ld
	$(LINK) --plugin-opt=O3 --relax --gc-sections -T build/$(2)/$(1)/app.ld -Map build/$(2)/$(1).map -o $$@ $$(filter %.o,$$^)
build/$(2)/$(1).app: build/$(2)/$(1).elf $(MB)/tools/stage2.py
	$(OBJCOPY) -O binary -j .text $$< build/$(2)/$(1).raw
	python3 $(MB)/tools/stage2.py --app $(2) build/$(2)/$(1).raw $$@
endef
$(foreach a,$(APPS) menu picker,$(eval $(call APP_template,$(a),fm1,,1)))
$(foreach a,$(APPS) menu picker,$(eval $(call APP_template,$(a),smk37,-DBOARD_SMK,2)))

build/%/MENU.BIN: build/%/menu.elf $(MB)/tools/stage2.py
	$(OBJCOPY) -O binary -j .text $< build/$*/menu.raw
	python3 $(MB)/tools/stage2.py build/$*/menu.raw $@
build/%/LAUNCH.BIN: build/%/picker.elf tools/launch.py
	$(OBJCOPY) -O binary -j .text $< build/$*/picker.raw
	python3 tools/launch.py $* build/$*/picker.raw $@

# make card BOARD=smk37: build/smk37/card/MBOOT, the launcher and these apps in their folders, to copy to a card.
CARD_APPS := hello usbdisk slots tone mp3
card: build/$(BOARD)/MENU.BIN build/$(BOARD)/LAUNCH.BIN $(foreach a,$(CARD_APPS),build/$(BOARD)/$(a).app)
	rm -rf build/$(BOARD)/card
	mkdir -p build/$(BOARD)/card/MBOOT/APPS
	cp build/$(BOARD)/MENU.BIN build/$(BOARD)/LAUNCH.BIN build/$(BOARD)/card/MBOOT/
	$(foreach a,$(CARD_APPS),d=build/$(BOARD)/card/MBOOT/APPS/$$(echo $(a) | tr a-z A-Z); mkdir -p $$d; \
		cp build/$(BOARD)/$(a).app $$d/BOOT.APP; cp apps/$(a)/META.TXT $$d/; \
		if [ -f apps/$(a)/ICON.QOI ]; then cp apps/$(a)/ICON.QOI $$d/; fi;)

# make release: build/release, what a release carries for the store: <app>-<board>.tar, an app's folder for
# /MBOOT/APPS/<APP>/, and mboot-<board>.tar, MENU.BIN and LAUNCH.BIN for /MBOOT, each a plain tar (tools/app_tar.py),
# and SHA256SUMS.
BOARDS := fm1 smk37
release:
	rm -rf build/release
	$(foreach b,$(BOARDS),$(MAKE) card BOARD=$(b) &&) true
	mkdir -p build/release
	set -e; for b in $(BOARDS); do \
		for a in $(CARD_APPS); do \
			python3 tools/app_tar.py build/release/$$a-$$b.tar build/$$b/card/MBOOT/APPS/$$(echo $$a | tr a-z A-Z); \
		done; \
		rm -rf build/$$b/mboot; mkdir -p build/$$b/mboot; cp build/$$b/MENU.BIN build/$$b/LAUNCH.BIN build/$$b/mboot/; \
		python3 tools/app_tar.py build/release/mboot-$$b.tar build/$$b/mboot; \
	done
	cd build/release && ls *.tar | LC_ALL=C sort | xargs sha256sum > SHA256SUMS

.PHONY: all clean test run log card release
