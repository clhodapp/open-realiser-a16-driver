/* SPDX-License-Identifier: MIT */
/*
 * Ploytec MIDI Framing
 *
 * MIDI out travels in frames the device times by its sample clock, so how
 * much fits depends on the sample rate; struct ploytec_midi is set up once
 * per stream for it.
 *
 * MIDI out is embedded in the PCM output packets. Every output sub-packet
 * (10 frames) has a MIDI slot followed by a control byte. A slot may carry
 * one byte, but filling every slot would be 9600 bytes/s at 96 kHz, three
 * times what a MIDI cable carries (31250 baud, 10 bits per byte = 3125
 * bytes/s). The official driver paces the slots so that 2500 bytes/s (80%)
 * get through at any rate (USBMidiPattern::initForBulk builds a table with an
 * error accumulator; PGDevice::rtsProcessBulkOut consults it per sub-packet).
 * Ozzy runs the same accumulator without the table. Over any stretch of time
 * T it sends at most 2500 * T + 1 bytes, so it is never more than one byte
 * ahead of the cable, and only within 1.6 ms (where 625 * T < 1).
 *
 * The control byte after each slot is 0xFF, which the official driver sends
 * when its control queue is empty (otherwise control | 0x18).
 *
 * Copyright (C) 2024 Marcel Bierling <marcel@hackerman.art>
 */

#ifndef PLOYTEC_MIDI_H
#define PLOYTEC_MIDI_H

#include "ploytec_protocol.h"

#ifdef __cplusplus
extern "C" {
#endif

/* MIDI out pace: 80% of 31250 baud at 10 bits per byte (3125 bytes/s) */
#define PLOYTEC_MIDI_OUT_BYTES_PER_SEC  2500

/*
 * struct ploytec_midi - MIDI out pacing for one stream at one sample rate.
 * Set up with ploytec_midi_init() whenever the rate changes.
 */
struct ploytec_midi {
	uint32_t out_step;   /* added per sub-packet: bytes/s * frames per sub-packet */
	uint32_t out_rate;   /* sample rate: a slot opens each time the sum passes it */
	uint32_t out_acc;    /* pacing accumulator, always < out_rate */
};

/*
 * ploytec_midi_init - Set up MIDI out pacing for a stream at @rate Hz.
 */
static inline void ploytec_midi_init(struct ploytec_midi *m, uint32_t rate)
{
	m->out_step = PLOYTEC_MIDI_OUT_BYTES_PER_SEC * PLOYTEC_FRAMES_PER_OUT_SUBPKT;
	m->out_rate = rate;
	m->out_acc = 0;
}

/*
 * ploytec_midi_out_slots - Pace the next output transfer, @subpkts sub-packets (at most
 * PLOYTEC_MAX_OUT_SUBPKTS).
 * Sets bit k of *@mask if sub-packet k may carry a MIDI byte and returns how
 * many may. Advances the pace whether or not there is data, so unused slots
 * don't add up to a burst later.
 */
static inline uint32_t ploytec_midi_out_slots(struct ploytec_midi *m, uint32_t subpkts, uint32_t *mask)
{
	uint32_t bits = 0, count = 0, k, open;

	for (k = 0; k < subpkts && k < PLOYTEC_MAX_OUT_SUBPKTS; k++) {
		m->out_acc += m->out_step;
		open = m->out_acc >= m->out_rate;
		m->out_acc -= open * m->out_rate;
		bits |= open << k;
		count += open;
	}
	*mask = bits;
	return count;
}

/*
 * ploytec_midi_out_place - Write the MIDI slots of an output transfer of @subpkts sub-packets
 * starting at @pkt (a sub-packet boundary).
 * @bytes holds one byte per bit set in @mask (PLOYTEC_MIDI_IDLE_BYTE where
 * there was no data); they go into those sub-packets in order. All other
 * slots get PLOYTEC_MIDI_IDLE_BYTE. The control bytes stay as
 * ploytec_out_init_packet() wrote them.
 */
static inline void ploytec_midi_out_place(enum ploytec_out_layout layout, uint8_t *pkt, uint32_t subpkts,
					  uint32_t mask, const uint8_t bytes[PLOYTEC_MAX_OUT_SUBPKTS])
{
	uint32_t k, i = 0, open;

	for (k = 0; k < subpkts && k < PLOYTEC_MAX_OUT_SUBPKTS; k++) {
		open = (mask >> k) & 1;
		pkt[ploytec_out_midi_offset(layout, k)] = open ? bytes[i] : PLOYTEC_MIDI_IDLE_BYTE;
		i += open;
	}
}

#ifdef __cplusplus
}
#endif

#endif /* PLOYTEC_MIDI_H */
