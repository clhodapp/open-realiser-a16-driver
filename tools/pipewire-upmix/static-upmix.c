/* SPDX-License-Identifier: MIT
 * Synchronous PipeWire SPA DSP: stereo f32le stdin -> Custom #2 16ch f32le stdout.
 * No server connection, clock adaptation, realtime thread or USB access.
 * Buffer setup follows PipeWire's MIT-licensed test-audioconvert.c (Wim Taymans).
 */
#include <errno.h>
#include <math.h>
#include <pipewire/impl.h>
#include <spa/filter-graph/filter-graph.h>
#include <spa/node/command.h>
#include <spa/node/io.h>
#include <spa/param/audio/format-utils.h>
#include <spa/param/port-config.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define BLOCK 512
#define CHANNELS 16
#define CHECK(call)                                                                                          \
    do {                                                                                                     \
        int res_ = (call);                                                                                   \
        if (res_ < 0) {                                                                                      \
            fprintf(stderr, "%s: %s\n", #call, strerror(-res_));                                             \
            exit(1);                                                                                         \
        }                                                                                                    \
    } while (0)
struct buffers {
    struct spa_buffer buffer;
    struct spa_data data[8];
    struct spa_chunk chunk[8];
    struct spa_io_buffers io;
};
static float input[2][BLOCK], bed[8][BLOCK], height[12][BLOCK];
static float interleaved[BLOCK * 2], output[BLOCK * CHANNELS];
static struct pw_properties *load_props(const char *path) {
    FILE *f = fopen(path, "rb");
    if (!f) {
        perror(path);
        exit(1);
    }
    if (fseek(f, 0, SEEK_END) || ftell(f) > 1024 * 1024) {
        fprintf(stderr, "Invalid config size\n");
        exit(1);
    }
    long n = ftell(f);
    rewind(f);
    char *s = calloc(1, (size_t)n + 1);
    if (!s)
        exit(1);
    if (fread(s, 1, (size_t)n, f) != (size_t)n) {
        fprintf(stderr, "Config read failed\n");
        exit(1);
    }
    fclose(f);
    struct pw_properties *p = pw_properties_new_string(s);
    free(s);
    if (!p) {
        fprintf(stderr, "Invalid properties: %s\n", path);
        exit(1);
    }
    return p;
}
static void format(struct spa_node *node, enum spa_direction dir, unsigned channels,
                   const uint32_t *positions) {
    uint8_t mem[4096];
    struct spa_pod_builder b = SPA_POD_BUILDER_INIT(mem, sizeof(mem));
    struct spa_audio_info_raw info = {.format = SPA_AUDIO_FORMAT_F32P, .rate = 48000, .channels = channels};
    memcpy(info.position, positions, channels * sizeof(*positions));
    struct spa_pod *f = spa_format_audio_raw_build(&b, SPA_PARAM_Format, &info);
    struct spa_pod *config = spa_pod_builder_add_object(
        &b, SPA_TYPE_OBJECT_ParamPortConfig, SPA_PARAM_PortConfig, SPA_PARAM_PORT_CONFIG_direction,
        SPA_POD_Id(dir), SPA_PARAM_PORT_CONFIG_mode, SPA_POD_Id(SPA_PARAM_PORT_CONFIG_MODE_convert));
    CHECK(spa_node_set_param(node, SPA_PARAM_PortConfig, 0, config));
    CHECK(spa_node_port_set_param(node, dir, 0, SPA_PARAM_Format, 0, f));
}
static void buffers(struct spa_node *node, struct buffers *b, enum spa_direction dir, unsigned channels,
                    float *planes[]) {
    memset(b, 0, sizeof(*b));
    b->buffer.n_datas = channels;
    b->buffer.datas = b->data;
    for (unsigned i = 0; i < channels; i++) {
        b->data[i] = (struct spa_data){.type = SPA_DATA_MemPtr,
                                       .flags = SPA_DATA_FLAG_READWRITE,
                                       .fd = -1,
                                       .maxsize = BLOCK * sizeof(float),
                                       .data = planes[i],
                                       .chunk = &b->chunk[i]};
        b->chunk[i].stride = sizeof(float);
    }
    struct spa_buffer *buf = &b->buffer;
    CHECK(spa_node_port_use_buffers(node, dir, 0, 0, &buf, 1));
    b->io = (struct spa_io_buffers){.status = SPA_STATUS_NEED_DATA, .buffer_id = SPA_ID_INVALID};
    CHECK(spa_node_port_set_io(node, dir, 0, SPA_IO_Buffers, &b->io, sizeof(b->io)));
}
int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr,
                "Usage: a16-static-dsp PSD-PROPS.json HEIGHT-PROPS.json < stereo.f32 > custom2.f32\n");
        return 2;
    }
    pw_init(&argc, &argv);
    struct pw_main_loop *loop = pw_main_loop_new(NULL);
    struct pw_context *ctx = pw_context_new(pw_main_loop_get_loop(loop), NULL, 0);
    if (!ctx) {
        perror("pw_context_new");
        return 1;
    }
    struct pw_properties *mix_props = load_props(argv[1]), *graph_props = load_props(argv[2]);
    pw_properties_set(mix_props, "clock.quantum-limit", "8192");
    struct spa_handle *mix_handle = pw_context_load_spa_handle(ctx, "audio.convert", &mix_props->dict);
    if (!mix_handle) {
        perror("load audioconvert");
        return 1;
    }
    struct spa_node *mix = NULL;
    CHECK(spa_handle_get_interface(mix_handle, SPA_TYPE_INTERFACE_Node, (void **)&mix));
    uint32_t positions[] = {SPA_AUDIO_CHANNEL_FL,  SPA_AUDIO_CHANNEL_FR, SPA_AUDIO_CHANNEL_FC,
                            SPA_AUDIO_CHANNEL_LFE, SPA_AUDIO_CHANNEL_SL, SPA_AUDIO_CHANNEL_SR,
                            SPA_AUDIO_CHANNEL_RL,  SPA_AUDIO_CHANNEL_RR};
    format(mix, SPA_DIRECTION_INPUT, 2, positions);
    format(mix, SPA_DIRECTION_OUTPUT, 8, positions);
    struct buffers inbuf, outbuf;
    float *inptr[2] = {input[0], input[1]}, *bedptr[8];
    for (unsigned i = 0; i < 8; i++)
        bedptr[i] = bed[i];
    buffers(mix, &inbuf, SPA_DIRECTION_INPUT, 2, inptr);
    buffers(mix, &outbuf, SPA_DIRECTION_OUTPUT, 8, bedptr);
    struct spa_command start = SPA_NODE_COMMAND_INIT(SPA_NODE_COMMAND_Start);
    CHECK(spa_node_send_command(mix, &start));
    pw_properties_set(graph_props, "library.name", "filter-graph/libspa-filter-graph");
    pw_properties_set(graph_props, "clock.quantum-limit", "8192");
    struct spa_handle *graph_handle = pw_context_load_spa_handle(ctx, "filter.graph", &graph_props->dict);
    if (!graph_handle) {
        perror("load filter graph");
        return 1;
    }
    struct spa_filter_graph *graph = NULL;
    CHECK(spa_handle_get_interface(graph_handle, SPA_TYPE_INTERFACE_FilterGraph, (void **)&graph));
    CHECK(spa_filter_graph_activate(graph, &SPA_DICT_ITEMS(SPA_DICT_ITEM("audio.rate", "48000"))));
    void *hout[12];
    const void *bin[8];
    for (unsigned i = 0; i < 8; i++)
        bin[i] = bed[i];
    for (unsigned i = 0; i < 12; i++)
        hout[i] = height[i];
    const unsigned usb[] = {0, 1, 2, 3, 6, 7, 8, 9, 11, 12, 13, 14};
    uint64_t frames = 0;
    double peaks[16] = {0};
    for (;;) {
        size_t bytes = fread(interleaved, 1, sizeof(interleaved), stdin);
        if (bytes % (2 * sizeof(float))) { fprintf(stderr, "Truncated stereo frame\n"); return 1; }
        size_t count = bytes / (2 * sizeof(float));
        if (!count) {
            if (ferror(stdin)) {
                perror("stdin");
                return 1;
            }
            break;
        }
        memset(input, 0, sizeof(input));
        for (size_t i = 0; i < count; i++)
            for (unsigned c = 0; c < 2; c++)
                input[c][i] = interleaved[i * 2 + c];
        for (unsigned c = 0; c < 2; c++) {
            inbuf.chunk[c].size = BLOCK * sizeof(float);
            inbuf.chunk[c].offset = 0;
        }
        inbuf.io.status = SPA_STATUS_HAVE_DATA;
        inbuf.io.buffer_id = 0;
        outbuf.io.status = SPA_STATUS_NEED_DATA;
        CHECK(spa_node_process(mix));
        if (outbuf.io.status != SPA_STATUS_HAVE_DATA || outbuf.chunk[0].size != BLOCK * sizeof(float)) {
            fprintf(stderr, "Short DSP output at frame %llu: status %d, bytes %u, expected %zu\n",
                    (unsigned long long)frames, outbuf.io.status, outbuf.chunk[0].size,
                    BLOCK * sizeof(float));
            return 1;
        }
        CHECK(spa_filter_graph_process(graph, bin, hout, BLOCK));
        memset(output, 0, count * 16 * sizeof(float));
        for (size_t i = 0; i < count; i++)
            for (unsigned c = 0; c < 12; c++) {
                float v = height[c][i];
                if (!isfinite(v)) {
                    fprintf(stderr, "Nonfinite output\n");
                    return 1;
                }
                output[i * 16 + usb[c]] = v;
                if (fabs(v) > peaks[usb[c]])
                    peaks[usb[c]] = fabs(v);
            }
        if (fwrite(output, 16 * sizeof(float), count, stdout) != count) {
            perror("stdout");
            return 1;
        }
        frames += count;
    }
    if (fflush(stdout)) {
        perror("fflush");
        return 1;
    }
    fprintf(stderr, "frames=%llu peaks=", (unsigned long long)frames);
    for (unsigned c = 0; c < 16; c++)
        fprintf(stderr, "%s%.9g", c ? "," : "", peaks[c]);
    fputc('\n', stderr);
    spa_filter_graph_deactivate(graph);
    pw_unload_spa_handle(graph_handle);
    pw_unload_spa_handle(mix_handle);
    pw_properties_free(mix_props);
    pw_properties_free(graph_props);
    pw_context_destroy(ctx);
    pw_main_loop_destroy(loop);
    pw_deinit();
    return 0;
}
