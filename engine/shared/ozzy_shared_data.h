/* SPDX-License-Identifier: MIT */
/*
 * The memory the engine shares with the host side of the driver: device status, the PCM and
 * MIDI rings and the clock anchors. Fields both sides touch are volatile and are read and
 * written with the explicit memory ordering of ozzy_platform.h.
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
#define OZZY_SHARED_VERSION             12           /* bump when ozzy_shared_memory changes layout */

/* Each PCM ring's bytes (struct ozzy_audio_buffer): the device's transfers end to end */
#define OZZY_PCM_RING_BYTES             (1024 * 1024)

/* Clock anchors are spaced by audio.timestamp_frames, a whole number of transfers.
 * OZZY_SETTING_IO_BUFFER_MAX selects the spacing for hosts that consume anchors. */
#define OZZY_TIMESTAMP_FRAMES           3200   /* the default spacing */

/* The isochronous families' plain rings, in frames (OZZY_DEVICE_FLAG_PLAIN_OUTPUT / _INPUT) */
#define OZZY_PLAIN_RING_FRAMES          10240

/* ozzy_audio_buffer.device_flags */
#define OZZY_DEVICE_FLAG_PLOYTEC_BULK   (1u << 0)    /* Ploytec 512-byte "bulk" output layout, else 482-byte "interrupt" */
#define OZZY_DEVICE_FLAG_PLAIN_OUTPUT   (1u << 1)    /* output_buffer is a ring of plain interleaved frames (see below) */
#define OZZY_DEVICE_FLAG_SPDIF_MIRROR   (1u << 2)    /* Ploytec bulk, 8 out: lanes 9/10 mirror a pair (ESU1808) */
#define OZZY_DEVICE_FLAG_PLAIN_INPUT    (1u << 3)    /* input_buffer starts with a ring of plain interleaved frames */

/* Isochronous IN transfers (feedback): 2 in flight, up to 32 packets of up to 256 bytes each */
#define OZZY_ISOC_IN_BUFFER_SIZE        (2 * 32 * 256)

#define OZZY_MIDI_RING_SIZE             1024
#define OZZY_MIDI_RING_MASK             (OZZY_MIDI_RING_SIZE - 1)

/* The device's clock: an anchor every audio.timestamp_frames (0: none, the host doesn't read
 * them), as a seqlock (odd sequence while the engine writes) */
struct ozzy_timestamp {
	ozzy_atomic_u32 sequence;
	ozzy_atomic_u64 sample_time;     /* frames */
	ozzy_atomic_u64 host_time;       /* the host's clock (ozzy_bus_ops.time) */
};

struct ozzy_audio_buffer {
	ozzy_atomic_bool hardware_present;
	ozzy_atomic_bool driver_ready;
	ozzy_atomic_u32 sample_rate;
	ozzy_atomic_u32 device_flags;          /* OZZY_DEVICE_FLAG_* */

	/* Frame format and ring layout, set by the engine at each start */
	uint32_t output_ring_frames;           /* frames in output_buffer (it wraps there) */
	uint32_t input_ring_frames;            /* frames in input_buffer */
	uint32_t timestamp_frames;             /* frames between clock anchors */
	uint32_t output_bytes_per_frame;       /* encoded bytes per output frame (e.g. 48) */
	uint32_t input_bytes_per_frame;        /* encoded bytes per input frame (e.g. 64) */
	uint32_t input_channels;               /* channels the device sends (e.g. 8, or 16 for the WZ4) */
	uint32_t output_channels;              /* channels the device receives (e.g. 8, or 2 for the WZ4) */
	uint32_t safety_offset_frames;         /* how far ahead of the clock the engine reads output */
	uint32_t input_safety_offset_frames;   /* how far behind the clock input is complete (0: as output) */

	/*
	 * Device settings (common/ozzy_settings.h) as the engine applied them at its last
	 * handshake, and what the device reported. Written before driver_ready at start, and before
	 * a settings change returns to the host.
	 */
	struct {
		uint32_t available;            /* bit (1 << enum ozzy_setting) per setting the device has */
		uint8_t value[OZZY_NUM_SETTINGS];
		uint8_t clock_locked;          /* locked to S/PDIF (clock source Automatic only) */
		uint32_t lock_rate;            /* Hz while clock_locked */
	} settings;

	struct ozzy_timestamp timestamp;

	uint8_t _pad1[64];
	OZZY_CACHE_ALIGN ozzy_atomic_u64 host_write_position;
	uint8_t _pad2[64];

