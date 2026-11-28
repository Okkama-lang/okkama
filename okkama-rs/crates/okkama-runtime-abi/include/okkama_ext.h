/* Extension ABI 1. Keep in sync with src/ext.rs. */
#ifndef OKKAMA_EXT_H
#define OKKAMA_EXT_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
#define OKKAMA_EXT_ABI 1
#define OKKAMA_EXT_STACK (768 * 1024)
typedef struct okk_host okk_host;
typedef struct okk_call { const okk_host *host; } okk_call;
typedef struct okk_cancel { uint32_t (*cancelled)(const struct okk_cancel *); } okk_cancel;
typedef struct { uint64_t bits, kind; } okk_slot;
enum { OKK_NONE, OKK_INT, OKK_FLOAT, OKK_BOOL, OKK_BYTE, OKK_CHAR,
       OKK_BIGINT, OKK_RAT, OKK_STRING, OKK_ARRAY, OKK_MAP, OKK_STATE };
enum { OKK_OK, OKK_FAILED, OKK_PANIC, OKK_STOPPED, OKK_BAD_TOKEN,
       OKK_BAD_KIND, OKK_BAD_SHAPE, OKK_BAD_TEXT, OKK_RANGE, OKK_UNSUPPORTED };
typedef uint32_t (*okk_sync)(okk_call *, const okk_slot *, okk_slot *);
/* An async call creates an ordinary cold Future: prepare starts on first poll.
 * prepare and complete run on the VM thread, each in a fresh call scope.
 * execute runs on a blocking pool thread without VM access. It must return.
 * OKK_OK from prepare transfers job ownership, including a null job, to the
 * runtime. For any other status the runtime does not own the pointer:
 * leave it null or free it before returning. complete never frees the job.
 * drop runs exactly once after complete, or instead of complete on cancellation,
 * a rejected successful prepare, or teardown; never concurrently with execute.
 * drop may run on the VM or a pool thread and must not use the host API.
 * cancelled() is valid only during execute; it stays 1 after task cancellation
 * or the run's timeout/interrupt. Cancellation waits for execute to return,
 * skips complete, drops the job, then resolves the Future as Cancelled.
 * Tokens, call pointers and views cannot be stored in a job. Copy inputs.
 * A state payload can be read in prepare, but release may run during execute:
 * the job must retain its own ownership (e.g. an Arc clone), not a borrowed
 * raw payload pointer. State drop itself still runs only on the VM thread.
 * Native work consumes no steps; host allocations in complete obey max-memory.
 */
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
/* sync, prepare, complete and state drop run on the VM thread.
 * Keep these callbacks below 1 ms; longer work belongs in async execute.
 * VM-thread callbacks have at least OKKAMA_EXT_STACK (786432 bytes / 768 KiB)
 * of stack: runtime STACK_HEADROOM (1 MiB) minus JIT MAX_FRAME (256 KiB).
 * Tokens and views expire on return.
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
 * State types are pointers into this extension's states table; names are short
 * native object names in the export's module. Payloads are opaque, including null.
 * Successful state_new transfers the payload: drop runs exactly once on this
 * runtime's VM thread at release, dead-cell sweep, or runtime teardown.
 * On failure state_new leaves payload ownership with the caller.
 * bytes charges collector pressure. States carry no language ownership token.
 * state_get/release require the same type; a foreign type is a contract fault.
 * After release both set NativeFailure already_closed and return OKK_FAILED.
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
    uint32_t (*state_release)(okk_call *, okk_slot, const okk_state_type *);
    uint32_t (*fail)(okk_call *, const uint8_t *, size_t, const uint8_t *, size_t);
    uint32_t (*interrupted)(okk_call *, uint32_t *);
};
#ifdef __cplusplus
}
#endif
#endif
