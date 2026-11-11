/* SPDX-License-Identifier: MIT */
#ifndef A16_LIVE_AUDIO_H
#define A16_LIVE_AUDIO_H
#include <math.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <string.h>
#define LIVE_CAPACITY 16384u
#define LIVE_TARGET 2048u
#define LIVE_CHANNELS 16u
/* Single producer/single consumer. Only the owner advances its index. */
struct live_fifo {
    _Atomic uint32_t read, write;
    uint32_t channels;
    float data[LIVE_CAPACITY * LIVE_CHANNELS];
};
static inline uint32_t live_available(struct live_fifo *q) {
    uint32_t r = atomic_load_explicit(&q->read, memory_order_acquire);
    uint32_t w = atomic_load_explicit(&q->write, memory_order_acquire);
    return w - r;
}
static inline uint32_t live_push(struct live_fifo *q, const float *src, uint32_t n) {
    uint32_t w = atomic_load_explicit(&q->write, memory_order_relaxed);
    uint32_t r = atomic_load_explicit(&q->read, memory_order_acquire);
    uint32_t count = n < LIVE_CAPACITY - (w - r) ? n : LIVE_CAPACITY - (w - r);
    for (uint32_t i = 0; i < count; i++)
        memcpy(q->data + ((w + i) % LIVE_CAPACITY) * q->channels, src + i * q->channels,
               q->channels * sizeof(float));
    atomic_store_explicit(&q->write, w + count, memory_order_release);
    return count;
}
static inline uint32_t live_peek(struct live_fifo *q, float *dst, uint32_t n) {
    uint32_t r = atomic_load_explicit(&q->read, memory_order_relaxed);
    uint32_t w = atomic_load_explicit(&q->write, memory_order_acquire);
    uint32_t count = n < w - r ? n : w - r;
    for (uint32_t i = 0; i < count; i++)
        memcpy(dst + i * q->channels, q->data + ((r + i) % LIVE_CAPACITY) * q->channels,
               q->channels * sizeof(float));
    return count;
}
static inline void live_consume(struct live_fifo *q, uint32_t n) {
    uint32_t r = atomic_load_explicit(&q->read, memory_order_relaxed);
    atomic_store_explicit(&q->read, r + n, memory_order_release);
}
static inline void live_discard(struct live_fifo *q) {
    uint32_t w = atomic_load_explicit(&q->write, memory_order_acquire);
    atomic_store_explicit(&q->read, w, memory_order_release);
}
static inline float live_unpack(const uint8_t *p) {
    uint32_t u = (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16;
    int32_t v = (int32_t)(u & 0x800000 ? u | 0xff000000 : u);
    return (float)v / 8388608.0f;
}
static inline void live_pack(uint8_t *p, float v) {
    if (!isfinite(v))
        v = 0;
    if (v > 0.99999988f)
        v = 0.99999988f;
    if (v < -1.0f)
        v = -1.0f;
    int32_t s = (int32_t)lrintf(v * 8388608.0f);
    uint32_t u = (uint32_t)s;
    p[0] = u;
    p[1] = u >> 8;
    p[2] = u >> 16;
}
struct live_clock {
    double filtered, integral;
    bool initialized;
};
static inline double live_ratio(struct live_clock *c, uint32_t level, double seconds) {
    if (!c->initialized) {
        c->filtered = LIVE_TARGET;
        c->initialized = true;
    }
    double alpha = fmin(seconds * 4, 1.0);
    c->filtered += alpha * ((double)level - c->filtered);
    double error = c->filtered - LIVE_TARGET;
    c->integral = fmax(-0.002, fmin(0.002, c->integral + error * seconds * 0.00000001));
    double correction = fmax(-0.005, fmin(0.005, error * 0.000001 + c->integral));
    return 1.0 - correction;
}
#endif
