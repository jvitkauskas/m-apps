/* SPDX-License-Identifier: MIT */
/* The card's launcher, /MBOOT/LAUNCH.BIN: the apps in /MBOOT/APPS as tiles over rising bubbles, and starts the chosen
 * one. MENU.BIN (apps/menu) loads it to 0x01C0A800, which an app that returns leaves alone (m-bootloader's docs/SPEC.md
 * §11.8), so an app can return to it. Its lists and pictures are in the apps' slot until an app loads there. */
#include "app.h"
#include "card.h"
#include "catalog.h"
#include "fat.h"
#include "load.h"
#include "qoi.h"
#include "scene.h"
#include <string.h>
#define ICON_CACHE 32768u
#define ICON_FILE_MAX 8192u
#define FRAME_MS 25u
#define REPEAT_FIRST_MS 400u
#define REPEAT_MS 120u
#define T4_CON (*(volatile uint32_t *)0x10800u)
#define T4_CNT (*(volatile uint32_t *)0x10804u)
#define T4_PRD (*(volatile uint32_t *)0x10808u)
#define T4_PER_MS 24000u
#if APP_BOARD == 2
#define KEYS "< > MOVE   SEQ PLAY START"
#define KEY_BACK_HINT "SEQ PLAY: BACK"
#else
#define KEYS "OCT-/+ MOVE   PLAY START"
#define KEY_BACK_HINT "PLAY: BACK"
#endif

/* All in the apps' slot, which is free until an app loads over it; the launcher reads and draws it all again then. */
struct work {
    uint8_t layer[SCENE_LAYER_BYTES];                   /* the scene's */
    uint16_t strip[SCENE_SIZE*48];                      /* what's being sent to the panel */
    struct catalog catalog;
    uint32_t icon_at[CATALOG_MOST], icon_size[CATALOG_MOST];    /* in the cache; size 0 without one */
    uint8_t icon_cache[ICON_CACHE];
    uint8_t icon_rgba[SCENE_ICON*SCENE_ICON*4];
};
/* It ends below the log, at 0x01C7F000 (kit/log.h). */
_Static_assert(sizeof(struct work) <= 0x4f000u, "the launcher's work area is too big");
#define W ((struct work *)(uintptr_t)APP_ADDRESS)
#define apps (W->catalog.apps)
#define icon_cache (W->icon_cache)
#define icon_rgba (W->icon_rgba)
#define strip (W->strip)
static unsigned count;
static bool more, card_ok;

/* ---- time: TIMER4 free-running from the 24 MHz crystal ---- */

/* Sets TIMER4 up unless it already runs that way, as after an app that left it alone. True if it started it again. */
static bool time_init(void) {
    if ((T4_CON&15u) == ((2u << 2)|1u) && T4_PRD == 0xffffffffu)
        return false;
    T4_CON = 0;
    T4_PRD = 0xffffffffu;
    T4_CNT = 0;
    T4_CON = (2u << 2)|1u|(1u << 14);
    return true;
}

static unsigned ms_since(uint32_t t) { return (T4_CNT-t)/T4_PER_MS; }

/* ---- the card ---- */

static const char *read_list(unsigned lsb_mhz) {
    count = 0;
    more = false;
    card_open(lsb_mhz);
    uint32_t sectors = card_start(lsb_mhz);
    card_ok = sectors != 0;
    const char *error = "NO CARD FOUND";
    if (card_ok) {
        card_fast(lsb_mhz);
        uint32_t folder = 0;
        error = fat_mount(sectors);
        if (!error) {
            folder = fat_root();
            error = fat_folder(&folder, "MBOOT      ", "NO /MBOOT/APPS");
        }
        if (!error)
            error = fat_folder(&folder, "APPS       ", "NO /MBOOT/APPS");
        if (!error)
            error = catalog_read(folder, &W->catalog);
        if (!error) {
            count = W->catalog.count;
            more = W->catalog.more;
        }
        /* the icons, into the cache while there's room */
        uint32_t used = 0;
        for (unsigned i = 0; i < count; ++i) {
            uint32_t size = apps[i].icon.size;
            W->icon_size[i] = 0;
            if (size && size <= ICON_FILE_MAX && size <= ICON_CACHE-used && fat_read(&apps[i].icon, 0, icon_cache+used, size)) {
                W->icon_at[i] = used;
                W->icon_size[i] = size;
                used += (size+3u)&~3u;
            }
            app_feed();
        }
    }
    card_close();
    return error;
}

/* ---- drawing ---- */

static void draw(int x0, int y0, int x1, int y1) {
    if (x0 < 0)
        x0 = 0;
    if (y0 < 0)
        y0 = 0;
    if (x1 > SCENE_SIZE)
        x1 = SCENE_SIZE;
    if (y1 > SCENE_SIZE)
        y1 = SCENE_SIZE;
    for (int y = y0; y < y1 && x0 < x1; y += 48) {
        int h = y1-y < 48 ? y1-y:48;
        scene_render(x0, y, x1, y+h, strip);
        display_pixels((unsigned)x0, (unsigned)y, (unsigned)(x1-x0), (unsigned)h, (const uint8_t *)strip);
    }
}

