#ifndef RS_SIGNER_SHIM_H
#define RS_SIGNER_SHIM_H

#include <stddef.h>
#include <stdint.h>

typedef int32_t (*RsSecureKeySignFn)(const uint8_t *msg,
                                     size_t len,
                                     uint8_t *sig_out,
                                     size_t *sig_len);

/*
 * Registers the iOS Secure Key signer with rs-mobile. The callback and SPKI
 * are retained by the caller for the lifetime of the process. A later call
 * replaces the previous callback and public key.
 */
void rs_secure_key_register_signer(RsSecureKeySignFn sign,
                                   const uint8_t *spki_der,
                                   size_t spki_len);

#endif
