/* SPDX-License-Identifier: MIT */
/* Lists the MP3 files in the card's root and plays one, polled, while showing the time. */
#include "app.h"
#include "audio.h"
#include "card.h"
#include "fat.h"
#include <string.h>
#define MINIMP3_ONLY_MP3
#include "minimp3.h"
#define WHITE 0xffff
#define GREEN 0x07e0
#define YELLOW 0xffe0
#define RED 0xf800
#define GREY 0x8410
#define MOST_FILES 64u
#define FIRST_ROW 2u
#define ROWS 10u
#define NOTE_ROW 13u
#define HINT_ROW 14u
#define COLUMNS 20u
#define POLL_US 20000u
#define CHUNK 4096u
#define INPUT (16u*1024u)
#define KEY_EVERY_HALVES 4u
/* As firmware has it: 16-bit samples at -6 dB of the DAC's 24 bits. */
#define SAMPLE_SHIFT 7
/* TIMER4, from the crystal, whatever the CPU's doing. */
#define TIMER_CON REG(0x10800u)
#define TIMER_COUNT REG(0x10804u)
#define TIMER_PERIOD REG(0x10808u)
#define TICKS_PER_US 24u
static struct fat_named files[MOST_FILES];
static unsigned count, lsb;
static bool more;
static mp3dec_t decoder;
static uint8_t chunk[CHUNK] __attribute__((aligned(4)));
static uint8_t input[INPUT];
static int16_t pcm[MINIMP3_MAX_SAMPLES_PER_FRAME];

struct stats {
    uint32_t frames, late, most_decode, most_read, most_draw, waited, busy;
};

static void timer_start(void) {
    if ((TIMER_CON&0xfu) == ((2u << 2) | 1u))
        return;
    TIMER_CON = 0;
    TIMER_PERIOD = 0xffffffffu;
    TIMER_COUNT = 0;
    TIMER_CON = (2u << 2) | 1u | (1u << 14);
}

static const char *read_files(void) {
    card_open(lsb);
    uint32_t sectors = card_start(lsb);
    const char *error = "NO CARD FOUND";
    count = 0;
    if (sectors) {
        error = fat_mount(sectors);
        if (!error)
            error = fat_list_named(fat_root(), "MP3", files, MOST_FILES, &count, &more);
        if (!error && !count)
            error = "NO MP3 FILES";
    }
    card_close();
    for (unsigned i = 0; i < count; ++i)
        app_log("%s %u", files[i].name, (unsigned)files[i].file.size);
    return error;
}

/* The name for the screen, which has no more than ASCII's capitals and a few others, and draws the rest as blanks. A
 * better font only has to change this. It leaves off the .MP3. */
static unsigned screen_name(char *out, unsigned room, const char *name) {
    unsigned n = 0;
    for (const uint8_t *p = (const uint8_t *)name; *p && n < room; ++p) {
        if (*p < 0x80)
            out[n++] = (char)*p;
        else if (*p >= 0xc0)                    /* the first byte of a character */
            out[n++] = '?';
    }
    if (n >= 4 && out[n-4] == '.' && (out[n-3]|32) == 'm' && (out[n-2]|32) == 'p' && out[n-1] == '3')
        n -= 4;
    return n;
}

/* The selected name, if it doesn't fit: it stays a moment, moves along to its end, and starts again. */
static void (*on_tick)(void);

