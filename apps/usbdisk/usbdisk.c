/* SPDX-License-Identifier: MIT */
/* A modded unit's SD card as a USB disk. Eject it on the computer, or press PLAY (SEQ PLAY on the SMK-37), to leave. */
#include "app.h"
#include "card.h"
#include "msc.h"
#define WHITE 0xffff
#define GREEN 0x07e0
#define RED 0xf800
#define TICK_US 100u
#define KEY_EVERY_TICKS 1000u           /* about 0.1 s when idle */
#define NO_HOST_CHECKS 1200u            /* about 2 minutes */
#define AFTER_EJECT_US 300000u

static void show_error(bool screen, const char *text) {
    app_log("%s", text);
    if (screen) {
        display_text(3, RED, text);
        hal_delay_us(5000000);
    }
}

int app_main(unsigned lsb_mhz) {
    board_init(APP_BOARD == 2);
    bool screen = display_init(APP_BOARD);
    if (screen) {
        for (unsigned row = 0; row < 15; ++row)
            display_text(row, WHITE, "");
        display_text(1, GREEN, "USB DISK");
        display_text(3, WHITE, "STARTING THE CARD");
    }
    card_open(lsb_mhz);
    uint32_t sectors = card_start(lsb_mhz);
    card_close();
    if (!sectors) {
        show_error(screen, "NO CARD FOUND");
        board_restore();
        return 1;
    }
    app_log("card: %u sectors, %u MiB", (unsigned)sectors, (unsigned)(sectors/2048));
    if (screen) {
        display_text(3, WHITE, "EJECT IT WHEN DONE");
        display_text(4, WHITE, APP_BOARD == 2 ? "OR PRESS SEQ PLAY":"OR PRESS PLAY");
        display_text(6, WHITE, "ALWAYS EJECT FIRST");
    }
    /* A press that opened this doesn't count as leaving. */
    while (board_keys()&KEY_SELECT)
        hal_delay_us(10000);
    card_open(lsb_mhz);
    card_fast(lsb_mhz);
    msc_start(sectors);
    unsigned ticks = 0, checks = 0;
    bool was_configured = false, pressed = false, no_host = false;
    while (!msc_ejected()) {
        msc_poll();
        hal_delay_us(TICK_US);
        if (++ticks < KEY_EVERY_TICKS)
            continue;
        ticks = 0;
        app_feed();
        if (msc_configured() && !was_configured) {
            app_log("host connected");
            was_configured = true;
        }
        if (board_keys()&KEY_SELECT) {
            pressed = true;
            break;
        }
        if (!was_configured && ++checks >= NO_HOST_CHECKS) {
            no_host = true;
            break;
        }
    }
    /* Let the host hear the end of its last command. */
    for (unsigned n = 0; n < AFTER_EJECT_US/TICK_US; ++n) {
        msc_poll();
        hal_delay_us(TICK_US);
    }
    msc_stop();
    card_finish();
    card_close();
    const struct msc_stats *stats = msc_stats();
    app_log("%s: read %u KiB, wrote %u KiB, %u errors", pressed ? "PLAY pressed":no_host ? "no host":"ejected",
            (unsigned)(stats->sectors_read/2), (unsigned)(stats->sectors_written/2), (unsigned)stats->errors);
    board_restore();
    return 0;
}
