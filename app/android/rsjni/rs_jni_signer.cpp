#include "rs_jni_common.h"
#include "rs_sign_buffer.h"

#include "rs_mobile.h"

#include <jni.h>

#include <array>
#include <cstdint>
#include <limits>
#include <mutex>
#include <vector>

namespace {

constexpr char kSignerClassName[] =
    "ai/gxlself/remotescreen/rsnative/securekey/SecureKeySigner";
constexpr char kSignMethodName[] = "signFromNative";
constexpr char kSignMethodSignature[] = "([B)[B";
constexpr size_t kSpkiLength = 91;

std::mutex signerMutex;
jclass signerClass = nullptr;
jmethodID signerMethod = nullptr;
// Keep old class references alive while Rust may still be finishing a callback
// from a previous registration. Registrations happen at most once per launch.
std::vector<jclass> retainedSignerClasses;

void clearJavaException(JNIEnv* env) {
  if (env != nullptr && env->ExceptionCheck() == JNI_TRUE) {
    env->ExceptionClear();
  }
}

int32_t signC(void*,
              const uint8_t* msg,
              size_t len,
              uint8_t* out,
              size_t* outLen) {
  if (msg == nullptr || len == 0 || len > static_cast<size_t>(std::numeric_limits<jint>::max())) {
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
    std::lock_guard<std::mutex> lock(signerMutex);
    cls = signerClass;
    method = signerMethod;
  }
  if (cls == nullptr || method == nullptr) {
    return -1;
  }

  jbyteArray message = rsjni::newByteArray(env, msg, len);
  if (message == nullptr) {
    return -1;
  }
  jobject returned = env->CallStaticObjectMethod(cls, method, message);
  if (env->ExceptionCheck() == JNI_TRUE) {
    clearJavaException(env);
    return -1;
  }
  if (returned == nullptr) {
    return -1;
  }

  const auto signature = static_cast<jbyteArray>(returned);
  const jsize signatureLength = env->GetArrayLength(signature);
  if (env->ExceptionCheck() == JNI_TRUE || signatureLength <= 0) {
    clearJavaException(env);
    return -1;
  }
  std::vector<uint8_t> bytes(static_cast<size_t>(signatureLength));
  env->GetByteArrayRegion(
      signature,
      0,
      signatureLength,
      reinterpret_cast<jbyte*>(bytes.data()));
  if (env->ExceptionCheck() == JNI_TRUE) {
    clearJavaException(env);
    return -1;
  }
  return rsjni::writeSignature(bytes.data(), bytes.size(), out, outLen);
}

}  // namespace

extern "C" JNIEXPORT jboolean JNICALL
Java_ai_gxlself_remotescreen_rsnative_securekey_SecureKeyNative_nativeRegister(
    JNIEnv* env,
    jclass,
    jbyteArray spki) {
  if (env == nullptr || spki == nullptr) {
    return JNI_FALSE;
  }

  const jsize length = env->GetArrayLength(spki);
  if (env->ExceptionCheck() == JNI_TRUE || length != static_cast<jsize>(kSpkiLength)) {
    clearJavaException(env);
    return JNI_FALSE;
  }

  std::array<uint8_t, kSpkiLength> spkiBytes{};
  env->GetByteArrayRegion(
      spki,
      0,
      length,
      reinterpret_cast<jbyte*>(spkiBytes.data()));
  if (env->ExceptionCheck() == JNI_TRUE) {
    clearJavaException(env);
    return JNI_FALSE;
  }

  jclass localClass = env->FindClass(kSignerClassName);
  if (env->ExceptionCheck() == JNI_TRUE || localClass == nullptr) {
    clearJavaException(env);
    return JNI_FALSE;
  }
  jmethodID localMethod = env->GetStaticMethodID(
      localClass,
      kSignMethodName,
      kSignMethodSignature);
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
    std::lock_guard<std::mutex> lock(signerMutex);
    signerClass = globalClass;
    signerMethod = localMethod;
    retainedSignerClasses.push_back(globalClass);
  }

  rs_register_signer(nullptr, signC, spkiBytes.data(), spkiBytes.size());
  return JNI_TRUE;
}
