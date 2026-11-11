/* SPDX-License-Identifier: MIT */
/* Included after make_stream: standard SPA Device profile control, PW loop only. */
static void device_info(struct live *l, struct spa_device_info *info) {
    static const struct spa_dict_item items[] = {{"device.name", "a16"},
                                                 {"device.description", "Smyth Realiser A16 USB"},
                                                 {"device.api", "a16"},
                                                 {"media.class", "Audio/Device"},
                                                 {"device.icon-name", "audio-card"}};
    static const struct spa_dict props = SPA_DICT_INIT_ARRAY(items);
    *info = SPA_DEVICE_INFO_INIT();
    info->change_mask = SPA_DEVICE_CHANGE_MASK_PROPS | SPA_DEVICE_CHANGE_MASK_PARAMS;
    info->props = &props;
    info->params = l->device_params;
    info->n_params = 2;
}
static int device_add_listener(void *data, struct spa_hook *listener, const struct spa_device_events *events,
                               void *userdata) {
    struct live *l = data;
    spa_hook_list_append(&l->device_hooks, listener, events, userdata);
    struct spa_device_info info;
    device_info(l, &info);
    if (events->info)
        events->info(userdata, &info);
    return 0;
}
static int device_sync(void *data, int seq) {
    struct live *l = data;
    spa_device_emit_result(&l->device_hooks, seq, 0, 0, NULL);
    return 0;
}
static int device_enum(void *data, int seq, uint32_t id, uint32_t index, uint32_t max,
                       const struct spa_pod *filter) {
    struct live *l = data;
    if (id != SPA_PARAM_EnumProfile && id != SPA_PARAM_Profile)
        return -ENOENT;
    uint32_t count = id == SPA_PARAM_EnumProfile ? A16_PROFILE_COUNT : 1;
    for (uint32_t emitted = 0; index < count && emitted < max; index++) {
        int selected = id == SPA_PARAM_Profile ? l->profile : (int)index;
        const struct a16_profile *p = &l->effective[selected].profile;
        uint8_t storage[2048], filtered[2048];
        struct spa_pod_builder b = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
        struct spa_pod_frame object, classes;
        spa_pod_builder_push_object(&b, &object, SPA_TYPE_OBJECT_ParamProfile, id);
        spa_pod_builder_add(&b, SPA_PARAM_PROFILE_index, SPA_POD_Int(selected), SPA_PARAM_PROFILE_name,
                            SPA_POD_String(p->name), SPA_PARAM_PROFILE_description,
                            SPA_POD_String(p->description), SPA_PARAM_PROFILE_priority,
                            SPA_POD_Int(selected == l->profile ? 100 : 50), SPA_PARAM_PROFILE_available,
                            SPA_POD_Id(p->available ? SPA_PARAM_AVAILABILITY_yes : SPA_PARAM_AVAILABILITY_no),
                            0);
        spa_pod_builder_prop(&b, SPA_PARAM_PROFILE_classes, 0);
        spa_pod_builder_push_struct(&b, &classes);
        spa_pod_builder_int(&b, p->available ? 2 : 0);
        if (p->available) {
            uint32_t sink = 0, source = 1;
            spa_pod_builder_add_struct(&b, SPA_POD_String("Audio/Sink"), SPA_POD_Int(1),
                                       SPA_POD_String("card.profile.devices"),
                                       SPA_POD_Array(sizeof(uint32_t), SPA_TYPE_Int, 1, &sink));
            spa_pod_builder_add_struct(&b, SPA_POD_String("Audio/Source"), SPA_POD_Int(1),
                                       SPA_POD_String("card.profile.devices"),
                                       SPA_POD_Array(sizeof(uint32_t), SPA_TYPE_Int, 1, &source));
        }
        spa_pod_builder_pop(&b, &classes);
        if (id == SPA_PARAM_Profile)
            spa_pod_builder_add(&b, SPA_PARAM_PROFILE_save, SPA_POD_Bool(l->profile_save), 0);
        struct spa_pod *param = spa_pod_builder_pop(&b, &object), *result;
        struct spa_pod_builder fb = SPA_POD_BUILDER_INIT(filtered, sizeof(filtered));
        if (spa_pod_filter(&fb, &result, param, filter) < 0)
            continue;
        struct spa_result_device_params reply = {
            .id = id, .index = index, .next = index + 1, .param = result};
        spa_device_emit_result(&l->device_hooks, seq, 0, SPA_RESULT_TYPE_DEVICE_PARAMS, &reply);
        emitted++;
    }
    return 0;
}
static int device_set(void *data, uint32_t id, uint32_t flags, const struct spa_pod *param) {
    (void)flags;
    struct live *l = data;
    int32_t selected = -1;
    bool save = false;
    if (id != SPA_PARAM_Profile)
        return -ENOENT;
    if (!param ||
        spa_pod_parse_object(param, SPA_TYPE_OBJECT_ParamProfile, NULL, SPA_PARAM_PROFILE_index,
                             SPA_POD_Int(&selected), SPA_PARAM_PROFILE_save, SPA_POD_OPT_Bool(&save)) < 0 ||
        selected < 0 || (unsigned int)selected >= A16_PROFILE_COUNT)
        return -EINVAL;
    if (!l->effective[selected].profile.available)
        return -ENOTSUP;
    if (selected != l->profile) {
        /* stream destroy synchronizes its data callback before endpoint reuse.
         * Only USB consumes/resets the playback FIFO, including on a mode change. */
        if (l->sink.stream) {
            pw_stream_destroy(l->sink.stream);
            l->sink.stream = NULL;
        }
        atomic_store(&l->sink.active, false);
        atomic_store(&l->sink.reset, true);
        l->profile = selected;
        if (l->device_id != SPA_ID_INVALID && !make_stream(l, &l->sink, false)) {
            atomic_store(&l->failed, true);
            return -EIO;
        }
        fprintf(stderr, "A16 profile: %s (%u graph channels, 16 USB channels)\n",
                l->effective[selected].profile.name, l->effective[selected].profile.channels);
    }
    l->profile_save = save;
    l->device_params[1].flags ^= SPA_PARAM_INFO_SERIAL;
    struct spa_device_info info;
    device_info(l, &info);
    info.change_mask = SPA_DEVICE_CHANGE_MASK_PARAMS;
    spa_device_emit_info(&l->device_hooks, &info);
    return 0;
}
static const struct spa_device_methods device_methods = {
    SPA_VERSION_DEVICE_METHODS, .add_listener = device_add_listener, .sync = device_sync,
    .enum_params = device_enum, .set_param = device_set};
