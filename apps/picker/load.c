/* SPDX-License-Identifier: MIT */
#include "load.h"
#include "core.h"
#include <string.h>
#define TRAILER 8u
#define SMALLEST 16u
/* §11.8 allows 0x4fd80, but this picker logs through the load, and the log starts here (kit/log.h). */
#define LARGEST 0x4f000u
#define FORMAT 1u
#define READ_ERROR "CARD READ ERROR"
#define BAD_FILE "NOT AN APP FILE"

const char *app_file_load(const struct fat_entry *file, void *to, unsigned board) {
    uint8_t trailer[TRAILER];
    if (file->size < SMALLEST)
        return BAD_FILE;
    /* First, as this also limits how far a damaged chain gets walked. */
    if (file->size > LARGEST)
        return "APP TOO BIG";
    if (!fat_read(file, file->size-TRAILER, trailer, TRAILER))
        return READ_ERROR;
    if (memcmp(trailer, "APP1", 4))
        return BAD_FILE;
    if (trailer[7] != FORMAT)
        return "NEWER APP FORMAT";
    if (trailer[6] && trailer[6] != board)
        return "OTHER BOARD";
    if (!fat_read(file, 0, to, file->size))
        return READ_ERROR;
    if (core_crc16(0, to, file->size-TRAILER) != core_le16(trailer+4))
        return "APP FILE DAMAGED";
    return 0;
}
