/* SPDX-License-Identifier: MIT */
#ifndef APP_LOG_H
#define APP_LOG_H
#include <stdint.h>
/* Below the retained area, and outside the app's image, so every route and any reset leave it alone.
 * tools/applog.py reads the same layout: a 32-byte header, then the ring. */
#define APP_LOG_ADDRESS 0x01c7f000u
#define APP_LOG_SIZE 0xd80u
#define APP_LOG_HEADER 32u
#define APP_LOG_CAPACITY (APP_LOG_SIZE-APP_LOG_HEADER)
/* Adds one line. Knows %s %c %d %u %x and %%, with widths such as %08x. */
void app_log(const char *format, ...) __attribute__((format(printf, 1, 2)));
/* For kit/app.c. */
void app_log_begin(const uint8_t trailer[8]);
void app_log_end(int result);
void app_log_crashed(void);
#endif
