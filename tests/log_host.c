/* SPDX-License-Identifier: MIT */
/* Runs kit/log.c on the computer against a plain buffer. tests/applog_test.py decodes what it writes. */
#include <stdio.h>
#include <stdint.h>
#include <string.h>
static _Alignas(4) uint8_t ram[0xd80];
#define APP_LOG_RAM ram
#define APP_LOG_SYNC() ((void)0)
static unsigned feeds;
#define APP_LOG_FEED() (++feeds)
#include "../kit/log.c"

static void save(const char *path) {
    FILE *f = fopen(path, "wb");
    if (!f || fwrite(ram, 1, sizeof ram, f) != sizeof ram || fclose(f))
        perror(path);
}

int main(int argc, char **argv) {
    (void)argc;
    static const uint8_t trailer[8] = {'A', 'P', 'P', '1', 0x12, 0x34, 1, 1};
    memset(ram, 0x5a, sizeof ram);
    app_log_begin(trailer);
    app_log("plain");
    app_log("%s %c %d %u %x %08x %5u %% %q", "text", 'c', -42, 42u, 0xabcu, 0xabcu, 7u);
    app_log("%d", INT32_MIN);
    save(argv[1]);
    app_log_end(-3);
    save(argv[2]);
    for (unsigned i = 0; i < 400; ++i)
        app_log("line %u", i);
    save(argv[3]);
    if (feeds != 403)
        return 1;
    app_log_crashed();
    save(argv[4]);
    return 0;
}
