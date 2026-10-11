/* Deterministic component regression for the pinned private PipeWire fixture.
 * Real pw_impl_node/link/work-queue code; controlled SPA endpoints hold replies.
 * This does not simulate the client-node wire protocol or compositor acceptance.
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <unistd.h>

#include <pipewire/impl.h>
#include <spa/node/utils.h>
#include <spa/param/video/format-utils.h>
#include <spa/pod/filter.h>
#include <spa/utils/result.h>

/* Exported in 1.0.5, declared in its private.h, not the installed API. */
int pw_impl_node_set_state(struct pw_impl_node *node, enum pw_node_state state);

#define CHECK(expr) do { if (!(expr)) { \
    fprintf(stderr, "FAIL line %d: %s\n", __LINE__, #expr); exit(1); \
} } while (0)

/* Only these observed regressions return 2; setup failures remain failures. */
#define PENDING(expr, message) do { if (!(expr)) { \
    fprintf(stderr, "REGRESSION: %s\n", message); exit(2); \
} } while (0)

struct endpoint {
    struct spa_node spa;
    struct spa_hook_list hooks;
    struct pw_impl_node *node;
    struct spa_hook listener;
    enum spa_direction direction;
    bool hold_command, hold_buffers;
    int seq, pending, params_received;
    struct spa_buffer **buffers;
    uint32_t n_buffers, received_buffers;
    int fd;
};

static int add_listener(void *object, struct spa_hook *listener,
                        const struct spa_node_events *events, void *data)
{
    struct endpoint *e = object;
    struct spa_hook_list saved;
    struct spa_node_info info = SPA_NODE_INFO_INIT();
    struct spa_param_info params[] = {
        { .id = SPA_PARAM_EnumFormat, .flags = SPA_PARAM_INFO_READ },
        { .id = SPA_PARAM_Format, .flags = SPA_PARAM_INFO_READWRITE },
        { .id = SPA_PARAM_Buffers, .flags = SPA_PARAM_INFO_READ },
    };
    struct spa_port_info port = SPA_PORT_INFO_INIT();
    info.max_input_ports = e->direction == SPA_DIRECTION_INPUT;
    info.max_output_ports = e->direction == SPA_DIRECTION_OUTPUT;
    info.change_mask = SPA_NODE_CHANGE_MASK_FLAGS;
    port.change_mask = SPA_PORT_CHANGE_MASK_FLAGS | SPA_PORT_CHANGE_MASK_PARAMS;
    port.flags = e->direction == SPA_DIRECTION_OUTPUT ? SPA_PORT_FLAG_CAN_ALLOC_BUFFERS : 0;
    port.params = params;
    port.n_params = SPA_N_ELEMENTS(params);
    spa_hook_list_isolate(&e->hooks, &saved, listener, events, data);
    spa_node_emit_info(&e->hooks, &info);
    spa_node_emit_port_info(&e->hooks, e->direction, 0, &port);
    spa_hook_list_join(&e->hooks, &saved);
    return 0;
}

static int sync_node(void *object, int seq)
{
    struct endpoint *e = object;
    CHECK(e->pending == 0);
    e->pending = SPA_RESULT_RETURN_ASYNC(++e->seq);
    return e->pending;
}

static int send_command(void *object, const struct spa_command *command)
{
    struct endpoint *e = object;
    return e->hold_command ? SPA_RESULT_RETURN_ASYNC(++e->seq) : 0;
}

static int enum_params(void *object, int seq, enum spa_direction direction,
                       uint32_t port, uint32_t id, uint32_t start, uint32_t max,
                       const struct spa_pod *filter)
{
    struct endpoint *e = object;
    uint8_t storage[1024];
    struct spa_pod_builder b = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
    struct spa_pod *param;
    if (start > 0 || max == 0)
        return 0;
    if (id == SPA_PARAM_EnumFormat || id == SPA_PARAM_Format) {
        struct spa_video_info_raw raw = {
            .format = SPA_VIDEO_FORMAT_RGBA,
            .size = SPA_RECTANGLE(16, 8),
            .framerate = SPA_FRACTION(30, 1),
        };
        param = spa_format_video_raw_build(&b, id, &raw);
    } else if (id == SPA_PARAM_Buffers) {
        param = spa_pod_builder_add_object(&b, SPA_TYPE_OBJECT_ParamBuffers, id,
            SPA_PARAM_BUFFERS_buffers, SPA_POD_Int(2),
            SPA_PARAM_BUFFERS_blocks, SPA_POD_Int(1),
            SPA_PARAM_BUFFERS_size, SPA_POD_Int(512),
            SPA_PARAM_BUFFERS_stride, SPA_POD_Int(64),
            SPA_PARAM_BUFFERS_dataType, SPA_POD_CHOICE_FLAGS_Int(1 << SPA_DATA_MemFd));
    } else {
        return -ENOENT;
    }
    struct spa_result_node_params result = { .id = id, .index = 0, .next = 1 };
    if (spa_pod_filter(&b, &result.param, param, filter) == 0)
        spa_node_emit_result(&e->hooks, seq, 0, SPA_RESULT_TYPE_NODE_PARAMS, &result);
    return 0;
}

