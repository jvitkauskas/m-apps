/* SPDX-License-Identifier: MIT */
/* Keeps whole flash images on the card, and swaps between them, with no computer. It writes flash, so after a load it
 * resets, and doesn't return. */
#include "app.h"
#include "card.h"
#include "core.h"
#include "slotfile.h"
#include <string.h>
#define WHITE 0xffff
#define GREEN 0x07e0
#define YELLOW 0xffe0
#define RED 0xf800
#define GREY 0x8410
#define FIRST_ROW 2u
#define ROWS 10u
#define BAR_ROW 12u
#define NOTE_ROW 13u
#define HINT_ROW 14u
#define COLUMNS 20u
#define POLL_US 20000u
#define HOLD_POLLS 150u
#define HOLD_KEY_NAME (APP_BOARD == 2 ? "HOLD SEQ PLAY 3 S":"HOLD PLAY 3 S")
#define HINT (APP_BOARD == 2 ? "< > MOVE SEQ PLAY GO":"OCT-/+ MOVE  PLAY GO")
static struct slots slots;
static unsigned lsb;
static uint16_t chip_key;
static uint8_t work[CORE_WORK_SIZE] __attribute__((aligned(4)));
static struct core_app checked;

/* The card and the panel share SPI1, so drawing has to step off the card. */
static void show_progress(const char *what, unsigned done, unsigned total) {
    app_feed();
    card_close();
    display_text(NOTE_ROW, WHITE, what);
    display_progress(done*100/total, GREEN);
    card_open(lsb);
    card_fast(lsb);
}

/* The same check the bootloader makes before starting firmware. */
static bool image_valid(bool (*read)(void *, uint32_t, void *, uint32_t), void *ctx) {
    struct core_flash flash = {.ctx = ctx, .capacity = SLOT_SIZE, .read = read};
    return core_app_mount(&flash, chip_key, &checked) && core_app_check(&flash, &checked, work) == CORE_APP_OK;
}

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

/* A long press, so that a slip can't overwrite the firmware or delete a slot. */
static bool hold_to_confirm(const char *title) {
    for (unsigned row = FIRST_ROW; row < HINT_ROW; ++row)
        display_text(row, WHITE, "");
    display_text(4, RED, title);
    display_text(6, YELLOW, HOLD_KEY_NAME);
    display_text(7, GREY, "OTHER KEYS: CANCEL");
    if (next_press() != KEY_SELECT)
        return false;
    for (unsigned n = 0; n < HOLD_POLLS; ++n) {
        hal_delay_us(POLL_US);
        app_feed();
        if (board_keys() != KEY_SELECT)
            return false;
        display_progress(n*100/HOLD_POLLS, GREEN);
    }
    display_text(7, GREEN, "RELEASE THE KEY");
    while (board_keys()) {
        hal_delay_us(POLL_US);
        app_feed();
    }
    return true;
}

static void put_name(char *line, const char name[8]) {
    memcpy(line, name, 8);
}

static void draw_row(unsigned row, const char *text, bool selected, uint16_t color) {
    char line[COLUMNS];
    memset(line, ' ', COLUMNS);
    if (selected)
        line[0] = '>';
    memcpy(line+2, text, strlen(text));
    display_row(row, selected ? YELLOW:color, 0, line, COLUMNS);
}

/* Two items follow the slots, and each slot says if it's what's on the unit. */
#define NEW_ITEM (slots.count)
#define BACK_ITEM (slots.count+1)
static void draw_list(unsigned top, unsigned selection, int here) {
    for (unsigned i = 0; i < ROWS; ++i) {
        unsigned item = top+i, row = FIRST_ROW+i;
        char text[COLUMNS-1];
        memset(text, 0, sizeof(text));
        if (item < slots.count) {
            put_name(text, slots.list[item].name);
            text[8] = ' ';
            if ((int)item == here)
                memcpy(text+9, "ON UNIT", 7);
            else if ((int)item == slots_pending(&slots))
                memcpy(text+9, "STOPPED", 7);
            draw_row(row, text, item == selection, WHITE);
        } else if (item == NEW_ITEM)
            draw_row(row, "SAVE AS NEW SLOT", item == selection, GREEN);
        else if (item == BACK_ITEM)
            draw_row(row, "BACK", item == selection, WHITE);
        else
            display_text(row, WHITE, "");
    }
}

static const char NO_CARD[] = "NO CARD FOUND";

static const char *open_slots(unsigned lsb_mhz, int *here) {
    card_open(lsb_mhz);
    uint32_t sectors = card_start(lsb_mhz);
    const char *error = NO_CARD;
    uint8_t uid[8];
    flash_uid(uid);
    if (sectors) {
        error = slots_open(&slots, sectors, uid);
        if (!error)
            *here = slots_on_unit(&slots);
    }
    card_close();
    return error;
}

static const char *on_card(const char *(*work_on)(unsigned), unsigned index) {
    card_open(lsb);
    card_fast(lsb);
    const char *error = work_on(index);
    card_close();
    return error;
}

static const char *do_save(unsigned ignored) {
    (void)ignored;
    char name[8];
    return slots_save_new(&slots, name);
}

static const char *do_load(unsigned index) {
    return slots_load(&slots, index);
}

static const char *do_delete(unsigned index) {
    return slots_delete(&slots, index);
}

