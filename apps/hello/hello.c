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
    app_log("hello on board %d, %s", APP_BOARD, app_folder() ? "from its folder":"not from a folder");
    board_init(APP_BOARD == 2);
    bool screen = display_init(APP_BOARD);
    app_log("screen %s", screen ? "found":"not found");
    if (screen) {
        for (unsigned row = 0; row < 15; ++row)
            display_text(row, WHITE, "");
        display_text(1, GREEN, "HELLO FROM RAM");
        display_text(3, WHITE, APP_BOARD == 2 ? "SMK-37" : "FM-1");
        show_mhz(4, lsb_mhz);
        if (app_folder())
            display_text(5, WHITE, "FROM ITS FOLDER");
        display_text(6, YELLOW, "ANY KEY: BACK");
        display_text(7, WHITE, "OR WAIT 10 SECONDS");
    }
    while (board_keys())
        hal_delay_us(1000);
    unsigned ms = 0;
    for (; ms < WAIT_MS && !board_keys(); ++ms)
        hal_delay_us(1000);
    app_log(ms < WAIT_MS ? "a key after %u ms":"no key in %u ms", ms);
    board_restore();
    return 0;
}
