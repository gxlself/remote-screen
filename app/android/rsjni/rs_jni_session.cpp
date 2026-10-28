#include "rs_attach_registry.h"
#include "rs_jni_common.h"
#include "rs_mobile.h"

#include <atomic>
#include <cstring>
#include <memory>
#include <mutex>

namespace {

struct SessionCtx {
  jobject sink;

  ~SessionCtx() {
    if (JNIEnv* env = rsjni::attachedEnv(); env != nullptr && sink != nullptr) {
      env->DeleteGlobalRef(sink);
    }
  }
};

rsjni::AttachRegistry<SessionCtx> registry;
std::once_flag sinkMethodsOnce;
jclass sinkClass = nullptr;
jmethodID onFrameMethod = nullptr;
jmethodID onEventMethod = nullptr;
jmethodID onStreamCropMethod = nullptr;
std::atomic<uint64_t> emptyFrames{0};
thread_local unsigned callbackDepth = 0;

class SessionCallbackScope {
 public:
  explicit SessionCallbackScope(SessionCtx* ctx) : scope_(registry, ctx) {
    ++callbackDepth;
  }

  ~SessionCallbackScope() {
    --callbackDepth;
  }

 private:
  rsjni::AttachRegistry<SessionCtx>::CallbackScope scope_;
};

bool cacheSinkMethods(JNIEnv* env) {
  std::call_once(sinkMethodsOnce, [env] {
    jclass localClass = env->FindClass("ai/gxlself/remotescreen/rsnative/jni/SessionSink");
    if (rsjni::clearException(env, "SessionSink.FindClass") || localClass == nullptr) {
      return;
    }
    onFrameMethod = env->GetMethodID(localClass, "onFrame", "(IZIIIZJJJ[B[B[B[B)V");
    if (rsjni::clearException(env, "SessionSink.onFrame") || onFrameMethod == nullptr) {
      env->DeleteLocalRef(localClass);
      return;
    }
    onEventMethod = env->GetMethodID(localClass, "onEvent", "(IIIIIIIZFFJJIJIIF[B[B)V");
    if (rsjni::clearException(env, "SessionSink.onEvent") || onEventMethod == nullptr) {
      env->DeleteLocalRef(localClass);
      return;
    }
    onStreamCropMethod = env->GetMethodID(localClass, "onStreamCrop", "(IFFFFII)V");
    if (!rsjni::clearException(env, "SessionSink.onStreamCrop") && onStreamCropMethod != nullptr) {
      sinkClass = static_cast<jclass>(env->NewGlobalRef(localClass));
      rsjni::clearException(env, "SessionSink.NewGlobalRef");
    }
    env->DeleteLocalRef(localClass);
  });
  return sinkClass != nullptr;
}

bool copyFailed(const uint8_t* bytes, size_t length, jbyteArray array) {
  return bytes != nullptr && length != 0 && array == nullptr;
}

void onFrameC(void* rawCtx, const RsFrame* frame) {
  auto* ctx = static_cast<SessionCtx*>(rawCtx);
  SessionCallbackScope scope(ctx);
  JNIEnv* env = rsjni::attachedEnv();
  rsjni::LocalFrame locals(env, 8);
  if (!locals.ok() || ctx == nullptr || frame == nullptr) {
    return;
  }
  if (frame->avcc == nullptr || frame->avcc_len == 0) {
    emptyFrames.fetch_add(1, std::memory_order_relaxed);
    return;
  }

  jbyteArray vps = rsjni::newByteArray(env, frame->vps, frame->vps_len);
  jbyteArray sps = rsjni::newByteArray(env, frame->sps, frame->sps_len);
  jbyteArray pps = rsjni::newByteArray(env, frame->pps, frame->pps_len);
  jbyteArray avcc = rsjni::newByteArray(env, frame->avcc, frame->avcc_len);
  if (copyFailed(frame->vps, frame->vps_len, vps) ||
      copyFailed(frame->sps, frame->sps_len, sps) ||
      copyFailed(frame->pps, frame->pps_len, pps) || avcc == nullptr) {
    return;
  }

  env->CallVoidMethod(ctx->sink, onFrameMethod,
                      static_cast<jint>(frame->frame_id),
                      static_cast<jboolean>(frame->keyframe != 0),
                      static_cast<jint>(frame->display_id),
                      static_cast<jint>(frame->epoch),
                      static_cast<jint>(frame->codec),
                      static_cast<jboolean>(frame->clock_synced != 0),
                      static_cast<jlong>(frame->capture_ts_us),
                      static_cast<jlong>(frame->capture_local_us),
                      static_cast<jlong>(frame->received_us), vps, sps, pps, avcc);
  rsjni::clearException(env, "onFrame");
}

void onEventC(void* rawCtx, const RsEvent* event) {
  auto* ctx = static_cast<SessionCtx*>(rawCtx);
  SessionCallbackScope scope(ctx);
  JNIEnv* env = rsjni::attachedEnv();
  rsjni::LocalFrame locals(env, 8);
  if (!locals.ok() || ctx == nullptr || event == nullptr) {
    return;
  }

  jbyteArray data = rsjni::newByteArray(env, event->data, event->data_len);
  const auto* messageBytes = reinterpret_cast<const uint8_t*>(event->message);
  const size_t messageLength = event->message == nullptr ? 0 : std::strlen(event->message);
  jbyteArray message = rsjni::newByteArray(env, messageBytes, messageLength);
  if (copyFailed(event->data, event->data_len, data) ||
      copyFailed(messageBytes, messageLength, message)) {
    return;
  }

  if (event->kind == RS_EV_STREAM_CONFIG) {
    env->CallVoidMethod(ctx->sink, onStreamCropMethod,
                        static_cast<jint>(event->epoch),
                        static_cast<jfloat>(event->src_x),
                        static_cast<jfloat>(event->src_y),
                        static_cast<jfloat>(event->src_w),
                        static_cast<jfloat>(event->src_h),
                        static_cast<jint>(event->display_w),
                        static_cast<jint>(event->display_h));
    rsjni::clearException(env, "onStreamCrop");
  }

  env->CallVoidMethod(ctx->sink, onEventMethod,
                      static_cast<jint>(event->kind),
                      static_cast<jint>(event->width),
                      static_cast<jint>(event->height),
                      static_cast<jint>(event->fps),
                      static_cast<jint>(event->display_id),
                      static_cast<jint>(event->epoch),
                      static_cast<jint>(event->codec),
                      static_cast<jboolean>(event->cursor_visible != 0),
                      static_cast<jfloat>(event->cursor_x),
                      static_cast<jfloat>(event->cursor_y),
                      static_cast<jlong>(event->rtt_us),
                      static_cast<jlong>(event->offset_us),
                      static_cast<jint>(event->code),
                      static_cast<jlong>(event->shape_id),
                      static_cast<jint>(event->hot_x),
                      static_cast<jint>(event->hot_y),
                      static_cast<jfloat>(event->scale), data, message);
  rsjni::clearException(env, "onEvent");
}

}  // namespace

