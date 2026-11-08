/* SPDX-License-Identifier: MIT */
#ifndef QOI_H
#define QOI_H
#include <stdbool.h>
#include <stdint.h>
/* A QOI image (qoiformat.org) of at most max x max pixels, decoded to RGBA, 4 bytes a pixel, row by row, into out. False
 * if it isn't one, is bigger, or ends early. */
bool qoi_decode(const uint8_t *data, uint32_t size, uint8_t *out, unsigned max, unsigned *width, unsigned *height);
#endif
