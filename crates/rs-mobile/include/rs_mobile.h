/* Avoid a main-file pragma warning when syntax-checking this header directly. */
#if !defined(__INCLUDE_LEVEL__) || __INCLUDE_LEVEL__ > 0
#pragma once
#endif
/* Native RsCore ABI.
 *
 * Video/cursor and terminal callbacks run on rs-core runtime worker threads,
 * serialized per attachment, including cached state replay during attach.
 * All callback pointers/buffers are valid ONLY during the callback; copy data
 * needed later. Never unwind or synchronously wait for the main thread in a
 * callback. ctx/callbacks must remain valid until detach/replacement returns.
 * Detach waits for in-flight callbacks; no further callback follows its return.
 * When called inside a callback, detach prevents subsequent callbacks; the
 * current callback must return before its ctx can be freed.
 */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef uint64_t RsSessionId;
typedef uint64_t RsSshId;

/* on_frame: all fields valid unless noted; parameter sets are from this frame
 * only (NULL/0 on non-keyframes). codec: latest StreamConfig, 1 HEVC, 2 H.264.
 */
typedef struct {
    uint32_t frame_id;
    uint8_t keyframe;
    uint8_t clock_synced;
    uint8_t display_id;
    uint8_t epoch;
    uint8_t codec;
    uint64_t capture_ts_us;    /* Host Unix microseconds. */
    int64_t capture_local_us;  /* Local Unix microseconds; valid if clock_synced. */
    uint64_t received_us;      /* Local Unix microseconds. */
    const uint8_t *vps; size_t vps_len; /* HEVC keyframe only; H.264 always NULL/0. */
    const uint8_t *sps; size_t sps_len; /* Keyframe SPS, without start code. */
    const uint8_t *pps; size_t pps_len; /* Keyframe PPS, without start code. */
    const uint8_t *avcc; size_t avcc_len; /* Access unit: 4-byte BE NAL lengths. */
} RsFrame;

/* 2/3/5 retain their original event numbers; 6-9 extend them. CONNECTED/HOST_ERROR go via JS. */
enum {
    RS_EV_STREAM_CONFIG = 2,
    RS_EV_CLOCK_SYNC = 3,
    RS_EV_CLOSED = 5,
    RS_EV_CURSOR_POS = 6,
    RS_EV_CURSOR_SHAPE = 7,
    RS_EV_CURSOR_USE = 8,
    RS_EV_CURSOR_HIDDEN = 9
};

typedef struct {
    int32_t kind;              /* Always valid; RS_EV_* above. */
    uint32_t width, height;    /* STREAM_CONFIG: stream pixels; CURSOR_SHAPE: PNG pixels. */
    uint32_t fps;              /* STREAM_CONFIG. */
    uint32_t display_id, epoch;/* STREAM_CONFIG. */
    uint8_t codec;             /* STREAM_CONFIG: 0 raw (adapter sessions), 1 HEVC, 2 H.264. */
    uint8_t cursor_visible;    /* STREAM_CONFIG, CURSOR_POS, CURSOR_HIDDEN: 0/1. */
    float cursor_x, cursor_y;  /* STREAM_CONFIG, CURSOR_POS: normalized 0..1. */
    uint64_t rtt_us;           /* CLOCK_SYNC: round-trip microseconds. */
    int64_t offset_us;         /* CLOCK_SYNC: host - local microseconds. */
    int32_t code;              /* Reserved, currently 0 (CLOSED has reason only). */
    uint64_t shape_id;         /* CURSOR_SHAPE, CURSOR_USE. */
    uint32_t hot_x, hot_y;     /* CURSOR_SHAPE: hotspot pixels. */
    float scale;              /* CURSOR_SHAPE: PNG pixels per point. */
    const uint8_t *data; size_t data_len; /* CURSOR_SHAPE: borrowed PNG bytes. */
    const char *message;      /* CLOSED: borrowed NUL-terminated reason, embedded NUL -> '?'. */
    /* RS_EV_STREAM_CONFIG only (M3, 03 §5.5): normalized display region of the
     * decoded picture for this epoch; 0,0,1,1 when not cropped. */
    float src_x, src_y, src_w, src_h;
    /* Native display pixels (after rotation) for this epoch; 0 when the Host did not send them. */
    uint32_t display_w, display_h;
} RsEvent;

