/* Extension ABI 1. Keep in sync with src/ext.rs. */
#ifndef OKKAMA_EXT_H
#define OKKAMA_EXT_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
#define OKKAMA_EXT_ABI 1
#define OKKAMA_EXT_STACK (512 * 1024)
typedef struct okk_host okk_host;
typedef struct okk_call { const okk_host *host; } okk_call;
typedef struct okk_cancel { uint32_t (*cancelled)(const struct okk_cancel *); } okk_cancel;
typedef struct { uint64_t bits, kind; } okk_slot;
enum { OKK_NONE, OKK_INT, OKK_FLOAT, OKK_BOOL, OKK_BYTE, OKK_CHAR,
       OKK_BIGINT, OKK_RAT, OKK_STRING, OKK_ARRAY, OKK_MAP, OKK_STATE };
enum { OKK_OK, OKK_FAILED, OKK_PANIC, OKK_STOPPED, OKK_BAD_TOKEN,
       OKK_BAD_KIND, OKK_BAD_SHAPE, OKK_BAD_TEXT, OKK_RANGE, OKK_UNSUPPORTED };
typedef uint32_t (*okk_sync)(okk_call *, const okk_slot *, okk_slot *);
typedef struct {
    uint32_t (*prepare)(okk_call *, const okk_slot *, void **);
    void (*execute)(void *, const okk_cancel *);
    uint32_t (*complete)(okk_call *, void *, okk_slot *);
    void (*drop)(void *);
} okk_async;
typedef struct {
    const char *symbol;
    const char *signature;
    const char *params;
    okk_sync sync;
    const okk_async *async;
} okk_export;
typedef struct { const char *name; void (*drop)(void *); } okk_state_type;
typedef struct {
    uint32_t abi, size;
    const char *name;
    const okk_export *exports;
    uint32_t export_count;
    const okk_state_type *states;
    uint32_t state_count;
} okk_extension;
extern const okk_extension okkama_extension;
/* All callbacks run on the VM thread. Tokens and views expire on return.
 * Scalars are bits (INT: signed i64, FLOAT: binary64); other bits are tokens.
 * Input buffers are copied. Null is allowed only for an empty input.
 * Output pointers must be writable. No exceptions or longjmp across the ABI.
 * fail sets a pending failure; the callback must return OKK_FAILED.
 * BAD_TOKEN, BAD_KIND and BAD_SHAPE are sticky contract faults, even if ignored.
 * BAD_TEXT (invalid UTF-8) and RANGE (data/bounds errors) are recoverable;
 * returning either status from the callback is a contract fault.
 * Host allocation beyond the run's memory budget sets sticky STOPPED.
 * bigint magnitudes are unsigned big-endian, signs -1/0/1. Leading zeroes
 * are accepted; sign 0 requires a zero magnitude. Values fitting i64 are INT.
 * bigint_read accepts INT or BIGINT; the view lasts until callback return.
 * rat_parts produces normalized int slots; rat_new reduces, makes the
 * denominator positive, and returns RANGE for zero denominator.
 * bytes_reserve returns zeroed writable bytes and an ARRAY slot. The buffer
 * stays writable until callback return, including after other host calls.
 * array_new requires one element shape; empty arrays fit every array type.
 * array_get checks the index; array_read copies count slots from start.
 * map_new requires one key shape and one value shape; duplicate keys are RANGE.
 * map_entries copies the first count keys and values in key order; count
 * beyond map_len is RANGE. Empty input/output buffers may be null.
 * Maybe is NONE (bits = 0) or its inner slot, never a token for the wrapper.
 * Unimplemented entries return OKK_UNSUPPORTED without changing outputs. */
struct okk_host {
    uint32_t (*text)(okk_call *, okk_slot, const uint8_t * *, size_t *);
    uint32_t (*bytes)(okk_call *, okk_slot, const uint8_t * *, size_t *);
    uint32_t (*text_new)(okk_call *, const uint8_t *, size_t, okk_slot *);
    uint32_t (*bytes_new)(okk_call *, const uint8_t *, size_t, okk_slot *);
    uint32_t (*bytes_reserve)(okk_call *, size_t, uint8_t * *, okk_slot *);
    uint32_t (*bigint_read)(okk_call *, okk_slot, int32_t *, const uint8_t * *, size_t *);
    uint32_t (*bigint_new)(okk_call *, int32_t, const uint8_t *, size_t, okk_slot *);
    uint32_t (*rat_parts)(okk_call *, okk_slot, okk_slot *, okk_slot *);
    uint32_t (*rat_new)(okk_call *, okk_slot, okk_slot, okk_slot *);
    uint32_t (*array_len)(okk_call *, okk_slot, size_t *);
    uint32_t (*array_get)(okk_call *, okk_slot, size_t, okk_slot *);
    uint32_t (*array_read)(okk_call *, okk_slot, size_t, okk_slot *, size_t);
    uint32_t (*array_new)(okk_call *, const okk_slot *, size_t, okk_slot *);
    uint32_t (*map_len)(okk_call *, okk_slot, size_t *);
    uint32_t (*map_entries)(okk_call *, okk_slot, okk_slot *, okk_slot *, size_t);
    uint32_t (*map_new)(okk_call *, const okk_slot *, const okk_slot *, size_t, okk_slot *);
    uint32_t (*state_new)(okk_call *, const okk_state_type *, void *, size_t, okk_slot *);
    uint32_t (*state_get)(okk_call *, okk_slot, const okk_state_type *, void * *);
    uint32_t (*state_release)(okk_call *, okk_slot);
    uint32_t (*fail)(okk_call *, const uint8_t *, size_t, const uint8_t *, size_t);
    uint32_t (*interrupted)(okk_call *, uint32_t *);
};
#ifdef __cplusplus
}
#endif
#endif
