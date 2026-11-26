# SPDX-License-Identifier: MIT
# The card's launcher (apps/picker), built as LAUNCH.BIN: its origin, size and stack are in the Makefile, beside
# MENU.BIN's.
APP_TESTS += tests/picker_test.py tests/scene_test.py
# PICKER_KEYS='>>s' builds a launcher that presses keys itself, one every 1.5 s, on through the apps it starts: < and >
# move, s starts, r is REC. What it's built with is remembered, so changing it builds the launcher again.
PICKER_KEYS ?=
export PICKER_KEYS
build/picker-keys.txt: FORCE
	@mkdir -p build
	@echo '$(PICKER_KEYS)' | cmp -s - $@ || echo '$(PICKER_KEYS)' > $@
picker_CFG := $(if $(PICKER_KEYS),--cfg picker_keys)
picker_DEPS := build/picker-keys.txt
