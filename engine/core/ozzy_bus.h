/* SPDX-License-Identifier: MIT */
/*
 * USB bus interface: transfers, control requests and a clock.
 * The libusb backend and fake test bus implement struct ozzy_bus_ops.
 * Completions return through ozzy_engine_complete().
 *
 * Copyright (C) 2024 Marcel Bierling <marcel@hackerman.art>
 */

#ifndef OZZY_BUS_H
#define OZZY_BUS_H

#include "../shared/ozzy_platform.h"

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Transfers that can be in flight at the same time never share an OZZY_DMA_ALIGN block of
 * memory: the largest cache line DMA has to respect on a non-coherent machine (arm64 Linux's
 * ARCH_DMA_MINALIGN), so one transfer's cache maintenance can't clobber another's data.
 */
#define OZZY_DMA_ALIGN 128
#define OZZY_DMA_ROUND(n) (((n) + OZZY_DMA_ALIGN - 1) & ~(uint32_t)(OZZY_DMA_ALIGN - 1))

enum ozzy_pcm_dir {
	OZZY_PCM_OUT,
	OZZY_PCM_IN,
	OZZY_PCM_DIRS,
};

/*
 * struct ozzy_bus_ops - The platform's services. @ctx is struct ozzy_bus.ctx.
 * @submit:       queue a bulk or interrupt transfer of @size bytes at @buf (in the shared
 *                memory) on endpoint @pipe; @xfer comes back with the completion
 * @control:      a control request, synchronous (the handshake)
 * @clear_halt:   clear ENDPOINT_HALT on endpoint @endpoint (with direction bit) and reset the
 *                host side of its pipe, synchronous (the handshake); 0 or a negative error.
 *                Optional.
 * @pcm_progress: the engine moved PCM through an attached ring (ozzy_engine_set_pcm_ring):
 *                @frames is the ring position after the packet. From the completion path.
 *                Without a ring attached, PCM out is silence.
 * @sleep_ms:     sleep (handshake only, never from a completion)
 * @time:         the host's monotonic clock
 * @abort:        cancel every transfer in flight and return once their completions have run
 *                (with OZZY_STATUS_ABORTED); the pipes stay usable. Process context, never
 *                from a completion. The engine calls it when it stops or suspends.
 * @time_hz:      @time's ticks per second
 * @stats_changed: an incident counter in the shared memory's stats moved (ozzy_stats.h): tell
 *                the clients, the way a setting change is told, at most a few times a second.
 *                From the completion path. Optional.
 */
struct ozzy_bus_ops {
	bool (*submit)(void *ctx, uint8_t pipe, void *buf, uint32_t size, void *xfer);
	bool (*control)(void *ctx, uint8_t type, uint8_t request, uint16_t value, uint16_t index,
			void *data, uint16_t length);
	int (*clear_halt)(void *ctx, uint8_t endpoint);
	void (*pcm_progress)(void *ctx, enum ozzy_pcm_dir dir, uint64_t frames);
	void (*sleep_ms)(void *ctx, unsigned int ms);
	uint64_t (*time)(void *ctx);
	void (*abort)(void *ctx);
	uint64_t (*time_hz)(void *ctx);
	void (*stats_changed)(void *ctx);
};

struct ozzy_bus {
	const struct ozzy_bus_ops *ops;
	void *ctx;
};

static inline bool ozzy_bus_submit(const struct ozzy_bus *bus, uint8_t pipe, void *buf, uint32_t size,
				   void *xfer)
{
	return bus->ops->submit(bus->ctx, pipe, buf, size, xfer);
}

static inline void ozzy_bus_pcm_progress(const struct ozzy_bus *bus, enum ozzy_pcm_dir dir, uint64_t frames)
{
	bus->ops->pcm_progress(bus->ctx, dir, frames);
}

static inline void ozzy_bus_abort(const struct ozzy_bus *bus)
{
	bus->ops->abort(bus->ctx);
}

static inline uint64_t ozzy_bus_time(const struct ozzy_bus *bus)
{
	return bus->ops->time(bus->ctx);
}

static inline uint64_t ozzy_bus_time_hz(const struct ozzy_bus *bus)
{
	return bus->ops->time_hz ? bus->ops->time_hz(bus->ctx) : 0;
}

static inline void ozzy_bus_stats_changed(const struct ozzy_bus *bus)
{
	if (bus->ops->stats_changed)
		bus->ops->stats_changed(bus->ctx);
}

#ifdef __cplusplus
}
#endif

#endif /* OZZY_BUS_H */
