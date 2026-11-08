/* SPDX-License-Identifier: MIT */
/* The card's MENU.BIN (m-bootloader's docs/SPEC.md §11.5): it loads the launcher, /MBOOT/LAUNCH.BIN (apps/picker), and
 * runs it. Stage 2 has 32 KiB at 0x01C28000; the launcher runs at 0x01C0A800, with up to 48 KiB, which an app that
 * returns doesn't touch (§11.8), so this stays out of its way and only says what went wrong. */
#include "app.h"
#include "card.h"
#include "fat.h"
#include "load.h"
#include <string.h>
#define LAUNCHER 0x01c0a800u
#define WHITE 0xffff
#define RED 0xf800
#define GREY 0x8410


static const char *load(unsigned lsb_mhz) {
    card_open(lsb_mhz);
    uint32_t sectors = card_start(lsb_mhz);
    const char *error = sectors ? 0:"NO CARD FOUND";
    if (!error) {
        card_fast(lsb_mhz);
        error = fat_mount(sectors);
    }
    uint32_t folder = 0;
    if (!error) {
        folder = fat_root();
        error = fat_folder(&folder, "MBOOT      ", "NO /MBOOT FOLDER");
    }
    struct fat_entry file;
    bool found = false;
    if (!error)
        error = fat_find(folder, "LAUNCH  BIN", &file, &found);
    if (!error && !found)
        error = "NO /MBOOT/LAUNCH.BIN";
    if (!error)
        error = image_load(&file, (void *)(uintptr_t)LAUNCHER, &launcher_kind, APP_BOARD);
    /* The launcher starts the card itself. */
    if (sectors)
        card_finish();
    card_close();
    return error;
}

int app_main(unsigned lsb_mhz) {
    const char *error = load(lsb_mhz);
    if (!error) {
        /* Like an app (§11.8): its own guard, and the watchdog off while it runs. */
        app_guard(false);
        sync_cpu();
        int result = ((int (*)(unsigned))(uintptr_t)LAUNCHER)(lsb_mhz);
        app_guard(true);
        app_log("the launcher returned %d", result);
        return 0;
    }
    app_log("launcher: %s", error);
    board_init(APP_BOARD == 2);
    if (display_init(APP_BOARD)) {
        for (unsigned row = 0; row < 15; ++row)
            display_text(row, WHITE, "");
        display_text(1, WHITE, "M-APPS");
        display_text(3, RED, error);
        display_text(5, GREY, "PUT LAUNCH.BIN AND");
        display_text(6, GREY, "MENU.BIN IN /MBOOT");
        display_text(8, WHITE, "ANY KEY: BACK");
    }
    while (board_keys())
        hal_delay_us(20000);
    while (!board_keys()) {
        hal_delay_us(20000);
        app_feed();
    }
    board_restore();
    return 0;
}
