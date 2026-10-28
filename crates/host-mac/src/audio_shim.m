// ScreenCaptureKit system-audio capture. This stream deliberately has a tiny
// throw-away screen output so that the audio stream is independent of the
// video capture pipeline.

#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <CoreAudioTypes/CoreAudioTypes.h>
#import <QuartzCore/QuartzCore.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <Foundation/Foundation.h>

#include <math.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

typedef void (*rs_audio_cb)(void *ctx, uint64_t capture_ts_us, uint32_t channels,
                            uint32_t frames, const float *interleaved);

// Defined by shim.m. The audio permission is covered by the same TCC grant as
// screen recording, so use the existing check and its user-facing wording.
extern int32_t rs_screen_capture_allowed(int32_t request);
// Pure Rust helpers are shared by the live callback and unit tests.
extern uint64_t rs_audio_pts_to_unix_us(uint64_t now_unix_us, double now_host_s, double pts_s);
extern bool rs_audio_interleave_f32(const float *const *planes, uint32_t channels,
                                   uint32_t frames, float *out);
extern void rs_audio_report_dropped(void *ctx, uint64_t count);

static uint64_t audio_unix_now_us(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (uint64_t)ts.tv_sec * 1000000ull + (uint64_t)ts.tv_nsec / 1000ull;
}

static void audio_set_error(int32_t *err, char *msg, size_t msg_len, int32_t code, NSString *text) {
    if (err) *err = code;
    if (msg && msg_len) strlcpy(msg, text.UTF8String ?: "audio capture failed", msg_len);
}

static uint16_t audio_swap16(uint16_t value) {
    return (uint16_t)((value >> 8) | (value << 8));
}

static uint32_t audio_swap32(uint32_t value) {
    return ((value & 0x000000ffu) << 24) |
           ((value & 0x0000ff00u) << 8) |
           ((value & 0x00ff0000u) >> 8) |
           ((value & 0xff000000u) >> 24);
}

static uint64_t audio_swap64(uint64_t value) {
    return ((value & 0x00000000000000ffull) << 56) |
           ((value & 0x000000000000ff00ull) << 40) |
           ((value & 0x0000000000ff0000ull) << 24) |
           ((value & 0x00000000ff000000ull) << 8) |
           ((value & 0x000000ff00000000ull) >> 8) |
           ((value & 0x0000ff0000000000ull) >> 24) |
           ((value & 0x00ff000000000000ull) >> 40) |
           ((value & 0xff00000000000000ull) >> 56);
}