typedef void (*RsFrameCb)(void *ctx, const RsFrame *frame);
typedef void (*RsEventCb)(void *ctx, const RsEvent *event);

/* Missing, disconnected, or closed sessions / NULL callbacks return false.
 * Attachment replaces the previous consumer and replays cached config/cursor.
 */
bool rs_session_attach(RsSessionId id, void *ctx, RsFrameCb on_frame, RsEventCb on_event);
void rs_session_detach(RsSessionId id);

/* Input ignores missing/disconnected sessions and non-finite numbers.
 * Pointer/button/touch coordinates are normalized to the current display 0..1.
 */
void rs_input_pointer(RsSessionId id, float x, float y);
/* button: 0 left, 1 right, 2 middle, 3 back, 4 forward; others ignored. */
void rs_input_button(RsSessionId id, int32_t button, bool down, float x, float y, uint8_t clicks);
/* Pixel deltas, natural scrolling; phase/momentum: 0 none, 1 began, 2 changed,
 * 3 ended. Invalid phase/momentum values are ignored. */
void rs_input_scroll(RsSessionId id, float dx, float dy, uint8_t phase, uint8_t momentum);
/* phase: 0 began, 1 moved, 2 ended, 3 cancelled; others ignored. */
void rs_input_touch(RsSessionId id, uint8_t touch_id, uint8_t phase, float x, float y, float pressure);
void rs_input_release_all(RsSessionId id);

/* Decode-side requests and reports (03 §5.3, §7); session missing -> ignored.
 * reason uses KeyframeRequest.Reason: only 2 DECODER_RESET (decode failure or
 * VT session rebuild) and 4 BACKLOG (decoder not ready or queue > 2) are
 * accepted. LOSS is determined by the Rust receiver, not the App. Other values
 * are ignored. Rust fills last_good_frame. */
void rs_session_request_keyframe(RsSessionId id, uint8_t reason);
/* Call once after each frame is decoded and submitted for display; Rust
 * aggregates decode_p50_us, decode_queue and render_fps into Feedback.
 * decode_us: decode submission to image; latency_us: RsFrame.received_us to
 * display submission; queue: frames waiting to decode at this moment. */
void rs_session_report_decoded(RsSessionId id, uint64_t decode_us, uint64_t latency_us, uint32_t queue);
/* Viewport report for M3 viewport-aware encoding (03 §5.5). x/y/w/h: visible part of the
 * current display, normalized 0..1 (whole display = 0,0,1,1). view_w_px/view_h_px: physical
 * device pixels that region covers on screen. flags: RS_VIEWPORT_ACTIVE while a gesture or
 * zoom animation runs; RS_VIEWPORT_CROP_OK when this view renders RsEvent.src_* crops.
 * Any thread; never blocks. Non-finite or empty rects are ignored; unknown or non-streaming
 * sessions are ignored. Rust throttles and deduplicates. */
#define RS_VIEWPORT_ACTIVE 1u
#define RS_VIEWPORT_CROP_OK 2u
void rs_session_set_viewport(uint64_t id, float x, float y, float w, float h,
                             uint32_t view_w_px, uint32_t view_h_px, uint32_t flags);

/* ---------- 声音（M2）---------- */
/* 不透明句柄。持有会话声音缓冲的引用；会话结束后仍可安全使用（只输出静音），直到 rs_audio_close。 */
typedef struct RsAudio RsAudio;

