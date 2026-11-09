/* SPDX-License-Identifier: MIT */
#ifndef LOAD_H
#define LOAD_H
#include "fat.h"
#define APP_ADDRESS 0x01c30000u
/* A file that's loaded whole and run: its trailer's magic, the largest it can be, and what to show when it's wrong. */
struct image_kind {
    char magic[4];
    uint32_t largest;
    const char *not_one, *too_big, *damaged;
};
/* An app (m-bootloader's docs/SPEC.md §11.8), and the card launcher's LAUNCH.BIN (apps/menu). */
extern const struct image_kind app_kind, launcher_kind;
/* Checks the file's trailer, loads it to `to` and checks its CRC. Returns 0, or what to show. */
const char *image_load(const struct fat_entry *file, void *to, const struct image_kind *kind, unsigned board);
/* An app file, checked and loaded. */
const char *app_file_load(const struct fat_entry *file, void *to, unsigned board);
/* What the card's launcher shows for an app, as text the screen can show: each string ends in a zero, and anything
 * outside printable ASCII is a '?'. */
struct app_text {
    char name[25], about[61], version[13];
};
/* Where the launcher calls the app with its folder: the kit's header (kit/app_launch.h), if the file has one and the
 * address is inside the app as loaded at APP_ADDRESS, and otherwise 0. */
uint32_t app_file_launched_entry(const struct fat_entry *file);
/* A folder's META.TXT, n bytes of it: lines of key=value, with name, about and version, as UTF-8. Each one there
 * replaces text's; other keys, and lines that aren't key=value, are skipped. Returns how many it took. */
#define APP_META_MAX 512u
unsigned app_meta_parse(const char *meta, uint32_t n, struct app_text *text);
#endif
