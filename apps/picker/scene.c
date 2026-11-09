/* SPDX-License-Identifier: MIT */
#include "scene.h"
#include "font.h"
#include <string.h>
#define N SCENE_SIZE
/* r, g, b out of 255 to RGB565 */
#define RGB(r, g, b) ((uint16_t)(((r)&0xf8u) << 8 | ((g)&0xfcu) << 3 | (b) >> 3))
#define INK RGB(40, 52, 66)
#define DIM RGB(108, 120, 134)
#define FAINT RGB(150, 160, 172)
#define LINE RGB(196, 206, 218)
#define BLUE RGB(56, 136, 228)
#define WORDMARK RGB(70, 140, 215)
#define BUBBLES 12u
#define TILE_X 8
#define TILE_W 224
#define TILE_H 56
#define TILE_Y0 10
#define TILE_STEP 62
#define TILE_R 12

static uint16_t *lc;                    /* the layer's colour */
static uint8_t *la;                     /* and its alpha, 0..16 */

/* ---- colour ---- */

static uint16_t mix(uint16_t under, uint16_t over, unsigned a, unsigned of) {
    unsigned r = under >> 11, g = (under >> 5)&63u, b = under&31u;
    unsigned R = over >> 11, G = (over >> 5)&63u, B = over&31u;
    r = (r*(of-a)+R*a+of/2)/of;
    g = (g*(of-a)+G*a+of/2)/of;
    b = (b*(of-a)+B*a+of/2)/of;
    return (uint16_t)(r << 11 | g << 5 | b);
}

/* colour c at alpha a (0..16) over the layer at i */
static void over(unsigned i, uint16_t c, unsigned a) {
    if (!a)
        return;
    if (a > 16)
        a = 16;
    unsigned under = la[i];
    if (!under || a == 16) {
        lc[i] = c;
        la[i] = (uint8_t)a;
        return;
    }
    /* straight alpha: the colour weighted by what shows of each */
    unsigned out = a+under*(16u-a)/16u;
    lc[i] = mix(lc[i], c, a*16u/out, 16);
    la[i] = (uint8_t)out;
}

/* ---- shapes, by signed distance in 1/16 pixel ---- */

static unsigned isqrt(uint32_t v) {
    uint32_t r = 0, bit = 1u << 30;
    while (bit > v)
        bit >>= 2;
    while (bit) {
        if (v >= r+bit) {
            v -= r+bit;
            r = (r >> 1)+bit;
        } else
            r >>= 1;
        bit >>= 2;
    }
    return r;
}

/* the distance from pixel px, py's centre to a rounded rectangle's edge, negative inside */
static int distance(int px, int py, int x, int y, int w, int h, int r) {
    int qx = px*16+8-(x*16+w*8), qy = py*16+8-(y*16+h*8);
    qx = (qx < 0 ? -qx:qx)-(w*8-r*16);
    qy = (qy < 0 ? -qy:qy)-(h*8-r*16);
    if (qx <= 0 || qy <= 0)                     /* beside a straight edge, or inside */
        return (qx > qy ? qx:qy)-r*16;
    return (int)isqrt((uint32_t)(qx*qx+qy*qy))-r*16;
}

/* how much of a pixel at distance d is covered, out of 16, with an edge soft pixels wide */
static unsigned cover(int d, int soft) {
    int f = soft*16, a = (f/2-d)*16/f;
    return a < 0 ? 0u:a > 16 ? 16u:(unsigned)a;
}

/* a rounded rectangle in colour c at alpha a, its edge soft pixels wide; ring > 0 draws only a band that wide */
static void shape(int x, int y, int w, int h, int r, uint16_t c, unsigned a, int soft, int ring) {
    int m = soft/2+1;
    for (int py = y-m; py < y+h+m; ++py) {
        if ((unsigned)py >= N)
            continue;
        for (int px = x-m; px < x+w+m; ++px) {
            if ((unsigned)px >= N)
                continue;
            int d = distance(px, py, x, y, w, h, r);
            unsigned k = cover(d, soft);
            if (ring && k)
                k = k*(16u-cover(d+ring*16, soft))/16u;
            over((unsigned)(py*N+px), c, a*k/16u);
        }
    }
}

/* ---- text ---- */

static unsigned glyph(const font_t *f, unsigned ch) {
    if (ch < f->first || ch > f->last)
        ch = '?';
    return ch-f->first;
}

/* the width of s at scale/8 */
static int text_w(const font_t *f, const char *s, int scale) {
    int w = 0;
    for (; *s; ++s)
        w += f->adv[glyph(f, (uint8_t)*s)];
    return w*scale/8;
}