typedef struct {
    uint64_t packets;        /* 收到并接受的包 */
    uint64_t lost;           /* 判定丢失（含 FEC 恢复的） */
    uint64_t concealed;      /* PLC 帧数 */
    uint64_t fec_recovered;  /* FEC 恢复帧数 */
    uint64_t late;           /* 迟到丢弃 */
    uint64_t underruns;      /* 缓冲见底次数 */
    uint32_t target_ms;      /* 当前目标缓冲 */
    uint32_t level_ms;       /* 当前缓冲水位 */
    uint32_t contended;      /* 渲染时拿不到锁、输出静音的次数 */
    bool     playing;        /* 正在输出有效声音 */
} RsAudioStats;

/* 打开会话的声音输出。会话不存在返回 NULL。任意非实时线程可调用。 */
RsAudio *rs_audio_open(RsSessionId id);
/* 实时音频线程调用：不阻塞、不分配、不打日志。填满 frames 帧（没有数据补 0），返回 frames；任一指针为空返回 0。 */
uint32_t rs_audio_render(RsAudio *audio, float *left, float *right, uint32_t frames);
/* 同上，交错立体声，lr 长度 2*frames（Android AAudio 用）。 */
uint32_t rs_audio_render_interleaved(RsAudio *audio, float *lr, uint32_t frames);
/* 本地静音，5 ms 淡出 / 淡入。非实时线程调用。 */
void rs_audio_set_muted(RsAudio *audio, bool muted);
/* 读统计；任一指针为空则不写。 */
void rs_audio_stats(RsAudio *audio, RsAudioStats *out);
/* 释放句柄；之后不得再用。NULL 忽略。 */
void rs_audio_close(RsAudio *audio);

/* Output is borrowed during callback; buffered output replays during attach.
 * (NULL, 0) signals closure exactly once to the attached consumer; no output
 * follows it. Missing/closed IDs or NULL callbacks return false.
 */
bool rs_term_attach(RsSshId id, void *ctx, void (*on_output)(void *ctx, const uint8_t *buf, size_t len));
void rs_term_detach(RsSshId id);
/* buf is copied; NULL or len=0 is ignored. Otherwise len bytes must be readable. */
void rs_term_input(RsSshId id, const uint8_t *buf, size_t len);
/* Zero dimensions are ignored. */
void rs_term_resize(RsSshId id, uint16_t cols, uint16_t rows);

/* msg is the ORIGINAL message: perform ECDSA-P256-SHA256 and return DER,
 * e.g. iOS .ecdsaSignatureMessageX962SHA256 (not a prehashed digest).
 * On entry *sig_len is sig_out capacity (Rust provides 128 bytes); never write
 * past it. On return set *sig_len to the actual DER length. Return 0 on success,
 * nonzero on failure. Zero/oversized lengths are failures.
 * May be called concurrently on ANY thread, including spawn_blocking: do not
 * synchronously wait for main. ctx/callback must remain valid for the process
 * lifetime or until the next registration AND all old signer uses complete.
 */
typedef int32_t (*RsSignCb)(void *ctx, const uint8_t *msg, size_t len, uint8_t *sig_out, size_t *sig_len);
/* Copies a 91-byte SPKI for an uncompressed P-256 public key. NULL/invalid SPKI
 * or NULL callback is ignored, preserving the previous registration. */
void rs_register_signer(void *ctx, RsSignCb sign, const uint8_t *spki_der, size_t spki_len);

/* ---------- 录制密钥包裹（M3 V5）---------- */
/* op: 1 = wrap（in 为 32 字节明文文件密钥），2 = unwrap（in 为 wrap 的输出）。
 * 入口 *out_len 为 out 容量（Rust 给 256 字节）；返回时设为实际长度，绝不越界写。
 * 返回 0 成功；-1 不可用（没有主密钥、钥匙串锁着）；-2 拒绝（数据被改、主密钥不对）；
 * -3 out 太小；其它非 0 按 -1 处理。wrap 的输出不透明，长度 1..=255。
 * 可能在任意线程（含 spawn_blocking）并发调用；不要同步等主线程，不要抛异常或 unwind。
 * ctx/cb 须在进程生命周期内有效，或者到下一次注册、且旧回调的调用全部结束为止。 */
