/*
 * vinput: one persistent virtual mouse + keyboard for the sway seat, driven over a unix socket.
 *
 * A real PC has one mouse and one keyboard that never disappear. Creating a fresh virtual
 * device per action (wtype, one-shot vpointer) churns the seat keymap; Chromium then crashes on
 * launch and loses selections. This daemon keeps both devices alive with a standard XKB keymap.
 *
 * Protocol: one command per line, one reply line ("ok", "ok X Y" or "err ...").
 *   size W H            screen extent for absolute moves
 *   abs X Y             absolute pointer move
 *   rel DX DY           relative pointer move (flat accel, so 1:1 pixels)
 *   pos                 -> "ok X Y", tracked pointer position
 *   btn N 0|1           N: 0 left, 1 right, 2 middle
 *   wheel V H           discrete wheel notches (V > 0 scrolls down, H > 0 scrolls right)
 *   key CODE 0|1        evdev keycode press/release
 *   sym NAME 0|1        keysym by name (Return, Control_L, a, F5, ...) press/release
 *   lookup NAME         -> "ok CODE LEVEL": evdev key that types keysym NAME (or U20AC-style
 *                       codepoint) on this layout; LEVEL bit 0 = Shift, bit 1 = AltGr
 *   state               -> "ok DEPRESSED LATCHED LOCKED LOCKED_LAYOUT EFFECTIVE_LAYOUT" for exact typing
 *   trace 0|1           pause/resume VINPUT_TRACE; replies with the previous enabled state
 *   release             release every held key and button
 */
#define _GNU_SOURCE
#include <errno.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>
#include <linux/input-event-codes.h>
#include <wayland-client.h>
#include <xkbcommon/xkbcommon.h>
#include "vpointer-client.h"
#include "vkeyboard-client.h"

static struct wl_display *display;
static struct wl_seat *seat;
static struct zwlr_virtual_pointer_manager_v1 *pointer_manager;
static struct zwp_virtual_keyboard_manager_v1 *keyboard_manager;
static struct zwlr_virtual_pointer_v1 *pointer;
static struct zwp_virtual_keyboard_v1 *keyboard;
static struct xkb_keymap *keymap;
static struct xkb_state *xstate;

static int extent_w = 1920, extent_h = 1080;
static double cur_x = 960, cur_y = 540;
static unsigned char keys_down[KEY_MAX + 1];
static unsigned char buttons_down[3];
static const uint32_t button_codes[3] = { BTN_LEFT, BTN_RIGHT, BTN_MIDDLE };
/* VINPUT_TRACE=<file>: append common JSONL input events for offline eval-lab analysis. */
static FILE *trace;
static const char *trace_path;

static void trace_event(const char *kind, int a, int b) {
    if (!trace) return;
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    unsigned long long timestamp_us = (unsigned long long)ts.tv_sec * 1000000ULL
        + (unsigned long long)ts.tv_nsec / 1000ULL;
    fprintf(trace, "{\"timestamp_us\":%llu,\"event\":\"%s\",\"a\":%d,\"b\":%d}\n",
        timestamp_us, kind, a, b);
}

static void registry_global(void *data, struct wl_registry *reg, uint32_t name,
    const char *iface, uint32_t version) {
    if (strcmp(iface, wl_seat_interface.name) == 0 && !seat)
        seat = wl_registry_bind(reg, name, &wl_seat_interface, 1);
    else if (strcmp(iface, zwlr_virtual_pointer_manager_v1_interface.name) == 0)
        pointer_manager = wl_registry_bind(reg, name, &zwlr_virtual_pointer_manager_v1_interface,
            version < 2 ? version : 2);
    else if (strcmp(iface, zwp_virtual_keyboard_manager_v1_interface.name) == 0)
        keyboard_manager = wl_registry_bind(reg, name, &zwp_virtual_keyboard_manager_v1_interface, 1);
}
static void registry_remove(void *data, struct wl_registry *reg, uint32_t name) {}
static const struct wl_registry_listener registry_listener = { registry_global, registry_remove };

static uint32_t now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint32_t)(ts.tv_sec * 1000 + ts.tv_nsec / 1000000);
}

static int upload_keymap(void) {
    struct xkb_context *ctx = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
    if (!ctx) return -1;
    /* NULL names: XKB_DEFAULT_LAYOUT etc. from the environment, else "us" */
    keymap = xkb_keymap_new_from_names(ctx, NULL, XKB_KEYMAP_COMPILE_NO_FLAGS);
    if (!keymap) return -1;
    xstate = xkb_state_new(keymap);
    char *text = xkb_keymap_get_as_string(keymap, XKB_KEYMAP_FORMAT_TEXT_V1);
    size_t size = strlen(text) + 1;
    int fd = memfd_create("vinput-keymap", MFD_CLOEXEC);
    if (fd < 0 || ftruncate(fd, size) < 0) return -1;
    void *map = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (map == MAP_FAILED) return -1;
    memcpy(map, text, size);
    munmap(map, size);
    free(text);
    zwp_virtual_keyboard_v1_keymap(keyboard, WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1, fd, size);
    close(fd);
    return 0;
}

