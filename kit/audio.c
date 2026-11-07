/* SPDX-License-Identifier: MPL-2.0
 * Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments
 * Copyright (C) 2026 Kerem Kilic (Ellic Studio) */
/* Adapted from felucca-bsp's boards/fm1/fm1_audio.h (MPL-2.0): polled rather than by interrupt, on the kit's helpers. The
 * pins are the same on the FM-1 and the SMK-37: PC0 MCLK, PC1 BCLK, PC2 LRCK, PC6 data. */
#include "audio.h"
#include "platform.h"
#define ALNK 0x12e00u
#define CON0 (*(volatile uint16_t *)(uintptr_t)(ALNK+0x00u))
#define CON1 (*(volatile uint16_t *)(uintptr_t)(ALNK+0x04u))
#define CON2 (*(volatile uint8_t *)(uintptr_t)(ALNK+0x08u))
#define CON3 (*(volatile uint8_t *)(uintptr_t)(ALNK+0x0cu))
#define ADR3 (*(volatile uint32_t *)(uintptr_t)(ALNK+0x1cu))
#define LEN (*(volatile uint16_t *)(uintptr_t)(ALNK+0x20u))
#define CLK_CON2 REG(0x10014u)
#define IOMAP_CON5 REG(0x51030u)
#define PLL_CON0 REG(0x119a0u)
#define PLL_CON1 REG(0x119a4u)
#define PLL_STOCK0 0x45400203u
#define PLL_STOCK1 0x3f503026u
#define PLL_CON1_MASK 0x07ffffffu
#define PENDING_HALF 0x80u
#define HALF_WORDS (2u*AUDIO_HALF_FRAMES)
static int32_t buffer[2u*HALF_WORDS] __attribute__((aligned(4)));

bool audio_clock_is_stock(void) {
    return PLL_CON0 == PLL_STOCK0 && (PLL_CON1&PLL_CON1_MASK) == (PLL_STOCK1&PLL_CON1_MASK);
}

static void pin_out(unsigned pin, bool high) {
    GPIO(2, 0xc) |= BIT(pin);
    gpio_output(2, pin, high);
}

void audio_start(void) {
    for (unsigned i = 0; i < 2u*HALF_WORDS; ++i)
        buffer[i] = 0;
    pin_out(6, false);
    hal_delay_us(5000);
    CON0 = 0;
    CON1 = 0;
    CON2 = 0;
    CON3 = 0;
    IOMAP_CON5 &= ~0xc0u;
    CON3 |= 3u;
    pin_out(0, true);
    CON0 |= 0x100u;
    pin_out(2, true);
    pin_out(1, true);
    CON0 |= 0x80u;
    CON0 &= (uint16_t)~0x40u;
    LEN = (uint16_t)HALF_WORDS;
    CON0 &= (uint16_t)~0x400u;
    CON0 &= (uint16_t)~0x200u;
    CLK_CON2 &= 0xfffff0ffu;
    CON3 &= (uint8_t)~0x1cu;
    CON3 = (uint8_t)((CON3&0x1fu) | 0x80u);
    CON1 |= 1u << 12;
    CON1 |= 1u << 14;
    ADR3 = (uint32_t)(uintptr_t)buffer;
    pin_out(6, true);
    CON1 &= (uint16_t)~(1u << 15);
    CON2 = 0x0fu;
    sync_cpu();
    CON0 |= 0x800u;
    hal_delay_us(5000);
    pin_out(6, true);
}

int32_t *audio_free_half(void) {
    uint8_t pending = CON2;
    if (pending&0x10u)
        CON2 |= 1u;
    if (pending&0x20u)
        CON2 |= 2u;
    if (pending&0x40u)
        CON2 |= 4u;
    if (!(pending&PENDING_HALF))
        return 0;
    return buffer+((CON0 >> 15&1u)^1u)*HALF_WORDS;
}

int32_t *audio_half(unsigned which) {
    return buffer+which*HALF_WORDS;
}

void audio_filled(void) {
    sync_cpu();
    CON2 |= 0x08u;
}

void audio_stop(void) {
    CON0 &= (uint16_t)~0x800u;
    sync_cpu();
    CON2 = 0x0fu;
    pin_out(6, false);
}