typedef int32_t (*RsSecretWrapCb)(void *ctx, uint8_t op, const uint8_t *in, size_t in_len, uint8_t *out, size_t *out_len);
/* NULL cb 被忽略，保留上一次注册。 */
void rs_register_secret_wrapper(void *ctx, RsSecretWrapCb cb);

/* ---------- 内置 VNC / RDP：原始帧（M3 V4）---------- */
#define RS_ADAPTER_ID_BASE 0x4000000000000000ULL
typedef struct { uint32_t x, y, w, h; } RsRect;
enum { RS_PIX_BGRA8 = 1, RS_PIX_RGBA8 = 2 };
typedef void (*RsRawReadyCb)(void *ctx, uint64_t seq, uint32_t width, uint32_t height);
bool rs_raw_attach(RsSessionId id, void *ctx, RsRawReadyCb on_ready, RsEventCb on_event);
void rs_raw_detach(RsSessionId id);
uint64_t rs_raw_copy(RsSessionId id, uint64_t since_seq, uint8_t *dst, size_t dst_stride,
                     uint32_t dst_w, uint32_t dst_h, uint8_t pix, RsRect *copied);

/* ---------- 内置 VNC / RDP：凭据存储（M3 V4）---------- */
typedef int32_t (*RsSecretGetCb)(void *ctx, const char *key, uint8_t *buf, size_t *len);
typedef int32_t (*RsSecretPutCb)(void *ctx, const char *key, const uint8_t *value, size_t len);
typedef int32_t (*RsSecretDelCb)(void *ctx, const char *key);
void rs_register_secret_store(void *ctx, RsSecretGetCb get, RsSecretPutCb put, RsSecretDelCb del);

/* Local wall clock in Unix microseconds; exported by rs-mobile. */
uint64_t rs_now_us(void);

