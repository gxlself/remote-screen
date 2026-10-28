#include "RsSignerShim.h"

#include "../../../../../crates/rs-mobile/include/rs_mobile.h"

#include <stdatomic.h>

static _Atomic(RsSecureKeySignFn) rs_secure_key_signer;

static int32_t rs_secure_key_trampoline(void *ctx,
                                        const uint8_t *msg,
                                        size_t len,
                                        uint8_t *sig_out,
                                        size_t *sig_len) {
    (void)ctx;
    RsSecureKeySignFn sign =
        atomic_load_explicit(&rs_secure_key_signer, memory_order_acquire);
    if (sign == NULL) {
        return -1;
    }
    return sign(msg, len, sig_out, sig_len);
}

void rs_secure_key_register_signer(RsSecureKeySignFn sign,
                                   const uint8_t *spki_der,
                                   size_t spki_len) {
    atomic_store_explicit(&rs_secure_key_signer, sign, memory_order_release);
    if (sign == NULL) {
        return;
    }
    rs_register_signer(NULL, rs_secure_key_trampoline, spki_der, spki_len);
}
