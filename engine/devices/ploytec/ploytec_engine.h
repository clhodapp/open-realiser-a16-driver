/* SPDX-License-Identifier: MIT */
/*
 * The Ploytec engine for the Realiser A16: bulk or interrupt PCM out, in which PCM and MIDI out
 * share every output packet; PCM in comes on a separate endpoint.
 *
 * Copyright (C) 2024 Marcel Bierling <marcel@hackerman.art>
 */

#ifndef PLOYTEC_ENGINE_H
#define PLOYTEC_ENGINE_H

#include "../../core/ozzy_engine.h"
#include "../../../common/devices/ploytec/ploytec_protocol.h"
#include "../../../common/devices/ploytec/ploytec_midi.h"
#include "../../../common/devices/ploytec/ploytec_codec.h"

#ifdef __cplusplus
extern "C" {
#endif

/*
 * struct ploytec_engine
 * @sample_rate:    what the device runs at
 * @default_rate:   what start asks for without a request: 48 kHz, the A16's SVS processing rate
 * @running:        completions resubmit (cleared by stop and suspend)
 * @encode/@decode: frame codecs for the model's channel counts
 */

struct ploytec_engine {
	struct ozzy_engine base;
	enum ploytec_out_layout out_layout;
	uint32_t sample_rate;
	uint32_t default_rate;
	volatile bool running;
	bool streamed;         /* has streamed since attach: a start is a restart */
	struct ploytec_midi midi;
	ploytec_frame_fn encode;
	ploytec_frame_fn decode;
	uint32_t out_frames;   /* frames per PCM transfer this run (OZZY_SETTING_PCM_OUT_FRAMES at start) */
	uint32_t in_frames;
	uint32_t out_queue;    /* PCM transfers in flight this run (OZZY_SETTING_PCM_OUT_QUEUE at start) */
	uint32_t in_queue;
};

void ploytec_engine_init(struct ploytec_engine *pe, const struct ozzy_device *model);

#ifdef __cplusplus
}
#endif

#endif /* PLOYTEC_ENGINE_H */
