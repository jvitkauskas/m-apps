/* SPDX-License-Identifier: MIT */
#include "catalog.h"
#include <string.h>
void app_feed(void);

/* the 8.3 name, without its padding, as the name to show */
static void name_from(char *to, const char id[8]) {
    unsigned n = 0;
    while (n < 8 && id[n] != ' ')
        ++n;
    memcpy(to, id, n);
    to[n] = 0;
}

static int compare(const char *a, const char *b) {
    for (;; ++a, ++b) {
        char x = *a >= 'a' && *a <= 'z' ? (char)(*a-32):*a, y = *b >= 'a' && *b <= 'z' ? (char)(*b-32):*b;
        if (x != y || !x)
            return (unsigned char)x < (unsigned char)y ? -1:(unsigned char)x > (unsigned char)y;
    }
}

const char *catalog_read(uint32_t folder, struct catalog *c) {
    c->count = 0;
    unsigned n;
    /* A folder without BOOT.APP isn't an app's. */
    const char *error = fat_list_folders(folder, c->folders, CATALOG_MOST, &n, &c->more);
    if (error)
        return error;
    for (unsigned i = 0; i < n; ++i) {
        struct catalog_app *a = &c->apps[c->count];
        memset(a, 0, sizeof *a);
        bool found;
        if (fat_find(c->folders[i].start, "BOOT    APP", &a->file, &found) || !found)
            continue;
        a->folder = c->folders[i].start;
        memcpy(a->id, c->folders[i].name, 8);
        if (fat_find(a->folder, "ICON    QOI", &a->icon, &found) || !found)
            a->icon.size = 0;
        a->launched_entry = app_file_launched_entry(&a->file);
        name_from(a->text.name, a->id);
        struct fat_entry meta;
        static char text[APP_META_MAX];
        if (!fat_find(a->folder, "META    TXT", &meta, &found) && found) {
            uint32_t size = meta.size < APP_META_MAX ? meta.size:APP_META_MAX;
            if (fat_read(&meta, 0, text, size))
                app_meta_parse(text, size, &a->text);
        }
        ++c->count;
        app_feed();
    }
    /* By the name shown, the way a person would look for it. */
    for (unsigned i = 1; i < c->count; ++i)
        for (unsigned j = i; j && compare(c->apps[j-1].text.name, c->apps[j].text.name) > 0; --j) {
            struct catalog_app t = c->apps[j];
            c->apps[j] = c->apps[j-1];
            c->apps[j-1] = t;
        }
    return c->count ? 0:"NO APPS IN /MBOOT/APPS";
}
