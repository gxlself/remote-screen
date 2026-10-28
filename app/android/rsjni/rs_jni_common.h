#pragma once

#include <jni.h>

#include <cstddef>
#include <cstdint>
#include <string>

// Rules for every JNI bridge called from Rust worker threads:
// - Never FindClass on a Rust callback thread: its class loader is wrong. Resolve
//   classes and method IDs on a Java thread (a Java_... entry or JNI_OnLoad), and
//   cache classes/objects with NewGlobalRef before registering callbacks.
// - Wrap every callback in a LocalFrame, check ok(), and call clearException
//   after calling Java. JNIEnv and local references must not cross threads.
// - Pass UTF-8 strings as byte[]; NewStringUTF uses Modified UTF-8 and breaks emoji.
// - Never synchronously wait for the main thread from a callback.
// - Rust callback pointers/buffers are valid only during that callback. Copy any
//   data that will be needed later, before returning to Rust.
namespace rsjni {

JavaVM* vm();
// Attaches detached threads as daemons; only threads attached here are detached
// by our pthread-key destructor when they exit.
JNIEnv* attachedEnv();

class LocalFrame {
 public:
  explicit LocalFrame(JNIEnv* env, jint cap = 16);
  ~LocalFrame();
  LocalFrame(const LocalFrame&) = delete;
  LocalFrame& operator=(const LocalFrame&) = delete;
  bool ok() const;

 private:
  JNIEnv* env_;
  bool pushed_;
};

bool clearException(JNIEnv* env, const char* where);
jbyteArray newByteArray(JNIEnv* env, const uint8_t* p, size_t n);
std::string bytesToString(JNIEnv* env, jbyteArray a);
// Preserve all 64 bits, including negative jlong values from ULong.toLong().
// Returns false only when out is null; session existence is checked by Rust.
bool parseSessionId(JNIEnv* env, jlong raw, uint64_t* out);
void logInfo(const char* tag, const char* fmt, ...);

}  // namespace rsjni
