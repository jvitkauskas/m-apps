/* SPDX-License-Identifier: MIT */
#ifndef MSC_H
#define MSC_H
#include <stdbool.h>
#include <stdint.h>
/* A USB mass storage device on USB0, bulk-only, polled, serving the card one sector at a time. A write has reached the
 * card before the host is told it succeeded. */
struct msc_stats {
    uint32_t sectors_read, sectors_written, errors;
};
void msc_start(uint32_t sectors);
void msc_poll(void);
bool msc_configured(void);
/* The host ejected the disk. */
bool msc_ejected(void);
void msc_stop(void);
const struct msc_stats *msc_stats(void);
#endif