	/*
	 * The PCM rings, which the transfers go to and come from as they are (the host writes and
	 * reads them by sample time, frame f at f % *_ring_frames):
	 * - Ploytec input: 512-byte sub-packets of 8 frames, end to end (frame f in sub-packet
	 *   f / 8, at (f % 8) * 64).
	 * - Ploytec output: sub-packets of 10 frames in the device's layout (512 bulk / 482
	 *   interrupt bytes, with their MIDI and control bytes), end to end.
	 * - With OZZY_DEVICE_FLAG_PLAIN_OUTPUT: plain frames of output_bytes_per_frame bytes
	 *   (interleaved S24_3LE), which isochronous transfers read directly.
	 * - With OZZY_DEVICE_FLAG_PLAIN_INPUT: plain frames of input_bytes_per_frame bytes at the
	 *   start of input_buffer, which the engine copies there from its isochronous transfers
	 *   (they land after the ring); frame f is the f-th the device sent since the start.
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
	struct ozzy_midi_buffer midi_in;       /* the engine fills it, clients drain it */

	/* MIDI IN transfers (as large as a bulk endpoint's packet) */
	uint8_t midi_in_usb_buffer[512];
	/* MIDI OUT transfers, for devices with a MIDI OUT endpoint */
	uint8_t midi_out_usb_buffer[64];
	/* Isochronous IN transfers (feedback endpoint) */
	uint8_t isoc_in_usb_buffer[OZZY_ISOC_IN_BUFFER_SIZE];

	OZZY_CACHE_ALIGN struct ozzy_audio_buffer audio;
};

/*
 * --- The timestamp seqlock ---
 * ozzy_timestamp_write (engine): the odd sequence is visible before the new values, the new
 * values before the even sequence. ozzy_timestamp_read: false if the writer kept it busy (try
 * again next time).
 */
static inline void ozzy_timestamp_write(volatile struct ozzy_timestamp *ts, uint64_t sample_time, uint64_t host_time)
{
	uint32_t seq = ts->sequence;

	ozzy_store_relaxed_u32(&ts->sequence, seq + 1);
	ozzy_fence_release();
	ozzy_store_relaxed_u64(&ts->sample_time, sample_time);
	ozzy_store_relaxed_u64(&ts->host_time, host_time);
	ozzy_store_release_u32(&ts->sequence, seq + 2);
}

static inline bool ozzy_timestamp_read(volatile struct ozzy_timestamp *ts, uint64_t *sample_time, uint64_t *host_time)
{
	unsigned int tries;

	for (tries = 0; tries < 100; tries++) {
		uint32_t seq = ozzy_load_acquire_u32(&ts->sequence);

		if (seq & 1)
			continue;
		*sample_time = ozzy_load_relaxed_u64(&ts->sample_time);
		*host_time = ozzy_load_relaxed_u64(&ts->host_time);
		ozzy_fence_acquire();
		if (ts->sequence == seq)
			return true;
	}
	return false;
}

/*
 * --- MIDI rings: the engine fills midi_in and drains midi_out, a client the other way round ---
 * ozzy_midi_ring_push appends what fits (returns how many), ozzy_midi_ring_pop takes up to
 * @max bytes (returns how many). Either side can call either one.
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

/*
 * struct ozzy_midi_queue - MIDI out waiting for room in midi_out, oldest first: a producer that
 * gets its bytes pushed (a host MIDI client) keeps them here, as the device takes
 * 2500 bytes/s and the ring holds 1 KB. @buf: the producer's storage, @mask + 1 bytes (a power
 * of two); @dropped: bytes lost because @buf was full too.
 */
struct ozzy_midi_queue {
	uint8_t *buf;
	uint32_t mask;
	uint32_t head, tail;
	uint32_t dropped;
};

/*
 * ozzy_midi_queue_write - Into @ring: what waits in @q first, then @data (@length bytes, none
 * for a flush); the rest waits in @q. Returns how many bytes reached the ring.
 */
static inline uint32_t ozzy_midi_queue_write(ozzy_midi_ring *ring, struct ozzy_midi_queue *q, const uint8_t *data,
					     uint32_t length)
{
	uint32_t moved = 0, n;

	while (q->head != q->tail) {
		uint32_t at = q->head & q->mask;
		uint32_t run = q->tail - q->head;

		if (run > q->mask + 1 - at)
			run = q->mask + 1 - at;   /* up to the end of @buf */
		n = ozzy_midi_ring_push(ring, q->buf + at, run);
		q->head += n;
		moved += n;
		if (n < run)
			break;
	}
	if (q->head == q->tail) {
		n = ozzy_midi_ring_push(ring, data, length);
		moved += n;
		data += n;
		length -= n;
	}
	for (; length; length--, data++) {
		if (q->tail - q->head > q->mask) {
			q->dropped += length;
			break;
		}
		q->buf[q->tail++ & q->mask] = *data;
	}
	return moved;
}

/* Whether bytes wait in @q */
static inline bool ozzy_midi_queue_pending(const struct ozzy_midi_queue *q)
{
	return q->head != q->tail;
}

#endif /* OZZY_SHARED_DATA_H */
