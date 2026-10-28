#include "rs_attach_registry.h"
#include "rs_jni_common.h"
#include "rs_mobile.h"

#include <cstdint>
#include <memory>
#include <mutex>
#include <vector>

namespace {

struct TermCtx {
  jobject sink = nullptr;

  ~TermCtx() {
    if (JNIEnv* env = rsjni::attachedEnv(); env != nullptr && sink != nullptr) {
      env->DeleteGlobalRef(sink);
    }
  }
};

rsjni::AttachRegistry<TermCtx> registry;
std::once_flag sinkMethodsOnce;
jclass sinkClass = nullptr;
jmethodID onOutputMethod = nullptr;
jmethodID onClosedMethod = nullptr;

bool cacheSinkMethods(JNIEnv* env) {
  if (env == nullptr) {
    return false;
  }
  std::call_once(sinkMethodsOnce, [env] {
    jclass localClass = env->FindClass("ai/gxlself/remotescreen/rsnative/jni/TermSink");
    if (rsjni::clearException(env, "TermSink.FindClass") || localClass == nullptr) {
      return;
    }

    const jmethodID output = env->GetMethodID(localClass, "onOutput", "([B)V");
    if (rsjni::clearException(env, "TermSink.onOutput") || output == nullptr) {
      env->DeleteLocalRef(localClass);
      return;
    }
    const jmethodID closed = env->GetMethodID(localClass, "onClosed", "()V");
    if (rsjni::clearException(env, "TermSink.onClosed") || closed == nullptr) {
      env->DeleteLocalRef(localClass);
      return;
    }

    jclass globalClass = static_cast<jclass>(env->NewGlobalRef(localClass));
    if (rsjni::clearException(env, "TermSink.NewGlobalRef") || globalClass == nullptr) {
      env->DeleteLocalRef(localClass);
      return;
    }
    sinkClass = globalClass;
    onOutputMethod = output;
    onClosedMethod = closed;
    env->DeleteLocalRef(localClass);
  });
  return sinkClass != nullptr && onOutputMethod != nullptr && onClosedMethod != nullptr;
}

void onOutputC(void* rawCtx, const uint8_t* buf, size_t len) {
  auto* ctx = static_cast<TermCtx*>(rawCtx);
  rsjni::AttachRegistry<TermCtx>::CallbackScope scope(registry, ctx);
  JNIEnv* env = rsjni::attachedEnv();
  rsjni::LocalFrame locals(env, 4);
  if (!locals.ok() || ctx == nullptr || ctx->sink == nullptr) {
    return;
  }

  if (buf == nullptr && len == 0) {
    env->CallVoidMethod(ctx->sink, onClosedMethod);
    rsjni::clearException(env, "TermSink.onClosed");
    return;
  }
  if (len == 0) {
    return;
  }

  jbyteArray data = rsjni::newByteArray(env, buf, len);
  if (data == nullptr) {
    return;
  }
  env->CallVoidMethod(ctx->sink, onOutputMethod, data);
  rsjni::clearException(env, "TermSink.onOutput");
}

}  // namespace

extern "C" JNIEXPORT jboolean JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsTermNative_nativeAttach(
    JNIEnv* env, jclass, jlong rawId, jobject sink) {
  bool accepted = false;
  if (sink != nullptr && cacheSinkMethods(env)) {
    auto ctx = std::make_unique<TermCtx>();
    ctx->sink = env->NewGlobalRef(sink);
    if (!rsjni::clearException(env, "TermSink.NewGlobalRef") && ctx->sink != nullptr) {
      const uint64_t id = static_cast<uint64_t>(rawId);
      accepted = registry.attach(id, std::move(ctx), [id](TermCtx* candidate) {
        return rs_term_attach(id, candidate, onOutputC);
      });
    }
  }
  rsjni::logInfo("JNI", "RS-JNI {\"term\":\"attach\",\"ok\":%s}",
                 accepted ? "true" : "false");
  return accepted ? JNI_TRUE : JNI_FALSE;
}

extern "C" JNIEXPORT void JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsTermNative_nativeDetach(
    JNIEnv*, jclass, jlong rawId) {
  const uint64_t id = static_cast<uint64_t>(rawId);
  registry.detach(id, [id] { rs_term_detach(id); });
  rsjni::logInfo("JNI", "RS-JNI {\"term\":\"detach\"}");
}

extern "C" JNIEXPORT void JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsTermNative_nativeInput(
    JNIEnv* env, jclass, jlong rawId, jbyteArray data) {
  if (env == nullptr || data == nullptr) {
    return;
  }
  const jsize length = env->GetArrayLength(data);
  if (rsjni::clearException(env, "TermInput.GetArrayLength") || length <= 0) {
    return;
  }

  std::vector<uint8_t> bytes(static_cast<size_t>(length));
  env->GetByteArrayRegion(data, 0, length, reinterpret_cast<jbyte*>(bytes.data()));
  if (rsjni::clearException(env, "TermInput.GetByteArrayRegion")) {
    return;
  }
  rs_term_input(static_cast<uint64_t>(rawId), bytes.data(), bytes.size());
}

extern "C" JNIEXPORT void JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsTermNative_nativeResize(
    JNIEnv*, jclass, jlong rawId, jint cols, jint rows) {
  rs_term_resize(static_cast<uint64_t>(rawId), static_cast<uint16_t>(cols),
                 static_cast<uint16_t>(rows));
}
