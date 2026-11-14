/* SPDX-License-Identifier: MIT */
/*
 * How the stream has gone since the driver took the device, for the tools: live counters the
 * engine keeps in the shared memory (struct ozzy_shared_memory's @stats).
 *
 * Each field has one writer: a direction's fields are written from that pipe's completions,
 * the recovery counters from the driver's recovery. Readers may see a field half a transfer old.
 *
 * Copyright (C) 2024 Marcel Bierling <marcel@hackerman.art>
 */

#ifndef OZZY_STATS_H
#define OZZY_STATS_H

#if defined(__KERNEL__) && defined(__linux__)
#include <linux/types.h>
#else
#include <stdint.h>
#endif

#ifdef __cplusplus
extern "C" {
#endif

/* Bump when struct ozzy_stats changes */
#define OZZY_STATS_VERSION 1

/*
 * struct ozzy_stats_dir - One PCM direction (out, in: enum ozzy_pcm_dir).
 * @transfers:  completed transfers
 * @errors:     transfers that completed with an error (not the cancels of a stop)
 * @short_transfers: transfers that moved less than asked (isochronous: packets that failed)
 * @late:       completions after a gap longer than the queued transfers cover: the device went
 *              unserved for a while (a host that stopped running, not the client), the USB
 *              side's xrun. Out: it ran dry and repeated or dropped; in: it overflowed.
 * @max_gap_us: the longest gap between two completions
 */
struct ozzy_stats_dir {
	uint64_t transfers;
	uint32_t errors;
	uint32_t short_transfers;
	uint32_t late;
	uint32_t max_gap_us;
};

/*
 * struct ozzy_stats - The counters since the driver took the device (ozzy_engine_attach).
 * @recoveries:  times the driver brought the device back after a pipe stopped completing
 * @recovery_failures: of those, the ones that didn't get it streaming again
 * @dir:         by enum ozzy_pcm_dir
 */
struct ozzy_stats {
	uint32_t version;
	uint32_t size;
	uint32_t recoveries;
	uint32_t recovery_failures;
	struct ozzy_stats_dir dir[2];
};

/* Everything a client hears about: a change in it is worth a notification */
static inline uint32_t ozzy_stats_incidents(const volatile struct ozzy_stats *s)
{
	return s->recoveries + s->recovery_failures +
	       s->dir[0].errors + s->dir[0].short_transfers + s->dir[0].late +
	       s->dir[1].errors + s->dir[1].short_transfers + s->dir[1].late;
}

#ifdef __cplusplus
}
#endif

#endif /* OZZY_STATS_H */
