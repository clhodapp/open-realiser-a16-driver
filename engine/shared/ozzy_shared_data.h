/* SPDX-License-Identifier: MIT */
/*
 * The memory the engine shares with the host side of the driver: device status, the PCM rings
 * and the MIDI out ring. Fields both sides touch are volatile and are read and written with the explicit
 * memory ordering of ozzy_platform.h.
 *
 * Copyright (C) 2024 Marcel Bierling <marcel@hackerman.art>
 */

#ifndef OZZY_SHARED_DATA_H
#define OZZY_SHARED_DATA_H

#include "ozzy_platform.h"
#include "../../common/ozzy_settings.h"
#include "ozzy_info.h"
#include "ozzy_stats.h"

typedef volatile uint32_t ozzy_atomic_u32;
typedef volatile uint64_t ozzy_atomic_u64;
typedef volatile bool     ozzy_atomic_bool;

#define OZZY_SHARED_MEM_NAME            "/ozzy_shared_mem"
#define OZZY_MAGIC                      0x4F5A5A59   /* "OZZY" */
#define OZZY_SHARED_VERSION             16           /* bump when ozzy_shared_memory changes layout */

/* Each PCM ring's bytes (struct ozzy_audio_buffer): the device's transfers end to end */
#define OZZY_PCM_RING_BYTES             (1024 * 1024)

/* ozzy_audio_buffer.device_flags */
#define OZZY_DEVICE_FLAG_PLOYTEC_BULK   (1u << 0)    /* Ploytec 512-byte "bulk" output layout, else 482-byte "interrupt" */

#define OZZY_MIDI_RING_SIZE             1024
#define OZZY_MIDI_RING_MASK             (OZZY_MIDI_RING_SIZE - 1)

struct ozzy_audio_buffer {
	ozzy_atomic_bool hardware_present;
	ozzy_atomic_bool driver_ready;
	ozzy_atomic_u32 sample_rate;
	ozzy_atomic_u32 device_flags;          /* OZZY_DEVICE_FLAG_* */

	/* Frame format and ring layout, set by the engine at each start */
	uint32_t output_ring_frames;           /* frames in output_buffer (it wraps there) */
	uint32_t input_ring_frames;            /* frames in input_buffer */
	uint32_t output_bytes_per_frame;       /* encoded bytes per output frame (e.g. 48) */
	uint32_t input_bytes_per_frame;        /* encoded bytes per input frame (e.g. 64) */
	uint32_t input_channels;               /* channels the device sends (2) */
	uint32_t output_channels;              /* channels the device receives (16) */
	uint32_t safety_offset_frames;         /* how far ahead of the clock the engine reads output */
	uint32_t input_safety_offset_frames;   /* how far behind the clock input is complete (0: as output) */

	/*
	 * Settings (common/ozzy_settings.h) as the engine ran them at its last start. Written
	 * before driver_ready.
	 */
	struct {
		uint8_t value[OZZY_NUM_SETTINGS];
	} settings;

	/*
	 * The PCM rings, which the transfers go to and come from as they are (frame f at
	 * f % *_ring_frames):
	 * - Ploytec input: 512-byte sub-packets of 8 frames, end to end (frame f in sub-packet
	 *   f / 8, at (f % 8) * 64).
	 * - Ploytec output: sub-packets of 10 frames in the device's layout (512 bulk / 482
	 *   interrupt bytes, with their MIDI and control bytes), end to end.
	 * A ring of transfers holds a whole number of them, so none wraps.
	 */
	uint8_t input_buffer[OZZY_PCM_RING_BYTES];
	uint8_t output_buffer[OZZY_PCM_RING_BYTES];
};

/* A single-producer, single-consumer byte ring */
struct ozzy_midi_buffer {
	OZZY_CACHE_ALIGN ozzy_atomic_u32 write_index;
	uint8_t _pad1[64];
	OZZY_CACHE_ALIGN ozzy_atomic_u32 read_index;
	uint8_t _pad2[64];
	uint8_t buffer[OZZY_MIDI_RING_SIZE];
};

struct ozzy_shared_memory {
	uint32_t magic;                        /* OZZY_MAGIC */
	uint32_t version;
	uint32_t session_id;
	ozzy_atomic_u32 heartbeat;

	/* The device, as the host publishes it */
	uint16_t vendor_id;
	uint16_t product_id;
	char manufacturer_name[64];
	char product_name[64];
	char serial_number[64];

	/* What the driver knows of the device and its stream, as of the last handshake
	 * (ozzy_engine_publish_info), for the tools (ozzy_info.h) */
	struct ozzy_info info;
	/* How the stream has gone since the driver took the device (ozzy_stats.h): live */
	struct ozzy_stats stats;

	struct ozzy_midi_buffer midi_out;      /* clients fill it, the engine drains it */

	OZZY_CACHE_ALIGN struct ozzy_audio_buffer audio;
};

/*
 * --- The MIDI out ring: a client fills it, the engine drains it into the PCM out packets ---
 * ozzy_midi_ring_push appends what fits (returns how many), ozzy_midi_ring_pop takes up to
 * @max bytes (returns how many).
 */
typedef volatile struct ozzy_midi_buffer ozzy_midi_ring;

/* Append @count bytes to @ring; what doesn't fit is dropped. Returns how many went in. */
static inline uint32_t ozzy_midi_ring_push(ozzy_midi_ring *ring, const uint8_t *bytes, uint32_t count)
{
	uint32_t w = ring->write_index;
	uint32_t r = ozzy_load_acquire_u32(&ring->read_index);
	uint32_t i;

	for (i = 0; i < count && ((w + 1) & OZZY_MIDI_RING_MASK) != r; i++) {
		ring->buffer[w] = bytes[i];
		w = (w + 1) & OZZY_MIDI_RING_MASK;
	}
	ozzy_store_release_u32(&ring->write_index, w);   /* release: the bytes before the index */
	return i;
}

/* Take up to @max bytes from @ring into @dst. Returns how many. */
static inline uint32_t ozzy_midi_ring_pop(ozzy_midi_ring *ring, uint8_t *dst, uint32_t max)
{
	uint32_t r = ring->read_index;
	uint32_t w = ozzy_load_acquire_u32(&ring->write_index);   /* acquire: the bytes before the index */
	uint32_t i;

	for (i = 0; i < max && r != w; i++) {
		dst[i] = ring->buffer[r];
		r = (r + 1) & OZZY_MIDI_RING_MASK;
	}
	ozzy_store_release_u32(&ring->read_index, r);
	return i;
}

#endif /* OZZY_SHARED_DATA_H */
