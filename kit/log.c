/* SPDX-License-Identifier: MIT */
/* The app's log: a ring in RAM that a host reads after the app has ended, on any route. See README. */
#include "log.h"
#include <stdarg.h>
#include <stdbool.h>
#ifndef APP_LOG_RAM
#define APP_LOG_RAM ((uint8_t *)(uintptr_t)APP_LOG_ADDRESS)
#define APP_LOG_SYNC() __asm__ volatile("csync" ::: "memory")
void app_feed(void);
#define APP_LOG_FEED() app_feed()
#endif
#define MAGIC 0x474f4c41u /* "ALOG" */
#define FORMAT 1u
#define STATE_RUNNING 1u
#define STATE_RETURNED 2u
#define STATE_CRASHED 3u

static volatile uint32_t *word(unsigned offset) {
    return (volatile uint32_t *)(void *)(APP_LOG_RAM+offset);
}

static void put(char c) {
    uint32_t total = *word(16);
    ((volatile uint8_t *)APP_LOG_RAM)[APP_LOG_HEADER+total%APP_LOG_CAPACITY] = (uint8_t)c;
    *word(16) = total+1;
}

static void put_text(const char *text) {
    while (*text)
        put(*text++);
}

static void put_number(uint32_t value, unsigned base, unsigned width, char pad) {
    char digits[11];
    unsigned n = 0;
    do {
        digits[n++] = "0123456789abcdef"[value%base];
        value /= base;
    } while (value);
    for (; width > n; --width)
        put(pad);
    while (n)
        put(digits[--n]);
}

void app_log(const char *format, ...) {
    va_list args;
    va_start(args, format);
    for (const char *p = format; *p; ++p) {
        if (*p != '%') {
            put(*p);
            continue;
        }
        char pad = ' ';
        unsigned width = 0;
        if (*++p == '0')
            pad = *p++;
        for (; *p >= '0' && *p <= '9'; ++p)
            width = width*10+(unsigned)(*p-'0');
        switch (*p) {
        case 's': put_text(va_arg(args, const char *)); break;
        case 'c': put((char)va_arg(args, int)); break;
        case 'u': put_number(va_arg(args, unsigned), 10, width, pad); break;
        case 'x': put_number(va_arg(args, unsigned), 16, width, pad); break;
        case 'd': {
            int value = va_arg(args, int);
            if (value < 0)
                put('-');
            put_number(value < 0 ? 0u-(unsigned)value:(unsigned)value, 10, width, pad);
            break;
        }
        case '%': put('%'); break;
        case 0: --p; break;
        default: put('%'); put(*p); break;
        }
    }
    va_end(args);
    put('\n');
    APP_LOG_SYNC();
    APP_LOG_FEED();
}

void app_log_begin(const uint8_t trailer[8]) {
    *word(0) = 0;
    APP_LOG_SYNC();
    *word(4) = FORMAT;
    for (unsigned i = 0; i < 8; ++i)
        ((volatile uint8_t *)APP_LOG_RAM)[8+i] = trailer[i];
    *word(16) = 0;
    *word(20) = STATE_RUNNING;
    *word(24) = 0;
    *word(28) = 0;
    *word(0) = MAGIC;
    APP_LOG_SYNC();
}

void app_log_end(int result) {
    *word(24) = (uint32_t)result;
    *word(20) = STATE_RETURNED;
    APP_LOG_SYNC();
}

void app_log_crashed(void) {
    *word(20) = STATE_CRASHED;
    APP_LOG_SYNC();
}