static int set_param(void *object, enum spa_direction direction, uint32_t port,
                     uint32_t id, uint32_t flags, const struct spa_pod *param)
{
    return id == SPA_PARAM_Format ? 0 : -ENOENT;
}

static int use_buffers(void *object, enum spa_direction direction, uint32_t port,
                       uint32_t flags, struct spa_buffer **buffers, uint32_t count)
{
    struct endpoint *e = object;
    if (count == 0)
        return 0;
    if (flags & SPA_NODE_BUFFERS_FLAG_ALLOC) {
        CHECK(e->direction == SPA_DIRECTION_OUTPUT);
        e->buffers = buffers;
        e->n_buffers = count;
        CHECK(e->hold_buffers);
    } else {
        e->received_buffers = count;
        for (uint32_t i = 0; i < count; i++) {
            CHECK(buffers[i]->n_datas == 1);
            PENDING(buffers[i]->datas[0].type != (1 << SPA_DATA_MemFd),
                    "unfilled MemFd mask 4 reached consumer before producer reply");
            CHECK(buffers[i]->datas[0].type == SPA_DATA_MemFd);
            CHECK(buffers[i]->datas[0].fd >= 0);
        }
    }
    return e->hold_buffers ? SPA_RESULT_RETURN_ASYNC(++e->seq) : 0;
}

static const struct spa_node_methods methods = {
    SPA_VERSION_NODE_METHODS,
    .add_listener = add_listener,
    .sync = sync_node,
    .send_command = send_command,
    .port_enum_params = enum_params,
    .port_set_param = set_param,
    .port_use_buffers = use_buffers,
};

static void result_received(void *data, int seq, int res, uint32_t type, const void *result)
{
    struct endpoint *e = data;
    if (SPA_RESULT_IS_ASYNC(e->pending) && seq == e->pending && type == SPA_RESULT_TYPE_NODE_PARAMS) {
        const struct spa_result_node_params *params = result;
        CHECK(params->id == SPA_PARAM_Buffers && params->param != NULL);
        e->params_received++;
    }
}

static const struct pw_impl_node_events node_events = {
    PW_VERSION_IMPL_NODE_EVENTS,
    .result = result_received,
};

static void init_endpoint(struct endpoint *e, struct pw_context *context,
                          enum spa_direction direction)
{
    *e = (struct endpoint) { .direction = direction, .fd = -1 };
    spa_hook_list_init(&e->hooks);
    e->spa.iface = SPA_INTERFACE_INIT(SPA_TYPE_INTERFACE_Node, SPA_VERSION_NODE, &methods, e);
    e->node = pw_context_create_node(context,
        pw_properties_new(PW_KEY_NODE_NAME, direction == SPA_DIRECTION_OUTPUT ? "producer" : "consumer", NULL), 0);
    CHECK(e->node != NULL);
    pw_impl_node_add_listener(e->node, &e->listener, &node_events, e);
    CHECK(pw_impl_node_set_implementation(e->node, &e->spa) == 0);
    CHECK(pw_impl_node_register(e->node, NULL) == 0);
}

/* Drain ready events, not a sleep or a deadline-based guess about completion. */
static void drain(struct pw_loop *loop)
{
    for (unsigned i = 0; i < 1000; i++) {
        int n = pw_loop_iterate(loop, 0);
        CHECK(n >= 0);
        if (n == 0)
            return;
    }
    CHECK(!"main loop did not quiesce");
}

static void enumerate_pending(struct endpoint *e, struct pw_loop *loop)
{
    CHECK(SPA_RESULT_IS_ASYNC(e->pending));
    int before = e->params_received;
    CHECK(spa_node_port_enum_params(&e->spa, e->pending, e->direction, 0,
                                    SPA_PARAM_Buffers, 0, 1, NULL) == 0);
    CHECK(e->params_received == before + 1);
    drain(loop);
}

static void reply(struct endpoint *e, bool error)
{
    int seq = e->pending;
    CHECK(SPA_RESULT_IS_ASYNC(seq));
    e->pending = 0;
    struct spa_result_node_error detail = { .message = "controlled producer failure" };
    spa_node_emit_result(&e->hooks, seq, error ? -EIO : 0,
                         error ? SPA_RESULT_TYPE_NODE_ERROR : 0, error ? &detail : NULL);
}

static void fill_buffers(struct endpoint *e)
{
    CHECK(e->n_buffers == 2);
    e->fd = memfd_create("captures-sync-probe", MFD_CLOEXEC);
    CHECK(e->fd >= 0 && ftruncate(e->fd, 1024) == 0);
    for (uint32_t i = 0; i < e->n_buffers; i++) {
        struct spa_data *data = &e->buffers[i]->datas[0];
        CHECK(data->type == (1 << SPA_DATA_MemFd));
        data->type = SPA_DATA_MemFd;
        data->fd = e->fd;
        data->mapoffset = i * 512;
        data->maxsize = 512;
    }
}