static void send_key(uint32_t code, int down) {
    if (code > KEY_MAX || keys_down[code] == down) return;
    keys_down[code] = down;
    trace_event("key", (int)code, down);
    zwp_virtual_keyboard_v1_key(keyboard, now_ms(), code,
        down ? WL_KEYBOARD_KEY_STATE_PRESSED : WL_KEYBOARD_KEY_STATE_RELEASED);
    xkb_state_update_key(xstate, code + 8, down ? XKB_KEY_DOWN : XKB_KEY_UP);
    zwp_virtual_keyboard_v1_modifiers(keyboard,
        xkb_state_serialize_mods(xstate, XKB_STATE_MODS_DEPRESSED),
        xkb_state_serialize_mods(xstate, XKB_STATE_MODS_LATCHED),
        xkb_state_serialize_mods(xstate, XKB_STATE_MODS_LOCKED),
        xkb_state_serialize_layout(xstate, XKB_STATE_LAYOUT_EFFECTIVE));
}

/* Modifiers that reach `level` of `kc`, as bit 0 Shift / bit 1 AltGr (Mod5); -1 when every
 * way to that level needs another modifier (NumLock, Lock, Ctrl...). */
static int level_mods(xkb_keycode_t kc, xkb_level_index_t level) {
    xkb_mod_mask_t shift = 1u << xkb_keymap_mod_get_index(keymap, XKB_MOD_NAME_SHIFT);
    xkb_mod_mask_t altgr = 1u << xkb_keymap_mod_get_index(keymap, "Mod5");
    xkb_mod_mask_t masks[16];
    size_t n = xkb_keymap_key_get_mods_for_level(keymap, kc, 0, level, masks, 16);
    for (size_t i = 0; i < n; i++)
        if (!(masks[i] & ~(shift | altgr)))
            return (masks[i] & shift ? 1 : 0) | (masks[i] & altgr ? 2 : 0);
    return -1;
}

/* Keycode carrying `sym` (or the same character), preferring the lowest level and the main
 * block over the keypad. With `mods`, only levels reachable by Shift/AltGr count and their
 * modifiers are stored there. O(keycodes x levels), ~250 x 4. */
static int find_key(xkb_keysym_t sym, int *mods) {
    /* xkeyboard-config's hidden <LVL3> key (evdev 84, no physical key) also carries AltGr and
     * sorts first; pages would see code "Unidentified". A real keyboard's AltGr is Right Alt. */
    if (sym == XKB_KEY_ISO_Level3_Shift) {
        const xkb_keysym_t *ralt;
        if (xkb_keymap_key_get_syms_by_level(keymap, KEY_RIGHTALT + 8, 0, 0, &ralt) == 1 && ralt[0] == sym) {
            if (mods) *mods = 0;
            return KEY_RIGHTALT;
        }
    }
    uint32_t cp = xkb_keysym_to_utf32(sym);
    xkb_keycode_t min = xkb_keymap_min_keycode(keymap), max = xkb_keymap_max_keycode(keymap);
    for (xkb_level_index_t level = 0; level < 4; level++) {
        for (xkb_keycode_t kc = min; kc <= max; kc++) {
            const xkb_keysym_t *syms;
            int n = xkb_keymap_key_get_syms_by_level(keymap, kc, 0, level, &syms);
            for (int i = 0; i < n; i++) {
                int same = syms[i] == sym || (cp && xkb_keysym_to_utf32(syms[i]) == cp
                    && !(syms[i] >= XKB_KEY_KP_Space && syms[i] <= XKB_KEY_KP_Equal));
                if (!same) continue;
                if (mods && (*mods = level_mods(kc, level)) < 0) continue;
                return (int)kc - 8;
            }
        }
    }
    return -1;
}

static xkb_keysym_t parse_sym(const char *name) {
    xkb_keysym_t sym = xkb_keysym_from_name(name, XKB_KEYSYM_NO_FLAGS);
    return sym != XKB_KEY_NoSymbol ? sym : xkb_keysym_from_name(name, XKB_KEYSYM_CASE_INSENSITIVE);
}