static void clear_body(void) {
    for (unsigned row = FIRST_ROW; row < HINT_ROW; ++row)
        display_text(row, WHITE, "");
}

/* What to do with a slot: 0 nothing, 1 load, 2 delete. */
static unsigned slot_menu(unsigned index, bool here) {
    char title[COLUMNS+1];
    memset(title, 0, sizeof(title));
    put_name(title, slots.list[index].name);
    static const char *const load[] = {"LOAD", "DELETE", "BACK"}, *const not_load[] = {"DELETE", "BACK"};
    const char *const *items = here ? not_load:load;
    unsigned count = here ? 2:3, at = 0;
    clear_body();
    display_text(FIRST_ROW, WHITE, title);
    for (;;) {
        for (unsigned i = 0; i < count; ++i)
            draw_row(FIRST_ROW+2+i, items[i], i == at, WHITE);
        unsigned keys = next_press();
        if (keys&KEY_PREV)
            at = at ? at-1:count-1;
        else if (keys&KEY_NEXT)
            at = at+1 < count ? at+1:0;
        else if (keys&KEY_SELECT) {
            if (items[at] == load[0])
                return 1;
            return items[at] == load[1] || items[at] == not_load[0] ? 2:0;
        }
    }
}

static void leave_card(void) {
    card_open(lsb);
    card_finish();
    card_close();
}

int app_main(unsigned lsb_mhz) {
    lsb = lsb_mhz;
    slots.progress = show_progress;
    slots.valid = image_valid;
    board_init(APP_BOARD == 2);
    display_init(APP_BOARD);
    clear_body();
    display_text(0, GREEN, "SLOTS");
    display_text(HINT_ROW, GREY, HINT);
    display_text(NOTE_ROW, WHITE, "READING THE CARD");
    if (!flash_init(1, 1)) {
        display_text(NOTE_ROW, RED, "FLASH ERROR");
        hal_delay_us(5000000);
        board_restore();
        return 1;
    }
    chip_key = (uint16_t)(hal_efuse(1, 3) >> 8);
    int here = NO_SLOT;
    const char *note = open_slots(lsb_mhz, &here);
    bool card_ok = note != NO_CARD;
    uint16_t note_color = RED;
    if (!note && slots_pending(&slots) != NO_SLOT)
        note = "LAST LOAD STOPPED";
    if (note)
        app_log("%s", note);
    app_log("%u slots, on the unit: %d", slots.count, here);
    unsigned selection = slots_pending(&slots) != NO_SLOT ? (unsigned)slots_pending(&slots):here != NO_SLOT ? (unsigned)here:0, top = 0;
    for (;;) {
        if (selection > BACK_ITEM)
            selection = BACK_ITEM;
        if (selection < top)
            top = selection;
        if (selection >= top+ROWS)
            top = selection-ROWS+1;
        draw_list(top, selection, here);
        display_text(BAR_ROW, WHITE, "");
        display_text(NOTE_ROW, note_color, note ? note:"");
        display_text(HINT_ROW, GREY, HINT);
        note = 0;
        note_color = RED;
        unsigned keys = next_press();
        if (keys&KEY_PREV)
            selection = selection ? selection-1:BACK_ITEM;
        else if (keys&KEY_NEXT)
            selection = selection < BACK_ITEM ? selection+1:0;
        else if (keys&KEY_SELECT) {
            if (selection == BACK_ITEM) {
                if (card_ok)
                    leave_card();
                board_restore();
                return 0;
            }
            if (selection == NEW_ITEM) {
                if (!card_ok)
                    continue;
                clear_body();
                note = on_card(do_save, 0);
                app_log("save: %s", note ? note:"ok");
                if (!note) {
                    card_open(lsb);
                    card_fast(lsb);
                    here = slots_on_unit(&slots);
                    card_close();
                    note = "SAVED";
                    note_color = GREEN;
                }
                clear_body();
                continue;
            }
            unsigned action = slot_menu(selection, (int)selection == here);
            char line[COLUMNS+1];
            memset(line, 0, sizeof(line));
            memcpy(line, action == 1 ? "LOAD ":"DELETE ", action == 1 ? 5:7);
            put_name(line+(action == 1 ? 5:7), slots.list[selection].name);
            if (action && !hold_to_confirm(line))
                action = 0;
            clear_body();
            if (action == 2) {
                note = on_card(do_delete, selection);
                app_log("delete: %s", note ? note:"ok");
                card_open(lsb);
                card_fast(lsb);
                here = slots_on_unit(&slots);
                card_close();
            } else if (action == 1) {
                /* The guard stays on: each sector feeds it, so only a real hang reaches it, and it ends in download mode
                 * rather than a unit that needs switching off. Off only for the reset, which should start the firmware. */
                display_text(HINT_ROW, YELLOW, "DO NOT SWITCH OFF");
                note = on_card(do_load, selection);
                app_log("load: %s", note ? note:"ok");
                if (!note) {
                    leave_card();
                    display_text(NOTE_ROW, GREEN, "DONE. RESTARTING");
                    hal_delay_us(1000000);
                    board_restore();
                    app_guard(false);
                    sync_cpu();
                    hal_reset();
                }
                /* It may have saved, so the list is read again. It's safe to try the load again from here. */
                card_open(lsb);
                card_fast(lsb);
                here = slots_on_unit(&slots);
                card_close();
            }
        }
    }
}