int main(int argc, char **argv)
{
    CHECK(argc == 2 || (argc == 3 && strcmp(argv[2], "--control") == 0));
    bool control = argc == 3;
    bool node_case = strcmp(argv[1], "node") == 0;
    bool error = strstr(argv[1], "error") != NULL;
    bool input = strstr(argv[1], "input") != NULL;
    CHECK(node_case || strcmp(argv[1], "output") == 0 || strcmp(argv[1], "input") == 0 ||
          strcmp(argv[1], "output-error") == 0 || strcmp(argv[1], "input-error") == 0);
    pw_init(NULL, NULL);
    struct pw_main_loop *main_loop = pw_main_loop_new(NULL);
    CHECK(main_loop != NULL);
    struct pw_loop *loop = pw_main_loop_get_loop(main_loop);
    pw_loop_enter(loop);
    struct pw_context *context = pw_context_new(loop, pw_properties_new(PW_KEY_CONFIG_NAME, "null", NULL), 0);
    CHECK(context != NULL);
    struct endpoint out, in;
    init_endpoint(&out, context, SPA_DIRECTION_OUTPUT);
    drain(loop);
    if (node_case) {
        CHECK(pw_impl_node_set_active(out.node, true) == 0);
        CHECK(pw_impl_node_set_state(out.node, PW_NODE_STATE_IDLE) == 0);
        drain(loop);
        out.hold_command = true;
        enum pw_node_state before = pw_impl_node_get_info(out.node)->state;
        CHECK(before == PW_NODE_STATE_IDLE);
        int res = pw_impl_node_set_state(out.node, PW_NODE_STATE_RUNNING);
        CHECK(SPA_RESULT_IS_ASYNC(res));
        if (!control)
            enumerate_pending(&out, loop);
        PENDING(pw_impl_node_get_info(out.node)->state == before,
                "node state completed before reply");
        reply(&out, false);
        drain(loop);
        CHECK(pw_impl_node_get_info(out.node)->state == PW_NODE_STATE_RUNNING);
        /* A typed error must complete, not hang as with a type == 0 filter. */
        res = pw_impl_node_set_state(out.node, PW_NODE_STATE_IDLE);
        CHECK(SPA_RESULT_IS_ASYNC(res));
        reply(&out, true);
        drain(loop);
        CHECK(pw_impl_node_get_info(out.node)->state == PW_NODE_STATE_ERROR);
        out.hold_command = false;
    } else {
        init_endpoint(&in, context, SPA_DIRECTION_INPUT);
        out.hold_buffers = true;
        in.hold_buffers = input;
        CHECK(pw_impl_node_set_active(out.node, true) == 0);
        CHECK(pw_impl_node_set_active(in.node, true) == 0);
        struct pw_impl_link *link = pw_context_create_link(context,
            pw_impl_node_find_port(out.node, PW_DIRECTION_OUTPUT, 0),
            pw_impl_node_find_port(in.node, PW_DIRECTION_INPUT, 0), NULL, NULL, 0);
        CHECK(link != NULL);
        CHECK(pw_impl_link_register(link, NULL) == 0);
        drain(loop);
        CHECK(pw_impl_link_get_info(link)->state == PW_LINK_STATE_ALLOCATING);
        CHECK(out.n_buffers == 2 && in.received_buffers == 0);
        struct endpoint *held = &out;
        if (input) {
            fill_buffers(&out);
            reply(&out, false);
            drain(loop);
            CHECK(in.received_buffers == 2);
            held = &in;
        }
        if (!control)
            enumerate_pending(held, loop);
        PENDING(pw_impl_link_get_info(link)->state == PW_LINK_STATE_ALLOCATING,
                "link allocation completed before reply");
        if (!input) {
            CHECK(in.received_buffers == 0);
            if (!error)
                fill_buffers(&out);
        }
        reply(held, error);
        drain(loop);
        enum pw_link_state state = pw_impl_link_get_info(link)->state;
        CHECK(error ? state == PW_LINK_STATE_ERROR : state >= PW_LINK_STATE_PAUSED);
        if (!error)
            CHECK(in.received_buffers == 2);
        pw_impl_link_destroy(link);
        pw_impl_node_destroy(in.node);
    }
    pw_impl_node_destroy(out.node);
    if (out.fd >= 0)
        close(out.fd);
    pw_context_destroy(context);
    pw_loop_leave(loop);
    pw_main_loop_destroy(main_loop);
    pw_deinit();
    printf("PASS %s %s: only completion/error released pending work\n",
           argv[1], control ? "control" : "params-delivered");
    return 0;
}
