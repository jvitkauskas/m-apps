/* SPDX-License-Identifier: MIT */
#ifndef CATALOG_H
#define CATALOG_H
#include "fat.h"
#include "load.h"
#define CATALOG_MOST 100u

/* An app on the card: a folder, /MBOOT/APPS/<ID>/, with BOOT.APP, and ICON.QOI and META.TXT if it likes. */
struct catalog_app {
    struct fat_entry file;          /* BOOT.APP */
    struct fat_entry icon;          /* ICON.QOI; size 0 without one */
    uint32_t folder;                /* the folder's cluster */
    char id[8];                     /* the folder's 8.3 name */
    struct app_text text;           /* META.TXT, or the folder's name */
    uint32_t launched_entry;        /* where to call it with its folder, or 0 (kit/app_launch.h) */
};

struct catalog {
    struct catalog_app apps[CATALOG_MOST];
    unsigned count;
    bool more;                      /* there were more than CATALOG_MOST */
    struct fat_entry folders[CATALOG_MOST];
};

/* The apps in folder (/MBOOT/APPS, on a mounted card), by name as shown, ignoring case. Returns 0, or what to show. */
const char *catalog_read(uint32_t folder, struct catalog *c);
#endif
