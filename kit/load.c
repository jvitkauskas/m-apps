/* SPDX-License-Identifier: MIT */
#include "load.h"
#include "app_launch.h"
#include "core.h"
#include <stddef.h>
#include <string.h>
#define TRAILER 8u
#define SMALLEST 16u
#define FORMAT 1u
#define READ_ERROR "CARD READ ERROR"

/* §11.8 allows 0x4fd80, but the launcher logs through the load, and the log starts here (kit/log.h). */
const struct image_kind app_kind = {{'A', 'P', 'P', '1'}, 0x4f000u, "NOT AN APP FILE", "APP TOO BIG", "APP FILE DAMAGED"};
/* Linked at 0x01C0A800, up to 0x01C16800 (apps/menu). */
const struct image_kind launcher_kind = {{'L', 'N', 'C', '1'}, 0xc000u, "NOT A LAUNCHER FILE", "LAUNCHER TOO BIG",
                                         "LAUNCHER FILE DAMAGED"};

const char *image_load(const struct fat_entry *file, void *to, const struct image_kind *kind, unsigned board) {
    uint8_t trailer[TRAILER];
    if (file->size < SMALLEST)
        return kind->not_one;
    /* First, as this also limits how far a damaged chain gets walked. */
    if (file->size > kind->largest)
        return kind->too_big;
    if (!fat_read(file, file->size-TRAILER, trailer, TRAILER))
        return READ_ERROR;
    if (memcmp(trailer, kind->magic, 4))
        return kind->not_one;
    if (trailer[7] != FORMAT)
        return "NEWER FILE FORMAT";
    if (trailer[6] && trailer[6] != board)
        return "OTHER BOARD";
    if (!fat_read(file, 0, to, file->size))
        return READ_ERROR;
    if (core_crc16(0, to, file->size-TRAILER) != core_le16(trailer+4))
        return kind->damaged;
    return 0;
}

const char *app_file_load(const struct fat_entry *file, void *to, unsigned board) {
    return image_load(file, to, &app_kind, board);
}

uint32_t app_file_launched_entry(const struct fat_entry *file) {
    struct app_header header;
    /* The header, then at least the trailer after it. */
    if (file->size < 4+sizeof header+TRAILER || file->size > app_kind.largest || !fat_read(file, 4, &header, sizeof header))
        return 0;
    if (memcmp(header.magic, "MAPP", 4) || header.format != APP_HEADER_FORMAT)
        return 0;
    uint32_t entry = header.launched_entry;
    /* an instruction's address: inside the image and on a word */
    return !(entry&3u) && entry >= APP_ADDRESS && entry < APP_ADDRESS+file->size-TRAILER ? entry:0;
}

/* value into field, up to the line's end: a character beyond ASCII as one '?' */
static void copy_value(char *to, unsigned room, const char *from, const char *end) {
    unsigned n = 0;
    while (from < end && n+1 < room) {
        uint8_t c = (uint8_t)*from++;
        if (c >= 0x80)
            while (from < end && ((uint8_t)*from&0xc0u) == 0x80u)
                ++from;
        to[n++] = c >= ' ' && c <= '~' ? (char)c:'?';
    }
    while (n && to[n-1] == ' ')
        --n;
    to[n] = 0;
}

unsigned app_meta_parse(const char *meta, uint32_t n, struct app_text *text) {
    static const struct { const char *key; unsigned offset, room; } keys[] = {
        {"name", offsetof(struct app_text, name), sizeof text->name},
        {"about", offsetof(struct app_text, about), sizeof text->about},
        {"version", offsetof(struct app_text, version), sizeof text->version},
    };
    const char *end = meta+(n > APP_META_MAX ? APP_META_MAX:n);
    unsigned taken = 0;
    while (meta < end) {
        const char *line_end = meta;
        while (line_end < end && *line_end != '\n' && *line_end != '\r')
            ++line_end;
        const char *eq = meta;
        while (eq < line_end && *eq != '=')
            ++eq;
        if (eq < line_end) {
            const char *k = meta, *k_end = eq, *v = eq+1;
            while (k < k_end && *k == ' ')
                ++k;
            while (k_end > k && k_end[-1] == ' ')
                --k_end;
            while (v < line_end && *v == ' ')
                ++v;
            for (unsigned i = 0; i < sizeof keys/sizeof keys[0]; ++i) {
                size_t len = strlen(keys[i].key);
                if ((size_t)(k_end-k) == len && !memcmp(k, keys[i].key, len)) {
                    char value[64];
                    copy_value(value, keys[i].room, v, line_end);
                    /* an empty value leaves what was there */
                    if (value[0]) {
                        memcpy((char *)text+keys[i].offset, value, strlen(value)+1);
                        ++taken;
                    }
                }
            }
        }
        meta = line_end;
        while (meta < end && (*meta == '\n' || *meta == '\r'))
            ++meta;
    }
    return taken;
}
