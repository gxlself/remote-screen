// Adapter (VNC / RDP) raw-frame sessions: rs_raw_attach / rs_raw_detach / rs_raw_copy.
// The structure follows rs_jni_session.cpp. onEventC and the SessionSink method
// signatures are copies of that file's (STREAM_CONFIG calls onStreamCrop, then onEvent):
// when SessionSink, RsEvent or that order changes, update both files together.
#include "rs_attach_registry.h"
#include "rs_jni_common.h"
#include "rs_mobile.h"

#include <cstdint>
#include <cstring>
#include <memory>
#include <mutex>

namespace {

struct RawCtx {
  jobject sink = nullptr;

  ~RawCtx() {
    if (JNIEnv* env = rsjni::attachedEnv(); env != nullptr && sink != nullptr) {
      env->DeleteGlobalRef(sink);
    }
  }
};

rsjni::AttachRegistry<RawCtx> registry;
std::once_flag sinkMethodsOnce;
// Keeps RawSink, and with it its SessionSink superinterface, loaded for the method IDs.
jclass sinkClass = nullptr;
jmethodID onRawReadyMethod = nullptr;
jmethodID onEventMethod = nullptr;
jmethodID onStreamCropMethod = nullptr;
thread_local unsigned callbackDepth = 0;

class RawCallbackScope {
 public:
  explicit RawCallbackScope(RawCtx* ctx) : scope_(registry, ctx) {
    ++callbackDepth;
  }

  ~RawCallbackScope() {
    --callbackDepth;
  }

 private:
  rsjni::AttachRegistry<RawCtx>::CallbackScope scope_;
};

bool cacheSinkMethods(JNIEnv* env) {
  std::call_once(sinkMethodsOnce, [env] {
    jclass sessionClass = env->FindClass("ai/gxlself/remotescreen/rsnative/jni/SessionSink");
    if (rsjni::clearException(env, "SessionSink.FindClass") || sessionClass == nullptr) {
      return;
    }
    jclass rawClass = env->FindClass("ai/gxlself/remotescreen/rsnative/jni/RawSink");
    if (rsjni::clearException(env, "RawSink.FindClass") || rawClass == nullptr) {
      env->DeleteLocalRef(sessionClass);
      return;
    }
    onEventMethod = env->GetMethodID(sessionClass, "onEvent", "(IIIIIIIZFFJJIJIIF[B[B)V");
    rsjni::clearException(env, "SessionSink.onEvent");
    onStreamCropMethod = env->GetMethodID(sessionClass, "onStreamCrop", "(IFFFFII)V");
    rsjni::clearException(env, "SessionSink.onStreamCrop");
    onRawReadyMethod = env->GetMethodID(rawClass, "onRawReady", "(JII)V");
    rsjni::clearException(env, "RawSink.onRawReady");
    if (onEventMethod != nullptr && onStreamCropMethod != nullptr && onRawReadyMethod != nullptr) {
      sinkClass = static_cast<jclass>(env->NewGlobalRef(rawClass));
      rsjni::clearException(env, "RawSink.NewGlobalRef");
    }
    env->DeleteLocalRef(rawClass);
    env->DeleteLocalRef(sessionClass);
  });
  return sinkClass != nullptr;
}

bool copyFailed(const uint8_t* bytes, size_t length, jbyteArray array) {
  return bytes != nullptr && length != 0 && array == nullptr;
}

void onReadyC(void* rawCtx, uint64_t seq, uint32_t width, uint32_t height) {
  auto* ctx = static_cast<RawCtx*>(rawCtx);
  RawCallbackScope scope(ctx);
  JNIEnv* env = rsjni::attachedEnv();
  rsjni::LocalFrame locals(env, 4);
  if (!locals.ok() || ctx == nullptr) {
    return;
  }
  env->CallVoidMethod(ctx->sink, onRawReadyMethod, static_cast<jlong>(seq),
                      static_cast<jint>(width), static_cast<jint>(height));
  rsjni::clearException(env, "onRawReady");
}

void onEventC(void* rawCtx, const RsEvent* event) {
  auto* ctx = static_cast<RawCtx*>(rawCtx);
  RawCallbackScope scope(ctx);
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
Java_ai_gxlself_remotescreen_rsnative_jni_RsRawNative_nativeAttach(
    JNIEnv* env, jclass, jlong rawId, jobject sink) {
  // Callback threads must not perform class lookup or wait for attachment.
  if (callbackDepth != 0) {
    return JNI_FALSE;
  }
  bool accepted = false;
  if (sink != nullptr && cacheSinkMethods(env)) {
    auto ctx = std::make_unique<RawCtx>();
    ctx->sink = env->NewGlobalRef(sink);
    if (!rsjni::clearException(env, "RawSink.NewGlobalRef") && ctx->sink != nullptr) {
      const uint64_t id = static_cast<uint64_t>(rawId);
      accepted = registry.attach(id, std::move(ctx), [id](RawCtx* candidate) {
        return rs_raw_attach(id, candidate, onReadyC, onEventC);
      });
    }
  }
  rsjni::logInfo("JNI", "RS-JNI {\"raw\":\"attach\",\"ok\":%s}", accepted ? "true" : "false");
  return accepted ? JNI_TRUE : JNI_FALSE;
}

extern "C" JNIEXPORT void JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsRawNative_nativeDetach(
    JNIEnv*, jclass, jlong rawId) {
  if (callbackDepth != 0) {
    return;
  }
  const uint64_t id = static_cast<uint64_t>(rawId);
  registry.detach(id, [id] { rs_raw_detach(id); });
  rsjni::logInfo("JNI", "RS-JNI {\"raw\":\"detach\"}");
}

extern "C" JNIEXPORT jlong JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsRawNative_nativeCopy(
    JNIEnv* env, jclass, jlong rawId, jlong sinceSeq, jobject dst, jint stride, jint width,
    jint height, jintArray rect) {
  // rs_raw_copy must not run inside this file's callbacks (rs_mobile.h).
  if (callbackDepth != 0 || dst == nullptr || rect == nullptr) {
    return 0;
  }
  const int64_t rowBytes = static_cast<int64_t>(width) * 4;
  if (width <= 0 || height <= 0 || stride < rowBytes) {
    return 0;
  }
  const jsize rectLength = env->GetArrayLength(rect);
  if (rsjni::clearException(env, "RawCopy.GetArrayLength") || rectLength < 4) {
    return 0;
  }
  auto* pixels = static_cast<uint8_t*>(env->GetDirectBufferAddress(dst));
  const jlong capacity = env->GetDirectBufferCapacity(dst);
  const int64_t required = static_cast<int64_t>(stride) * (height - 1) + rowBytes;
  if (pixels == nullptr || capacity < required) {
    return 0;
  }

  RsRect copied{};
  const uint64_t seq = rs_raw_copy(static_cast<uint64_t>(rawId), static_cast<uint64_t>(sinceSeq),
                                   pixels, static_cast<size_t>(stride),
                                   static_cast<uint32_t>(width), static_cast<uint32_t>(height),
                                   static_cast<uint8_t>(RS_PIX_RGBA8), &copied);
  const jint values[4] = {
      static_cast<jint>(copied.x), static_cast<jint>(copied.y),
      static_cast<jint>(copied.w), static_cast<jint>(copied.h),
  };
  env->SetIntArrayRegion(rect, 0, 4, values);
  rsjni::clearException(env, "RawCopy.SetIntArrayRegion");
  return static_cast<jlong>(seq);
}