static BOOL audio_format_supported(const AudioStreamBasicDescription *asbd) {
    if (!asbd || asbd->mFormatID != kAudioFormatLinearPCM || asbd->mChannelsPerFrame == 0 ||
        asbd->mSampleRate != 48000 || asbd->mBytesPerFrame == 0 || asbd->mBitsPerChannel == 0) {
        return NO;
    }
    BOOL isFloat = (asbd->mFormatFlags & kAudioFormatFlagIsFloat) != 0;
    BOOL supportedBits = isFloat
        ? (asbd->mBitsPerChannel == 32 || asbd->mBitsPerChannel == 64)
        : (asbd->mBitsPerChannel == 8 || asbd->mBitsPerChannel == 16 ||
           asbd->mBitsPerChannel == 24 || asbd->mBitsPerChannel == 32);
    BOOL isSigned = (asbd->mFormatFlags & kAudioFormatFlagIsSignedInteger) != 0;
    BOOL planar = (asbd->mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0;
    uint32_t frameChannels = planar ? 1 : asbd->mChannelsPerFrame;
    // Only tightly packed PCM is handled. Drop unknown/padded/fixed-point
    // encodings rather than interpreting them with the wrong sample stride.
    uint32_t allowedFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsSignedInteger |
                            kAudioFormatFlagIsBigEndian | kAudioFormatFlagIsPacked |
                            kAudioFormatFlagIsNonInterleaved | kAudioFormatFlagIsNonMixable;
    return supportedBits && (isFloat || isSigned || asbd->mBitsPerChannel == 8) &&
           (asbd->mFormatFlags & ~allowedFlags) == 0 &&
           asbd->mBytesPerFrame == frameChannels * (asbd->mBitsPerChannel / 8);
}

static float audio_decode_sample(const uint8_t *bytes, const AudioStreamBasicDescription *asbd) {
    const uint32_t bits = asbd->mBitsPerChannel;
    const BOOL bigEndian = (asbd->mFormatFlags & kAudioFormatFlagIsBigEndian) != 0;
    const BOOL isFloat = (asbd->mFormatFlags & kAudioFormatFlagIsFloat) != 0;
    const BOOL isSigned = (asbd->mFormatFlags & kAudioFormatFlagIsSignedInteger) != 0;

    if (isFloat && bits == 32) {
        uint32_t raw = 0;
        memcpy(&raw, bytes, sizeof(raw));
        if (bigEndian) raw = audio_swap32(raw);
        float value = 0.0f;
        memcpy(&value, &raw, sizeof(value));
        return isfinite(value) ? fmaxf(-1.0f, fminf(1.0f, value)) : 0.0f;
    }
    if (isFloat && bits == 64) {
        uint64_t raw = 0;
        memcpy(&raw, bytes, sizeof(raw));
        if (bigEndian) raw = audio_swap64(raw);
        double value = 0.0;
        memcpy(&value, &raw, sizeof(value));
        if (!isfinite(value)) return 0.0f;
        return (float)fmax(-1.0, fmin(1.0, value));
    }

    if (!isSigned && bits == 8) {
        return ((float)bytes[0] - 128.0f) / 128.0f;
    }
    if (bits == 8) {
        return (float)(*(const int8_t *)bytes) / 128.0f;
    }
    if (bits == 16) {
        uint16_t raw = 0;
        memcpy(&raw, bytes, sizeof(raw));
        if (bigEndian) raw = audio_swap16(raw);
        int16_t value;
        memcpy(&value, &raw, sizeof(value));
        return (float)value / 32768.0f;
    }
    if (bits == 24) {
        uint32_t raw;
        if (bigEndian) {
            raw = ((uint32_t)bytes[0] << 16) | ((uint32_t)bytes[1] << 8) | bytes[2];
        } else {
            raw = ((uint32_t)bytes[2] << 16) | ((uint32_t)bytes[1] << 8) | bytes[0];
        }
        if (raw & 0x00800000u) raw |= 0xff000000u;
        int32_t value;
        memcpy(&value, &raw, sizeof(value));
        return (float)value / 8388608.0f;
    }
    if (bits == 32) {
        uint32_t raw = 0;
        memcpy(&raw, bytes, sizeof(raw));
        if (bigEndian) raw = audio_swap32(raw);
        int32_t value;
        memcpy(&value, &raw, sizeof(value));
        return (float)value / 2147483648.0f;
    }
    return 0.0f;
}

@interface RSAudioCapture : NSObject <SCStreamOutput, SCStreamDelegate>
@end

@implementation RSAudioCapture {
@public
    SCStream *_stream;
    dispatch_queue_t _queue;
    void *_ctx;
    rs_audio_cb _cb;
    uint32_t _channels;
    atomic_bool _stopped;
    atomic_uint_fast64_t _unsupported;
    float *_interleaved;
    size_t _interleavedCapacity;
    AudioBufferList *_bufferList;
    size_t _bufferListCapacity;
}

- (instancetype)init {
    self = [super init];
    if (self) {
        atomic_init(&_stopped, false);
        atomic_init(&_unsupported, 0);
    }
    return self;
}

- (void)dealloc {
    free(_interleaved);
    free(_bufferList);
}

- (uint64_t)captureTimestampForSampleBuffer:(CMSampleBufferRef)sampleBuffer {
    CMTime pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer);
    double ptsSeconds = CMTIME_IS_VALID(pts) ? CMTimeGetSeconds(pts) : NAN;
    double nowHostSeconds = CACurrentMediaTime();
    uint64_t nowUnixUs = audio_unix_now_us();
    return rs_audio_pts_to_unix_us(nowUnixUs, nowHostSeconds, ptsSeconds);
}

