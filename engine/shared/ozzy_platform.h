/* SPDX-License-Identifier: MIT */
/*
 * What the engine needs from the compiler and the C library: memory ordering on the shared
 * memory, string helpers and 64-bit division. The userspace driver and the host tests build
 * it with OZZY_USER_ENGINE.
 *
 * Copyright (C) 2024 Marcel Bierling <marcel@hackerman.art>
 */

#ifndef OZZY_PLATFORM_H
#define OZZY_PLATFORM_H

#if defined(__KERNEL__) && defined(__linux__)
#define OZZY_LINUX_KERNEL 1
#endif

#ifdef OZZY_LINUX_KERNEL
#include <linux/types.h>
#include <linux/stddef.h>
#include <linux/stdarg.h>
#include <linux/string.h>
#include <linux/errno.h>
#include <linux/kernel.h>
#include <linux/math64.h>
#include <asm/barrier.h>
#include <linux/atomic.h>
#else
#include <stdint.h>
#include <stddef.h>
#include <stdarg.h>
#include <string.h>
#ifndef __cplusplus
#include <stdbool.h>
#endif
#endif

/*
 * --- Transfer status the bus reports when a transfer was aborted (pipe abort, stop) ---
 * The Linux bus maps a killed or unlinked URB (-ENOENT, -ECONNRESET, -ESHUTDOWN) to this.
 */
#if defined(OZZY_LINUX_KERNEL)
#define OZZY_STATUS_ABORTED (-ENOENT)
#else
#define OZZY_STATUS_ABORTED (-125)  /* -ECANCELED: host builds (tests); a bus maps its own */
#endif

#ifndef OZZY_LINUX_KERNEL
#include <errno.h>
#endif

/* printf-style format checking where the compiler has it */
#if defined(__GNUC__) || defined(__clang__)
#define OZZY_PRINTF_FORMAT(fmt, args) __attribute__((format(printf, fmt, args)))
#else
#define OZZY_PRINTF_FORMAT(fmt, args)
#endif

/* --- Strings: ozzy_vsnprintf (always terminates @dst) and ozzy_strlcpy --- */
#ifndef OZZY_LINUX_KERNEL
#include <stdio.h>
#endif
static inline OZZY_PRINTF_FORMAT(3, 0) void ozzy_vsnprintf(char *dst, size_t size, const char *fmt, va_list args)
{
	vsnprintf(dst, size, fmt, args);
}

static inline void ozzy_strlcpy(char *dst, const char *src, size_t size)
{
	size_t i = 0;

	if (!size)
		return;
	for (; i + 1 < size && src[i]; i++)
		dst[i] = src[i];
	dst[i] = 0;
}

/* --- @a % @b for a 64-bit position (32-bit Linux has no 64-bit division in the kernel) --- */
static inline uint32_t ozzy_mod_u64(uint64_t a, uint32_t b)
{
#ifdef OZZY_LINUX_KERNEL
	uint32_t rem;

	div_u64_rem(a, b, &rem);
	return rem;
#else
	return (uint32_t)(a % b);
#endif
}

/* @a / @b, both 64-bit, for the same reason */
static inline uint64_t ozzy_div_u64(uint64_t a, uint64_t b)
{
#ifdef OZZY_LINUX_KERNEL
	return div64_u64(a, b);
#else
	return a / b;
#endif
}

/*
 * --- Memory ordering on the engine side ---
 * volatile only stops the compiler from reordering; on arm64 the CPU can still make a ring
 * index visible before the data it publishes. Pair these with the other side's acquire loads
 * and release stores.
 */
#if defined(OZZY_LINUX_KERNEL)
/* The kernel's own primitives: __atomic on 64-bit values would need libatomic on 32-bit ARM. */
#define OZZY_CACHE_ALIGN __attribute__((aligned(64)))

static inline uint32_t ozzy_load_acquire_u32(const volatile uint32_t *p) { return smp_load_acquire(p); }
static inline const void *ozzy_load_acquire_ptr(const void *const volatile *p) { return smp_load_acquire(p); }
static inline void ozzy_store_release_u32(volatile uint32_t *p, uint32_t v) { smp_store_release(p, v); }
static inline void ozzy_store_release_bool(volatile bool *p, bool v) { smp_store_release(p, v); }
static inline void ozzy_store_release_ptr(const void *volatile *p, const void *v) { smp_store_release(p, v); }
static inline void ozzy_store_relaxed_u32(volatile uint32_t *p, uint32_t v) { WRITE_ONCE(*p, v); }
static inline void ozzy_store_relaxed_u64(volatile uint64_t *p, uint64_t v) { WRITE_ONCE(*p, v); }
static inline void ozzy_fence_release(void) { smp_wmb(); }
static inline void ozzy_fence_acquire(void) { smp_rmb(); }
static inline uint64_t ozzy_load_relaxed_u64(const volatile uint64_t *p) { return READ_ONCE(*p); }
static inline bool ozzy_xchg_acquire_bool(volatile bool *p, bool v) { return xchg_acquire(p, v); }
#else
#define OZZY_CACHE_ALIGN __attribute__((aligned(64)))

#define OZZY_LOAD_ACQUIRE(type, p)       { return __atomic_load_n(p, __ATOMIC_ACQUIRE); }
#define OZZY_STORE_RELEASE(p, v)         { __atomic_store_n(p, v, __ATOMIC_RELEASE); }
#define OZZY_STORE_RELAXED(p, v)         { __atomic_store_n(p, v, __ATOMIC_RELAXED); }

static inline void ozzy_fence_release(void) { __atomic_thread_fence(__ATOMIC_RELEASE); }
static inline void ozzy_fence_acquire(void) { __atomic_thread_fence(__ATOMIC_ACQUIRE); }
static inline uint64_t ozzy_load_relaxed_u64(const volatile uint64_t *p) { return __atomic_load_n(p, __ATOMIC_RELAXED); }

static inline bool ozzy_xchg_acquire_bool(volatile bool *p, bool v)
{
	return __atomic_exchange_n(p, v, __ATOMIC_ACQUIRE);
}

static inline uint32_t ozzy_load_acquire_u32(const volatile uint32_t *p) OZZY_LOAD_ACQUIRE(uint32_t, p)
static inline const void *ozzy_load_acquire_ptr(const void *const volatile *p) OZZY_LOAD_ACQUIRE(const void *, p)
static inline void ozzy_store_release_u32(volatile uint32_t *p, uint32_t v) OZZY_STORE_RELEASE(p, v)
static inline void ozzy_store_release_bool(volatile bool *p, bool v) OZZY_STORE_RELEASE(p, v)
static inline void ozzy_store_release_ptr(const void *volatile *p, const void *v) OZZY_STORE_RELEASE(p, v)
static inline void ozzy_store_relaxed_u32(volatile uint32_t *p, uint32_t v) OZZY_STORE_RELAXED(p, v)
static inline void ozzy_store_relaxed_u64(volatile uint64_t *p, uint64_t v) OZZY_STORE_RELAXED(p, v)
#endif

#endif /* OZZY_PLATFORM_H */