static void device_bound(void *data, uint32_t id) {
    struct live *l = data;
    l->device_id = id;
    if (!make_stream(l, &l->sink, false) || !make_stream(l, &l->source, true))
        atomic_store(&l->failed, true);
    fprintf(stderr, "A16 card published: id=%u profile=%s\n", id, l->effective[l->profile].profile.name);
}
static const struct pw_proxy_events proxy_events = {PW_VERSION_PROXY_EVENTS, .bound = device_bound};
static bool export_device(struct live *l) {
    spa_hook_list_init(&l->device_hooks);
    l->device.iface = SPA_INTERFACE_INIT(SPA_TYPE_INTERFACE_Device, SPA_VERSION_DEVICE, &device_methods, l);
    l->device_params[0] = SPA_PARAM_INFO(SPA_PARAM_EnumProfile, SPA_PARAM_INFO_READ);
    l->device_params[1] = SPA_PARAM_INFO(SPA_PARAM_Profile, SPA_PARAM_INFO_READWRITE);
    l->device_proxy = pw_core_export(l->core, SPA_TYPE_INTERFACE_Device, NULL, &l->device, 0);
    if (!l->device_proxy)
        return false;
    pw_proxy_add_listener(l->device_proxy, &l->proxy_listener, &proxy_events, l);
    return true;
}
