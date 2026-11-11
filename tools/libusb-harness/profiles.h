/* SPDX-License-Identifier: MIT */
#ifndef A16_PROFILES_H
#define A16_PROFILES_H
#include <spa/param/audio/raw.h>
#include <stdbool.h>
#include <stdint.h>
#include <string.h>
struct a16_profile {
    const char *name, *description, *positions, *room, *speakers;
    bool advanced, custom, pulse_compatible, available;
    uint32_t channels;
    uint8_t usb[16];
    uint32_t position[16];
};
#include "profiles_generated.inc"
#define A16_PROFILE_COUNT (sizeof(a16_profiles) / sizeof(a16_profiles[0]))
static inline int a16_profile_find(const char *name) {
    for (unsigned int i = 0; i < A16_PROFILE_COUNT; i++)
        if (!strcmp(name, a16_profiles[i].name))
            return (int)i;
    for (unsigned int i = 0; i < sizeof(a16_profile_aliases) / sizeof(a16_profile_aliases[0]); i++)
        if (!strcmp(name, a16_profile_aliases[i].name))
            return a16_profile_find(a16_profile_aliases[i].target);
    return -1;
}
static inline void a16_profile_expand(const struct a16_profile *p, float *out, const float *in) {
    memset(out, 0, 16 * sizeof(float));
    for (uint32_t c = 0; c < p->channels; c++)
        out[p->usb[c]] = in[c];
}
#endif
