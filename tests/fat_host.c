/* SPDX-License-Identifier: MIT */
/* Runs apps/picker's FAT and loader code on the computer, on a card image. See tests/picker_test.py. */
#include "fat.h"
#include "load.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define GUARD 64u
#define BUFFER (0x4fd80u+GUARD)
static uint8_t *image;
static uint32_t sectors, fail_sector = 0xffffffffu;

void app_feed(void) {
}

bool card_read(uint32_t sector, uint8_t *data) {
    if (sector >= sectors) {
        fprintf(stderr, "read beyond the card: sector %u\n", sector);
        abort();
    }
    if (sector == fail_sector)
        return false;
    memcpy(data, image+sector*512ull, 512);
    return true;
}

/* image, board, name to load or -, out file for what was loaded, sector that fails or -. */
int main(int argc, char **argv) {
    if (argc != 6)
        return 2;
    FILE *in = fopen(argv[1], "rb");
    if (!in)
        return 2;
    fseek(in, 0, SEEK_END);
    long bytes = ftell(in);
    rewind(in);
    image = malloc((size_t)bytes);
    if (fread(image, 1, (size_t)bytes, in) != (size_t)bytes)
        return 2;
    fclose(in);
    sectors = (uint32_t)(bytes/512);
    if (strcmp(argv[5], "-"))
        fail_sector = (uint32_t)strtoul(argv[5], 0, 0);
    static struct fat_entry apps[100];
    unsigned count = 0;
    bool more = false;
    const char *error = fat_mount(sectors);
    uint32_t folder = 0;
    if (!error) {
        folder = fat_root();
        error = fat_folder(&folder, "MBOOT      ", "NO /MBOOT/APPS");
    }
    if (!error)
        error = fat_folder(&folder, "APPS       ", "NO /MBOOT/APPS");
    if (!error)
        error = fat_list(folder, "APP", apps, 100, &count, &more);
    printf("list %s %u %u\n", error ? error:"ok", count, more);
    for (unsigned i = 0; i < count; ++i)
        printf("app %.8s %u\n", apps[i].name, apps[i].size);
    if (strcmp(argv[3], "-"))
        for (unsigned i = 0; i < count; ++i)
            if (!memcmp(apps[i].name, argv[3], strlen(argv[3]))) {
                uint8_t *to = malloc(BUFFER);
                if (!to)
                    return 2;
                memset(to, 0xa5, BUFFER);
                const char *result = app_file_load(&apps[i], to, (unsigned)atoi(argv[2]));
                printf("load %s\n", result ? result:"ok");
                FILE *out = fopen(argv[4], "wb");
                if (!out)
                    return 2;
                fwrite(to, 1, BUFFER, out);
                fclose(out);
                free(to);
            }
    free(image);
    return 0;
}
