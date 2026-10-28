#include "rs_jni_common.h"

#include "rs_mobile.h"

#include <jni.h>

#include <cstddef>
#include <cstdint>
#include <mutex>
#include <vector>

namespace {

constexpr char kRecordingKeyClassName[] =
    "ai/gxlself/remotescreen/rsnative/recording/RecordingKeyNative";
constexpr char kWrapMethodName[] = "wrapFromNative";
constexpr char kWrapMethodSignature[] = "(I[B)[B";
constexpr size_t kMaxInputLength = 64;

std::mutex recordingKeyMutex;
jclass recordingKeyClass = nullptr;
jmethodID recordingKeyMethod = nullptr;
// Keep old class references alive while Rust may still be finishing a callback
// from a previous registration. Registrations happen at most once per launch.
std::vector<jclass> retainedRecordingKeyClasses;

void clearJavaException(JNIEnv* env) {
  if (env != nullptr && env->ExceptionCheck() == JNI_TRUE) {
    env->ExceptionClear();
  }
}

int32_t wrapC(void*,
              uint8_t op,
              const uint8_t* in,
              size_t inLen,
              uint8_t* out,
              size_t* outLen) {
  if (in == nullptr || inLen > kMaxInputLength) {
    return -2;
  }
  if (outLen == nullptr) {
    return -1;
  }

  JNIEnv* env = rsjni::attachedEnv();
  if (env == nullptr) {
    return -1;
  }
  rsjni::LocalFrame frame(env, 16);
  if (!frame.ok()) {
    return -1;
  }

  jclass cls = nullptr;
  jmethodID method = nullptr;
  {
    std::lock_guard<std::mutex> lock(recordingKeyMutex);
    cls = recordingKeyClass;
    method = recordingKeyMethod;
  }
  if (cls == nullptr || method == nullptr) {
    return -1;
  }

  jbyteArray input = nullptr;
  if (inLen == 0) {
    input = env->NewByteArray(0);
    if (env->ExceptionCheck() == JNI_TRUE || input == nullptr) {
      clearJavaException(env);
      return -1;
    }
  } else {
    input = rsjni::newByteArray(env, in, inLen);
    if (input == nullptr) {
      return -1;
    }
  }

  jobject returned = env->CallStaticObjectMethod(
      cls,
      method,
      static_cast<jint>(op),
      input);
  if (env->ExceptionCheck() == JNI_TRUE) {
    clearJavaException(env);
    return -1;
  }
  if (returned == nullptr) {
    return -1;
  }

  const auto bytes = static_cast<jbyteArray>(returned);
  const jsize returnedLength = env->GetArrayLength(bytes);
  if (env->ExceptionCheck() == JNI_TRUE) {
    clearJavaException(env);
    return -1;
  }
  if (returnedLength == 0) {
    return -2;
  }

  const size_t resultLength = static_cast<size_t>(returnedLength);
  if (resultLength > *outLen || out == nullptr) {
    return -3;
  }
  env->GetByteArrayRegion(
      bytes,
      0,
      returnedLength,
      reinterpret_cast<jbyte*>(out));
  if (env->ExceptionCheck() == JNI_TRUE) {
    clearJavaException(env);
    return -1;
  }
  *outLen = resultLength;
  return 0;
}

}  // namespace

extern "C" JNIEXPORT jboolean JNICALL
Java_ai_gxlself_remotescreen_rsnative_recording_RecordingKeyNative_nativeRegister(
    JNIEnv* env,
    jclass) {
  if (env == nullptr) {
    return JNI_FALSE;
  }

  jclass localClass = env->FindClass(kRecordingKeyClassName);
  if (env->ExceptionCheck() == JNI_TRUE || localClass == nullptr) {
    clearJavaException(env);
    return JNI_FALSE;
  }
  jmethodID localMethod = env->GetStaticMethodID(
      localClass,
      kWrapMethodName,
      kWrapMethodSignature);
  if (env->ExceptionCheck() == JNI_TRUE || localMethod == nullptr) {
    clearJavaException(env);
    env->DeleteLocalRef(localClass);
    return JNI_FALSE;
  }

  jclass globalClass = static_cast<jclass>(env->NewGlobalRef(localClass));
  env->DeleteLocalRef(localClass);
  if (env->ExceptionCheck() == JNI_TRUE || globalClass == nullptr) {
    clearJavaException(env);
    return JNI_FALSE;
  }

  {
    std::lock_guard<std::mutex> lock(recordingKeyMutex);
    recordingKeyClass = globalClass;
    recordingKeyMethod = localMethod;
    retainedRecordingKeyClasses.push_back(globalClass);
  }

  rs_register_secret_wrapper(nullptr, wrapC);
  return JNI_TRUE;
}
