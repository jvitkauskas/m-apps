# SPDX-License-Identifier: MIT
# The WiFi app. Its C was all -O2; here it's the kit's size (RUST_OPT, in place of the Makefile's wifi_OPT := 2, which
# made it 170 KB where the C is 121 KB), and its hot paths are built for speed with optimize(speed): the key's SHA-1,
# HMAC and PBKDF2 rounds, the SHA unit, the frames' receive loop, and the radio's start-up and calibration, whose
# register sequences keep the C's -O2 pace. WIFI_TEST=autotest builds the version that needs no keys (wifi_CFG and
# wifi_DEPS, in the Makefile).
wifi_OPT := $(RUST_OPT)
APP_TESTS += tests/wifi_list_test.py tests/wifi_screens_test.py
