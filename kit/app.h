/* SPDX-License-Identifier: MIT */
#ifndef APP_H
#define APP_H
#include "board.h"
#include "display.h"
#include "platform.h"
/* Set by the build: 1 FM-1, 2 SMK-37, as display_init() and the file's trailer count them. */
#ifndef APP_BOARD
#error "APP_BOARD is set by the Makefile"
#endif
/* The app's main, on its own stack, with interrupts off, and the timer set for lsb_mhz. Returning goes back to
 * what started it, or resets if that was the ROM. See m-bootloader's docs/SPEC.md §11.8 for what it may touch. */
int app_main(unsigned lsb_mhz);
#endif
