/* SPDX-License-Identifier: MIT */
/* Lists the apps in /MBOOT/APPS on the card and starts the chosen one. It's the card's MENU.BIN (m-bootloader's
 * docs/SPEC.md §11.5), below the apps' slot, so an app can return to it. */
#include "app.h"
#include "card.h"
#include "fat.h"
#include "load.h"
#include <string.h>
#define WHITE 0xffff
#define GREEN 0x07e0
#define YELLOW 0xffe0
#define RED 0xf800
#define GREY 0x8410
#define MOST_APPS 100u
#define FIRST_ROW 2u
#define ROWS 10u
#define MORE_ROW 12u
#define NOTE_ROW 13u
#define HINT_ROW 14u
#define COLUMNS 20u
#define POLL_US 20000u
#define NAME_COLUMN 2u
static struct fat_entry apps[MOST_APPS];
static unsigned count;
static bool more, card_ok;

static const char *read_list(unsigned lsb_mhz) {
    count = 0;
    more = false;
    card_open(lsb_mhz);
    uint32_t sectors = card_start(lsb_mhz);
    card_ok = sectors != 0;
    const char *error = "NO CARD FOUND";
    if (card_ok) {
        uint32_t folder = 0;
        error = fat_mount(sectors);
        if (!error) {
            folder = fat_root();
            error = fat_folder(&folder, "MBOOT      ", "NO /MBOOT/APPS");
        }
        if (!error)
            error = fat_folder(&folder, "APPS       ", "NO /MBOOT/APPS");
        if (!error)
            error = fat_list(folder, "APP", apps, MOST_APPS, &count, &more);
        if (!error && !count)
            error = "NO APPS IN FOLDER";
    }
    card_close();
    return error;
}

static void put_number(char *end, unsigned value) {
    do {
        *--end = (char)('0'+value%10);
        value /= 10;
    } while (value);
}

static void draw_item(unsigned row, unsigned item, bool selected) {
    char line[COLUMNS];
    memset(line, ' ', COLUMNS);
    if (selected)
        line[0] = '>';
    if (item < count) {
        memcpy(line+NAME_COLUMN, apps[item].name, sizeof(apps[item].name));
        line[COLUMNS-1] = 'K';
        put_number(line+COLUMNS-1, (apps[item].size >> 10)+((apps[item].size&1023) != 0));
    } else
        memcpy(line+NAME_COLUMN, "BACK", 4);
    display_row(row, selected ? YELLOW:WHITE, 0, line, COLUMNS);
}

static void draw_list(unsigned top, unsigned selection) {
    for (unsigned i = 0; i < ROWS; ++i) {
        if (top+i <= count)
            draw_item(FIRST_ROW+i, top+i, top+i == selection);
        else
            display_text(FIRST_ROW+i, WHITE, "");
    }
}

/* The next key to be pressed. Two equal polls in a row make a change count, and a held key doesn't repeat. */
static unsigned next_press(void) {
    unsigned held = board_keys(), last = held;
    for (;;) {
        hal_delay_us(POLL_US);
        app_feed();
        unsigned now = board_keys();
        if (now == last) {
            unsigned pressed = now&~held;
            held = now;
            if (pressed)
                return pressed;
        }
        last = now;
    }
}

/* Fits the screen's width for any int: RETURNED -2147483648. */
static void describe_result(int result, char *line) {
    char digits[11];
    unsigned n = 0, value = result < 0 ? 0u-(unsigned)result:(unsigned)result, at = 9;
    do {
        digits[n++] = (char)('0'+value%10);
        value /= 10;
    } while (value);
    memcpy(line, "RETURNED ", at);
    if (result < 0)
        line[at++] = '-';
    while (n)
        line[at++] = digits[--n];
    line[at] = 0;
}

static int run_app(unsigned lsb_mhz) {
    /* Whatever starts an app has the watchdog off (§11.8), and the app sets up a guard of its own. */
    app_guard(false);
    sync_cpu();
    int result = ((int (*)(unsigned))(uintptr_t)APP_ADDRESS)(lsb_mhz);
    app_guard(true);
    return result;
}

int app_main(unsigned lsb_mhz) {
    static char result_note[COLUMNS+1];
    static char last_run[sizeof(apps[0].name)];
    const char *note = 0;
    unsigned selection = 0, top = 0;
    for (;;) {
        board_init(APP_BOARD == 2);
        display_init(APP_BOARD);
        for (unsigned row = 0; row < 15; ++row)
            display_text(row, WHITE, "");
        display_text(0, GREEN, "APPS");
        display_text(NOTE_ROW, WHITE, "READING THE CARD");
        const char *error = read_list(lsb_mhz);
        if (error)
            app_log("list: %s", error);
        app_log("%u apps%s", count, more ? ", and more":"");
        display_text(NOTE_ROW, WHITE, "");
        display_text(MORE_ROW, GREY, more ? "ONLY THE FIRST 100":"");
        display_text(HINT_ROW, GREY, APP_BOARD == 2 ? "< > MOVE SEQ PLAY GO":"OCT-/+ MOVE  PLAY GO");
        /* The folder can have changed, as the app may have been what changed it. */
        if (selection > count)
            selection = count;
        for (unsigned i = 0; i < count; ++i)
            if (!memcmp(apps[i].name, last_run, sizeof(last_run)))
                selection = i;
        if (!note)
            note = error;
        while (true) {
            if (selection < top)
                top = selection;
            if (selection >= top+ROWS)
                top = selection-ROWS+1;
            draw_list(top, selection);
            display_text(NOTE_ROW, RED, note ? note:"");
            note = error;
            unsigned keys = next_press();
            if (keys&KEY_PREV)
                selection = selection ? selection-1:count;
            else if (keys&KEY_NEXT)
                selection = selection < count ? selection+1:0;
            else if (keys&KEY_SELECT) {
                if (selection >= count) {
                    if (card_ok) {
                        card_open(lsb_mhz);
                        card_finish();
                        card_close();
                    }
                    board_restore();
                    return 0;
                }
                display_text(NOTE_ROW, WHITE, "LOADING");
                card_open(lsb_mhz);
                card_fast(lsb_mhz);
                note = app_file_load(&apps[selection], (void *)(uintptr_t)APP_ADDRESS, APP_BOARD);
                if (note) {
                    app_log("%c%c%c%c%c%c%c%c: %s", apps[selection].name[0], apps[selection].name[1], apps[selection].name[2],
                            apps[selection].name[3], apps[selection].name[4], apps[selection].name[5], apps[selection].name[6],
                            apps[selection].name[7], note);
                    card_close();
                    continue;
                }
                /* The app starts the card itself, and may not expect the CRCs on. */
                card_finish();
                card_close();
                board_restore();
                memcpy(last_run, apps[selection].name, sizeof(last_run));
                int result = run_app(lsb_mhz);
                app_log("the app returned %d", result);
                if (result) {
                    describe_result(result, result_note);
                    note = result_note;
                }
                break;
            }
        }
    }
}
