/* SPDX-License-Identifier: MIT */
/* The smallest useful app: says what it can see, and goes back after a key or 10 seconds. */
#include "app.h"
#define WHITE 0xffff
#define GREEN 0x07e0
#define YELLOW 0xffe0
#define WAIT_MS 10000u

static void show_mhz(unsigned row, unsigned mhz) {
    char line[] = "LSB 00 MHZ";
    line[4] = (char)('0'+mhz/10%10);
    line[5] = (char)('0'+mhz%10);
    display_text(row, WHITE, line);
}

int app_main(unsigned lsb_mhz) {
    board_init(APP_BOARD == 2);
    if (display_init(APP_BOARD)) {
        for (unsigned row = 0; row < 15; ++row)
            display_text(row, WHITE, "");
        display_text(1, GREEN, "HELLO FROM RAM");
        display_text(3, WHITE, APP_BOARD == 2 ? "SMK-37" : "FM-1");
        show_mhz(4, lsb_mhz);
        display_text(6, YELLOW, "ANY KEY: BACK");
        display_text(7, WHITE, "OR WAIT 10 SECONDS");
    }
    while (board_keys())
        hal_delay_us(1000);
    for (unsigned ms = 0; ms < WAIT_MS && !board_keys(); ++ms)
        hal_delay_us(1000);
    board_restore();
    return 0;
}