/* a glyph's alpha, 0..15, at gx, gy of its lit box, and 0 outside it */
static unsigned lit(const uint8_t *rows, int w, int h, int gx, int gy) {
    if (gx < 0 || gy < 0 || gx >= w || gy >= h)
        return 0;
    unsigned a = rows[gy*((w+1)/2)+gx/2];
    return gx&1 ? a&15u:a >> 4;
}

/* s with the pen at x and the face's top at y, scale/8 times its size, smoothed when scaled: the pen after it */
static int text(int x, int y, const font_t *f, const char *s, uint16_t c, int scale) {
    int pen = x*8;
    for (; *s; ++s) {
        unsigned gi = glyph(f, (uint8_t)*s);
        const uint8_t *gd = f->data+f->off[gi];
        int w = f->bw[gi], h = gd[2];
        int bx = (pen+(gd[0]-f->pad)*scale)/8, by = y+gd[1]*scale/8;
        int ow = (w*scale+7)/8+1, oh = (h*scale+7)/8+1;
        for (int oy = 0; oy < oh; ++oy)
            for (int ox = 0; ox < ow; ++ox) {
                unsigned a;
                if (scale == 8)
                    a = lit(gd+3, w, h, ox, oy);
                else {
                    /* where the pixel's centre falls in the glyph, in 1/16, and the four around it */
                    int sx = ((2*ox+1)*64/scale-8), sy = ((2*oy+1)*64/scale-8);
                    int gx = sx >> 4, gy = sy >> 4, fx = sx&15, fy = sy&15;
                    if (sx < 0)
                        gx = -1, fx = sx+16;
                    if (sy < 0)
                        gy = -1, fy = sy+16;
                    unsigned top = lit(gd+3, w, h, gx, gy)*(16u-(unsigned)fx)+lit(gd+3, w, h, gx+1, gy)*(unsigned)fx;
                    unsigned bottom = lit(gd+3, w, h, gx, gy+1)*(16u-(unsigned)fx)+lit(gd+3, w, h, gx+1, gy+1)*(unsigned)fx;
                    a = (top*(16u-(unsigned)fy)+bottom*(unsigned)fy) >> 8;
                }
                int px = bx+ox, py = by+oy;
                if (a && (unsigned)px < N && (unsigned)py < N)
                    over((unsigned)(py*N+px), c, (a*17u+8u) >> 4);
            }
        pen += f->adv[gi]*scale;
    }
    return pen/8;
}

/* s cut to fit w, ending in "..." */
static const char *fit(const font_t *f, const char *s, int w) {
    static char out[64];
    unsigned n = 0;
    while (s[n] && n < sizeof out-4)
        ++n;
    memcpy(out, s, n);
    out[n] = 0;
    if (text_w(f, out, 8) <= w)
        return out;
    while (n && text_w(f, out, 8)+text_w(f, "...", 8) > w)
        out[--n] = 0;
    while (n && out[n-1] == ' ')
        out[--n] = 0;
    memcpy(out+n, "...", 4);
    return out;
}

/* ---- the background ---- */

static const uint8_t bayer[16] = {0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5};

/* near white at the top, a cool pale blue-grey at the bottom, dithered */
static uint16_t back(int x, int y) {
    unsigned d = bayer[(y&3)*4+(x&3)];
    unsigned r = 246u-(unsigned)y*22u/N+d/2, g = 248u-(unsigned)y*16u/N+d/4, b = 252u-(unsigned)y*6u/N+d/2;
    return (uint16_t)((r > 255 ? 255:r) >> 3 << 11 | (g > 255 ? 255:g) >> 2 << 5 | (b > 255 ? 255:b) >> 3);
}

/* ---- bubbles ---- */

struct bubble { int x, y, r, speed, phase, ox, oy; };
static struct bubble bubbles[BUBBLES];
static uint32_t seed;

static unsigned rnd(unsigned n) {
    seed = seed*1103515245u+12345u;
    return (seed >> 16)%n;
}

/* x and y in 1/16 pixel */
static void spawn(struct bubble *b, bool anywhere) {
    b->r = 5+(int)rnd(20);
    b->x = (int)rnd(N) << 4;
    b->y = (anywhere ? (int)rnd(N):N+b->r+1) << 4;
    b->speed = 5+(int)rnd(8)+(26-b->r)/3;    /* the small ones a little faster */
    b->phase = (int)rnd(64);
    b->ox = b->x >> 4;
    b->oy = b->y >> 4;
}

static int sway(int p) {                     /* -8..8, a period of 64 steps */
    static const int8_t q[16] = {0, 1, 2, 2, 3, 4, 5, 5, 6, 6, 7, 7, 7, 8, 8, 8};
    p &= 63;
    return p < 16 ? q[p]:p < 32 ? q[31-p]:p < 48 ? -q[p-32]:-q[63-p];
}

