/* SPDX-License-Identifier: MIT */
/* Written from the QOI specification, version 1.0 (qoiformat.org). */
#include "qoi.h"
#include <string.h>

bool qoi_decode(const uint8_t *data, uint32_t size, uint8_t *out, unsigned max, unsigned *width, unsigned *height) {
    if (size < 14+8 || memcmp(data, "qoif", 4))
        return false;
    uint32_t w = (uint32_t)data[4] << 24 | (uint32_t)data[5] << 16 | (uint32_t)data[6] << 8 | data[7];
    uint32_t h = (uint32_t)data[8] << 24 | (uint32_t)data[9] << 16 | (uint32_t)data[10] << 8 | data[11];
    if (!w || !h || w > max || h > max)
        return false;
    uint8_t index[64][4], px[4] = {0, 0, 0, 255};
    memset(index, 0, sizeof index);
    uint32_t at = 14, n = w*h, i = 0, end = size-8;
    while (i < n) {
        if (at >= end)
            return false;
        uint8_t b = data[at++];
        unsigned run = 1;
        if (b == 0xfe || b == 0xff) {
            unsigned k = b == 0xfe ? 3u:4u;
            if (at+k > end)
                return false;
            memcpy(px, data+at, k);
            at += k;
        } else if (b >> 6 == 0)
            memcpy(px, index[b], 4);
        else if (b >> 6 == 1) {
            px[0] = (uint8_t)(px[0]+(b >> 4&3)-2);
            px[1] = (uint8_t)(px[1]+(b >> 2&3)-2);
            px[2] = (uint8_t)(px[2]+(b&3)-2);
        } else if (b >> 6 == 2) {
            if (at >= end)
                return false;
            int dg = (b&63)-32;
            uint8_t d = data[at++];
            px[0] = (uint8_t)(px[0]+dg+(d >> 4)-8);
            px[1] = (uint8_t)(px[1]+dg);
            px[2] = (uint8_t)(px[2]+dg+(d&15)-8);
        } else
            run = (b&63)+1u;
        if (b >> 6 != 3 || b >= 0xfe)
            memcpy(index[(px[0]*3u+px[1]*5u+px[2]*7u+px[3]*11u)%64u], px, 4);
        for (; run && i < n; --run, ++i)
            memcpy(out+4*i, px, 4);
    }
    *width = w;
    *height = h;
    return true;
}
