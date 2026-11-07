/* SPDX-License-Identifier: MIT */
#include "load.h"
#include "core.h"
#include <string.h>
#define TRAILER 8u
#define SMALLEST 16u
#define LARGEST 0x4fd80u
#define FORMAT 1u
#define READ_ERROR "CARD READ ERROR"
#define BAD_FILE "NOT AN APP FILE"

const char *app_file_load(const struct fat_entry *file, void *to, unsigned board) {
    uint8_t trailer[TRAILER];
    if (file->size < SMALLEST)
        return BAD_FILE;
    if (!fat_read(file, file->size-TRAILER, trailer, TRAILER))
        return READ_ERROR;
    if (memcmp(trailer, "APP1", 4))
        return BAD_FILE;
    if (trailer[7] != FORMAT)
        return "NEWER APP FORMAT";
    if (trailer[6] && trailer[6] != board)
        return "OTHER BOARD";
    if (file->size > LARGEST)
        return "APP TOO BIG";
    if (!fat_read(file, 0, to, file->size))
        return READ_ERROR;
    if (core_crc16(0, to, file->size-TRAILER) != core_le16(trailer+4))
        return "APP FILE DAMAGED";
    return 0;
}
