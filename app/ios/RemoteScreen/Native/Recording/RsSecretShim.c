#include "RsSecretShim.h"

#include "../../../../../crates/rs-mobile/include/rs_mobile.h"

#include <stdatomic.h>

static _Atomic(RsRecordingWrapFn) rs_recording_wrap_fn;

static int32_t rs_recording_wrap_trampoline(void *ctx,
                                            uint8_t op,
                                            const uint8_t *in,
                                            size_t in_len,
                                            uint8_t *out,
                                            size_t *out_len) {
    (void)ctx;
    RsRecordingWrapFn fn =
        atomic_load_explicit(&rs_recording_wrap_fn, memory_order_acquire);
    if (fn == NULL) {
        return -1;
    }
    return fn(op, in, in_len, out, out_len);
}

void rs_recording_register_wrapper(RsRecordingWrapFn fn) {
    if (fn == NULL) {
        return;
    }
    atomic_store_explicit(&rs_recording_wrap_fn, fn, memory_order_release);
    rs_register_secret_wrapper(NULL, rs_recording_wrap_trampoline);
}
