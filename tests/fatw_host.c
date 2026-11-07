/* SPDX-License-Identifier: MIT */
/* Runs kit/fat.c's writing on the computer, on a card image. See tests/fat_write_test.py. */
#include "fat.h"
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
static uint8_t *image;
static uint32_t sectors;
static long writes, cut_after = -1;

void app_feed(void) {
}

bool card_read(uint32_t sector, uint8_t *data) {
    if (sector >= sectors) {
        fprintf(stderr, "read beyond the card: sector %u\n", sector);
        abort();
    }
    memcpy(data, image+sector*512ull, 512);
    return true;
}

bool card_write(uint32_t sector, const uint8_t *data) {
    if (sector >= sectors) {
        fprintf(stderr, "write beyond the card: sector %u\n", sector);
        abort();
    }
    /* Power goes before this write. */
    if (writes++ == cut_after)
        _exit(42);
    memcpy(image+sector*512ull, data, 512);
    return true;
}

static void pad(char out[11], const char *name) {
    memset(out, ' ', 11);
    const char *dot = strchr(name, '.');
    size_t base = dot ? (size_t)(dot-name):strlen(name);
    memcpy(out, name, base);
    if (dot)
        memcpy(out+8, dot+1, strlen(dot+1));
}

static void check(const char *error) {
    if (error) {
        printf("error %s\n", error);
        exit(3);
    }
}

/* Follows a path, making its folders if asked. */
static uint32_t folder(const char *path, bool make) {
    uint32_t c = fat_root();
    char copy[64], name[11];
    strncpy(copy, path, sizeof(copy)-1);
    copy[sizeof(copy)-1] = 0;
    for (char *part = strtok(copy, "/"); part; part = strtok(0, "/")) {
        pad(name, part);
        check(make ? fat_make_folder(&c, name):fat_folder(&c, name, "NO FOLDER"));
    }
    return c;
}

/* A chain holding a file's bytes, padded with zeros to whole sectors. */
static uint32_t write_data(const char *path, uint32_t *size) {
    FILE *in = fopen(path, "rb");
    if (!in)
        exit(2);
    fseek(in, 0, SEEK_END);
    *size = (uint32_t)ftell(in);
    rewind(in);
    uint32_t rounded = (*size+511)/512*512;
    uint8_t *data = calloc(rounded ? rounded:1, 1);
    if (fread(data, 1, *size, in) != *size)
        exit(2);
    fclose(in);
    uint32_t start = 0;
    check(fat_reserve(*size, &start));
    if (rounded && !fat_write(start, 0, data, rounded))
        check("write failed");
    free(data);
    return start;
}

/* image, cut after this many writes or -1, then: mkdir PATH | new PATH NAME FILE | swap PATH NAME FILE | remove PATH NAME
 * | reserve BYTES */
int main(int argc, char **argv) {
    if (argc < 4)
        return 2;
    int fd = open(argv[1], O_RDWR);
    struct stat st;
    if (fd < 0 || fstat(fd, &st))
        return 2;
    image = mmap(0, (size_t)st.st_size, PROT_READ|PROT_WRITE, MAP_SHARED, fd, 0);
    if (image == MAP_FAILED)
        return 2;
    sectors = (uint32_t)(st.st_size/512);
    cut_after = atol(argv[2]);
    check(fat_mount(sectors));
    const char *op = argv[3];
    char name[11];
    if (!strcmp(op, "mkdir"))
        folder(argv[4], true);
    else if (!strcmp(op, "new")) {
        uint32_t size, dir = folder(argv[4], false);
        pad(name, argv[5]);
        uint32_t start = write_data(argv[6], &size);
        check(fat_add(dir, name, FAT_ATTR_ARCHIVE, start, size));
    } else if (!strcmp(op, "swap")) {
        uint32_t size, dir = folder(argv[4], false);
        pad(name, argv[5]);
        uint32_t start = write_data(argv[6], &size);
        check(fat_swap(dir, name, start, size));
    } else if (!strcmp(op, "remove")) {
        pad(name, argv[5]);
        check(fat_remove(folder(argv[4], false), name));
    } else if (!strcmp(op, "reserve")) {
        uint32_t start = 0;
        check(fat_reserve((uint32_t)atol(argv[4]), &start));
    } else
        return 2;
    check(fat_commit());
    printf("ok %ld\n", writes);
    return 0;
}
