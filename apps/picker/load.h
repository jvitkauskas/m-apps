/* SPDX-License-Identifier: MIT */
#ifndef LOAD_H
#define LOAD_H
#include "fat.h"
#define APP_ADDRESS 0x01c30000u
/* Checks an app file (m-bootloader's docs/SPEC.md §11.8) and loads it. Returns 0, or what to show. */
const char *app_file_load(const struct fat_entry *file, void *to, unsigned board);
#endif
