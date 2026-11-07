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
/* The same with each file's name, as a computer shows it, in UTF-8: its long name if it has one, cut at FAT_NAME_BYTES-1
 * bytes between characters, and otherwise its short one, as NAME.EXT. They come in name order, ignoring ASCII case. What
 * to do with characters the screen doesn't have is the app's business. */
#define FAT_NAME_BYTES 72u
struct fat_named {
    struct fat_entry file;
    char name[FAT_NAME_BYTES];
};
const char *fat_list_named(uint32_t cluster, const char extension[3], struct fat_named *entries, unsigned max, unsigned *count,
                           bool *more);
/* Writes n bytes and no more. False if the card or the file's chain fails, or it's beyond the file's end. */
bool fat_read(const struct fat_entry *file, uint32_t offset, void *to, uint32_t n);

/* Changing the volume. These keep to an order in which losing power at any write leaves, at worst, clusters no file
 * uses, which fsck can reclaim. A file a directory names is complete, either as it was or as it now is:
 * - a new file: fat_reserve(), fat_write(), fat_add();
 * - a file replaced: fat_reserve(), fat_write(), fat_swap(), which changes one directory sector, then frees the old one;
 * - a file removed: fat_remove() takes it out of the directory, then frees its clusters.
 * Call fat_commit() at the end, which updates the volume's count of free clusters. */
#define FAT_ATTR_ARCHIVE 0x20u
#define FAT_ATTR_FOLDER 0x10u
/* A chain of enough clusters for size bytes, none of them written yet. If there isn't room, nothing changes. */
const char *fat_reserve(uint32_t size, uint32_t *start);
/* Whole sectors only, within a chain from fat_reserve(). */
bool fat_write(uint32_t start, uint32_t offset, const void *from, uint32_t n);
/* Frees a chain no directory names. */
const char *fat_release(uint32_t start);
/* Adds a directory entry: an 8.3 name, padded with spaces to 11 bytes, that the directory doesn't hold yet. */
const char *fat_add(uint32_t directory, const char name[11], unsigned attributes, uint32_t start, uint32_t size);
/* Points a directory entry at another chain, and frees the old one. */
const char *fat_swap(uint32_t directory, const char name[11], uint32_t start, uint32_t size);
const char *fat_remove(uint32_t directory, const char name[11]);
/* Finds or makes a folder. */
const char *fat_make_folder(uint32_t *cluster, const char name[11]);
/* Whether a directory holds this name, with what it holds if so. */
const char *fat_find(uint32_t directory, const char name[11], struct fat_entry *entry, bool *found);
const char *fat_commit(void);
#endif
