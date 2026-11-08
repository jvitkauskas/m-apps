/* SPDX-License-Identifier: MIT */
#ifndef APP_LAUNCH_H
#define APP_LAUNCH_H
#include <stdint.h>
/* At offset 4 of every app the kit builds, after the entry's jump (kit/start.S): the second way in, which the card's
 * launcher (apps/picker) calls with a struct app_launch instead of starting at the first byte. */
struct app_header {
    char magic[4];             /* MAPP */
    uint8_t format, reserved[3];
    uint32_t launched_entry;
};
#define APP_HEADER_FORMAT 1u

/* What the launcher tells an app it starts from its folder, /MBOOT/APPS/<folder>/BOOT.APP: the folder, so it can read
 * its own files there (kit/app.h's app_folder()). The cluster is the card's that was in when it started. */
struct app_launch {
    char magic[4];             /* LNCH */
    uint8_t format, reserved[3];
    uint32_t folder;           /* the folder's first cluster */
    char folder_name[8];       /* its 8.3 name, padded with spaces */
};
#define APP_LAUNCH_FORMAT 1u
#endif