static void pointer_abs(void) {
    trace_event("pos", (int)cur_x, (int)cur_y);
    zwlr_virtual_pointer_v1_motion_absolute(pointer, now_ms(),
        (uint32_t)cur_x, (uint32_t)cur_y, (uint32_t)extent_w, (uint32_t)extent_h);
    zwlr_virtual_pointer_v1_frame(pointer);
}

static double clampd(double v, double lo, double hi) { return v < lo ? lo : v > hi ? hi : v; }

static void wheel(uint32_t axis, int notches) {
    int dir = notches > 0 ? 1 : -1;
    for (int i = 0; i < abs(notches); i++) {
        trace_event("wheel", axis == WL_POINTER_AXIS_VERTICAL_SCROLL ? dir : 0,
            axis == WL_POINTER_AXIS_HORIZONTAL_SCROLL ? dir : 0);
        zwlr_virtual_pointer_v1_axis_source(pointer, WL_POINTER_AXIS_SOURCE_WHEEL);
        zwlr_virtual_pointer_v1_axis_discrete(pointer, now_ms(), axis,
            wl_fixed_from_int(dir * 15), dir);
        zwlr_virtual_pointer_v1_frame(pointer);
    }
}

static void release_all(void) {
    for (int b = 0; b < 3; b++) {
        if (buttons_down[b]) {
            buttons_down[b] = 0;
            zwlr_virtual_pointer_v1_button(pointer, now_ms(), button_codes[b],
                WL_POINTER_BUTTON_STATE_RELEASED);
            zwlr_virtual_pointer_v1_frame(pointer);
        }
    }
    for (uint32_t k = 0; k <= KEY_MAX; k++)
        if (keys_down[k]) send_key(k, 0);
}

static void handle(char *line, char *reply, size_t cap) {
    char cmd[16] = {0}, name[64] = {0};
    double a = 0, b = 0;
    int n = 0, down = 0;
    snprintf(reply, cap, "ok");
    if (sscanf(line, "%15s", cmd) != 1) { snprintf(reply, cap, "err empty"); return; }

    if (!strcmp(cmd, "size") && sscanf(line, "%*s %lf %lf", &a, &b) == 2 && a > 0 && b > 0) {
        extent_w = (int)a; extent_h = (int)b;
        cur_x = clampd(cur_x, 0, extent_w - 1); cur_y = clampd(cur_y, 0, extent_h - 1);
    } else if (!strcmp(cmd, "abs") && sscanf(line, "%*s %lf %lf", &a, &b) == 2) {
        cur_x = clampd(a, 0, extent_w - 1); cur_y = clampd(b, 0, extent_h - 1);
        pointer_abs();
    } else if (!strcmp(cmd, "rel") && sscanf(line, "%*s %lf %lf", &a, &b) == 2) {
        double nx = clampd(cur_x + a, 0, extent_w - 1), ny = clampd(cur_y + b, 0, extent_h - 1);
        zwlr_virtual_pointer_v1_motion(pointer, now_ms(),
            wl_fixed_from_double(nx - cur_x), wl_fixed_from_double(ny - cur_y));
        zwlr_virtual_pointer_v1_frame(pointer);
        cur_x = nx; cur_y = ny;
        trace_event("pos", (int)cur_x, (int)cur_y);
    } else if (!strcmp(cmd, "pos")) {
        snprintf(reply, cap, "ok %d %d", (int)cur_x, (int)cur_y);
    } else if (!strcmp(cmd, "btn") && sscanf(line, "%*s %d %d", &n, &down) == 2 && n >= 0 && n < 3) {
        if (buttons_down[n] != !!down) {
            buttons_down[n] = !!down;
            trace_event("btn", n, !!down);
            zwlr_virtual_pointer_v1_button(pointer, now_ms(), button_codes[n],
                down ? WL_POINTER_BUTTON_STATE_PRESSED : WL_POINTER_BUTTON_STATE_RELEASED);
            zwlr_virtual_pointer_v1_frame(pointer);
        }
    } else if (!strcmp(cmd, "wheel") && sscanf(line, "%*s %lf %lf", &a, &b) == 2) {
        if ((int)a) wheel(WL_POINTER_AXIS_VERTICAL_SCROLL, (int)a);
        if ((int)b) wheel(WL_POINTER_AXIS_HORIZONTAL_SCROLL, (int)b);
    } else if (!strcmp(cmd, "key") && sscanf(line, "%*s %d %d", &n, &down) == 2 && n > 0 && n <= KEY_MAX) {
        send_key((uint32_t)n, !!down);
    } else if (!strcmp(cmd, "sym") && sscanf(line, "%*s %63s %d", name, &down) == 2) {
        xkb_keysym_t sym = parse_sym(name);
        int code = sym == XKB_KEY_NoSymbol ? -1 : find_key(sym, NULL);
        if (code <= 0) snprintf(reply, cap, "err no key for %s on this layout", name);
        else send_key((uint32_t)code, !!down);
    } else if (!strcmp(cmd, "lookup") && sscanf(line, "%*s %63s", name) == 1) {
        xkb_keysym_t sym = parse_sym(name);
        int mods = 0, code = sym == XKB_KEY_NoSymbol ? -1 : find_key(sym, &mods);
        if (code <= 0) snprintf(reply, cap, "err no key for %s on this layout", name);
        else snprintf(reply, cap, "ok %d %d", code, mods);
    } else if (!strcmp(cmd, "state")) {
        snprintf(reply, cap, "ok %u %u %u %u %u",
            xkb_state_serialize_mods(xstate, XKB_STATE_MODS_DEPRESSED),
            xkb_state_serialize_mods(xstate, XKB_STATE_MODS_LATCHED),
            xkb_state_serialize_mods(xstate, XKB_STATE_MODS_LOCKED),
            xkb_state_serialize_layout(xstate, XKB_STATE_LAYOUT_LOCKED),
            xkb_state_serialize_layout(xstate, XKB_STATE_LAYOUT_EFFECTIVE));
    } else if (!strcmp(cmd, "trace") && sscanf(line, "%*s %d", &n) == 1 && (n == 0 || n == 1)) {
        int was_enabled = trace != NULL;
        if (n == 0) {
            if (trace) fclose(trace);
            trace = NULL;
        } else if (!trace && trace_path && *trace_path) {
            trace = fopen(trace_path, "a");
            if (trace) setvbuf(trace, NULL, _IOLBF, 0);
            else { snprintf(reply, cap, "err could not reopen trace file"); return; }
        }
        snprintf(reply, cap, "ok %d", was_enabled);
    } else if (!strcmp(cmd, "release")) {
        release_all();
    } else {
        snprintf(reply, cap, "err bad command: %s", cmd);
    }
}

