/* SPDX-License-Identifier: MIT */
#ifndef APP_H
#define APP_H
#include <stdbool.h>
#include "app_launch.h"
#include "board.h"
#include "display.h"
#include "log.h"
#include "platform.h"
/* Set by the build: 1 FM-1, 2 SMK-37, as display_init() and the file's trailer count them. */
#ifndef APP_BOARD
#error "APP_BOARD is set by the Makefile"
#endif
/* The app's main, on its own stack, with interrupts off, and the timer set for lsb_mhz. Returning goes back to
 * what started it, or to download mode if that was the ROM. See m-bootloader's docs/SPEC.md §11.8 for what it may
 * touch. */
int app_main(unsigned lsb_mhz);
/* The guard is on when app_main starts. If the app hangs or crashes, the unit resets into download mode with the log
 * intact. A hang is 32 s without app_log() or app_feed(), so call one of them in anything slower. */
void app_feed(void);
/* The first cluster of the app's folder on the card, /MBOOT/APPS/<folder>, when the card's launcher started it, and
 * otherwise 0. Mount the card first (card_start(), fat_mount()), then use it as a directory, such as with fat_find(). */
uint32_t app_folder(void);
/* Off for work that must not be cut short, such as writing flash, and when an app would rather restart normally. */
void app_guard(bool on);
#endif