static uint16_t colour_for(const char *name) {
    static const uint16_t palette[] = {0xe9a6, 0x2c9f, 0x8c1f, 0x2d2b, 0xfc60, 0x4ad9, 0xd2b9, 0x0596};
    unsigned h = 0;
    for (unsigned i = 0; i < 8; ++i)
        h = h*31u+(uint8_t)name[i];
    return palette[h%(sizeof palette/sizeof palette[0])];
}

static char letter_for(const char *name) {
    for (; *name; ++name) {
        char c = *name;
        if (c >= 'a' && c <= 'z')
            return (char)(c-32);
        if ((c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9'))
            return c;
    }
    return '?';
}

/* app i, or the way back after the last, into slot */
static void tile(unsigned slot, unsigned i, bool selected) {
    struct scene_tile t;
    if (i >= count) {
        t = (struct scene_tile){"Back", "To the bootloader's menu", "", 0, 0x8c71, '<'};
    } else {
        const struct catalog_app *a = &apps[i];
        unsigned w, h;
        bool icon = W->icon_size[i] && qoi_decode(icon_cache+W->icon_at[i], W->icon_size[i], icon_rgba, SCENE_ICON, &w, &h)
                    && w == SCENE_ICON && h == SCENE_ICON;
        t = (struct scene_tile){a->text.name, a->text.about, a->text.version, icon ? icon_rgba:0,
                                colour_for(a->id), letter_for(a->text.name)};
    }
    scene_tile(slot, &t, selected);
}

static void put_number(char *to, unsigned *n, unsigned v) {
    char digits[10];
    unsigned k = 0;
    do digits[k++] = (char)('0'+v%10); while ((v /= 10));
    while (k)
        to[(*n)++] = digits[--k];
}

static void bar(unsigned selection, const char *note, uint16_t colour) {
    char where[24];
    unsigned n = 0;
    if (selection < count) {
        put_number(where, &n, selection+1);
        memcpy(where+n, " / ", 3);
        n += 3;
        put_number(where, &n, count);
    }
    where[n] = 0;
    scene_bar(where, KEYS, note, colour);
}

static void layout(unsigned first, unsigned selection) {
    scene_clear_tiles();
    for (unsigned s = 0; s < SCENE_TILES && first+s <= count; ++s)
        tile(s, first+s, first+s == selection);
    scene_more(first > 0, first+SCENE_TILES <= count);
}

/* ---- keys ---- */

/* A key that's just gone down, or repeats while held. Two equal reads a frame apart make a change count. */
static unsigned held, last_read, repeat_ms;
static uint32_t held_since;

#ifdef KEY_SCRIPT
/* A test build's keys, one every 1.5 s, on through the apps it starts: < > and s for start. */
static const char script[] = KEY_SCRIPT;
static unsigned script_at;
static uint32_t script_last;
#endif

static unsigned press(void) {
#ifdef KEY_SCRIPT
    if (script[script_at] && ms_since(script_last) >= 1500u) {
        char k = script[script_at++];
        script_last = T4_CNT;
        app_log("script: %c", k);
        return k == '<' ? KEY_PREV:k == '>' ? KEY_NEXT:k == 's' ? KEY_SELECT:0u;
    }
#endif
    unsigned now = board_keys(), pressed = 0;
    if (now == last_read) {
        pressed = now&~held;
        if (now != held) {
            held = now;
            held_since = T4_CNT;
            repeat_ms = REPEAT_FIRST_MS;
        } else if (now&(KEY_PREV|KEY_NEXT) && ms_since(held_since) >= repeat_ms) {
            pressed = now&(KEY_PREV|KEY_NEXT);
            repeat_ms += REPEAT_MS;
        }
    }
    last_read = now;
    return pressed;
}

/* ---- running an app ---- */

/* An app with the kit's header gets told its folder (kit/app_launch.h). The record stays in the launcher's RAM, which
 * the app leaves alone. */
static struct app_launch launch;

static int run_app(const struct catalog_app *a, unsigned lsb_mhz) {
    /* Whatever starts an app has the watchdog off (§11.8), and the app sets up a guard of its own. */
    app_guard(false);
    sync_cpu();
    int result;
    if (a->launched_entry) {
        launch = (struct app_launch){{'L', 'N', 'C', 'H'}, APP_LAUNCH_FORMAT, {0, 0, 0}, a->folder, {0}};
        memcpy(launch.folder_name, a->id, sizeof launch.folder_name);
        result = ((int (*)(unsigned, const struct app_launch *))(uintptr_t)a->launched_entry)(lsb_mhz, &launch);
    } else
        result = ((int (*)(unsigned))(uintptr_t)APP_ADDRESS)(lsb_mhz);
    app_guard(true);
    return result;
}

static void describe_result(const char *name, int result, char *line) {
    unsigned n = 0;
    while (name[n] && n < 24) {
        line[n] = name[n];
        ++n;
    }
    memcpy(line+n, " returned ", 10);
    n += 10;
    if (result < 0)
        line[n++] = '-';
    put_number(line, &n, result < 0 ? 0u-(unsigned)result:(unsigned)result);
    line[n] = 0;
}

int app_main(unsigned lsb_mhz) {
    static char note_text[48];
    static char last_run[8];
    const char *note = 0;
    unsigned selection = 0, first = 0;
    time_init();
#ifdef KEY_SCRIPT
    script_last = T4_CNT;
#endif
    for (;;) {
        board_init(APP_BOARD == 2);
        display_init(APP_BOARD);
#if APP_BOARD == 2
        /* The SMK-37's panel takes a faster clock than the bootloader gives it: from LSB/5 to LSB/2. */
        REG(0x11d04) = 1;
#endif
        scene_init(W->layer, T4_CNT);
        const char *error = read_list(lsb_mhz);
        if (error)
            app_log("list: %s", error);
        app_log("%u apps%s", count, more ? ", and more":"");
        /* The folder can have changed, as the app may have been what changed it. */
        if (selection > count)
            selection = count;
        for (unsigned i = 0; i < count; ++i)
            if (!memcmp(apps[i].id, last_run, sizeof last_run))
                selection = i;
        if (error) {
            scene_clear_tiles();
            scene_message("Nothing to start", error);
            scene_bar("", KEY_BACK_HINT, 0, 0);
            selection = count = 0;
        } else {
            if (selection < first || selection >= first+SCENE_TILES)
                first = selection >= SCENE_TILES-1 ? selection-(SCENE_TILES-1):0;
            layout(first, selection);
            bar(selection, note, SCENE_RED);
            note = 0;
        }
        draw(0, 0, SCENE_SIZE, SCENE_SIZE);
        held = last_read = board_keys();
        bool start = false;
        while (!start) {
            uint32_t frame = T4_CNT;
            unsigned keys = press();
            if (keys&KEY_SELECT)
                start = true;
            else if (!error && keys&(KEY_PREV|KEY_NEXT)) {
                unsigned was = selection, old_first = first;
                if (keys&KEY_PREV)
                    selection = selection ? selection-1:count;
                else
                    selection = selection < count ? selection+1:0;
                if (selection < first)
                    first = selection;
                else if (selection >= first+SCENE_TILES)
                    first = selection-(SCENE_TILES-1);
                layout(first, selection);
                bar(selection, 0, 0);
                if (first == old_first) {
                    /* only the two tiles that changed */
                    int a0, a1, b0, b1;
                    scene_tile_rows(was-first, &a0, &a1);
                    scene_tile_rows(selection-first, &b0, &b1);
                    draw(0, a0, SCENE_SIZE, a1);
                    draw(0, b0, SCENE_SIZE, b1);
                } else
                    draw(0, 0, SCENE_SIZE, SCENE_BAR_Y);
                draw(0, SCENE_BAR_Y, SCENE_SIZE, SCENE_SIZE);
#ifdef KEY_SCRIPT
                app_log("moved to %u in %u ms", selection, ms_since(frame));
#endif
            }
            scene_step(draw);
            app_feed();
            while (ms_since(frame) < FRAME_MS)
                ;
        }
        if (selection >= count) {
            if (card_ok) {
                card_open(lsb_mhz);
                card_finish();
                card_close();
            }
            REG(0x11d04) = 4;
            board_restore();
            return 0;
        }
        /* The app loads over the list, so what's needed afterwards is kept here. */
        static struct catalog_app chosen;
        chosen = apps[selection];
        const struct catalog_app *a = &chosen;
        memcpy(note_text, "Starting ", 9);
        unsigned n = 9;
        for (unsigned i = 0; a->text.name[i] && n < sizeof note_text-1; ++i)
            note_text[n++] = a->text.name[i];
        note_text[n] = 0;
        scene_bar("", "", note_text, 0x3a6c);
        draw(0, SCENE_BAR_Y, SCENE_SIZE, SCENE_SIZE);
        card_open(lsb_mhz);
        card_fast(lsb_mhz);
        note = app_file_load(&a->file, (void *)(uintptr_t)APP_ADDRESS, APP_BOARD);
        if (note) {
            app_log("%s: %s", a->text.name, note);
            card_close();
            /* the load wrote over the layer */
            continue;
        }
        /* The app starts the card itself, and may not expect the CRCs on. */
        card_finish();
        card_close();
        REG(0x11d04) = 4;
        board_restore();
        memcpy(last_run, a->id, sizeof last_run);
        int result = run_app(a, lsb_mhz);
        app_log("%s returned %d", a->text.name, result);
        /* An app may have stopped TIMER4 or used it for itself; without it the frame wait would never end. */
        if (time_init()) {
            held_since = T4_CNT;
#ifdef KEY_SCRIPT
            script_last = T4_CNT;
#endif
        }
        if (result) {
            describe_result(a->text.name, result, note_text);
            note = note_text;
        }
    }
}
