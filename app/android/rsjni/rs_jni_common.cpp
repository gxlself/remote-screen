#include "rs_jni_common.h"

#include <android/log.h>
#include <pthread.h>

#include <atomic>
#include <cstdarg>
#include <cstdio>
#include <limits>

#include "rs_mobile.h"

namespace {

std::atomic<JavaVM*> javaVm{nullptr};
pthread_once_t detachKeyOnce = PTHREAD_ONCE_INIT;
pthread_key_t detachKey;
int detachKeyError = 0;

void detachThread(void* value) {
  auto* threadVm = static_cast<JavaVM*>(value);
  if (threadVm != nullptr) {
    threadVm->DetachCurrentThread();
  }
}

void createDetachKey() {
  detachKeyError = pthread_key_create(&detachKey, detachThread);
}

}  // namespace

namespace rsjni {

JavaVM* vm() {
  return javaVm.load(std::memory_order_acquire);
}

JNIEnv* attachedEnv() {
  JavaVM* currentVm = vm();
  if (currentVm == nullptr) {
    return nullptr;
  }

  JNIEnv* env = nullptr;
  const jint status = currentVm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6);
  if (status == JNI_OK) {
    return env;
  }
  if (status != JNI_EDETACHED) {
    logInfo("JNI", "GetEnv failed: %d", status);
    return nullptr;
  }

  if (pthread_once(&detachKeyOnce, createDetachKey) != 0 || detachKeyError != 0) {
    logInfo("JNI", "Cannot create thread detach key");
    return nullptr;
  }
  if (currentVm->AttachCurrentThreadAsDaemon(&env, nullptr) != JNI_OK) {
    logInfo("JNI", "AttachCurrentThreadAsDaemon failed");
    return nullptr;
  }
  if (pthread_setspecific(detachKey, currentVm) != 0) {
    currentVm->DetachCurrentThread();
    logInfo("JNI", "Cannot register thread detach cleanup");
    return nullptr;
  }
  return env;
}

LocalFrame::LocalFrame(JNIEnv* env, jint cap)
    : env_(env), pushed_(env != nullptr && cap >= 0 && env->PushLocalFrame(cap) == JNI_OK) {
  if (!pushed_) {
    clearException(env_, "PushLocalFrame");
  }
}

LocalFrame::~LocalFrame() {
  if (pushed_) {
    env_->PopLocalFrame(nullptr);
  }
}

bool LocalFrame::ok() const {
  return pushed_;
}

bool clearException(JNIEnv* env, const char* where) {
  if (env == nullptr || !env->ExceptionCheck()) {
    return false;
  }
  env->ExceptionDescribe();
  env->ExceptionClear();
  logInfo("JNI", "Cleared Java exception at %s", where != nullptr ? where : "unknown");
  return true;
}

jbyteArray newByteArray(JNIEnv* env, const uint8_t* p, size_t n) {
  if (env == nullptr || p == nullptr || n == 0 ||
      n > static_cast<size_t>(std::numeric_limits<jint>::max())) {
    return nullptr;
  }
  const auto length = static_cast<jsize>(n);
  jbyteArray result = env->NewByteArray(length);
  if (clearException(env, "NewByteArray") || result == nullptr) {
    return nullptr;
  }
  env->SetByteArrayRegion(result, 0, length, reinterpret_cast<const jbyte*>(p));
  if (clearException(env, "SetByteArrayRegion")) {
    env->DeleteLocalRef(result);
    return nullptr;
  }
  return result;
}

std::string bytesToString(JNIEnv* env, jbyteArray a) {
  if (env == nullptr || a == nullptr) {
    return {};
  }
  const jsize length = env->GetArrayLength(a);
  if (clearException(env, "GetArrayLength") || length == 0) {
    return {};
  }
  std::string result(static_cast<size_t>(length), '\0');
  env->GetByteArrayRegion(a, 0, length, reinterpret_cast<jbyte*>(result.data()));
  if (clearException(env, "GetByteArrayRegion")) {
    return {};
  }
  return result;
}

bool parseSessionId(JNIEnv*, jlong raw, uint64_t* out) {
  if (out == nullptr) {
    return false;
  }
  // Signed-to-unsigned conversion is modulo 2^64, preserving the original bits.
  *out = static_cast<uint64_t>(raw);
  return true;
}

void logInfo(const char* tag, const char* fmt, ...) {
  char message[1024];
  va_list args;
  va_start(args, fmt);
  std::vsnprintf(message, sizeof(message), fmt, args);
  va_end(args);
  __android_log_print(ANDROID_LOG_INFO, "RS", "[%s] %s", tag != nullptr ? tag : "JNI", message);
}

}  // namespace rsjni

extern "C" JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM* vm, void*) {
  javaVm.store(vm, std::memory_order_release);
  return JNI_VERSION_1_6;
}

extern "C" JNIEXPORT jlong JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsNative_nativeNowUs(JNIEnv*, jclass) {
  return static_cast<jlong>(rs_now_us());
}

extern "C" JNIEXPORT jintArray JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsNative_nativeAbiSizes(JNIEnv* env, jclass) {
  const jint sizes[] = {sizeof(RsFrame), sizeof(RsEvent), sizeof(RsAudioStats)};
  jintArray result = env->NewIntArray(3);
  if (rsjni::clearException(env, "NewIntArray") || result == nullptr) {
    return nullptr;
  }
  env->SetIntArrayRegion(result, 0, 3, sizes);
  if (rsjni::clearException(env, "SetIntArrayRegion")) {
    env->DeleteLocalRef(result);
    return nullptr;
  }
  return result;
}