- (BOOL)prepareBufferListForSampleBuffer:(CMSampleBufferRef)sampleBuffer {
    size_t needed = 0;
    OSStatus status = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
        sampleBuffer, &needed, NULL, 0, NULL, NULL, 0, NULL);
    if (needed < sizeof(AudioBufferList) ||
        (status != noErr && status != kCMSampleBufferError_ArrayTooSmall)) {
        atomic_fetch_add_explicit(&_unsupported, 1, memory_order_relaxed);
        return NO;
    }
    if (_bufferListCapacity < needed) {
        AudioBufferList *replacement = realloc(_bufferList, needed);
        if (!replacement) {
            atomic_fetch_add_explicit(&_unsupported, 1, memory_order_relaxed);
            return NO;
        }
        _bufferList = replacement;
        _bufferListCapacity = needed;
    }
    memset(_bufferList, 0, needed);
    return YES;
}

- (void)handleAudioSampleBuffer:(CMSampleBufferRef)sampleBuffer {
    if (!CMSampleBufferIsValid(sampleBuffer) || !CMSampleBufferDataIsReady(sampleBuffer) ||
        atomic_load_explicit(&_stopped, memory_order_acquire)) {
        return;
    }
    CMFormatDescriptionRef format = CMSampleBufferGetFormatDescription(sampleBuffer);
    const AudioStreamBasicDescription *asbd = format
        ? CMAudioFormatDescriptionGetStreamBasicDescription((CMAudioFormatDescriptionRef)format)
        : NULL;
    if (!audio_format_supported(asbd) || asbd->mChannelsPerFrame != _channels) {
        atomic_fetch_add_explicit(&_unsupported, 1, memory_order_relaxed);
        return;
    }
    CMItemCount sampleCount = CMSampleBufferGetNumSamples(sampleBuffer);
    if (sampleCount <= 0 || (uint64_t)sampleCount > UINT32_MAX ||
        ![self prepareBufferListForSampleBuffer:sampleBuffer]) {
        return;
    }

    CMBlockBufferRef retained = NULL;
    OSStatus status = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
        sampleBuffer, NULL, _bufferList, _bufferListCapacity, NULL, NULL,
        kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment, &retained);
    if (status != noErr) {
        if (retained) CFRelease(retained);
        atomic_fetch_add_explicit(&_unsupported, 1, memory_order_relaxed);
        return;
    }

    size_t frames = (size_t)sampleCount;
    size_t channels = (size_t)_channels;
    size_t totalSamples = 0;
    if (__builtin_mul_overflow(frames, channels, &totalSamples) ||
        totalSamples > SIZE_MAX / sizeof(float)) {
        if (retained) CFRelease(retained);
        atomic_fetch_add_explicit(&_unsupported, 1, memory_order_relaxed);
        return;
    }
    if (_interleavedCapacity < totalSamples) {
        float *replacement = realloc(_interleaved, totalSamples * sizeof(float));
        if (!replacement) {
            if (retained) CFRelease(retained);
            atomic_fetch_add_explicit(&_unsupported, 1, memory_order_relaxed);
            return;
        }
        _interleaved = replacement;
        _interleavedCapacity = totalSamples;
    }

    const size_t bytesPerSample = asbd->mBitsPerChannel / 8;
    const BOOL nonInterleaved = (asbd->mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0;
    BOOL valid = bytesPerSample > 0;
    if (nonInterleaved) {
        valid = valid && _bufferList->mNumberBuffers == _channels;
        const float *planes[2] = { NULL, NULL };
        for (size_t channel = 0; valid && channel < channels; channel++) {
            AudioBuffer *buffer = &_bufferList->mBuffers[channel];
            size_t bytesNeeded = 0;
            valid = !__builtin_mul_overflow(frames, bytesPerSample, &bytesNeeded) &&
                    buffer->mData != NULL && buffer->mNumberChannels == 1 &&
                    buffer->mDataByteSize >= bytesNeeded;
            if (!valid) break;
            const uint8_t *data = (const uint8_t *)buffer->mData;
            planes[channel] = (const float *)data;
        }
        BOOL nativeFloat = (asbd->mFormatFlags & kAudioFormatFlagIsFloat) != 0 &&
                           (asbd->mFormatFlags & kAudioFormatFlagIsBigEndian) == 0 &&
                           asbd->mBitsPerChannel == 32;
        if (valid && nativeFloat) {
            valid = rs_audio_interleave_f32(planes, _channels, (uint32_t)frames, _interleaved);
        } else if (valid) {
            for (size_t channel = 0; channel < channels; channel++) {
                const uint8_t *data = (const uint8_t *)_bufferList->mBuffers[channel].mData;
                for (size_t frame = 0; frame < frames; frame++) {
                    _interleaved[frame * channels + channel] =
                        audio_decode_sample(data + frame * bytesPerSample, asbd);
                }
            }
        }
    } else {
        valid = valid && _bufferList->mNumberBuffers == 1;
        AudioBuffer *buffer = valid ? &_bufferList->mBuffers[0] : NULL;
        size_t frameBytes = (size_t)asbd->mBytesPerFrame;
        size_t bytesNeeded = 0;
        valid = valid && frameBytes >= bytesPerSample * channels &&
                !__builtin_mul_overflow(frames, frameBytes, &bytesNeeded) &&
                buffer->mData != NULL && buffer->mNumberChannels == _channels &&
                buffer->mDataByteSize >= bytesNeeded;
        if (valid) {
            const uint8_t *data = (const uint8_t *)buffer->mData;
            for (size_t frame = 0; frame < frames; frame++) {
                const uint8_t *frameData = data + frame * frameBytes;
                for (size_t channel = 0; channel < channels; channel++) {
                    _interleaved[frame * channels + channel] =
                        audio_decode_sample(frameData + channel * bytesPerSample, asbd);
                }
            }
        }
    }
    if (retained) CFRelease(retained);
    if (!valid || atomic_load_explicit(&_stopped, memory_order_acquire) || !_cb || !_ctx) {
        if (!valid) atomic_fetch_add_explicit(&_unsupported, 1, memory_order_relaxed);
        return;
    }
    _cb(_ctx, [self captureTimestampForSampleBuffer:sampleBuffer], _channels,
        (uint32_t)frames, _interleaved);
}

