#define __ANDROID_UNAVAILABLE_SYMBOLS_ARE_WEAK__

#include "rs_jni_common.h"

#include "rs_mobile.h"

#include <aaudio/AAudio.h>

#include <atomic>
#include <cstdint>
#include <cstring>
#include <mutex>
#include <thread>

namespace {

std::atomic<RsAudio*> audioHandle{nullptr};
std::mutex audioMutex;
AAudioStream* audioStream = nullptr;
bool streamRequested = false;
bool reopenInFlight = false;
uint64_t reopenCount = 0;

aaudio_data_callback_result_t dataCallback(AAudioStream*, void*, void* audioData,
                                           int32_t numFrames) {
  RsAudio* handle = audioHandle.load(std::memory_order_acquire);
  if (audioData == nullptr || numFrames <= 0) {
    return AAUDIO_CALLBACK_RESULT_CONTINUE;
  }
  const size_t samples = static_cast<size_t>(numFrames) * 2U;
  if (handle != nullptr) {
    (void)rs_audio_render_interleaved(handle, static_cast<float*>(audioData),
                                      static_cast<uint32_t>(numFrames));
  } else {
    std::memset(audioData, 0, samples * sizeof(float));
  }
  return AAUDIO_CALLBACK_RESULT_CONTINUE;
}

void closeStreamLocked() {
  AAudioStream* stream = audioStream;
  audioStream = nullptr;
  if (stream == nullptr) {
    return;
  }
  (void)AAudioStream_requestStop(stream);
  AAudioStream_close(stream);
}

aaudio_result_t openAndStartStreamLocked() {
  if (!streamRequested || audioHandle.load(std::memory_order_acquire) == nullptr) {
    return AAUDIO_ERROR_INVALID_STATE;
  }
  if (audioStream != nullptr) {
    return AAUDIO_OK;
  }

  AAudioStreamBuilder* builder = nullptr;
  aaudio_result_t result = AAudio_createStreamBuilder(&builder);
  if (result != AAUDIO_OK || builder == nullptr) {
    return result != AAUDIO_OK ? result : AAUDIO_ERROR_INTERNAL;
  }
  AAudioStreamBuilder_setDirection(builder, AAUDIO_DIRECTION_OUTPUT);
  AAudioStreamBuilder_setSharingMode(builder, AAUDIO_SHARING_MODE_SHARED);
  AAudioStreamBuilder_setPerformanceMode(builder, AAUDIO_PERFORMANCE_MODE_LOW_LATENCY);
  AAudioStreamBuilder_setFormat(builder, AAUDIO_FORMAT_PCM_FLOAT);
  AAudioStreamBuilder_setChannelCount(builder, 2);
  AAudioStreamBuilder_setSampleRate(builder, 48000);
  AAudioStreamBuilder_setDataCallback(builder, dataCallback, nullptr);
  AAudioStreamBuilder_setErrorCallback(builder, [](AAudioStream* stream, void*, aaudio_result_t error) {
    if (error == AAUDIO_ERROR_DISCONNECTED) {
      // AAudio forbids closing a stream from its error callback. The helper
      // only schedules work; the detached worker takes the control mutex.
      std::thread([stream, error] {
        std::lock_guard<std::mutex> guard(audioMutex);
        if (!streamRequested || audioStream != stream ||
            audioHandle.load(std::memory_order_acquire) == nullptr) {
          return;
        }
        if (reopenInFlight) {
          return;
        }
        reopenInFlight = true;
        ++reopenCount;
        closeStreamLocked();
        const aaudio_result_t reopenResult = openAndStartStreamLocked();
        reopenInFlight = false;
        rsjni::logInfo(
            "AUDIO", "RS-AUDIO {\"disconnect\":true,\"reopen\":\"%s\",\"code\":%d}",
            reopenResult == AAUDIO_OK ? "ok" : "fail", static_cast<int>(error));
      }).detach();
    }
  }, nullptr);
  if (__builtin_available(android 28, *)) {
    AAudioStreamBuilder_setUsage(builder, AAUDIO_USAGE_MEDIA);
    AAudioStreamBuilder_setContentType(builder, AAUDIO_CONTENT_TYPE_MUSIC);
  }

  AAudioStream* opened = nullptr;
  result = AAudioStreamBuilder_openStream(builder, &opened);
  AAudioStreamBuilder_delete(builder);
  if (result != AAUDIO_OK || opened == nullptr) {
    return result != AAUDIO_OK ? result : AAUDIO_ERROR_INTERNAL;
  }

  const int32_t actualRate = AAudioStream_getSampleRate(opened);
  if (actualRate != 48000) {
    rsjni::logInfo("AUDIO", "RS-AUDIO {\"rate\":%d,\"expected\":48000}", actualRate);
    AAudioStream_close(opened);
    return AAUDIO_ERROR_INVALID_RATE;
  }

  const int32_t framesPerBurst = AAudioStream_getFramesPerBurst(opened);
  if (framesPerBurst > 0) {
    (void)AAudioStream_setBufferSizeInFrames(opened, framesPerBurst * 2);
  }
  audioStream = opened;
  result = AAudioStream_requestStart(opened);
  if (result != AAUDIO_OK) {
    audioStream = nullptr;
    AAudioStream_close(opened);
    return result;
  }
  return AAUDIO_OK;
}

}  // namespace