/* a bubble over colour c at x, y: a faint body, a stronger rim, a soft edge, a highlight up and to the left */
static uint16_t bubble_px(uint16_t c, const struct bubble *b, int x, int y) {
    int dx = 2*(x-(b->x >> 4))+1, dy = 2*(y-(b->y >> 4))+1, r = b->r;
    int d2 = dx*dx+dy*dy, r2 = 4*r*r;           /* in half pixels */
    if (d2 >= r2)
        return c;
    unsigned t = (unsigned)(d2*64/r2);          /* 0 centre .. 63 edge */
    unsigned a = t < 36 ? 4:t < 58 ? 4+(t-36)*12/22:16-(t-58)*3;
    c = mix(c, RGB(110, 170, 236), a, 32);
    /* the highlight: an ellipse 0.38r by 0.24r, its centre at -0.4r, -0.42r */
    int u = (dx*50+80*r)*100/(76*r), v = (dy*50+84*r)*100/(48*r), e = u*u+v*v;
    if (e < 10000) {
        unsigned k = (unsigned)((10000-e)*26/10000);
        c = mix(c, 0xffff, k > 20 ? 20:k, 32);
    }
    return c;
}

/* ---- the scene ---- */

void scene_init(uint8_t *layer, uint32_t s) {
    lc = (uint16_t *)(void *)layer;
    la = layer+2u*N*N;
    memset(la, 0, N*N);
    seed = s;
    for (unsigned i = 0; i < BUBBLES; ++i)
        spawn(&bubbles[i], true);
}

void scene_clear_tiles(void) {
    memset(la, 0, (size_t)SCENE_BAR_Y*N);
}

static void icon(int x, int y, const struct scene_tile *t) {
    int s = (int)SCENE_ICON;
    if (t->icon) {
        for (int j = 0; j < s; ++j)
            for (int i = 0; i < s; ++i) {
                const uint8_t *p = t->icon+4*(j*s+i);
                unsigned a = (p[3]*cover(distance(x+i, y+j, x, y, s, s, 11), 1)+127u)/255u;
                over((unsigned)((y+j)*N+x+i), RGB(p[0], p[1], p[2]), a);
            }
        return;
    }
    unsigned R = (t->colour >> 11) << 3, G = ((t->colour >> 5)&63u) << 2, B = (t->colour&31u) << 3;
    for (int j = 0; j < s; ++j) {
        unsigned k = (unsigned)(s-j)*3;              /* lighter at the top */
        uint16_t c = RGB(R+(255-R)*k/400, G+(255-G)*k/400, B+(255-B)*k/400);
        for (int i = 0; i < s; ++i)
            over((unsigned)((y+j)*N+x+i), c, cover(distance(x+i, y+j, x, y, s, s, 11), 1));
    }
    shape(x+2, y+2, s-4, s/2-3, 9, 0xffff, 4, 2, 0);         /* a gloss on the top half */
    char letter[2] = {t->letter, 0};
    text(x+s/2-text_w(&FONT_B, letter, 18)/2, y+7, &FONT_B, letter, 0xffff, 18);
}

void scene_tile_rows(unsigned slot, int *y0, int *y1) {
    int y = TILE_Y0+(int)slot*TILE_STEP;
    *y0 = y-8 < 0 ? 0:y-8;
    *y1 = y+TILE_H+10 > SCENE_BAR_Y ? SCENE_BAR_Y:y+TILE_H+10;
}

void scene_tile(unsigned slot, const struct scene_tile *t, bool selected) {
    int x = TILE_X, y = TILE_Y0+(int)slot*TILE_STEP, w = TILE_W, h = TILE_H;
    shape(x, y+3, w, h, TILE_R, RGB(30, 50, 80), selected ? 5:3, 10, 0);       /* shadow */
    if (selected)
        shape(x-3, y-3, w+6, h+6, TILE_R+3, BLUE, 6, 8, 0);                    /* glow */
    shape(x, y, w, h, TILE_R, 0xffff, selected ? 15:12, 1, 0);
    shape(x, y, w, h, TILE_R, selected ? BLUE:LINE, 16, 1, selected ? 2:1);
    icon(x+7, y+6, t);
    int vw = t->version && *t->version ? text_w(&FONT_XS, t->version, 8):0;
    text(x+60, y+9, &FONT_B, fit(&FONT_B, t->name, w-70-vw), INK, 8);
    if (vw)
        text(x+w-10-vw, y+10, &FONT_XS, t->version, FAINT, 8);
    if (t->about && *t->about)
        text(x+60, y+30, &FONT_XS, fit(&FONT_XS, t->about, w-68), DIM, 8);
}