- (void)stream:(SCStream *)stream didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer
         ofType:(SCStreamOutputType)type {
    (void)stream;
    if (type == SCStreamOutputTypeAudio) [self handleAudioSampleBuffer:sampleBuffer];
    // Screen buffers are intentionally discarded. Keeping this output attached
    // prevents ScreenCaptureKit from repeatedly reporting a missing consumer.
}

- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
    (void)stream;
    (void)error;
}
@end

bool rs_audio_capture_supported(void) {
    if (@available(macOS 13.0, *)) return true;
    return false;
}

static void stop_audio_capture(RSAudioCapture *capture) {
    if (!capture || atomic_exchange_explicit(&capture->_stopped, true, memory_order_acq_rel)) return;
    SCStream *stream = capture->_stream;
    if (stream) {
        dispatch_semaphore_t semaphore = dispatch_semaphore_create(0);
        [stream stopCaptureWithCompletionHandler:^(NSError *error) {
            (void)error;
            dispatch_semaphore_signal(semaphore);
        }];
        dispatch_semaphore_wait(semaphore, dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC));
        NSError *removeError = nil;
        [stream removeStreamOutput:capture type:SCStreamOutputTypeAudio error:&removeError];
        [stream removeStreamOutput:capture type:SCStreamOutputTypeScreen error:&removeError];
    }
    if (capture->_queue) {
        dispatch_sync(capture->_queue, ^{
            rs_audio_report_dropped(capture->_ctx,
                atomic_load_explicit(&capture->_unsupported, memory_order_relaxed));
            capture->_cb = NULL;
            capture->_ctx = NULL;
        });
    } else {
        capture->_cb = NULL;
        capture->_ctx = NULL;
    }
    capture->_stream = nil;
}

