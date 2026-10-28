#ifndef RS_SECRET_SHIM_H
#define RS_SECRET_SHIM_H

#include <stddef.h>
#include <stdint.h>

/* Swift 侧的包裹回调：op 1 wrap / 2 unwrap；语义同 rs_mobile.h 的 RsSecretWrapCb（没有 ctx）。 */
typedef int32_t (*RsRecordingWrapFn)(uint8_t op,
                                     const uint8_t *in,
                                     size_t in_len,
                                     uint8_t *out,
                                     size_t *out_len);

/* 注册到 rs-mobile；NULL 被忽略。 */
void rs_recording_register_wrapper(RsRecordingWrapFn fn);

#endif
