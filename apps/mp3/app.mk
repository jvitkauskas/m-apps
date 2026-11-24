# SPDX-License-Identifier: MIT
# The MP3 player. Its C was -O2 throughout. Here what runs for every frame is optimised for speed, with optimize(speed):
# the decoder, the ring and its pump, the resampling and the analyser. The rest is built small, as the kit is, since
# -O2 made all of it half as big again as the C (120 KB to 78 KB), inlining the kit's FAT and drawing everywhere.
mp3_OPT := z
# The decoder keeps about 16 KiB on the stack.
mp3_STACK := 0x8000
APP_TESTS += tests/mp3_ui_test.py tests/mp3_decoder_test.py