extern "C" JNIEXPORT jboolean JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsAudioNative_nativeOpen(
    JNIEnv*, jclass, jlong rawId) {
  uint64_t id = 0;
  if (!rsjni::parseSessionId(nullptr, rawId, &id)) {
    return JNI_FALSE;
  }

  std::lock_guard<std::mutex> guard(audioMutex);
  RsAudio* opened = rs_audio_open(id);
  if (opened == nullptr) {
    return JNI_FALSE;
  }
  streamRequested = false;
  closeStreamLocked();
  RsAudio* previous = audioHandle.exchange(opened, std::memory_order_acq_rel);
  reopenCount = 0;
  if (previous != nullptr) {
    rs_audio_close(previous);
  }
  return JNI_TRUE;
}

extern "C" JNIEXPORT void JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsAudioNative_nativeClose(JNIEnv*, jclass) {
  std::lock_guard<std::mutex> guard(audioMutex);
  streamRequested = false;
  RsAudio* previous = audioHandle.exchange(nullptr, std::memory_order_acq_rel);
  closeStreamLocked();
  if (previous != nullptr) {
    rs_audio_close(previous);
  }
}

extern "C" JNIEXPORT jint JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsAudioNative_nativeStartStream(JNIEnv*, jclass) {
  std::lock_guard<std::mutex> guard(audioMutex);
  if (audioHandle.load(std::memory_order_acquire) == nullptr) {
    return AAUDIO_ERROR_INVALID_STATE;
  }
  streamRequested = true;
  // Repeated starts are intentionally idempotent while AAudio owns a stream.
  const aaudio_result_t result = openAndStartStreamLocked();
  if (result != AAUDIO_OK) {
    streamRequested = false;
  }
  return static_cast<jint>(result);
}

extern "C" JNIEXPORT void JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsAudioNative_nativeStopStream(JNIEnv*, jclass) {
  std::lock_guard<std::mutex> guard(audioMutex);
  streamRequested = false;
  closeStreamLocked();
}

extern "C" JNIEXPORT void JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsAudioNative_nativeSetMuted(
    JNIEnv*, jclass, jboolean muted) {
  std::lock_guard<std::mutex> guard(audioMutex);
  RsAudio* handle = audioHandle.load(std::memory_order_acquire);
  if (handle != nullptr) {
    rs_audio_set_muted(handle, muted != JNI_FALSE);
  }
}

extern "C" JNIEXPORT jboolean JNICALL
Java_ai_gxlself_remotescreen_rsnative_jni_RsAudioNative_nativeStats(
    JNIEnv* env, jclass, jlongArray out) {
  if (env == nullptr || out == nullptr || env->GetArrayLength(out) < 14) {
    return JNI_FALSE;
  }
  std::lock_guard<std::mutex> guard(audioMutex);
  RsAudio* handle = audioHandle.load(std::memory_order_acquire);
  if (handle == nullptr) {
    return JNI_FALSE;
  }

  RsAudioStats stats{};
  rs_audio_stats(handle, &stats);
  jlong values[14] = {
      static_cast<jlong>(stats.packets),
      static_cast<jlong>(stats.lost),
      static_cast<jlong>(stats.concealed),
      static_cast<jlong>(stats.fec_recovered),
      static_cast<jlong>(stats.late),
      static_cast<jlong>(stats.underruns),
      static_cast<jlong>(stats.target_ms),
      static_cast<jlong>(stats.level_ms),
      static_cast<jlong>(stats.contended),
      static_cast<jlong>(stats.playing ? 1 : 0),
      0,
      0,
      0,
      0,
  };
  if (audioStream != nullptr) {
    if (__builtin_available(android 26, *)) {
      values[10] = static_cast<jlong>(AAudioStream_getXRunCount(audioStream));
    }
    values[11] = static_cast<jlong>(AAudioStream_getFramesPerBurst(audioStream));
    values[12] = static_cast<jlong>(AAudioStream_getBufferSizeInFrames(audioStream));
    values[13] = static_cast<jlong>(reopenCount);
  }
  env->SetLongArrayRegion(out, 0, 14, values);
  if (rsjni::clearException(env, "RsAudioNative.SetLongArrayRegion")) {
    return JNI_FALSE;
  }
  return JNI_TRUE;
}
