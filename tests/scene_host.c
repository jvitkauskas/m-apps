/* SPDX-License-Identifier: MIT */
/* Draws the launcher's screen on the computer (apps/picker/scene.c and qoi.c), for tests/scene_test.py and to see it:
 * scene_host OUT FRAMES [ICON.QOI...] writes FRAMES frames of 240 x 240 RGB565 big-endian to OUT, the selection
 * moving down a tile every 40 frames. The icons go to the first apps in turn. */
#include "qoi.h"
#include "scene.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void nothing(int x0, int y0, int x1, int y1) { (void)x0; (void)y0; (void)x1; (void)y1; }

int main(int argc, char **argv) {
    if (argc < 3)
        return 2;
    FILE *out = fopen(argv[1], "wb");
    unsigned frames = (unsigned)atoi(argv[2]);
    struct scene_tile apps[] = {
        {"MP3 Player", "Plays the MP3 files on the card", "1.0", 0, 0xe9a6, 'M'},
        {"USB Disk", "The card as a USB drive", "1.0", 0, 0x2c9f, 'U'},
        {"Slots", "Swap firmware and its settings from the card", "1.0", 0, 0x8c1f, 'S'},
        {"Tone", "Two quiet tones, and the real sample rate", "1.0", 0, 0xfc60, 'T'},
        {"HELLO", "", "", 0, 0x2d2b, 'H'},
        {"Back", "To the bootloader's menu", "", 0, 0x8c71, '<'},
    };
    unsigned count = sizeof apps/sizeof apps[0];
    static uint8_t icons[4][SCENE_ICON*SCENE_ICON*4];
    for (int i = 3; i < argc && i-3 < 4; ++i) {
        static uint8_t file[16384];
        memset(file, 0, sizeof file);
        FILE *f = fopen(argv[i], "rb");
        if (!f)
            return 2;
        size_t n = fread(file, 1, sizeof file, f);
        fclose(f);
        unsigned w, h;
        if (!qoi_decode(file, (uint32_t)n, icons[i-3], SCENE_ICON, &w, &h) || w != SCENE_ICON || h != SCENE_ICON)
            return 3;
        apps[i-3].icon = icons[i-3];
    }
    static uint8_t layer[SCENE_LAYER_BYTES];
    static uint16_t frame[SCENE_SIZE*SCENE_SIZE];
    scene_init(layer, 7);
    unsigned selection = 0, first = 0;
    for (unsigned f = 0; f < frames; ++f) {
        if (f == 0 || f%40 == 0) {
            if (f)
                selection = (selection+1)%count;
            first = selection < first ? selection:selection >= first+SCENE_TILES ? selection-(SCENE_TILES-1):first;
            scene_clear_tiles();
            for (unsigned s = 0; s < SCENE_TILES && first+s < count; ++s)
                scene_tile(s, &apps[first+s], first+s == selection);
            scene_more(first > 0, first+SCENE_TILES < count);
            char where[16];
            snprintf(where, sizeof where, "%u / %u", selection+1, count-1);
            scene_bar(selection+1 < count ? where:"", "< > MOVE   SEQ PLAY START", 0, 0);
        }
        if (f)
            scene_step(nothing);
        scene_render(0, 0, SCENE_SIZE, SCENE_SIZE, frame);
        fwrite(frame, 2, SCENE_SIZE*SCENE_SIZE, out);
    }
    /* and the panel for a card with nothing to start, last */
    scene_clear_tiles();
    scene_message("Nothing to start", "NO APPS IN /MBOOT/APPS");
    scene_bar("", "SEQ PLAY: BACK", 0, 0);
    scene_render(0, 0, SCENE_SIZE, SCENE_SIZE, frame);
    fwrite(frame, 2, SCENE_SIZE*SCENE_SIZE, out);
    fclose(out);
    return 0;
}