static unsigned next_press(void) {
    unsigned held = board_keys(), last = held;
    for (;;) {
        hal_delay_us(POLL_US);
        app_feed();
        if (on_tick)
            on_tick();
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

static void draw_row(unsigned row, const char *text, unsigned n, bool selected) {
    char line[COLUMNS];
    memset(line, ' ', COLUMNS);
    if (selected)
        line[0] = '>';
    memcpy(line+2, text, n > COLUMNS-2 ? COLUMNS-2:n);
    display_row(row, selected ? YELLOW:WHITE, 0, line, COLUMNS);
}

#define NAME_ROOM (COLUMNS-2)
#define SCROLL_WAIT_TICKS 50u
#define SCROLL_STEP_TICKS 15u
static char scroll_text[FAT_NAME_BYTES];
static unsigned scroll_length, scroll_row, scroll_at, scroll_ticks;

static void scroll_set(unsigned row, const char *name) {
    scroll_length = screen_name(scroll_text, sizeof(scroll_text), name);
    scroll_row = row;
    scroll_at = 0;
    scroll_ticks = 0;
}

static void scroll_tick(void) {
    if (scroll_length <= NAME_ROOM)
        return;
    bool at_start = !scroll_at, at_end = scroll_at+NAME_ROOM >= scroll_length;
    if (++scroll_ticks < (at_start || at_end ? SCROLL_WAIT_TICKS:SCROLL_STEP_TICKS))
        return;
    scroll_ticks = 0;
    scroll_at = at_end ? 0:scroll_at+1;
    draw_row(scroll_row, scroll_text+scroll_at, scroll_length-scroll_at, true);
}

static void clear_body(void) {
    for (unsigned row = FIRST_ROW; row < HINT_ROW; ++row)
        display_text(row, WHITE, "");
}

static char *put_time(char *to, unsigned seconds) {
    unsigned minutes = seconds/60;
    if (minutes > 999)
        minutes = 999;
    if (minutes >= 100)
        *to++ = (char)('0'+minutes/100);
    *to++ = (char)('0'+minutes/10%10);
    *to++ = (char)('0'+minutes%10);
    *to++ = ':';
    *to++ = (char)('0'+seconds%60/10);
    *to++ = (char)('0'+seconds%10%10);
    *to = 0;
    return to;
}

/* A VBR file says its frame count in its first frame. Otherwise, the size and the bitrate say about how long it is. */
static uint32_t frames_said(const uint8_t *frame, unsigned size) {
    static const unsigned places[] = {36, 21, 13};
    for (unsigned i = 0; i < 3; ++i) {
        unsigned at = places[i];
        if (at+12 > size || (memcmp(frame+at, "Xing", 4) && memcmp(frame+at, "Info", 4)))
            continue;
        if (frame[at+7]&1)
            return (uint32_t)frame[at+8] << 24 | (uint32_t)frame[at+9] << 16 | (uint32_t)frame[at+10] << 8 | frame[at+11];
    }
    return 0;
}

static void show_time(unsigned elapsed, unsigned total, struct stats *stats) {
    char line[COLUMNS+1], *p = line;
    memcpy(p, "ELAPSED ", 8);
    put_time(p+8, elapsed);
    uint32_t before = TIMER_COUNT;
    card_close();
    display_text(6, WHITE, line);
    memcpy(line, "LEFT    ", 8);
    put_time(line+8, total > elapsed ? total-elapsed:0);
    display_text(7, WHITE, line);
    card_open(lsb);
    card_fast(lsb);
    uint32_t took = (TIMER_COUNT-before)/TICKS_PER_US;
    if (took > stats->most_draw)
        stats->most_draw = took;
}

/* Where the output is going, a half at a time. */
static int32_t *half;
static unsigned half_at;
static int last_half;

static int32_t *next_half(struct stats *stats) {
    uint32_t before = TIMER_COUNT;
    int32_t *h;
    while (!(h = audio_free_half()))
        ;
    uint32_t waited = (TIMER_COUNT-before)/TICKS_PER_US;
    stats->waited += waited;
    /* It always takes turns, so the same half twice means one was missed, and played twice. */
    int which = h == audio_half(0) ? 0:1;
    if (which == last_half)
        ++stats->late;
    last_half = which;
    return h;
}

static void put_frame(int32_t left, int32_t right, struct stats *stats) {
    if (!half) {
        half = next_half(stats);
        half_at = 0;
    }
    half[2*half_at] = left << SAMPLE_SHIFT;
    half[2*half_at+1] = right << SAMPLE_SHIFT;
    if (++half_at == AUDIO_HALF_FRAMES) {
        audio_filled();
        half = 0;
    }
}

/* Runs to the end of the file or a key. It returns what to show, if not fine. */
static const char *play(const struct fat_entry *file, struct stats *stats) {
    uint32_t offset = 0, have = 0, step = 0, position = 0, samples = 0, total_frames = 0, per_frame = 0;
    unsigned hz = 0, total = 0, shown = 0xffffffffu, halves = 0, keys = 0;
    int32_t before_left = 0, before_right = 0;
    bool first = true, pressed_at_start = board_keys() != 0;
    memset(stats, 0, sizeof(*stats));
    memset(&decoder, 0, sizeof(decoder));
    mp3dec_init(&decoder);
    half = 0;
    last_half = -1;
    audio_start();
    card_open(lsb);
    card_fast(lsb);
    const char *error = 0;
    uint32_t started = TIMER_COUNT;
    for (;;) {
        uint32_t a = TIMER_COUNT;
        /* Before a frame could run off the end of what's read. */
        while (have < INPUT-CHUNK && offset < file->size) {
            uint32_t n = file->size-offset < CHUNK ? file->size-offset:CHUNK;
            if (!fat_read(file, offset, chunk, n)) {
                error = "CARD READ ERROR";
                break;
            }
            memcpy(input+have, chunk, n);
            have += n;
            offset += n;
        }
        if (error)
            break;
        uint32_t b = TIMER_COUNT;
        mp3dec_frame_info_t info;
        int decoded = mp3dec_decode_frame(&decoder, input, (int)have, pcm, &info);
        uint32_t c = TIMER_COUNT;
        if (!info.frame_bytes)
            break;
        if (first)
            total_frames = frames_said(input+info.frame_offset, (unsigned)(info.frame_bytes-info.frame_offset));
        have -= (uint32_t)info.frame_bytes;
        memmove(input, input+info.frame_bytes, have);
        if ((b-a)/TICKS_PER_US > stats->most_read)
            stats->most_read = (b-a)/TICKS_PER_US;
        if ((c-b)/TICKS_PER_US > stats->most_decode)
            stats->most_decode = (c-b)/TICKS_PER_US;
        app_feed();
        if (first) {
            first = false;
            hz = (unsigned)info.hz;
            per_frame = decoded ? (uint32_t)decoded:1152u;
            uint32_t seconds = total_frames ? total_frames*per_frame/hz
                               :info.bitrate_kbps ? (file->size-offset+have)/((unsigned)info.bitrate_kbps*125u):0;
            total = (unsigned)seconds;
            app_log("%u Hz, %d channels, %d kbps, %u s, %u frames said", hz, info.channels, info.bitrate_kbps, total, (unsigned)total_frames);
            /* In 1/65536ths of a frame, from the rate the DAC really runs at. */
            step = hz == 44100u ? 0:(uint32_t)(((uint64_t)hz*AUDIO_RATE_DEN << 16)/AUDIO_RATE_NUM);
        }
        if (!decoded)
            continue;
        ++stats->frames;
        samples += (uint32_t)decoded;
        for (int i = 0; i < decoded; ++i) {
            int32_t left = pcm[info.channels == 2 ? 2*i:i], right = pcm[info.channels == 2 ? 2*i+1:i];
            if (!step) {
                put_frame(left, right, stats);
                continue;
            }
            /* Linear, from the last frame to this one. */
            while (position < 65536u) {
                int32_t t = (int32_t)position;
                put_frame(before_left+(((left-before_left)*t) >> 16), before_right+(((right-before_right)*t) >> 16), stats);
                position += step;
            }
            position -= 65536u;
            before_left = left;
            before_right = right;
        }
        unsigned elapsed = hz ? samples/hz:0;
        if (elapsed != shown) {
            shown = elapsed;
            show_time(elapsed, total, stats);
        }
        if (++halves%KEY_EVERY_HALVES == 0) {
            unsigned k = board_keys();
            if (k && !pressed_at_start)
                break;
            if (!k)
                pressed_at_start = false;
            (void)keys;
        }
    }
    stats->busy = (TIMER_COUNT-started)/TICKS_PER_US;
    if (half) {
        while (half_at < AUDIO_HALF_FRAMES)
            put_frame(0, 0, stats);
    }
    /* Silence in both halves, so that stopping doesn't leave the DAC holding a sample. */
    for (unsigned i = 0; i < 2; ++i) {
        int32_t *h;
        while (!(h = audio_free_half()))
            ;
        memset(h, 0, 2*AUDIO_HALF_FRAMES*sizeof(int32_t));
        audio_filled();
    }
    audio_stop();
    card_close();
    return error;
}

int app_main(unsigned lsb_mhz) {
    lsb = lsb_mhz;
    timer_start();
    board_init(APP_BOARD == 2);
    display_init(APP_BOARD);
    clear_body();
    display_text(0, GREEN, "MP3 PLAYER");
    display_text(NOTE_ROW, WHITE, "READING THE CARD");
    const char *note = read_files();
    app_log("%u files, CPU check LSB %u, audio clock stock %u", count, lsb_mhz, audio_clock_is_stock());
    unsigned selection = 0, top = 0;
    for (;;) {
        if (selection < top)
            top = selection;
        if (selection >= top+ROWS)
            top = selection-ROWS+1;
        for (unsigned i = 0; i < ROWS; ++i) {
            unsigned item = top+i;
            if (item < count) {
                char shown[FAT_NAME_BYTES];
                unsigned n = screen_name(shown, sizeof(shown), files[item].name);
                draw_row(FIRST_ROW+i, shown, n, item == selection);
                if (item == selection)
                    scroll_set(FIRST_ROW+i, files[item].name);
            } else if (item == count)
                draw_row(FIRST_ROW+i, "BACK", 4, item == selection);
            else
                display_text(FIRST_ROW+i, WHITE, "");
        }
        display_text(NOTE_ROW, RED, note ? note:"");
        display_text(HINT_ROW, GREY, APP_BOARD == 2 ? "< > MOVE SEQ PLAY GO":"OCT-/+ MOVE  PLAY GO");
        note = 0;
        if (selection >= count)
            scroll_length = 0;
        on_tick = scroll_tick;
        unsigned keys = next_press();
        on_tick = 0;
        if (keys&KEY_PREV)
            selection = selection ? selection-1:count;
        else if (keys&KEY_NEXT)
            selection = selection < count ? selection+1:0;
        else if (keys&KEY_SELECT) {
            if (selection >= count) {
                card_open(lsb);
                card_finish();
                card_close();
                board_restore();
                return 0;
            }
            clear_body();
            display_text(1, GREEN, "PLAYING");
            /* All of it, on as many rows as it takes, up to where the time goes. */
            char shown[FAT_NAME_BYTES];
            unsigned n = screen_name(shown, sizeof(shown), files[selection].name);
            for (unsigned part = 0; part < 2 && part*COLUMNS < n; ++part) {
                unsigned left = n-part*COLUMNS;
                display_row(3+part, WHITE, 0, shown+part*COLUMNS, left > COLUMNS ? COLUMNS:left);
            }
            display_text(HINT_ROW, GREY, "ANY KEY: BACK");
            struct stats stats;
            note = play(&files[selection].file, &stats);
            app_log("played %u frames in %u ms: %u late, longest read %u us, decode %u us, draw %u us, waited %u ms",
                    (unsigned)stats.frames, (unsigned)(stats.busy/1000u), (unsigned)stats.late, (unsigned)stats.most_read, (unsigned)stats.most_decode,
                    (unsigned)stats.most_draw, (unsigned)(stats.waited/1000u));
            clear_body();
        }
    }
}
