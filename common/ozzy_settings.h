/* SPDX-License-Identifier: MIT */
/*
 * Ozzy Device Settings: the PCM transfer settings of the Ploytec engine
 *
 * How large the PCM transfers are and how many are in flight, each way. The engine takes them
 * at its next start, so a change restarts streaming (ozzy_engine_restart).
 *
 * Pure C with no platform dependencies -- usable in both kernel space
 * and userspace on any platform.
 *
 * Copyright (C) 2024 Marcel Bierling <marcel@hackerman.art>
 */

#ifndef OZZY_SETTINGS_H
#define OZZY_SETTINGS_H

#if defined(__KERNEL__) && defined(__linux__)
#include <linux/types.h>
#else
#include <stdint.h>
#include <stdbool.h>
#endif

#ifdef __cplusplus
extern "C" {
#endif

enum ozzy_setting {
	OZZY_SETTING_PCM_OUT_FRAMES,  /* frames per PCM out transfer */
	OZZY_SETTING_PCM_IN_FRAMES,   /* frames per PCM in transfer */
	OZZY_SETTING_PCM_OUT_QUEUE,   /* PCM out transfers in flight */
	OZZY_SETTING_PCM_IN_QUEUE,    /* PCM in transfers in flight */
	OZZY_NUM_SETTINGS
};

/*
 * OZZY_SETTING_PCM_OUT_FRAMES / _IN_FRAMES values: how many of the device's sub-packets one PCM
 * transfer carries, so how many frames (value + 1 sub-packets: 10 frames each out, 8 in), so
 * how far ahead of the device the driver works (transfers queued x frames: the latency). Only
 * whole packets of the endpoint work (ozzy_pcm_step: on the 964-byte interrupt endpoint 20
 * frames out, on the 1024-byte one 16 in). .claude/re/ploytec_bulk_devices.md. Ozzy offers up
 * to 200 frames, the official driver's size. The default is 80 frames each way.
 */
#define OZZY_NUM_PCM_OUT_FRAMES 20    /* 10 ... 200 */
#define OZZY_NUM_PCM_IN_FRAMES  25    /* 8 ... 200 */

/*
 * OZZY_SETTING_PCM_OUT_QUEUE / _IN_QUEUE values: how many PCM transfers the driver keeps in
 * flight, value + 1 (2-4; value 0, one, isn't offered and runs as two). With the transfer size it
 * is the latency on USB: queue x frames. The device paces the stream itself (it refuses OUT
 * packets until it has room), so the device's buffer covers the gap while the next transfer is
 * being submitted (.claude/re/ploytec_bulk_devices.md). The official driver never runs one in
 * flight. Out, each transfer queued is latency (a frame waits behind them all); in, the frames
 * are there once their one transfer completes, and the others queued are margin, buffers the
 * device can fill while the host is late. The default is 4.
 */
#define OZZY_NUM_PCM_QUEUES 4

#define OZZY_PCM_QUEUE_MIN 1           /* the lowest value offered: two in flight */

static inline uint32_t ozzy_pcm_queue(unsigned int value)
{
	if (value < OZZY_PCM_QUEUE_MIN)
		value = OZZY_PCM_QUEUE_MIN;
	return value < OZZY_NUM_PCM_QUEUES ? value + 1 : OZZY_NUM_PCM_QUEUES;
}

/* The frames of value @value of the out (@in false) or in setting; out of range: the default */
static inline uint32_t ozzy_pcm_frames(bool in, unsigned int value)
{
	if (value >= (in ? OZZY_NUM_PCM_IN_FRAMES : OZZY_NUM_PCM_OUT_FRAMES) + 0u)
		value = in ? 9 : 7;
	return (value + 1) * (in ? 8 : 10);
}

/*
 * ozzy_pcm_step - What a PCM transfer's frames must be a multiple of, on an endpoint of
 * @transfer_type (bmAttributes & 3) with @max_packet bytes per service interval and frames of
 * @frame_bytes: the frames in one whole packet. On bulk and interrupt the device takes packets
 * whole, both ways (DB4 fw 1.4.1, .claude/re/ploytec_bulk_devices.md): in, it fills 1024-byte
 * packets of 16 frames, and a request for 8 is overrun (babble); out, it reads every packet as a
 * 964-byte one of 20 frames, and a 10-frame transfer (one 482-byte half) plays garbage.
 * 0: no step (not bulk or interrupt, or not known).
 */
static inline uint32_t ozzy_pcm_step(uint8_t transfer_type, uint32_t max_packet, uint32_t frame_bytes)
{
	if ((transfer_type != 2 && transfer_type != 3) || !frame_bytes)
		return 0;
	return max_packet / frame_bytes;
}

/* Whether a device whose PCM steps are @out_step / @in_step (ozzy_pcm_step) can take value
 * @value of setting @id: the transfer sizes depend on the endpoints; one transfer in flight is
 * never offered (ozzy_pcm_queue) */
static inline bool ozzy_setting_value_fits(unsigned int id, unsigned int value, uint32_t out_step, uint32_t in_step)
{
	if (id == OZZY_SETTING_PCM_OUT_QUEUE || id == OZZY_SETTING_PCM_IN_QUEUE)
		return value >= OZZY_PCM_QUEUE_MIN;
	if (id == OZZY_SETTING_PCM_OUT_FRAMES)
		return !out_step || ozzy_pcm_frames(false, value) % out_step == 0;
	if (id == OZZY_SETTING_PCM_IN_FRAMES)
		return !in_step || ozzy_pcm_frames(true, value) % in_step == 0;
	return true;
}

/*
 * struct ozzy_setting_desc - One setting: its name and values.
 * @name:        the setting's name
 * @value_names: names indexed by value
 * @num_values:  how many values
 * @def:         the value the engine starts with
 */
struct ozzy_setting_desc {
	const char *name;
	const char *const *value_names;
	uint8_t num_values;
	uint8_t def;
};

static const char *const ozzy_pcm_out_frames_names[OZZY_NUM_PCM_OUT_FRAMES] = {
	"10 frames", "20 frames", "30 frames", "40 frames", "50 frames", "60 frames",
	"70 frames", "80 frames", "90 frames", "100 frames", "110 frames", "120 frames",
	"130 frames", "140 frames", "150 frames", "160 frames", "170 frames", "180 frames",
	"190 frames", "200 frames" };
static const char *const ozzy_pcm_in_frames_names[OZZY_NUM_PCM_IN_FRAMES] = {
	"8 frames", "16 frames", "24 frames", "32 frames", "40 frames", "48 frames",
	"56 frames", "64 frames", "72 frames", "80 frames", "88 frames", "96 frames",
	"104 frames", "112 frames", "120 frames", "128 frames", "136 frames", "144 frames",
	"152 frames", "160 frames", "168 frames", "176 frames", "184 frames", "192 frames",
	"200 frames" };

static const char *const ozzy_pcm_queue_names[OZZY_NUM_PCM_QUEUES] = {
	"1 transfer", "2 transfers", "3 transfers", "4 transfers" };

static const struct ozzy_setting_desc ozzy_setting_descs[OZZY_NUM_SETTINGS] = {
	/* Positional initializer follows struct field order. */
	{ "PCM Playback Transfer", ozzy_pcm_out_frames_names, OZZY_NUM_PCM_OUT_FRAMES, 7 },
	{ "PCM Capture Transfer", ozzy_pcm_in_frames_names, OZZY_NUM_PCM_IN_FRAMES, 9 },
	{ "PCM Playback Queue", ozzy_pcm_queue_names, OZZY_NUM_PCM_QUEUES, OZZY_NUM_PCM_QUEUES - 1 },
	{ "PCM Capture Queue", ozzy_pcm_queue_names, OZZY_NUM_PCM_QUEUES, OZZY_NUM_PCM_QUEUES - 1 },
};

/*
 * struct ozzy_settings - What the user chose, one value per enum ozzy_setting.
 * Platforms keep it across device resets and engine restarts.
 */
struct ozzy_settings {
	uint8_t value[OZZY_NUM_SETTINGS];
};

static inline void ozzy_settings_defaults(struct ozzy_settings *s)
{
	unsigned int i;

	for (i = 0; i < OZZY_NUM_SETTINGS; i++)
		s->value[i] = ozzy_setting_descs[i].def;
}

#ifdef __cplusplus
}
#endif

#endif /* OZZY_SETTINGS_H */
