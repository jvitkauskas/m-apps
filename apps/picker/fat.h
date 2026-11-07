/* SPDX-License-Identifier: MIT */
#ifndef FAT_H
#define FAT_H
#include <stdbool.h>
#include <stdint.h>
/* Reads a FAT32 volume on the card, in the first partition or with no partition table. Short 8.3 names only. What's on
 * the card can't be trusted, so everything here is bounded, and a loop in the FAT ends with an error. A function
 * returning text returns 0 on success, and otherwise what to show. */
struct fat_entry {
    char name[8];
    uint32_t start, size;
};
/* The card's size in sectors, from card_start(). */
const char *fat_mount(uint32_t sectors);
/* The root directory's cluster. */
uint32_t fat_root(void);
/* Finds a folder in a directory: its 8.3 name, padded with spaces to 11 bytes. Returns what to show if it's missing. */
const char *fat_folder(uint32_t *cluster, const char name[11], const char *missing);
/* The files with this extension, in name order. With more than max, it keeps the first max and sets *more. */
const char *fat_list(uint32_t cluster, const char extension[3], struct fat_entry *entries, unsigned max, unsigned *count,
                     bool *more);
/* Writes n bytes and no more. False if the card or the file's chain fails, or it's beyond the file's end. */
bool fat_read(const struct fat_entry *file, uint32_t offset, void *to, uint32_t n);
#endif
