/* SPDX-License-Identifier: MIT */
#ifndef A16_PROFILE_OVERRIDE_H
#define A16_PROFILE_OVERRIDE_H
#include "profiles.h"
#include <ctype.h>
#include <errno.h>
#include <spa/utils/json.h>
#include <stdio.h>
struct a16_missing {
    unsigned int count;
    char names[64][16];
    char text[1024];
};
struct a16_effective {
    struct a16_profile profile;
    char positions[256], speakers[256], description[256];
};
static inline bool a16_word_in(const char *words, const char *name) {
    size_t n = strlen(name);
    while (*words) {
        while (*words && isspace((unsigned char)*words))
            words++;
        const char *start = words;
        while (*words && !isspace((unsigned char)*words))
            words++;
        if ((size_t)(words - start) == n && !memcmp(start, name, n))
            return true;
    }
    return false;
}
static inline bool a16_missing_has(const struct a16_missing *m, const char *name) {
    for (unsigned int i = 0; i < m->count; i++)
        if (!strcmp(m->names[i], name))
            return true;
    return false;
}
static inline bool a16_missing_add(struct a16_missing *m, const char *name) {
    bool known = false;
    for (unsigned int i = 0; i < A16_PROFILE_COUNT; i++)
        if (a16_word_in(a16_profiles[i].speakers, name))
            known = true;
    if (!known || !*name || strlen(name) >= sizeof(m->names[0]) || m->count >= 64)
        return false;
    if (a16_missing_has(m, name))
        return true;
    strcpy(m->names[m->count++], name);
    if (*m->text)
        strcat(m->text, ",");
    strcat(m->text, name);
    return true;
}
static inline bool a16_missing_parse(struct a16_missing *m, const char *text) {
    while (*text) {
        while (*text == ',' || isspace((unsigned char)*text))
            text++;
        if (!*text)
            break;
        char word[16];
        size_t n = 0;
        while (*text && *text != ',' && !isspace((unsigned char)*text)) {
            if (n + 1 >= sizeof(word))
                return false;
            word[n++] = *text++;
        }
        word[n] = 0;
        if (!a16_missing_add(m, word))
            return false;
    }
    return true;
}
static inline bool a16_missing_json(struct a16_missing *m, const char *text, size_t size) {
    struct spa_json root, object, array;
    char key[64], name[16];
    const char *value;
    int len;
    bool seen = false;
    spa_json_init(&root, text, size);
    if (spa_json_enter_object(&root, &object) <= 0)
        return false;
    while ((len = spa_json_object_next(&object, key, sizeof(key), &value)) > 0) {
        if (strcmp(key, "missing_speakers") || seen || !spa_json_is_array(value, len))
            return false;
        seen = true;
        spa_json_enter(&object, &array);
        while ((len = spa_json_next(&array, &value)) > 0) {
            if (!spa_json_is_string(value, len) ||
                spa_json_parse_stringn(value, len, name, sizeof(name)) <= 0 || !a16_missing_add(m, name))
                return false;
        }
        if (len < 0)
            return false;
    }
    return len == 0;
}
static inline bool a16_missing_load(struct a16_missing *m, const char *path, bool optional) {
    FILE *f = fopen(path, "r");
    if (!f)
        return optional && errno == ENOENT;
    char buffer[4097];
    size_t n = fread(buffer, 1, sizeof(buffer), f);
    bool ok = !ferror(f) && n < sizeof(buffer);
    fclose(f);
    return ok && a16_missing_json(m, buffer, n);
}
static inline bool a16_apply_missing(struct a16_effective *out, const struct a16_profile *base,
                                     const struct a16_missing *missing) {
    memset(out, 0, sizeof(*out));
    out->profile = *base;
    out->profile.channels = 0;
    char labels[256], positions[256];
    if (strlen(base->speakers) >= sizeof(labels) || strlen(base->positions) >= sizeof(positions))
        return false;
    strcpy(labels, base->speakers);
    strcpy(positions, base->positions);
    char *label_state = NULL, *position_state = NULL;
    char *label = strtok_r(labels, " ", &label_state),
         *position = strtok_r(positions, "[] ", &position_state);
    strcpy(out->positions, "[");
    for (unsigned int i = 0; i < base->channels; i++) {
        if (!label || !position)
            return false;
        if (!a16_missing_has(missing, label)) {
            unsigned int c = out->profile.channels++;
            out->profile.usb[c] = base->usb[i];
            out->profile.position[c] = base->position[i];
            if (*out->speakers)
                strcat(out->speakers, " ");
            strcat(out->speakers, label);
            strcat(out->positions, " ");
            strcat(out->positions, position);
        }
        label = strtok_r(NULL, " ", &label_state);
        position = strtok_r(NULL, "[] ", &position_state);
    }
    if (label || position)
        return false;
    strcat(out->positions, " ]");
    out->profile.positions = out->positions;
    out->profile.speakers = out->speakers;
    out->profile.available = base->available && out->profile.channels > 0;
    if (out->profile.channels != base->channels) {
        snprintf(out->description, sizeof(out->description), "%s [%u/%u speakers]", base->description,
                 out->profile.channels, base->channels);
        out->profile.description = out->description;
    }
    return true;
}
#endif