#if defined(__LP64__)
#ifdef __cplusplus
#define RS_MOBILE_ASSERT static_assert
#else
#define RS_MOBILE_ASSERT _Static_assert
#endif
RS_MOBILE_ASSERT(sizeof(RsFrame) == 104, "RsFrame size");
RS_MOBILE_ASSERT(offsetof(RsFrame, frame_id) == 0, "RsFrame.frame_id");
RS_MOBILE_ASSERT(offsetof(RsFrame, keyframe) == 4, "RsFrame.keyframe");
RS_MOBILE_ASSERT(offsetof(RsFrame, clock_synced) == 5, "RsFrame.clock_synced");
RS_MOBILE_ASSERT(offsetof(RsFrame, display_id) == 6, "RsFrame.display_id");
RS_MOBILE_ASSERT(offsetof(RsFrame, epoch) == 7, "RsFrame.epoch");
RS_MOBILE_ASSERT(offsetof(RsFrame, codec) == 8, "RsFrame.codec");
RS_MOBILE_ASSERT(offsetof(RsFrame, capture_ts_us) == 16, "RsFrame.capture_ts_us");
RS_MOBILE_ASSERT(offsetof(RsFrame, capture_local_us) == 24, "RsFrame.capture_local_us");
RS_MOBILE_ASSERT(offsetof(RsFrame, received_us) == 32, "RsFrame.received_us");
RS_MOBILE_ASSERT(offsetof(RsFrame, vps) == 40, "RsFrame.vps");
RS_MOBILE_ASSERT(offsetof(RsFrame, vps_len) == 48, "RsFrame.vps_len");
RS_MOBILE_ASSERT(offsetof(RsFrame, sps) == 56, "RsFrame.sps");
RS_MOBILE_ASSERT(offsetof(RsFrame, sps_len) == 64, "RsFrame.sps_len");
RS_MOBILE_ASSERT(offsetof(RsFrame, pps) == 72, "RsFrame.pps");
RS_MOBILE_ASSERT(offsetof(RsFrame, pps_len) == 80, "RsFrame.pps_len");
RS_MOBILE_ASSERT(offsetof(RsFrame, avcc) == 88, "RsFrame.avcc");
RS_MOBILE_ASSERT(offsetof(RsFrame, avcc_len) == 96, "RsFrame.avcc_len");
RS_MOBILE_ASSERT(sizeof(RsEvent) == 136, "RsEvent size");
RS_MOBILE_ASSERT(sizeof(RsRect) == 16, "RsRect size");
RS_MOBILE_ASSERT(offsetof(RsEvent, kind) == 0, "RsEvent.kind");
RS_MOBILE_ASSERT(offsetof(RsEvent, width) == 4, "RsEvent.width");
RS_MOBILE_ASSERT(offsetof(RsEvent, height) == 8, "RsEvent.height");
RS_MOBILE_ASSERT(offsetof(RsEvent, fps) == 12, "RsEvent.fps");
RS_MOBILE_ASSERT(offsetof(RsEvent, display_id) == 16, "RsEvent.display_id");
RS_MOBILE_ASSERT(offsetof(RsEvent, epoch) == 20, "RsEvent.epoch");
RS_MOBILE_ASSERT(offsetof(RsEvent, codec) == 24, "RsEvent.codec");
RS_MOBILE_ASSERT(offsetof(RsEvent, cursor_visible) == 25, "RsEvent.cursor_visible");
RS_MOBILE_ASSERT(offsetof(RsEvent, cursor_x) == 28, "RsEvent.cursor_x");
RS_MOBILE_ASSERT(offsetof(RsEvent, cursor_y) == 32, "RsEvent.cursor_y");
RS_MOBILE_ASSERT(offsetof(RsEvent, rtt_us) == 40, "RsEvent.rtt_us");
RS_MOBILE_ASSERT(offsetof(RsEvent, offset_us) == 48, "RsEvent.offset_us");
RS_MOBILE_ASSERT(offsetof(RsEvent, code) == 56, "RsEvent.code");
RS_MOBILE_ASSERT(offsetof(RsEvent, shape_id) == 64, "RsEvent.shape_id");
RS_MOBILE_ASSERT(offsetof(RsEvent, hot_x) == 72, "RsEvent.hot_x");
RS_MOBILE_ASSERT(offsetof(RsEvent, hot_y) == 76, "RsEvent.hot_y");
RS_MOBILE_ASSERT(offsetof(RsEvent, scale) == 80, "RsEvent.scale");
RS_MOBILE_ASSERT(offsetof(RsEvent, data) == 88, "RsEvent.data");
RS_MOBILE_ASSERT(offsetof(RsEvent, data_len) == 96, "RsEvent.data_len");
RS_MOBILE_ASSERT(offsetof(RsEvent, message) == 104, "RsEvent.message");
RS_MOBILE_ASSERT(offsetof(RsEvent, src_x) == 112, "RsEvent.src_x");
RS_MOBILE_ASSERT(offsetof(RsEvent, src_h) == 124, "RsEvent.src_h");
RS_MOBILE_ASSERT(offsetof(RsEvent, display_w) == 128, "RsEvent.display_w");
RS_MOBILE_ASSERT(offsetof(RsEvent, display_h) == 132, "RsEvent.display_h");
RS_MOBILE_ASSERT(sizeof(RsAudioStats) == 64, "RsAudioStats size");
RS_MOBILE_ASSERT(offsetof(RsAudioStats, target_ms) == 48, "RsAudioStats.target_ms");
RS_MOBILE_ASSERT(offsetof(RsAudioStats, contended) == 56, "RsAudioStats.contended");
RS_MOBILE_ASSERT(offsetof(RsAudioStats, playing) == 60, "RsAudioStats.playing");
#undef RS_MOBILE_ASSERT
#endif

#ifdef __cplusplus
}
#endif
