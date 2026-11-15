/* SPDX-License-Identifier: MIT */
/*
 * Golden trace: what the engine does on the bus for these devices and paths:
 *   Xone:DB4     interrupt layout, PCM rings attached at packet 20, detached at 300
 *   ESI ESU1808  bulk layout, the S/PDIF mirror (pair 2), the same rings
 *   ESI U168 XT  the interrupt layout with the ESI selector on Internal and its C-port writes
 * with MIDI going out every 5 completions. Every submission,
 * control request and progress report is printed, then hashes of the shared memory and the
 * rings. Compared with golden/trace.txt by run.sh; after an intended change: make golden.
 *
 * Copyright (C) 2024 Marcel Bierling <marcel@hackerman.art>
 */

#include "fake_bus.h"
#include <stdlib.h>
#include <string.h>

struct fill_state {
	uint32_t counter[256];
};

static void fill(struct fake_xfer *x, void *arg)
{
	struct fill_state *st = arg;
	uint32_t i;

	for (i = 0; i < x->size; i++)
		x->buf[i] = (uint8_t)(st->counter[x->pipe]++ * 37 + 11);
}

static uint64_t hash(const void *p, size_t n)
{
	const uint8_t *b = p;
	uint64_t h = 1469598103934665603ULL;

	while (n--) {
		h ^= *b++;
		h *= 1099511628211ULL;
	}
	return h;
}

static void run(uint16_t vid, uint16_t pid, int setting, int value, const uint8_t (*eps)[6])
{
	const struct ozzy_device *model = ozzy_device_lookup(vid, pid);
	static struct ozzy_shared_memory shm;
	static uint8_t out_ring[960 * 16 * 3], in_ring[960 * 16 * 3];
	struct ozzy_pcm_ring ro = { out_ring, 960, model->out_channels, 0 };
	struct ozzy_pcm_ring ri = { in_ring, 960, model->in_channels, 0 };
	struct fill_state st;
	struct fake_bus fb;
	struct fake_xfer x;
	struct ozzy_engine *e;
	unsigned int done = 0, k;

	memset(&shm, 0, sizeof(shm));
	memset(&st, 0, sizeof(st));
	memset(in_ring, 0, sizeof(in_ring));
	for (k = 0; k < sizeof(out_ring); k++)
		out_ring[k] = (uint8_t)(k * 13 + 5);
	fake_bus_init(&fb, &shm, stdout);

	printf("== %s\n", model->name);
	e = ozzy_engine_init(malloc(ozzy_engine_size(model)), model);
	for (k = 0; eps[k][0] != 0xFF; k++)
		ozzy_engine_set_endpoint(e, (enum ozzy_pipe_role)eps[k][0], eps[k][1], eps[k][2],
					 (uint16_t)(eps[k][3] | eps[k][4] << 8), eps[k][5]);
	ozzy_engine_attach(e, &fb.bus, &shm);
	if (setting >= 0)
		ozzy_engine_set_setting(e, (unsigned int)setting, (uint8_t)value);
	printf("start %d\n", ozzy_engine_start(e));

	/* 600 completions */
	while (done < 600 && fake_bus_pop(&fb, &x)) {
		done++;
		if (done == 20) {
			ozzy_engine_set_pcm_ring(e, OZZY_PCM_OUT, &ro);
			ozzy_engine_set_pcm_ring(e, OZZY_PCM_IN, &ri);
		}
		if (done == 300) {
			ozzy_engine_set_pcm_ring(e, OZZY_PCM_OUT, NULL);
			ozzy_engine_set_pcm_ring(e, OZZY_PCM_IN, NULL);
			printf("detached\n");
		}
		if (done % 5 == 0) {
			uint8_t note[3] = { 0x90, (uint8_t)(done & 0x7F), 0x7F };

			ozzy_midi_ring_push(&shm.midi_out, note, 3);
		}
		fake_bus_complete(&fb, e, &x, fill, &st);
	}
	ozzy_engine_stop(e);

	printf("shm %016llx out_ring %016llx in_ring %016llx\n", (unsigned long long)hash(&shm, sizeof(shm)),
	       (unsigned long long)hash(out_ring, sizeof(out_ring)), (unsigned long long)hash(in_ring, sizeof(in_ring)));
	free(e);
}

int main(void)
{
	/* role, address, bmAttributes & 3, wMaxPacketSize (lo, hi), bInterval */
	static const uint8_t db4[][6] = { { 0, 0x05, 3, 0xC4, 0x03, 4 }, { 1, 0x86, 3, 0x00, 0x04, 4 }, { 0xFF } };
	static const uint8_t esu[][6] = { { 0, 0x05, 2, 0x00, 0x02, 4 }, { 1, 0x86, 2, 0x00, 0x02, 4 }, { 0xFF } };

	run(0x0A4A, 0xFFDB, -1, 0, db4);
	run(0x0A92, 0x0111, OZZY_SETTING_SPDIF_MIRROR, 2, esu);
	run(0x2573, 0x0012, OZZY_SETTING_ESI_CLOCK, OZZY_ESI_CLOCK_INTERNAL, db4);
	return 0;
}
