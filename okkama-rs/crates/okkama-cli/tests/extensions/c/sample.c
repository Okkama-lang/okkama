#include "okkama_ext.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define TRY(call) do { uint32_t status = (call); if (status != OKK_OK) return status; } while (0)

static uint32_t add(okk_call *c, const okk_slot *args, okk_slot *out) {
    (void)c;
    *out = (okk_slot){args[0].bits + args[1].bits, OKK_INT};
    return OKK_OK;
}

static uint32_t negate(okk_call *c, const okk_slot *args, okk_slot *out) {
    (void)c;
    *out = (okk_slot){!args[0].bits, OKK_BOOL};
    return OKK_OK;
}

static uint32_t text_copy(okk_call *c, const okk_slot *args, okk_slot *out) {
    const uint8_t *data;
    size_t len;
    TRY(c->host->text(c, args[0], &data, &len));
    return c->host->text_new(c, data, len, out);
}

static uint32_t bytes_copy(okk_call *c, const okk_slot *args, okk_slot *out) {
    const uint8_t *data;
    size_t len;
    TRY(c->host->bytes(c, args[0], &data, &len));
    return c->host->bytes_new(c, data, len, out);
}

static void state_drop(void *payload) {
    free(payload);
    fputs("sample state dropped\n", stderr);
}
static const okk_state_type states[] = {{"Counter", state_drop}};

static uint32_t create(okk_call *c, const okk_slot *args, okk_slot *out) {
    uint64_t *value = malloc(sizeof(*value));
    if (!value) return OKK_PANIC;
    *value = args[0].bits;
    uint32_t status = c->host->state_new(c, &states[0], value, sizeof(*value), out);
    if (status != OKK_OK) free(value);
    return status;
}

static uint32_t read_counter(okk_call *c, const okk_slot *args, okk_slot *out) {
    void *payload;
    TRY(c->host->state_get(c, args[0], &states[0], &payload));
    *out = (okk_slot){*(uint64_t *)payload, OKK_INT};
    return OKK_OK;
}

static uint32_t release(okk_call *c, const okk_slot *args, okk_slot *out) {
    *out = (okk_slot){0, OKK_NONE};
    return c->host->state_release(c, args[0], &states[0]);
}

typedef struct { size_t len; uint8_t data[]; } job;
static uint32_t prepare(okk_call *c, const okk_slot *args, void **out) {
    const uint8_t *data;
    size_t len;
    TRY(c->host->text(c, args[0], &data, &len));
    job *work = malloc(sizeof(*work) + len);
    if (!work) return OKK_PANIC;
    work->len = len;
    memcpy(work->data, data, len);
    *out = work;
    return OKK_OK;
}
static void execute(void *payload, const okk_cancel *cancel) {
    job *work = payload;
    for (size_t i = 0; i < work->len; ++i) {
        if (cancel->cancelled(cancel)) return;
        if (work->data[i] >= 'a' && work->data[i] <= 'z') work->data[i] -= 'a' - 'A';
    }
}
static uint32_t complete(okk_call *c, void *payload, okk_slot *out) {
    job *work = payload;
    return c->host->text_new(c, work->data, work->len, out);
}
static const okk_async uppercase = {prepare, execute, complete, free};

#define SYNC(name, signature, params, callback) {"sample@0::" name, signature, params, callback, NULL}
static const okk_export exports[] = {
    SYNC("add/2", "(int, int) -> int", "a,b", add),
    SYNC("negate/1", "(bool) -> bool", "value", negate),
    SYNC("text_copy/1", "(string) -> string", "value", text_copy),
    SYNC("bytes_copy/1", "(@byte) -> @byte", "value", bytes_copy),
    SYNC("create/1", "(int) -> Counter", "value", create),
    SYNC("read/1", "(Counter) -> int", "counter", read_counter),
    SYNC("release/1", "(Counter) -> void", "counter", release),
    {"sample@0::uppercase/1", "async (string) -> string", "value", NULL, &uppercase},
};
#ifndef SAMPLE_ABI
#define SAMPLE_ABI OKKAMA_EXT_ABI
#endif
#if SAMPLE_ABI != OKKAMA_EXT_ABI
__attribute__((constructor)) static void loaded_bad_library(void) {
    fputs("bad library constructor ran\n", stderr);
}
#endif
const okk_extension okkama_extension = {
    SAMPLE_ABI, sizeof(okk_extension), "C sample", exports,
    sizeof(exports) / sizeof(exports[0]), states, sizeof(states) / sizeof(states[0])
};