int main(int argc, char **argv) {
    const char *dir = getenv("XDG_RUNTIME_DIR");
    char path[108];
    snprintf(path, sizeof path, "%s/taboom-input.sock", argc > 1 ? argv[1] : dir ? dir : "/tmp");

    trace_path = getenv("VINPUT_TRACE");
    if (trace_path && *trace_path) {
        trace = fopen(trace_path, "a");
        if (trace) setvbuf(trace, NULL, _IOLBF, 0);
    }

    display = wl_display_connect(NULL);
    if (!display) { fprintf(stderr, "vinput: no wayland display\n"); return 1; }
    struct wl_registry *reg = wl_display_get_registry(display);
    wl_registry_add_listener(reg, &registry_listener, NULL);
    wl_display_roundtrip(display);
    if (!seat || !pointer_manager || !keyboard_manager) {
        fprintf(stderr, "vinput: compositor lacks virtual pointer/keyboard support\n");
        return 1;
    }
    pointer = zwlr_virtual_pointer_manager_v1_create_virtual_pointer(pointer_manager, seat);
    keyboard = zwp_virtual_keyboard_manager_v1_create_virtual_keyboard(keyboard_manager, seat);
    if (upload_keymap() < 0) { fprintf(stderr, "vinput: keymap failed\n"); return 1; }
    pointer_abs();
    wl_display_roundtrip(display);

    int srv = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    struct sockaddr_un addr = { .sun_family = AF_UNIX };
    strncpy(addr.sun_path, path, sizeof addr.sun_path - 1);
    unlink(path);
    if (bind(srv, (struct sockaddr *)&addr, sizeof addr) < 0 || listen(srv, 8) < 0) {
        perror("vinput: socket");
        return 1;
    }
    fprintf(stderr, "vinput: ready on %s\n", path);

    for (;;) {
        int c = accept4(srv, NULL, NULL, SOCK_CLOEXEC);
        if (c < 0) { if (errno == EINTR) continue; perror("vinput: accept"); return 1; }
        FILE *in = fdopen(c, "r");
        char *line = NULL;
        size_t len = 0;
        while (getline(&line, &len, in) > 0) {
            char reply[128];
            handle(line, reply, sizeof reply);
            wl_display_flush(display);
            if (wl_display_roundtrip(display) < 0) { fprintf(stderr, "vinput: compositor gone\n"); return 1; }
            size_t rl = strlen(reply);
            reply[rl] = '\n';
            if (write(c, reply, rl + 1) < 0) break;
        }
        free(line);
        /* a client that vanishes mid-action must not leave keys or buttons stuck */
        release_all();
        wl_display_roundtrip(display);
        fclose(in);
    }
}
