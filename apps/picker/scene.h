/* SPDX-License-Identifier: MIT */
/* The launcher's screen: a light background, bubbles rising behind everything, and a layer of tiles and text over
 * them. Only the bubbles move, so a frame redraws just the rectangles they pass through. It draws into memory only, so
 * it also builds on a computer (tests/scene_host.c). */
#ifndef SCENE_H
#define SCENE_H
#include <stdbool.h>
#include <stdint.h>
#define SCENE_SIZE 240
/* The layer: RGB565 colour and alpha out of 16 for each pixel, 3 bytes of RAM a pixel in all. */
#define SCENE_LAYER_BYTES (SCENE_SIZE*SCENE_SIZE*3u)
/* Three tiles show at once, above the bar. */
#define SCENE_TILES 3u
#define SCENE_ICON 44u

/* What a tile shows. icon: SCENE_ICON x SCENE_ICON RGBA pixels, or 0 for a letter on a colour. */
struct scene_tile {
    const char *name, *about, *version;
    const uint8_t *icon;
    uint16_t colour;
    char letter;
};

/* layer: SCENE_LAYER_BYTES, 2-byte aligned. seed: where the bubbles start. */
void scene_init(uint8_t *layer, uint32_t seed);
/* The tiles' part of the layer, cleared. */
void scene_clear_tiles(void);
/* Tile 0..2 from the top, marked or not. */
void scene_tile(unsigned slot, const struct scene_tile *tile, bool selected);
/* The rows a tile covers, its shadow and glow included, so as to draw them again. */
void scene_tile_rows(unsigned slot, int *y0, int *y1);
/* Small marks at the top and the bottom of the list, when there's more that way. */
void scene_more(bool above, bool below);
/* A panel instead of the tiles, for a card that can't be read or has no apps. */
void scene_message(const char *title, const char *text);
/* The bar at the bottom: where the selection is (such as 3 / 7), and the keys, or a note in its colour instead. */
void scene_bar(const char *where, const char *keys, const char *note, uint16_t note_colour);
#define SCENE_BAR_Y 200
#define SCENE_RED 0xc8a4u
/* The pixels of x0..x1 by y0..y1 (exclusive), RGB565 big-endian, x1-x0 to a row. */
void scene_render(int x0, int y0, int x1, int y1, uint16_t *out);
/* The bubbles a step on. dirty is called with each rectangle that needs drawing again. */
void scene_step(void (*dirty)(int x0, int y0, int x1, int y1));
#endif