extern "C" JNIEXPORT jboolean JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsSessionNative_nativeAttach(
    JNIEnv* env, jclass, jlong rawId, jobject sink) {
  // Callback threads must not perform class lookup or wait for attachment.
  if (callbackDepth != 0) {
    return JNI_FALSE;
  }
  bool accepted = false;
  if (sink != nullptr && cacheSinkMethods(env)) {
    auto ctx = std::make_unique<SessionCtx>();
    ctx->sink = env->NewGlobalRef(sink);
    if (!rsjni::clearException(env, "SessionSink.NewGlobalRef") && ctx->sink != nullptr) {
      const uint64_t id = static_cast<uint64_t>(rawId);
      accepted = registry.attach(id, std::move(ctx), [id](SessionCtx* candidate) {
        return rs_session_attach(id, candidate, onFrameC, onEventC);
      });
    }
  }
  rsjni::logInfo("JNI", "RS-JNI {\"session\":\"attach\",\"ok\":%s}", accepted ? "true" : "false");
  return accepted ? JNI_TRUE : JNI_FALSE;
}

extern "C" JNIEXPORT void JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsSessionNative_nativeDetach(
    JNIEnv*, jclass, jlong rawId) {
  if (callbackDepth != 0) {
    return;
  }
  const uint64_t id = static_cast<uint64_t>(rawId);
  registry.detach(id, [id] { rs_session_detach(id); });
  rsjni::logInfo("JNI", "RS-JNI {\"session\":\"detach\"}");
}

extern "C" JNIEXPORT void JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsSessionNative_nativePointer(
    JNIEnv*, jclass, jlong id, jfloat x, jfloat y) {
  rs_input_pointer(static_cast<uint64_t>(id), x, y);
}

extern "C" JNIEXPORT void JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsSessionNative_nativeSetViewport(
    JNIEnv*, jclass, jlong id, jfloat x, jfloat y, jfloat w, jfloat h,
    jint viewW, jint viewH, jint flags) {
  rs_session_set_viewport(static_cast<uint64_t>(id), x, y, w, h,
                          static_cast<uint32_t>(viewW), static_cast<uint32_t>(viewH),
                          static_cast<uint32_t>(flags));
}

extern "C" JNIEXPORT void JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsSessionNative_nativeButton(
    JNIEnv*, jclass, jlong id, jint button, jboolean down, jfloat x, jfloat y, jint clicks) {
  rs_input_button(static_cast<uint64_t>(id), button, down != JNI_FALSE, x, y,
                  static_cast<uint8_t>(clicks));
}

extern "C" JNIEXPORT void JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsSessionNative_nativeScroll(
    JNIEnv*, jclass, jlong id, jfloat dx, jfloat dy, jint phase, jint momentum) {
  rs_input_scroll(static_cast<uint64_t>(id), dx, dy, static_cast<uint8_t>(phase),
                  static_cast<uint8_t>(momentum));
}

extern "C" JNIEXPORT void JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsSessionNative_nativeTouch(
    JNIEnv*, jclass, jlong id, jint touchId, jint phase, jfloat x, jfloat y, jfloat pressure) {
  rs_input_touch(static_cast<uint64_t>(id), static_cast<uint8_t>(touchId),
                 static_cast<uint8_t>(phase), x, y, pressure);
}

extern "C" JNIEXPORT void JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsSessionNative_nativeReleaseAll(
    JNIEnv*, jclass, jlong id) {
  rs_input_release_all(static_cast<uint64_t>(id));
}

extern "C" JNIEXPORT void JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsSessionNative_nativeRequestKeyframe(
    JNIEnv*, jclass, jlong id, jint reason) {
  rs_session_request_keyframe(static_cast<uint64_t>(id), static_cast<uint8_t>(reason));
}

extern "C" JNIEXPORT void JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsSessionNative_nativeReportDecoded(
    JNIEnv*, jclass, jlong id, jlong decodeUs, jlong latencyUs, jint queue) {
  rs_session_report_decoded(static_cast<uint64_t>(id), static_cast<uint64_t>(decodeUs),
                            static_cast<uint64_t>(latencyUs), static_cast<uint32_t>(queue));
}