void *rs_audio_capture_start(uint32_t sample_rate, uint32_t channels, bool exclude_self,
                             void *ctx, rs_audio_cb cb, int32_t *err, char *msg, size_t msg_len) {
    @autoreleasepool {
        if (err) *err = 3;
        if (msg && msg_len) msg[0] = '\0';
        if (sample_rate != 48000 || (channels != 1 && channels != 2)) {
            audio_set_error(err, msg, msg_len, 2,
                            @"ScreenCaptureKit audio supports only 48000 Hz and one or two channels");
            return NULL;
        }
        if (!rs_audio_capture_supported()) {
            audio_set_error(err, msg, msg_len, 2,
                            @"ScreenCaptureKit audio requires macOS 13 or later");
            return NULL;
        }
        if (!ctx || !cb) {
            audio_set_error(err, msg, msg_len, 3, @"audio callback context is missing");
            return NULL;
        }
        if (!rs_screen_capture_allowed(0)) {
            audio_set_error(err, msg, msg_len, 1,
                            @"screen recording permission not granted: System Settings > Privacy & Security > "
                            @"Screen & System Audio Recording");
            return NULL;
        }

        __block SCShareableContent *content = nil;
        __block NSError *contentError = nil;
        dispatch_semaphore_t semaphore = dispatch_semaphore_create(0);
        [SCShareableContent getShareableContentExcludingDesktopWindows:NO onScreenWindowsOnly:YES
            completionHandler:^(SCShareableContent *result, NSError *error) {
                content = result;
                contentError = error;
                dispatch_semaphore_signal(semaphore);
            }];
        if (dispatch_semaphore_wait(semaphore, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) != 0) {
            audio_set_error(err, msg, msg_len, 3, @"timed out listing displays");
            return NULL;
        }
        if (!content) {
            NSString *detail = contentError.localizedDescription ?: @"cannot list displays";
            audio_set_error(err, msg, msg_len,
                            contentError.code == SCStreamErrorUserDeclined ? 1 : 3, detail);
            return NULL;
        }
        CGDirectDisplayID mainId = CGMainDisplayID();
        SCDisplay *display = nil;
        for (SCDisplay *candidate in content.displays) {
            if (candidate.displayID == mainId) {
                display = candidate;
                break;
            }
        }
        if (!display) display = content.displays.firstObject;
        if (!display) {
            audio_set_error(err, msg, msg_len, 3, @"no display found");
            return NULL;
        }

        RSAudioCapture *capture = [RSAudioCapture new];
        capture->_ctx = ctx;
        capture->_cb = cb;
        capture->_channels = channels;
        capture->_queue = dispatch_queue_create(
            "rs.audio.capture",
            dispatch_queue_attr_make_with_qos_class(DISPATCH_QUEUE_SERIAL, QOS_CLASS_USER_INTERACTIVE, 0));

        SCContentFilter *filter = [[SCContentFilter alloc] initWithDisplay:display excludingWindows:@[]];
        SCStreamConfiguration *configuration = [SCStreamConfiguration new];
        configuration.width = 2;
        configuration.height = 2;
        configuration.minimumFrameInterval = CMTimeMake(1, 1);
        configuration.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange;
        configuration.showsCursor = NO;
        configuration.queueDepth = 3;
        configuration.capturesAudio = YES;
        configuration.excludesCurrentProcessAudio = exclude_self;
        configuration.sampleRate = sample_rate;
        configuration.channelCount = channels;

        capture->_stream = [[SCStream alloc] initWithFilter:filter configuration:configuration delegate:capture];
        NSError *addError = nil;
        if (![capture->_stream addStreamOutput:capture type:SCStreamOutputTypeAudio
                             sampleHandlerQueue:capture->_queue error:&addError] ||
            ![capture->_stream addStreamOutput:capture type:SCStreamOutputTypeScreen
                             sampleHandlerQueue:capture->_queue error:&addError]) {
            stop_audio_capture(capture);
            audio_set_error(err, msg, msg_len, 3,
                            addError.localizedDescription ?: @"cannot add audio stream output");
            return NULL;
        }

        __block NSError *startError = nil;
        semaphore = dispatch_semaphore_create(0);
        [capture->_stream startCaptureWithCompletionHandler:^(NSError *error) {
            startError = error;
            dispatch_semaphore_signal(semaphore);
        }];
        BOOL timedOut = dispatch_semaphore_wait(semaphore, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) != 0;
        if (timedOut || startError) {
            stop_audio_capture(capture);
            audio_set_error(err, msg, msg_len,
                            !timedOut && startError.code == SCStreamErrorUserDeclined ? 1 : 3,
                            timedOut ? @"timed out starting audio capture"
                                     : (startError.localizedDescription ?: @"cannot start audio capture"));
            return NULL;
        }
        if (err) *err = 0;
        return (__bridge_retained void *)capture;
    }
}

void rs_audio_capture_stop(void *handle) {
    @autoreleasepool {
        if (!handle) return;
        RSAudioCapture *capture = (__bridge_transfer RSAudioCapture *)handle;
        stop_audio_capture(capture);
    }
}
