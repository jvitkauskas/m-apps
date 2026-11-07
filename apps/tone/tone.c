/* SPDX-License-Identifier: MIT */
/* Plays two quiet tones and measures the real sample rate: the first step towards playing files. */
#include "app.h"
#include "audio.h"
#define WHITE 0xffff
#define GREEN 0x07e0
#define TONE_S 3u
#define TABLE 1024u
#define LEVEL (8388607.0f*0.1f)
#define HALVES_PER_S (AUDIO_RATE_NUM/AUDIO_RATE_DEN/AUDIO_HALF_FRAMES)
static int32_t wave[TABLE];

/* One cycle, by a rotation, as there's no maths library. */
static void make_wave(void) {
    float c = 1.0f, s = 0.0f, step = 2.0f*3.14159265f/TABLE;
    for (unsigned i = 0; i < TABLE; ++i) {
        wave[i] = (int32_t)(s*LEVEL);
        float next_c = c-s*step, next_s = s+c*step;
        float fix = 1.0f/(next_c*next_c+next_s*next_s)*0.5f+0.5f;
        c = next_c*fix;
        s = next_s*fix;
    }
}

static unsigned play(unsigned hz, unsigned lsb_mhz, uint32_t *late) {
    uint32_t phase = 0, step = (uint32_t)(((uint64_t)hz*TABLE << 16)*AUDIO_RATE_DEN/AUDIO_RATE_NUM);
    /* Timer 0 running free, as hal_delay_us() isn't used while this plays. */
    REG(0x10400) = BIT(14);
    REG(0x10404) = 0;
    REG(0x10408) = 0xffffffffu;
    REG(0x10400) = BIT(0);
    unsigned halves = HALVES_PER_S*TONE_S;
    uint32_t started = 0, last = 0, most = 0;
    for (unsigned h = 0; h < halves; ) {
        int32_t *half = audio_free_half();
        uint32_t now = REG(0x10404);
        if (!half) {
            if (now-last > 400000000u)
                return 0;
            continue;
        }
        if (!h)
            started = now;
        else if (now-last > most)
            most = now-last;
        last = now;
        for (unsigned f = 0; f < AUDIO_HALF_FRAMES; ++f) {
            int32_t v = wave[(phase >> 16)%TABLE];
            phase += step;
            half[2*f] = v;
            half[2*f+1] = v;
        }
        audio_filled();
        ++h;
        app_feed();
    }
    uint32_t ticks = last-started;
    REG(0x10400) = BIT(14);
    *late = most/lsb_mhz;
    /* Frames per second, from what was counted. */
    return (unsigned)(((uint64_t)(halves-1)*AUDIO_HALF_FRAMES*lsb_mhz*1000000u)/ticks);
}

int app_main(unsigned lsb_mhz) {
    display_init(APP_BOARD);
    for (unsigned row = 0; row < 15; ++row)
        display_text(row, WHITE, "");
    display_text(1, GREEN, "TONE TEST");
    app_log("LSB %u, PLL %08x %08x, audio clock stock: %u", lsb_mhz, (unsigned)REG(0x119a0), (unsigned)REG(0x119a4), audio_clock_is_stock());
    make_wave();
    audio_start();
    app_log("started, CON0 %x CON3 %x", (unsigned)*(volatile uint16_t *)0x12e00, (unsigned)*(volatile uint8_t *)0x12e0c);
    uint32_t gap = 0;
    display_text(3, WHITE, "440 HZ");
    unsigned rate = play(440, lsb_mhz, &gap);
    app_log("440 Hz: %u frames/s, longest half %u us", rate, (unsigned)gap);
    display_text(4, WHITE, "880 HZ");
    rate = play(880, lsb_mhz, &gap);
    app_log("880 Hz: %u frames/s, longest half %u us", rate, (unsigned)gap);
    audio_stop();
    hal_timer_clock(lsb_mhz);
    display_text(6, GREEN, "DONE");
    hal_delay_us(1000000);
    return rate ? 0:1;
}