void scene_more(bool above, bool below) {
    if (above)
        shape(N/2-10, 2, 20, 4, 2, BLUE, 10, 1, 0);
    if (below)
        shape(N/2-10, SCENE_BAR_Y-6, 20, 4, 2, BLUE, 10, 1, 0);
}

void scene_message(const char *title, const char *s) {
    int x = 20, y = 40, w = N-40, h = 120;
    shape(x, y+3, w, h, 14, RGB(30, 50, 80), 4, 10, 0);
    shape(x, y, w, h, 14, 0xffff, 15, 1, 0);
    shape(x, y, w, h, 14, LINE, 16, 1, 1);
    text(x+(w-text_w(&FONT_B, title, 8))/2, y+16, &FONT_B, title, INK, 8);
    /* the text by words, on up to four lines */
    char line[64];
    int ly = y+44;
    while (*s && ly < y+h-16) {
        unsigned n = 0, fit_n = 0;
        while (s[n] && n < sizeof line-1) {
            line[n] = s[n];
            line[n+1] = 0;
            if (text_w(&FONT_XS, line, 8) > w-28)
                break;
            ++n;
            if (!s[n] || s[n] == ' ')
                fit_n = n;
        }
        if (!fit_n)
            fit_n = n ? n:1;
        memcpy(line, s, fit_n);
        line[fit_n] = 0;
        text(x+(w-text_w(&FONT_XS, line, 8))/2, ly, &FONT_XS, line, DIM, 8);
        s += fit_n;
        while (*s == ' ')
            ++s;
        ly += 17;
    }
}

void scene_bar(const char *where, const char *keys, const char *note, uint16_t note_colour) {
    memset(la+SCENE_BAR_Y*N, 0, (size_t)(N-SCENE_BAR_Y)*N);
    shape(0, SCENE_BAR_Y, N, N-SCENE_BAR_Y, 0, 0xffff, 9, 1, 0);
    shape(0, SCENE_BAR_Y, N, 1, 0, LINE, 16, 1, 0);
    int mark = text(12, SCENE_BAR_Y+7, &FONT_B, "m-apps", WORDMARK, 12);
    if (where && *where)
        text(N-12-text_w(&FONT_XS, where, 8), SCENE_BAR_Y+5, &FONT_XS, where, DIM, 8);
    const char *last = note && *note ? note:keys;
    if (last && *last) {
        const char *s = fit(&FONT_XS, last, N-12-mark-10);
        text(N-12-text_w(&FONT_XS, s, 8), SCENE_BAR_Y+21, &FONT_XS, s, note && *note ? note_colour:FAINT, 8);
    }
}

void scene_render(int x0, int y0, int x1, int y1, uint16_t *out) {
    for (int y = y0; y < y1; ++y) {
        /* the bubbles on this row */
        const struct bubble *on[BUBBLES];
        unsigned n = 0;
        for (unsigned i = 0; i < BUBBLES; ++i) {
            int cy = bubbles[i].y >> 4, cx = bubbles[i].x >> 4, r = bubbles[i].r;
            if (y >= cy-r && y < cy+r && cx+r > x0 && cx-r < x1)
                on[n++] = &bubbles[i];
        }
        const uint8_t *a = la+y*N;
        const uint16_t *c = lc+y*N;
        for (int x = x0; x < x1; ++x) {
            uint16_t p;
            if (a[x] >= 16)
                p = c[x];
            else {
                p = back(x, y);
                for (unsigned i = 0; i < n; ++i)
                    p = bubble_px(p, on[i], x, y);
                if (a[x])
                    p = mix(p, c[x], a[x], 16);
            }
            *out++ = (uint16_t)(p >> 8 | p << 8);
        }
    }
}

void scene_step(void (*dirty)(int x0, int y0, int x1, int y1)) {
    for (unsigned i = 0; i < BUBBLES; ++i) {
        struct bubble *b = &bubbles[i];
        b->ox = b->x >> 4;
        b->oy = b->y >> 4;
        b->y -= b->speed;
        b->x += sway(++b->phase);
        if ((b->y >> 4)+b->r < 0) {
            int ox = b->ox, oy = b->oy, r = b->r;
            dirty(ox-r-1, oy-r-1, ox+r+1, oy+r+1);
            spawn(b, false);
            continue;
        }
        int nx = b->x >> 4, ny = b->y >> 4, r = b->r+1;
        dirty((nx < b->ox ? nx:b->ox)-r, (ny < b->oy ? ny:b->oy)-r, (nx > b->ox ? nx:b->ox)+r,
            (ny > b->oy ? ny:b->oy)+r);
    }
}
