#include "rs_jni_common.h"

#include "rs_mobile.h"

#include <jni.h>

#include <algorithm>
#include <cstdint>
#include <cstring>
#include <mutex>
#include <vector>

namespace {

constexpr char kStoreClassName[] =
    "ai/gxlself/remotescreen/rsnative/adapters/AdapterSecretStore";
constexpr char kGetMethodName[] = "getFromNative";
constexpr char kPutMethodName[] = "putFromNative";
constexpr char kDeleteMethodName[] = "deleteFromNative";
constexpr char kGetMethodSignature[] = "([B)[B";
constexpr char kPutMethodSignature[] = "([B[B)Z";
constexpr char kDeleteMethodSignature[] = "([B)Z";
constexpr size_t kMaxKeyLength = 300;
constexpr size_t kMaxValueLength = 4096;

std::mutex storeMutex;
jclass storeClass = nullptr;
jmethodID getMethod = nullptr;
jmethodID putMethod = nullptr;
jmethodID deleteMethod = nullptr;
std::vector<jclass> retainedStoreClasses;

void clearJavaException(JNIEnv* env) {
  if (env != nullptr && env->ExceptionCheck() == JNI_TRUE) {
    env->ExceptionClear();
  }
}

bool keyLength(const char* key, size_t* length) {
  if (key == nullptr || length == nullptr) return false;
  const size_t measured = std::strlen(key);
  if (measured == 0 || measured > kMaxKeyLength) return false;
  *length = measured;
  return true;
}

bool cachedMethods(jclass* cls, jmethodID* get, jmethodID* put, jmethodID* del) {
  std::lock_guard<std::mutex> lock(storeMutex);
  if (cls != nullptr) *cls = storeClass;
  if (get != nullptr) *get = getMethod;
  if (put != nullptr) *put = putMethod;
  if (del != nullptr) *del = deleteMethod;
  return storeClass != nullptr && getMethod != nullptr && putMethod != nullptr &&
      deleteMethod != nullptr;
}

int32_t getC(void*, const char* key, uint8_t* buf, size_t* len) {
  size_t keyLen = 0;
  if (!keyLength(key, &keyLen) || len == nullptr) return -1;

  JNIEnv* env = rsjni::attachedEnv();
  if (env == nullptr) return -1;
  rsjni::LocalFrame frame(env, 16);
  if (!frame.ok()) return -1;

  jclass cls = nullptr;
  jmethodID method = nullptr;
  if (!cachedMethods(&cls, &method, nullptr, nullptr)) return -1;

  auto keyArray = rsjni::newByteArray(
      env, reinterpret_cast<const uint8_t*>(key), keyLen);
  if (keyArray == nullptr) return -1;
  jobject returned = env->CallStaticObjectMethod(cls, method, keyArray);
  if (env->ExceptionCheck() == JNI_TRUE) {
    clearJavaException(env);
    return -1;
  }
  if (returned == nullptr) return 1;

  const auto valueArray = static_cast<jbyteArray>(returned);
  const jsize valueLength = env->GetArrayLength(valueArray);
  if (env->ExceptionCheck() == JNI_TRUE || valueLength < 0 ||
      static_cast<size_t>(valueLength) > kMaxValueLength) {
    clearJavaException(env);
    return -1;
  }
  const size_t capacity = *len;
  if (static_cast<size_t>(valueLength) > capacity ||
      (valueLength > 0 && buf == nullptr)) {
    return -1;
  }

  try {
    std::vector<uint8_t> bytes(static_cast<size_t>(valueLength));
    if (valueLength > 0) {
      env->GetByteArrayRegion(
          valueArray,
          0,
          valueLength,
          reinterpret_cast<jbyte*>(bytes.data()));
      if (env->ExceptionCheck() == JNI_TRUE) {
        clearJavaException(env);
        std::fill(bytes.begin(), bytes.end(), 0);
        return -1;
      }
      std::memcpy(buf, bytes.data(), bytes.size());
    }
    *len = static_cast<size_t>(valueLength);
    std::fill(bytes.begin(), bytes.end(), 0);
    return 0;
  } catch (...) {
    return -1;
  }
}

int32_t putC(void*, const char* key, const uint8_t* value, size_t len) {
  size_t keyLen = 0;
  if (!keyLength(key, &keyLen) || value == nullptr || len == 0 || len > kMaxValueLength) {
    return -1;
  }

  JNIEnv* env = rsjni::attachedEnv();
  if (env == nullptr) return -1;
  rsjni::LocalFrame frame(env, 16);
  if (!frame.ok()) return -1;

  jclass cls = nullptr;
  jmethodID method = nullptr;
  if (!cachedMethods(&cls, nullptr, &method, nullptr)) return -1;
  auto keyArray = rsjni::newByteArray(
      env, reinterpret_cast<const uint8_t*>(key), keyLen);
  auto valueArray = rsjni::newByteArray(env, value, len);
  if (keyArray == nullptr || valueArray == nullptr) return -1;

  const jboolean accepted = env->CallStaticBooleanMethod(cls, method, keyArray, valueArray);
  if (env->ExceptionCheck() == JNI_TRUE) {
    clearJavaException(env);
    return -1;
  }
  return accepted == JNI_TRUE ? 0 : -1;
}

int32_t delC(void*, const char* key) {
  size_t keyLen = 0;
  if (!keyLength(key, &keyLen)) return -1;

  JNIEnv* env = rsjni::attachedEnv();
  if (env == nullptr) return -1;
  rsjni::LocalFrame frame(env, 12);
  if (!frame.ok()) return -1;

  jclass cls = nullptr;
  jmethodID method = nullptr;
  if (!cachedMethods(&cls, nullptr, nullptr, &method)) return -1;
  auto keyArray = rsjni::newByteArray(
      env, reinterpret_cast<const uint8_t*>(key), keyLen);
  if (keyArray == nullptr) return -1;
  const jboolean accepted = env->CallStaticBooleanMethod(cls, method, keyArray);
  if (env->ExceptionCheck() == JNI_TRUE) {
    clearJavaException(env);
    return -1;
  }
  return accepted == JNI_TRUE ? 0 : -1;
}

}  // namespace

extern "C" JNIEXPORT jboolean JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsAdapterSecretNative_nativeRegister(
    JNIEnv* env, jclass) {
  if (env == nullptr) return JNI_FALSE;

  jclass localClass = env->FindClass(kStoreClassName);
  if (env->ExceptionCheck() == JNI_TRUE || localClass == nullptr) {
    clearJavaException(env);
    return JNI_FALSE;
  }
  const jmethodID localGet = env->GetStaticMethodID(
      localClass, kGetMethodName, kGetMethodSignature);
  const jmethodID localPut = env->GetStaticMethodID(
      localClass, kPutMethodName, kPutMethodSignature);
  const jmethodID localDelete = env->GetStaticMethodID(
      localClass, kDeleteMethodName, kDeleteMethodSignature);
  if (env->ExceptionCheck() == JNI_TRUE || localGet == nullptr || localPut == nullptr ||
      localDelete == nullptr) {
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
    std::lock_guard<std::mutex> lock(storeMutex);
    storeClass = globalClass;
    getMethod = localGet;
    putMethod = localPut;
    deleteMethod = localDelete;
    retainedStoreClasses.push_back(globalClass);
  }
  rs_register_secret_store(nullptr, getC, putC, delC);
  return JNI_TRUE;
}
