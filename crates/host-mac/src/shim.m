// macOS platform layer for the host agent: ScreenCaptureKit capture,
// VideoToolbox HEVC encoding and CGEvent input injection, exposed as a small
// C API that the Rust side (src/mac/mod.rs) calls.
//
// The legacy M0 API keeps one `g_cap`; Platform uses independent native handles.

#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <CoreGraphics/CoreGraphics.h>
#import <CoreGraphics/CGSession.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <IOKit/pwr_mgt/IOPMLib.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <VideoToolbox/VideoToolbox.h>
#import <Security/Security.h>
#import <objc/message.h>
#include <mach/mach_time.h>
#include <stdatomic.h>
#include <pthread.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

// Called on VT's output thread; data is borrowed only until the call returns.
// flags bit0 = keyframe, bit1 = still refinement (mutually exclusive).
typedef void (*rs_encoded_cb)(void *ctx, const uint8_t *data, size_t len, int32_t flags, uint64_t capture_ts_us);
typedef void (*rs_frame_cb)(void *ctx, void *pixel, uint32_t width, uint32_t height, uint64_t capture_ts_us);

typedef struct {
    uint32_t codec;
    int32_t low_latency;
    int32_t hardware;
    int32_t quality_supported;
} rs_encoder_info_t;

typedef struct {
    uint64_t submitted;
    uint64_t output;
    uint64_t dropped;
    uint64_t callback_error;
    uint64_t submit_error;
    uint64_t callback_status_error;
    uint64_t unexpected_null;
    uint64_t conversion_error;
    uint64_t forced_key_retries;
} rs_encoder_stats_t;

// The caller and the output callback each own one reference. Keeping the
// submission reference until EncodeFrame returns handles synchronous callbacks.
typedef struct {
    atomic_uint references;
    atomic_bool callback_seen;
    atomic_bool drop_counted;
    atomic_bool key_retry_counted;
    uintptr_t restore_generation;
    uint64_t key_generation;
    BOOL forced;
    BOOL refine;
} RSEncodeTicket;

static void release_ticket(RSEncodeTicket *ticket) {
    if (ticket && atomic_fetch_sub_explicit(&ticket->references, 1, memory_order_acq_rel) == 1) free(ticket);
}

// ---------------------------------------------------------------------------
// clocks

static uint64_t unix_now_us(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (uint64_t)ts.tv_sec * 1000000ull + (uint64_t)ts.tv_nsec / 1000ull;
}

// Converts a mach_absolute_time value (as used by SCStreamFrameInfoDisplayTime)
// to unix-epoch microseconds.
static uint64_t mach_to_unix_us(uint64_t mach) {
    static mach_timebase_info_data_t tb;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ mach_timebase_info(&tb); });
    uint64_t now_mach = mach_absolute_time();
    uint64_t now_unix = unix_now_us();
    if (mach >= now_mach) return now_unix;
    uint64_t delta_ns = (now_mach - mach) * tb.numer / tb.denom;
    return now_unix - delta_ns / 1000ull;
}

static void set_err(char *err, size_t len, NSString *msg) {
    if (err && len) strlcpy(err, msg.UTF8String ?: "unknown error", len);
}

static void display_pixels(CGDirectDisplayID d, size_t *w, size_t *h) {
    CGDisplayModeRef m = CGDisplayCopyDisplayMode(d);
    if (m) {
        *w = CGDisplayModeGetPixelWidth(m);
        *h = CGDisplayModeGetPixelHeight(m);
        CGDisplayModeRelease(m);
    } else {
        *w = CGDisplayPixelsWide(d);
        *h = CGDisplayPixelsHigh(d);
    }
}

// ---------------------------------------------------------------------------
// capture + encode

@interface RSCapture : NSObject <SCStreamOutput, SCStreamDelegate>
- (BOOL)createEncoderWidth:(int32_t)w height:(int32_t)h fps:(uint32_t)fps kbps:(uint32_t)kbps error:(NSString **)err;
- (BOOL)createEncoderWidth:(int32_t)w height:(int32_t)h fps:(uint32_t)fps kbps:(uint32_t)kbps codec:(uint32_t)codec error:(NSString **)err;
- (OSStatus)encode:(CVPixelBufferRef)px captureUs:(uint64_t)captureUs;
- (OSStatus)encode:(CVPixelBufferRef)px captureUs:(uint64_t)captureUs boost:(BOOL)boost;
@end

@implementation RSCapture {
@public
    SCStream *_stream;
    VTCompressionSessionRef _session;
    dispatch_queue_t _queue;
    void *_ctx;
    rs_encoded_cb _cb;
    rs_frame_cb _frameCb;
    atomic_int _forceKey;    // legacy capture request latch, consumed on _queue
    atomic_uint_fast64_t _keyRequested;
    atomic_uint_fast64_t _keyConfirmed;
    CVPixelBufferRef _last;   // last captured frame, only touched on _queue
    int64_t _lastPts;         // only touched on _queue
    BOOL _lowLatency;
    BOOL _hardware;
    BOOL _qualitySupported;
    double _qualityBaseline;
    uint32_t _codec;
    int _rateLimitScheme; // 0=current, 1=H.264 1.2x, 2=H.264 + 250 ms pair
    uint32_t _baseKbps;
    uint32_t _baseFps;
    double _rateWindow;
    uintptr_t _refineSequence;
    uintptr_t _pendingRateRestore;
    BOOL _qualityBoost;
    atomic_uint_fast64_t _submitted;
    atomic_uint_fast64_t _output;
    atomic_uint_fast64_t _dropped;
    atomic_uint_fast64_t _callbackError;
    atomic_uint_fast64_t _submitError;
    atomic_uint_fast64_t _callbackStatusError;
    atomic_uint_fast64_t _unexpectedNull;
    atomic_uint_fast64_t _conversionError;
    atomic_uint_fast64_t _forcedKeyRetries;
}

static void copy_encoder_stats(RSCapture *encoder, rs_encoder_stats_t *out) {
    if (!out) return;
    out->submitted = atomic_load_explicit(&encoder->_submitted, memory_order_relaxed);
    out->output = atomic_load_explicit(&encoder->_output, memory_order_relaxed);
    out->dropped = atomic_load_explicit(&encoder->_dropped, memory_order_relaxed);
    out->callback_error = atomic_load_explicit(&encoder->_callbackError, memory_order_relaxed);
    out->submit_error = atomic_load_explicit(&encoder->_submitError, memory_order_relaxed);
    out->callback_status_error = atomic_load_explicit(&encoder->_callbackStatusError, memory_order_relaxed);
    out->unexpected_null = atomic_load_explicit(&encoder->_unexpectedNull, memory_order_relaxed);
    out->conversion_error = atomic_load_explicit(&encoder->_conversionError, memory_order_relaxed);
    out->forced_key_retries = atomic_load_explicit(&encoder->_forcedKeyRetries, memory_order_relaxed);
}

static void request_keyframe(RSCapture *encoder) {
    atomic_fetch_add(&encoder->_keyRequested, 1);
}

static BOOL keyframe_pending(RSCapture *encoder) {
    return atomic_load(&encoder->_keyRequested) > atomic_load(&encoder->_keyConfirmed);
}

static void confirm_keyframe(RSCapture *encoder, uint64_t generation) {
    // A callback for an older frame must not consume a newer loss request,
    // including a request made by the Rust sink while delivering this frame.
    uint_fast64_t confirmed = atomic_load(&encoder->_keyConfirmed);
    while (confirmed < generation &&
           !atomic_compare_exchange_weak(&encoder->_keyConfirmed, &confirmed, generation)) {}
}

static void count_key_retry(RSCapture *encoder, RSEncodeTicket *ticket) {
    // The request remains pending until a keyframe is actually delivered.
    // Count each failed forced attempt once, even when both submit and callback
    // report the same failure. No rate-limit properties are changed for retries.
    if (ticket && ticket->forced && !atomic_exchange(&ticket->key_retry_counted, true))
        atomic_fetch_add_explicit(&encoder->_forcedKeyRetries, 1, memory_order_relaxed);
}

static void count_dropped(RSCapture *encoder, RSEncodeTicket *ticket) {
    // VT may report the same synchronous drop both via infoFlagsOut and via
    // vt_output. Count that frame once, including drops without a callback.
    if (!ticket || !atomic_exchange_explicit(&ticket->drop_counted, true, memory_order_relaxed))
        atomic_fetch_add_explicit(&encoder->_dropped, 1, memory_order_relaxed);
    count_key_retry(encoder, ticket);
}

static OSStatus set_prop(VTCompressionSessionRef s, CFStringRef key, CFTypeRef value) {
    OSStatus st = VTSessionSetProperty(s, key, value);
    if (st != noErr) NSLog(@"[rs-host] encoder property %@ not applied (%d)", key, (int)st);
    return st;
}

static int requested_rate_limit_scheme(void) {
    const char *value = getenv("RS_HOST_H264_LIMIT");
    if (!value) return 0;
    if (strcmp(value, "tight12") == 0 || strcmp(value, "tight_1.2x") == 0 ||
        strcmp(value, "1.2") == 0 || strcmp(value, "1.2x") == 0)
        return 1;
    if (strcmp(value, "short025") == 0 || strcmp(value, "short_0.25s") == 0 ||
        strcmp(value, "0.25") == 0)
        return 2;
    return 0;
}

// All properties and baseline bookkeeping are touched only on _queue.
static void set_rate_props(RSCapture *encoder, uint32_t kbps, uint32_t fps, double windowSeconds) {
    set_prop(encoder->_session, kVTCompressionPropertyKey_AverageBitRate,
             (__bridge CFNumberRef) @((uint64_t)kbps * 1000));
    set_prop(encoder->_session, kVTCompressionPropertyKey_ExpectedFrameRate, (__bridge CFNumberRef) @(fps));
    // The platform probe uses this process-local switch to isolate
    // AverageBitRate from the hard byte budget. Production sessions leave it
    // unset, so DataRateLimits remains the established default.
    const char *disableLimits = getenv("RS_HOST_DISABLE_DATA_RATE_LIMITS");
    if (disableLimits && strcmp(disableLimits, "1") == 0) {
        // VT accepts an empty array as an explicitly empty byte-budget list;
        // this also clears a previously installed limit for refine A/B.
        OSStatus clearStatus = set_prop(encoder->_session, kVTCompressionPropertyKey_DataRateLimits,
                                        (__bridge CFArrayRef) @[]);
        if (clearStatus != noErr) {
            // Older VT implementations remove the property when passed NULL.
            set_prop(encoder->_session, kVTCompressionPropertyKey_DataRateLimits, NULL);
        }
        return;
    }
    // Scale the byte budget with the window, preserving the 1.5x peak rate.
    // H.264 compensation is opt-in through RS_HOST_H264_LIMIT so production
    // sessions retain the established setting until real VT data selects one.
    double multiplier = (encoder->_codec == 2 && encoder->_rateLimitScheme == 1) ? 1.2 : 1.5;
    double bytes = floor((double)kbps * 1000.0 / 8.0 * multiplier * windowSeconds);
    NSArray *limits;
    if (encoder->_codec == 2 && encoder->_rateLimitScheme == 2) {
        double shortBytes = floor((double)kbps * 1000.0 / 8.0 * 1.5 * 0.25);
        limits = @[@(bytes), @(windowSeconds), @(shortBytes), @(0.25)];
    } else {
        limits = @[@(bytes), @(windowSeconds)];
    }
    set_prop(encoder->_session, kVTCompressionPropertyKey_DataRateLimits, (__bridge CFArrayRef) limits);
}

static void restore_rate(RSCapture *encoder) {
    encoder->_pendingRateRestore = 0;
    if (encoder->_session) set_rate_props(encoder, encoder->_baseKbps, encoder->_baseFps, encoder->_rateWindow);
}

// Never dispatch_sync here: VT can invoke this callback during EncodeFrame or
// CompleteFrames while the caller already owns _queue. A generation prevents
// an older callback from restoring over a newer boost or a set_rate update.
static void restore_rate_after_output(RSCapture *encoder, uintptr_t generation) {
    if (!generation) return;
    dispatch_async(encoder->_queue, ^{
        if (encoder->_pendingRateRestore == generation) restore_rate(encoder);
    });
}

// Return -1 on conversion failure, 0 for an emitted P frame, 1 for a keyframe.
static int emit_encoded_sample(RSCapture *cap, CMSampleBufferRef sb, BOOL stillRefine) {
    BOOL key = YES;
    CFArrayRef atts = CMSampleBufferGetSampleAttachmentsArray(sb, false);
    if (atts && CFArrayGetCount(atts) > 0) {
        CFDictionaryRef d = CFArrayGetValueAtIndex(atts, 0);
        CFBooleanRef notSync = NULL;
        if (CFDictionaryGetValueIfPresent(d, kCMSampleAttachmentKey_NotSync, (const void **)&notSync) && notSync) {
            key = !CFBooleanGetValue(notSync);
        }
    }

    static const uint8_t startCode[4] = {0, 0, 0, 1};
    CMFormatDescriptionRef fd = CMSampleBufferGetFormatDescription(sb);
    if (!fd) { atomic_fetch_add(&cap->_conversionError, 1); return -1; }
    CMVideoCodecType codec = CMVideoFormatDescriptionGetCodecType(fd);
    if (codec != kCMVideoCodecType_H264 && codec != kCMVideoCodecType_HEVC) {
        atomic_fetch_add(&cap->_conversionError, 1); return -1;
    }
    BOOL h264 = codec == kCMVideoCodecType_H264;
    size_t psCount = 0;
    int nalLenSize = 0;
    OSStatus psStatus = h264
        ? CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fd, 0, NULL, NULL, &psCount, &nalLenSize)
        : CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(fd, 0, NULL, NULL, &psCount, &nalLenSize);
    if (psStatus != noErr || nalLenSize < 1 || nalLenSize > 4) {
        atomic_fetch_add(&cap->_conversionError, 1); return -1;
    }

    CMBlockBufferRef bb = CMSampleBufferGetDataBuffer(sb);
    size_t total = bb ? CMBlockBufferGetDataLength(bb) : 0;
    if (!total) { atomic_fetch_add(&cap->_conversionError, 1); return -1; }
    NSMutableData *out = [NSMutableData dataWithCapacity:total + 256];
    if (key) {
        for (size_t i = 0; i < psCount; i++) {
            const uint8_t *ps = NULL;
            size_t psLen = 0;
            OSStatus psResult = h264
                ? CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fd, i, &ps, &psLen, NULL, NULL)
                : CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(fd, i, &ps, &psLen, NULL, NULL);
            if (psResult != noErr || !ps || !psLen) { atomic_fetch_add(&cap->_conversionError, 1); return -1; }
            [out appendBytes:startCode length:4];
            [out appendBytes:ps length:psLen];
        }
    }

    // Both codecs carry the same length-prefixed layout in a VT sample.
    uint8_t *raw = malloc(total);
    if (!raw) { atomic_fetch_add(&cap->_conversionError, 1); return -1; }
    if (CMBlockBufferCopyDataBytes(bb, 0, total, raw) != noErr) {
        free(raw); atomic_fetch_add(&cap->_conversionError, 1); return -1;
    }
    size_t off = 0;
    BOOL valid = YES;
    while (off < total) {
        if (total - off < (size_t)nalLenSize) { valid = NO; break; }
        uint32_t n = 0;
        for (int i = 0; i < nalLenSize; i++) n = (n << 8) | raw[off + i];
        off += (size_t)nalLenSize;
        if (!n || n > total - off) { valid = NO; break; }
        [out appendBytes:startCode length:4];
        [out appendBytes:raw + off length:n];
        off += n;
    }
    free(raw);
    if (!valid) { atomic_fetch_add(&cap->_conversionError, 1); return -1; }

    CMTime pts = CMSampleBufferGetPresentationTimeStamp(sb);
    uint64_t captureUs = (uint64_t)CMTimeConvertScale(pts, 1000000, kCMTimeRoundingMethod_Default).value;
    // Keyframes always take precedence, including spontaneous VT keyframes.
    atomic_fetch_add_explicit(&cap->_output, 1, memory_order_relaxed);
    cap->_cb(cap->_ctx, out.bytes, out.length, key ? 1 : (stillRefine ? 2 : 0), captureUs);
    return key ? 1 : 0;
}

static void vt_output(void *refcon, void *frameRefcon, OSStatus status, VTEncodeInfoFlags flags,
                      CMSampleBufferRef sb) {
    @autoreleasepool {
        RSCapture *cap = (__bridge RSCapture *)refcon;
        RSEncodeTicket *ticket = frameRefcon;
        BOOL stillRefine = ticket && ticket->refine;
        if (ticket) atomic_store_explicit(&ticket->callback_seen, true, memory_order_release);
        restore_rate_after_output(cap, ticket ? ticket->restore_generation : 0);
        BOOL dropped = (flags & kVTEncodeInfo_FrameDropped) != 0;
        if (dropped) count_dropped(cap, ticket);
        // A normal dropped-frame callback may have no sample buffer. Preserve
        // the raw counter, distinguishing actual errors from expected drops.
        if (status != noErr || sb == NULL)
            atomic_fetch_add_explicit(&cap->_callbackError, 1, memory_order_relaxed);
        if (status != noErr)
            atomic_fetch_add_explicit(&cap->_callbackStatusError, 1, memory_order_relaxed);
        if (status == noErr && sb == NULL && !dropped)
            atomic_fetch_add_explicit(&cap->_unexpectedNull, 1, memory_order_relaxed);
        int emitted = -1;
        if (status == noErr && sb != NULL && !dropped && cap->_cb != NULL)
            emitted = emit_encoded_sample(cap, sb, stillRefine);
        if (emitted == 1) {
            if (ticket) confirm_keyframe(cap, ticket->key_generation);
        } else {
            // Also keep retrying if VT ignores ForceKeyFrame or a valid
            // keyframe cannot be converted/delivered to the Rust sink.
            count_key_retry(cap, ticket);
        }
        release_ticket(ticket);
    }
}

// Keep the M0 capture entry point unchanged; it always requests HEVC.
- (BOOL)createEncoderWidth:(int32_t)w height:(int32_t)h fps:(uint32_t)fps kbps:(uint32_t)kbps error:(NSString **)err {
    return [self createEncoderWidth:w height:h fps:fps kbps:kbps codec:1 error:err];
}

- (BOOL)createEncoderWidth:(int32_t)w height:(int32_t)h fps:(uint32_t)fps kbps:(uint32_t)kbps
                     codec:(uint32_t)codec error:(NSString **)err {
    BOOL isH264 = codec == 2;
    CMVideoCodecType cmCodec = isH264 ? kCMVideoCodecType_H264 : kCMVideoCodecType_HEVC;
    NSDictionary *lowLatency = @{
        (__bridge NSString *)kVTVideoEncoderSpecification_EnableLowLatencyRateControl : @YES,
        (__bridge NSString *)kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder : @YES,
    };
    NSDictionary *hardware = @{
        (__bridge NSString *)kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder : @YES,
    };
    NSDictionary *software = @{
        (__bridge NSString *)kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder : @NO,
    };
    OSStatus st = VTCompressionSessionCreate(NULL, w, h, cmCodec, (__bridge CFDictionaryRef)lowLatency,
                                             NULL, NULL, vt_output, (__bridge void *)self, &_session);
    _lowLatency = st == noErr;
    _hardware = st == noErr;
    if (st != noErr) {
        NSLog(@"[rs-host] low-latency %@ rate control unavailable (%d)", isH264 ? @"H.264" : @"HEVC", (int)st);
        st = VTCompressionSessionCreate(NULL, w, h, cmCodec, (__bridge CFDictionaryRef)hardware, NULL,
                                        NULL, vt_output, (__bridge void *)self, &_session);
        _lowLatency = NO;
        _hardware = st == noErr;
    }
    if (st != noErr && isH264) {
        NSLog(@"[rs-host] hardware H.264 encoder unavailable (%d), allowing software encoding", (int)st);
        st = VTCompressionSessionCreate(NULL, w, h, cmCodec, (__bridge CFDictionaryRef)software, NULL,
                                        NULL, vt_output, (__bridge void *)self, &_session);
        _lowLatency = NO;
        _hardware = NO;
    }
    if (st != noErr) {
        *err = [NSString stringWithFormat:@"cannot create %@ encoder (%d)", isH264 ? @"H.264" : @"HEVC", (int)st];
        return NO;
    }

    _codec = codec;
    _rateLimitScheme = isH264 ? requested_rate_limit_scheme() : 0;
    _baseKbps = kbps;
    _baseFps = fps;
    _qualitySupported = NO;
    _qualityBaseline = 0.0;
    _rateWindow = 1.0;
    atomic_init(&_forceKey, 0);
    atomic_init(&_keyRequested, 0);
    atomic_init(&_keyConfirmed, 0);
    atomic_init(&_submitted, 0);
    atomic_init(&_output, 0);
    atomic_init(&_dropped, 0);
    atomic_init(&_callbackError, 0);
    atomic_init(&_submitError, 0);
    atomic_init(&_callbackStatusError, 0);
    atomic_init(&_unexpectedNull, 0);
    atomic_init(&_conversionError, 0);
    atomic_init(&_forcedKeyRetries, 0);
    set_prop(_session, kVTCompressionPropertyKey_RealTime, kCFBooleanTrue);
    set_prop(_session, kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse);
    set_prop(_session, kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality, kCFBooleanTrue);
    set_rate_props(self, kbps, fps, _rateWindow);
    set_prop(_session, kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, (__bridge CFNumberRef) @10);
    if (isH264) {
        set_prop(_session, kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_High_AutoLevel);
    } else {
        set_prop(_session, kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_HEVC_Main_AutoLevel);
        set_prop(_session, kVTCompressionPropertyKey_AllowOpenGOP, kCFBooleanFalse);
    }

    st = VTCompressionSessionPrepareToEncodeFrames(_session);
    if (st != noErr) {
        *err = [NSString stringWithFormat:@"cannot prepare encoder (%d)", (int)st];
        VTCompressionSessionInvalidate(_session);
        CFRelease(_session);
        _session = NULL;
        return NO;
    }
    CFTypeRef hardwareValue = NULL;
    if (VTSessionCopyProperty(_session, kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder,
                              NULL, &hardwareValue) == noErr && hardwareValue) {
        if (CFGetTypeID(hardwareValue) == CFBooleanGetTypeID()) _hardware = CFBooleanGetValue(hardwareValue);
    }
    if (hardwareValue) CFRelease(hardwareValue);

    CFTypeRef quality = NULL;
    OSStatus qualityStatus = VTSessionCopyProperty(_session, kVTCompressionPropertyKey_Quality, NULL, &quality);
    if (qualityStatus == noErr && quality && CFGetTypeID(quality) == CFNumberGetTypeID()) {
        _qualitySupported = CFNumberGetValue((CFNumberRef)quality, kCFNumberDoubleType, &_qualityBaseline);
    }
    if (quality) CFRelease(quality);
    if (_qualitySupported) {
        // Some low-latency encoders let us read Quality but reject writes.
        OSStatus boostStatus = set_prop(_session, kVTCompressionPropertyKey_Quality, (__bridge CFNumberRef)@0.9);
        OSStatus restoreStatus = set_prop(_session, kVTCompressionPropertyKey_Quality,
                                          (__bridge CFNumberRef)@(_qualityBaseline));
        _qualitySupported = boostStatus == noErr && restoreStatus == noErr;
    }
    if (!_qualitySupported) NSLog(@"[rs-host] encoder Quality unavailable; still refine uses temporary 2x bitrate");
    return YES;
}

- (void)destroyEncoder {
    if (_session) {
        VTCompressionSessionCompleteFrames(_session, kCMTimeInvalid);
        VTCompressionSessionInvalidate(_session);
        CFRelease(_session);
        _session = NULL;
    }
}

// The M0 capture path never requests refinement.
- (OSStatus)encode:(CVPixelBufferRef)px captureUs:(uint64_t)captureUs {
    return [self encode:px captureUs:captureUs boost:NO];
}

// Must run on _queue. The property changes bracket just this submission.
- (OSStatus)encode:(CVPixelBufferRef)px captureUs:(uint64_t)captureUs boost:(BOOL)boost {
    atomic_fetch_add_explicit(&_submitted, 1, memory_order_relaxed);
    if (atomic_exchange(&_forceKey, 0)) request_keyframe(self);
    uint64_t keyGeneration = atomic_load(&_keyRequested);
    BOOL force = keyGeneration > atomic_load(&_keyConfirmed);
    if (!_session) {
        atomic_fetch_add(&_submitError, 1);
        if (force) atomic_fetch_add(&_forcedKeyRetries, 1);
        return kVTInvalidSessionErr;
    }
    RSEncodeTicket *ticket = calloc(1, sizeof(*ticket));
    if (!ticket) {
        atomic_fetch_add(&_submitError, 1);
        if (force) atomic_fetch_add(&_forcedKeyRetries, 1);
        return memFullErr;
    }
    atomic_init(&ticket->references, 2);
    atomic_init(&ticket->callback_seen, false);
    atomic_init(&ticket->drop_counted, false);
    atomic_init(&ticket->key_retry_counted, false);
    ticket->key_generation = keyGeneration;
    ticket->forced = force;
    int64_t pts = (int64_t)captureUs;
    if (pts <= _lastPts) pts = _lastPts + 1;
    _lastPts = pts;
    BOOL refine = (boost || _qualityBoost) && !force;
    NSDictionary *props = force ? @{(__bridge NSString *)kVTEncodeFrameOptionKey_ForceKeyFrame : @YES} : nil;

    // A following submission may arrive before the boost's callback. Restore
    // here as well so it cannot inherit boosted rate limits; its old callback
    // is then harmless. This also protects forced keyframes and rate updates.
    if (_pendingRateRestore) restore_rate(self);
    if (refine && _qualitySupported) {
        if (set_prop(_session, kVTCompressionPropertyKey_Quality, (__bridge CFNumberRef)@0.9) != noErr) {
            _qualitySupported = NO;
        }
    }
    uintptr_t generation = 0;
    if (refine && !_qualitySupported) {
        _refineSequence = (_refineSequence + 1) & (UINTPTR_MAX >> 1);
        if (!_refineSequence) _refineSequence = 1;
        generation = _pendingRateRestore = _refineSequence;
        uint32_t boosted = _baseKbps > UINT32_MAX / 2 ? UINT32_MAX : _baseKbps * 2;
        set_rate_props(self, boosted, _baseFps, _rateWindow);
    }
    ticket->restore_generation = generation;
    ticket->refine = refine;
    VTEncodeInfoFlags infoFlags = 0;
    OSStatus st = VTCompressionSessionEncodeFrame(_session, px, CMTimeMake(pts, 1000000), kCMTimeInvalid,
                                                  (__bridge CFDictionaryRef)props, ticket, &infoFlags);
    if (infoFlags & kVTEncodeInfo_FrameDropped) count_dropped(self, ticket);
    if (st != noErr) count_key_retry(self, ticket);
    if ((st != noErr || ((infoFlags & kVTEncodeInfo_FrameDropped) && !(infoFlags & kVTEncodeInfo_Asynchronous))) &&
        !atomic_load_explicit(&ticket->callback_seen, memory_order_acquire)) {
        // A failed submission, or a synchronous drop with no callback, has
        // no future callback owner. The submission still owns this ticket.
        release_ticket(ticket);
    }
    release_ticket(ticket);
    if (refine && _qualitySupported) {
        set_prop(_session, kVTCompressionPropertyKey_Quality, (__bridge CFNumberRef)@(_qualityBaseline));
    }
    if (generation && (st != noErr || (infoFlags & kVTEncodeInfo_FrameDropped))) restore_rate(self);
    if (st != noErr) {
        atomic_fetch_add_explicit(&_submitError, 1, memory_order_relaxed);
        NSLog(@"[rs-host] encode failed (%d)", (int)st);
    }
    return st;
}

- (void)stream:(SCStream *)stream didOutputSampleBuffer:(CMSampleBufferRef)sb ofType:(SCStreamOutputType)type {
    if (type != SCStreamOutputTypeScreen || !CMSampleBufferIsValid(sb)) return;
    CFArrayRef atts = CMSampleBufferGetSampleAttachmentsArray(sb, false);
    if (!atts || CFArrayGetCount(atts) < 1) return;
    NSDictionary *info = (__bridge NSDictionary *)CFArrayGetValueAtIndex(atts, 0);
    NSNumber *status = info[SCStreamFrameInfoStatus];
    // Idle frames (nothing changed on screen) carry no image: send nothing.
    if (!status || status.integerValue != SCFrameStatusComplete) return;
    CVPixelBufferRef px = CMSampleBufferGetImageBuffer(sb);
    if (!px) return;
    NSNumber *displayTime = info[SCStreamFrameInfoDisplayTime];
    uint64_t captureUs = displayTime ? mach_to_unix_us(displayTime.unsignedLongLongValue) : unix_now_us();
    if (_frameCb) {
        _frameCb(_ctx, px, (uint32_t)CVPixelBufferGetWidth(px), (uint32_t)CVPixelBufferGetHeight(px), captureUs);
        return;
    }
    if (!_session) return;
    if (_last) CVPixelBufferRelease(_last);
    _last = CVPixelBufferRetain(px);
    [self encode:px captureUs:captureUs];
}

- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
    NSLog(@"[rs-host] capture stopped: %@", error.localizedDescription);
}
@end

static RSCapture *g_cap = nil;

// ---------------------------------------------------------------------------
// display power: a connected viewer must see a lit screen, like Screen Sharing

static IOPMAssertionID g_noSleep = kIOPMNullAssertionID;
static uint32_t g_awakeCaptures = 0;
static bool g_platformAwake = false;

// Wakes the displays if they are asleep and waits until the main one is
// active again (ScreenCaptureKit lists no displays while they sleep).
int32_t rs_wake_display(void);
int32_t rs_screen_capture_allowed(int32_t request);

static void wake_displays(void) {
    if (CGDisplayIsActive(CGMainDisplayID())) return;
    rs_wake_display();
    for (int i = 0; i < 40 && !CGDisplayIsActive(CGMainDisplayID()); i++) usleep(50 * 1000);
    NSLog(@"[rs-host] woke displays (main active: %@)", CGDisplayIsActive(CGMainDisplayID()) ? @"yes" : @"no");
}

static void hold_display_awake(bool on) {
    // Capture handles and Platform::power_keep_awake share this one assertion.
    // Capture handles are counted; the explicit platform hold is idempotent.
    @synchronized([RSCapture class]) {
        if (on) g_awakeCaptures++;
        else if (g_awakeCaptures > 0) g_awakeCaptures--;
        uint32_t total = g_awakeCaptures + (g_platformAwake ? 1u : 0u);
        if (total > 0 && g_noSleep == kIOPMNullAssertionID) {
            IOPMAssertionCreateWithName(kIOPMAssertionTypePreventUserIdleDisplaySleep, kIOPMAssertionLevelOn,
                                        CFSTR("rs-host streaming"), &g_noSleep);
        } else if (total == 0 && g_noSleep != kIOPMNullAssertionID) {
            IOPMAssertionRelease(g_noSleep);
            g_noSleep = kIOPMNullAssertionID;
        }
    }
}

void rs_power_keep_awake(int32_t on) {
    @synchronized([RSCapture class]) {
        g_platformAwake = on != 0;
        uint32_t total = g_awakeCaptures + (g_platformAwake ? 1u : 0u);
        if (total > 0 && g_noSleep == kIOPMNullAssertionID) {
            IOPMAssertionCreateWithName(kIOPMAssertionTypePreventUserIdleDisplaySleep, kIOPMAssertionLevelOn,
                                        CFSTR("rs-host streaming"), &g_noSleep);
        } else if (total == 0 && g_noSleep != kIOPMNullAssertionID) {
            IOPMAssertionRelease(g_noSleep);
            g_noSleep = kIOPMNullAssertionID;
        }
    }
}

int32_t rs_wake_display(void) {
    IOPMAssertionID act = kIOPMNullAssertionID;
    IOReturn status = IOPMAssertionDeclareUserActivity(CFSTR("rs-host viewer connected"),
                                                       kIOPMUserActiveLocal, &act);
    if (act != kIOPMNullAssertionID) IOPMAssertionRelease(act);
    return status == kIOReturnSuccess ? 0 : (int32_t)status;
}

void rs_capture_stop(void);

// Starts capturing the main display. Returns 0 on success, 2 when screen
// recording permission is missing, 1 for other errors (message in `err`).
int32_t rs_capture_start(uint32_t fps, uint32_t bitrate_kbps, uint32_t max_width, void *ctx, rs_encoded_cb cb,
                         uint32_t *out_w, uint32_t *out_h, char *err, size_t errlen) {
    @autoreleasepool {
        if (g_cap) rs_capture_stop();
        if (!rs_screen_capture_allowed(0)) {
            rs_screen_capture_allowed(1);
            set_err(err, errlen,
                    @"screen recording permission not granted: System Settings > Privacy & Security > Screen & "
                    @"System Audio Recording");
            return 2;
        }
        wake_displays();

        __block SCShareableContent *content = nil;
        __block NSError *contentErr = nil;
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        [SCShareableContent getShareableContentExcludingDesktopWindows:NO
                                                   onScreenWindowsOnly:YES
                                                     completionHandler:^(SCShareableContent *c, NSError *e) {
                                                       content = c;
                                                       contentErr = e;
                                                       dispatch_semaphore_signal(sem);
                                                     }];
        if (dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) != 0) {
            set_err(err, errlen, @"timed out listing displays");
            return 1;
        }
        if (!content) {
            set_err(err, errlen,
                    [NSString stringWithFormat:@"cannot list displays: %@", contentErr.localizedDescription]);
            return contentErr.code == SCStreamErrorUserDeclined ? 2 : 1;
        }

        CGDirectDisplayID mainId = CGMainDisplayID();
        SCDisplay *display = nil;
        for (SCDisplay *d in content.displays) {
            if (d.displayID == mainId) display = d;
        }
        if (!display) display = content.displays.firstObject;
        if (!display) {
            set_err(err, errlen, @"no display found");
            return 1;
        }

        size_t pw = 0, ph = 0;
        display_pixels(display.displayID, &pw, &ph);
        uint32_t w = (uint32_t)pw, h = (uint32_t)ph;
        if (max_width > 0 && w > max_width) {
            h = (uint32_t)llround((double)ph * max_width / (double)pw);
            w = max_width;
        }
        w &= ~1u;
        h &= ~1u;

        RSCapture *cap = [RSCapture new];
        dispatch_queue_attr_t attr =
            dispatch_queue_attr_make_with_qos_class(DISPATCH_QUEUE_SERIAL, QOS_CLASS_USER_INTERACTIVE, 0);
        cap->_queue = dispatch_queue_create("rs.capture", attr);
        cap->_ctx = ctx;
        cap->_cb = cb;
        NSString *vtErr = nil;
        if (![cap createEncoderWidth:(int32_t)w height:(int32_t)h fps:fps kbps:bitrate_kbps error:&vtErr]) {
            set_err(err, errlen, vtErr);
            return 1;
        }

        SCContentFilter *filter = [[SCContentFilter alloc] initWithDisplay:display excludingWindows:@[]];
        SCStreamConfiguration *cfg = [SCStreamConfiguration new];
        cfg.width = w;
        cfg.height = h;
        cfg.minimumFrameInterval = CMTimeMake(1, (int32_t)fps);
        cfg.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange;
        cfg.colorMatrix = kCGDisplayStreamYCbCrMatrix_ITU_R_709_2;
        cfg.showsCursor = NO;  // the client draws its own cursor
        cfg.queueDepth = 5;
        cfg.capturesAudio = NO;

        cap->_stream = [[SCStream alloc] initWithFilter:filter configuration:cfg delegate:cap];
        NSError *addErr = nil;
        if (![cap->_stream addStreamOutput:cap type:SCStreamOutputTypeScreen sampleHandlerQueue:cap->_queue
                                     error:&addErr]) {
            [cap destroyEncoder];
            set_err(err, errlen, [NSString stringWithFormat:@"cannot add stream output: %@", addErr.localizedDescription]);
            return 1;
        }
        __block NSError *startErr = nil;
        sem = dispatch_semaphore_create(0);
        [cap->_stream startCaptureWithCompletionHandler:^(NSError *e) {
          startErr = e;
          dispatch_semaphore_signal(sem);
        }];
        if (dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) != 0) {
            [cap destroyEncoder];
            set_err(err, errlen, @"timed out starting capture");
            return 1;
        }
        if (startErr) {
            [cap destroyEncoder];
            set_err(err, errlen, [NSString stringWithFormat:@"cannot start capture: %@", startErr.localizedDescription]);
            return startErr.code == SCStreamErrorUserDeclined ? 2 : 1;
        }
        NSLog(@"[rs-host] capturing display %u at %ux%u (from %zux%zu), %u fps, %u kbps, low-latency RC: %@",
              display.displayID, w, h, pw, ph, fps, bitrate_kbps, cap->_lowLatency ? @"yes" : @"no");
        g_cap = cap;
        hold_display_awake(true);
        *out_w = w;
        *out_h = h;
        return 0;
    }
}

// Next encoded frame will be a keyframe. If the screen is static (no new
// frames), re-encodes the last captured frame shortly after.
void rs_capture_request_keyframe(void) {
    RSCapture *cap = g_cap;
    if (!cap) return;
    atomic_store(&cap->_forceKey, 1);
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 60 * NSEC_PER_MSEC), cap->_queue, ^{
      if (atomic_load(&cap->_forceKey) && cap->_last) [cap encode:cap->_last captureUs:unix_now_us()];
    });
}

void rs_capture_stop(void) {
    @autoreleasepool {
        RSCapture *cap = g_cap;
        if (!cap) return;
        g_cap = nil;
        hold_display_awake(false);
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        [cap->_stream stopCaptureWithCompletionHandler:^(NSError *e) {
          dispatch_semaphore_signal(sem);
        }];
        dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC));
        dispatch_sync(cap->_queue, ^{
          [cap destroyEncoder];
          if (cap->_last) {
              CVPixelBufferRelease(cap->_last);
              cap->_last = NULL;
          }
          cap->_cb = NULL;
        });
    }
}

void rs_display_pixel_size(uint32_t *w, uint32_t *h) {
    size_t pw = 0, ph = 0;
    display_pixels(CGMainDisplayID(), &pw, &ph);
    *w = (uint32_t)pw;
    *h = (uint32_t)ph;
}

// ---------------------------------------------------------------------------
// permissions

int32_t rs_screen_capture_allowed(int32_t request) {
    if (CGPreflightScreenCaptureAccess()) return 1;
    if (request) CGRequestScreenCaptureAccess();
    return 0;
}

int32_t rs_accessibility_trusted(int32_t prompt) {
    NSDictionary *opts = @{(__bridge NSString *)kAXTrustedCheckOptionPrompt : @(prompt != 0)};
    return AXIsProcessTrustedWithOptions((__bridge CFDictionaryRef)opts) ? 1 : 0;
}

// ---------------------------------------------------------------------------
// session state and actions

static void post(CGEventRef e);

int32_t rs_console_session(int32_t *on_console, int32_t *screen_locked) {
    if (on_console) *on_console = -1;
    if (screen_locked) *screen_locked = -1;
    CFDictionaryRef dict = CGSessionCopyCurrentDictionary();
    if (!dict) return 0;

    // kCGSessionOnConsoleKey is the SDK constant for the
    // "kCGSSessionOnConsoleKey" dictionary key (CoreGraphics/CGSession.h).
    CFTypeRef on = CFDictionaryGetValue(dict, kCGSessionOnConsoleKey);
    // CGSSessionScreenIsLocked is the window-server key described in 06-host.md;
    // unlike the on-console key it has no public SDK constant.
    CFTypeRef locked = CFDictionaryGetValue(dict, CFSTR("CGSSessionScreenIsLocked"));
    if (on_console && on && CFGetTypeID(on) == CFBooleanGetTypeID()) {
        *on_console = CFBooleanGetValue((CFBooleanRef)on) ? 1 : 0;
    }
    if (screen_locked) {
        // The dictionary omits the lock key while unlocked. A present value
        // with an unexpected type retains -1 (unknown).
        if (!locked) {
            *screen_locked = 0;
        } else if (CFGetTypeID(locked) == CFBooleanGetTypeID()) {
            *screen_locked = CFBooleanGetValue((CFBooleanRef)locked) ? 1 : 0;
        }
    }
    CFRelease(dict);
    return 1;
}

int32_t rs_lock_screen(void) {
    // kVK_ANSI_Q is virtual key code 12 (Carbon's keycode header is not
    // imported by this shim, so keep the documented value local).
    CGEventRef down = CGEventCreateKeyboardEvent(NULL, (CGKeyCode)12, true);
    CGEventRef up = CGEventCreateKeyboardEvent(NULL, (CGKeyCode)12, false);
    if (!down || !up) {
        if (down) CFRelease(down);
        if (up) CFRelease(up);
        return -1;
    }
    CGEventFlags flags = kCGEventFlagMaskControl | kCGEventFlagMaskCommand;
    CGEventSetFlags(down, flags);
    CGEventSetFlags(up, flags);
    post(down);
    post(up);
    return 0;
}

// ---------------------------------------------------------------------------
// display reconfiguration watcher

typedef void (*rs_display_watch_cb)(uint32_t display, uint32_t flags, void *ctx);

typedef struct {
    pthread_mutex_t lock;
    CFRunLoopRef run_loop;
    bool stopped;
    rs_display_watch_cb callback;
    void *ctx;
} rs_display_watch;

static void display_reconfiguration(CGDirectDisplayID display, CGDisplayChangeSummaryFlags flags, void *ctx) {
    rs_display_watch *watch = (rs_display_watch *)ctx;
    if (!watch || !watch->callback) return;
    watch->callback((uint32_t)display, (uint32_t)flags, watch->ctx);
}

void *rs_display_watch_create(rs_display_watch_cb callback, void *ctx) {
    rs_display_watch *watch = calloc(1, sizeof(*watch));
    if (!watch) return NULL;
    pthread_mutex_init(&watch->lock, NULL);
    watch->callback = callback;
    watch->ctx = ctx;
    return watch;
}

int32_t rs_display_watch_run(void *opaque) {
    rs_display_watch *watch = (rs_display_watch *)opaque;
    if (!watch) return -1;
    pthread_mutex_lock(&watch->lock);
    watch->run_loop = CFRunLoopGetCurrent();
    bool stopped = watch->stopped;
    pthread_mutex_unlock(&watch->lock);
    if (stopped) return 0;
    CGDisplayRegisterReconfigurationCallback(display_reconfiguration, watch);
    pthread_mutex_lock(&watch->lock);
    stopped = watch->stopped;
    pthread_mutex_unlock(&watch->lock);
    if (!stopped) CFRunLoopRun();
    CGDisplayRemoveReconfigurationCallback(display_reconfiguration, watch);
    pthread_mutex_lock(&watch->lock);
    watch->run_loop = NULL;
    pthread_mutex_unlock(&watch->lock);
    return 0;
}

void rs_display_watch_stop(void *opaque) {
    rs_display_watch *watch = (rs_display_watch *)opaque;
    if (!watch) return;
    pthread_mutex_lock(&watch->lock);
    watch->stopped = true;
    CFRunLoopRef run_loop = watch->run_loop;
    pthread_mutex_unlock(&watch->lock);
    if (run_loop) CFRunLoopStop(run_loop);
}

void rs_display_watch_destroy(void *opaque) {
    rs_display_watch *watch = (rs_display_watch *)opaque;
    if (!watch) return;
    pthread_mutex_destroy(&watch->lock);
    free(watch);
}

// ---------------------------------------------------------------------------
// local activity event tap

typedef void (*rs_activity_watch_cb)(int32_t value, void *ctx);

typedef struct {
    pthread_mutex_t lock;
    CFRunLoopRef run_loop;
    CFMachPortRef tap;
    bool stopped;
    rs_activity_watch_cb callback;
    void *ctx;
} rs_activity_watch;

static CGEventRef activity_event(CGEventTapProxy proxy, CGEventType type, CGEventRef event, void *ctx) {
    (void)proxy;
    rs_activity_watch *watch = (rs_activity_watch *)ctx;
    if (!watch) return event;
    if (type == kCGEventTapDisabledByTimeout || type == kCGEventTapDisabledByUserInput) {
        pthread_mutex_lock(&watch->lock);
        CFMachPortRef tap = watch->tap;
        pthread_mutex_unlock(&watch->lock);
        if (tap) CGEventTapEnable(tap, true);
        return event;
    }
    if (event && watch->callback &&
        CGEventGetIntegerValueField(event, kCGEventSourceUserData) != 0x5253) {
        // Do not inspect or log the event; only report that local activity
        // occurred.  Remote injection carries the 0x5253 "RS" marker.
        watch->callback(1, watch->ctx);
    }
    return event;
}

void *rs_activity_watch_create(rs_activity_watch_cb callback, void *ctx) {
    rs_activity_watch *watch = calloc(1, sizeof(*watch));
    if (!watch) return NULL;
    pthread_mutex_init(&watch->lock, NULL);
    watch->callback = callback;
    watch->ctx = ctx;
    return watch;
}

int32_t rs_activity_watch_run(void *opaque) {
    rs_activity_watch *watch = (rs_activity_watch *)opaque;
    if (!watch) return -1;
    CGEventMask mask = 0;
    mask |= CGEventMaskBit(kCGEventMouseMoved);
    mask |= CGEventMaskBit(kCGEventLeftMouseDown) | CGEventMaskBit(kCGEventLeftMouseUp);
    mask |= CGEventMaskBit(kCGEventLeftMouseDragged);
    mask |= CGEventMaskBit(kCGEventRightMouseDown) | CGEventMaskBit(kCGEventRightMouseUp);
    mask |= CGEventMaskBit(kCGEventRightMouseDragged);
    mask |= CGEventMaskBit(kCGEventOtherMouseDown) | CGEventMaskBit(kCGEventOtherMouseUp);
    mask |= CGEventMaskBit(kCGEventOtherMouseDragged);
    mask |= CGEventMaskBit(kCGEventScrollWheel);
    mask |= CGEventMaskBit(kCGEventKeyDown) | CGEventMaskBit(kCGEventKeyUp);
    mask |= CGEventMaskBit(kCGEventFlagsChanged);
    CFMachPortRef tap = CGEventTapCreate(kCGSessionEventTap, kCGHeadInsertEventTap,
                                         kCGEventTapOptionListenOnly, mask, activity_event, watch);
    if (!tap) {
        if (watch->callback) watch->callback(-1, watch->ctx);
        return -1;
    }
    CFRunLoopSourceRef source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0);
    if (!source) {
        CFRelease(tap);
        if (watch->callback) watch->callback(-1, watch->ctx);
        return -1;
    }
    pthread_mutex_lock(&watch->lock);
    watch->tap = tap;
    watch->run_loop = CFRunLoopGetCurrent();
    bool stopped = watch->stopped;
    pthread_mutex_unlock(&watch->lock);
    CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
    if (!stopped) CFRunLoopRun();
    CFRunLoopRemoveSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
    CFRelease(source);
    CGEventTapEnable(tap, false);
    CFMachPortInvalidate(tap);
    CFRelease(tap);
    pthread_mutex_lock(&watch->lock);
    watch->tap = NULL;
    watch->run_loop = NULL;
    pthread_mutex_unlock(&watch->lock);
    return 0;
}

void rs_activity_watch_stop(void *opaque) {
    rs_activity_watch *watch = (rs_activity_watch *)opaque;
    if (!watch) return;
    pthread_mutex_lock(&watch->lock);
    watch->stopped = true;
    CFRunLoopRef run_loop = watch->run_loop;
    pthread_mutex_unlock(&watch->lock);
    if (run_loop) CFRunLoopStop(run_loop);
}

void rs_activity_watch_destroy(void *opaque) {
    rs_activity_watch *watch = (rs_activity_watch *)opaque;
    if (!watch) return;
    pthread_mutex_destroy(&watch->lock);
    free(watch);
}

// ---------------------------------------------------------------------------
// input (coordinates normalized to the main display, origin top-left)

static int g_buttons = 0;  // bits 0..4: left, right, middle, back, forward
static CGDirectDisplayID g_inputDisplay = 0;

static CGPoint to_global(double nx, double ny) {
    CGRect b = CGDisplayBounds(g_inputDisplay ?: CGMainDisplayID());
    return CGPointMake(b.origin.x + nx * b.size.width, b.origin.y + ny * b.size.height);
}

static void post(CGEventRef e) {
    if (!e) return;
    CGEventSetIntegerValueField(e, kCGEventSourceUserData, 0x5253);
    CGEventPost(kCGHIDEventTap, e);
    CFRelease(e);
}

void rs_input_mouse_move(double nx, double ny) {
    CGEventType type = kCGEventMouseMoved;
    CGMouseButton button = kCGMouseButtonLeft;
    if (g_buttons & 1) {
        type = kCGEventLeftMouseDragged;
    } else if (g_buttons & 2) {
        type = kCGEventRightMouseDragged;
        button = kCGMouseButtonRight;
    } else if (g_buttons & 4) {
        type = kCGEventOtherMouseDragged;
        button = kCGMouseButtonCenter;
    }
    post(CGEventCreateMouseEvent(NULL, type, to_global(nx, ny), button));
}

void rs_input_mouse_button(int32_t button, int32_t down, double nx, double ny, int32_t clicks) {
    CGPoint p = to_global(nx, ny);
    CGEventType type;
    CGMouseButton cgButton;
    switch (button) {
        case 1:
            type = down ? kCGEventRightMouseDown : kCGEventRightMouseUp;
            cgButton = kCGMouseButtonRight;
            break;
        case 2:
            type = down ? kCGEventOtherMouseDown : kCGEventOtherMouseUp;
            cgButton = kCGMouseButtonCenter;
            break;
        default:
            type = down ? kCGEventLeftMouseDown : kCGEventLeftMouseUp;
            cgButton = kCGMouseButtonLeft;
            break;
    }
    int bit = 1 << (button < 0 || button > 2 ? 0 : button);
    if (down) {
        // Move first so the press lands where the client pointed.
        rs_input_mouse_move(nx, ny);
        g_buttons |= bit;
    } else {
        g_buttons &= ~bit;
    }
    CGEventRef e = CGEventCreateMouseEvent(NULL, type, p, cgButton);
    if (e) CGEventSetIntegerValueField(e, kCGMouseEventClickState, clicks > 0 ? clicks : 1);
    post(e);
}

void rs_input_scroll(double dx, double dy) {
    post(CGEventCreateScrollWheelEvent(NULL, kCGScrollEventUnitPixel, 2, (int32_t)lround(dy), (int32_t)lround(dx)));
}

void rs_cursor_position(double *nx, double *ny) {
    CGEventRef e = CGEventCreate(NULL);
    CGPoint p = e ? CGEventGetLocation(e) : CGPointZero;
    if (e) CFRelease(e);
    CGRect b = CGDisplayBounds(CGMainDisplayID());
    *nx = b.size.width > 0 ? (p.x - b.origin.x) / b.size.width : 0.5;
    *ny = b.size.height > 0 ? (p.y - b.origin.y) / b.size.height : 0.5;
}

// Returns the current cursor position, visibility and a PNG representation of
// the selected 1x/2x image representation. All AppKit objects are confined to
// this autorelease pool; the caller owns the copied bytes in its output
// buffer.
int32_t rs_cursor_snapshot(double *x, double *y, int32_t *visible, uint8_t *png, size_t capacity,
                           size_t *png_len, uint32_t *width, uint32_t *height, double *hot_x,
                           double *hot_y, double *scale) {
    @autoreleasepool {
        CGEventRef event = CGEventCreate(NULL);
        CGPoint point = event ? CGEventGetLocation(event) : CGPointZero;
        if (event) CFRelease(event);
        if (x) *x = point.x;
        if (y) *y = point.y;
        if (visible) *visible = CGCursorIsVisible() ? 1 : 0;
        if (png_len) *png_len = 0;
        if (width) *width = 0;
        if (height) *height = 0;
        if (hot_x) *hot_x = 0;
        if (hot_y) *hot_y = 0;
        if (scale) *scale = 1;
        if (!png || !png_len || !width || !height || !hot_x || !hot_y || !scale) return 0;

        NSCursor *cursor = nil;
        // Swift spells this NSCursor.currentSystem; the Objective-C SDK
        // exposes it as the deprecated currentSystemCursor property.
        SEL systemSelector = NSSelectorFromString(@"currentSystemCursor");
        if ([NSCursor respondsToSelector:systemSelector]) {
            NSCursor *(*sendCursor)(id, SEL) = (NSCursor *(*)(id, SEL))objc_msgSend;
            cursor = sendCursor([NSCursor class], systemSelector);
        }
        if (!cursor) cursor = [NSCursor currentCursor];
        NSImage *image = cursor.image;
        if (!image) return 0;

        CGFloat desiredScale = [NSScreen mainScreen].backingScaleFactor >= 1.5 ? 2.0 : 1.0;
        NSImageRep *selected = nil;
        CGFloat selectedDistance = CGFLOAT_MAX;
        for (NSImageRep *representation in image.representations) {
            CGFloat representationScale =
                representation.pixelsWide > 0 && image.size.width > 0
                    ? (CGFloat)representation.pixelsWide / image.size.width
                    : 1.0;
            CGFloat distance = fabs(representationScale - desiredScale);
            if (!selected || distance < selectedDistance) {
                selected = representation;
                selectedDistance = distance;
            }
        }

        NSData *pngData = nil;
        NSInteger pixelWidth = 0;
        NSInteger pixelHeight = 0;
        if (selected) {
            pngData = [NSBitmapImageRep representationOfImageRepsInArray:@[selected]
                                                                  usingType:NSBitmapImageFileTypePNG
                                                                 properties:@{}];
            pixelWidth = selected.pixelsWide;
            pixelHeight = selected.pixelsHigh;
        }
        if (!pngData) {
            NSRect proposed = NSMakeRect(0, 0, image.size.width * desiredScale, image.size.height * desiredScale);
            CGImageRef cgImage = [image CGImageForProposedRect:&proposed context:nil hints:nil];
            if (cgImage) {
                NSBitmapImageRep *bitmap = [[NSBitmapImageRep alloc] initWithCGImage:cgImage];
                pngData = [bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
                pixelWidth = bitmap.pixelsWide;
                pixelHeight = bitmap.pixelsHigh;
            }
        }
        if (!pngData) return 0;
        if (pngData.length > capacity) {
            *png_len = pngData.length;
            return -2;
        }
        [pngData getBytes:png length:pngData.length];
        *png_len = pngData.length;
        *width = (uint32_t)MAX(pixelWidth, 0);
        *height = (uint32_t)MAX(pixelHeight, 0);
        if (*width == 0) *width = (uint32_t)MAX(llround(image.size.width * desiredScale), 1);
        if (*height == 0) *height = (uint32_t)MAX(llround(image.size.height * desiredScale), 1);
        *scale = image.size.width > 0 ? (double)*width / image.size.width : desiredScale;
        NSPoint hotSpot = cursor.hotSpot;
        *hot_x = hotSpot.x * *scale;
        *hot_y = hotSpot.y * *scale;
        return 0;
    }
}

// ---------------------------------------------------------------------------
// clipboard

static NSData *clipboard_data_for_kind(int32_t *kind) {
    NSPasteboard *pasteboard = [NSPasteboard generalPasteboard];
    NSData *text = [pasteboard dataForType:@"public.utf8-plain-text"];
    if (text != nil) {
        if (kind) *kind = 1;
        return text;
    }
    NSData *png = [pasteboard dataForType:@"public.png"];
    if (png.length > 0) {
        if (kind) *kind = 2;
        return png;
    }
    NSData *tiff = [pasteboard dataForType:@"public.tiff"];
    if (tiff.length > 0) {
        NSBitmapImageRep *bitmap = [NSBitmapImageRep imageRepWithData:tiff];
        NSData *converted = [bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
        if (converted.length > 0) {
            if (kind) *kind = 2;
            return converted;
        }
    }
    return nil;
}

int32_t rs_clipboard_get(int32_t *kind, uint8_t *data, size_t capacity, size_t *length) {
    @autoreleasepool {
        if (!length) return -1;
        *length = 0;
        NSData *value = clipboard_data_for_kind(kind);
        if (!value) return 0;
        *length = value.length;
        if (!data || capacity == 0) return 1;
        if (value.length > capacity) return -2;
        [value getBytes:data length:value.length];
        return 1;
    }
}

int32_t rs_clipboard_set(int32_t kind, const uint8_t *data, size_t length, uint64_t *change_count) {
    @autoreleasepool {
        if (!data && length > 0) return -1;
        NSPasteboard *pasteboard = [NSPasteboard generalPasteboard];
        [pasteboard clearContents];
        NSData *value = [NSData dataWithBytes:data length:length];
        BOOL written = NO;
        if (kind == 1) {
            written = [pasteboard setData:value forType:@"public.utf8-plain-text"];
        } else if (kind == 2) {
            written = [pasteboard setData:value forType:@"public.png"];
        } else {
            return -2;
        }
        if (!written) return -3;
        if (change_count) *change_count = (uint64_t)pasteboard.changeCount;
        return 0;
    }
}

uint64_t rs_clipboard_change_count(void) {
    @autoreleasepool {
        return (uint64_t)[[NSPasteboard generalPasteboard] changeCount];
    }
}

// ---------------------------------------------------------------------------
// Keychain

static NSDictionary *keychain_attributes(NSString *account) {
    return @{
        (__bridge NSString *)kSecClass: (__bridge NSString *)kSecClassGenericPassword,
        (__bridge NSString *)kSecAttrService: [NSString stringWithUTF8String:"ai.gxlself.remotehost"],
        (__bridge NSString *)kSecAttrAccount: account,
        (__bridge NSString *)kSecAttrSynchronizable: @NO,
    };
}

int32_t rs_keychain_load(const char *account, uint8_t *out, size_t capacity, size_t *length) {
    @autoreleasepool {
        if (!account || !length) return -1;
        NSString *name = [NSString stringWithUTF8String:account];
        if (!name) return -1;
        NSMutableDictionary *query = [keychain_attributes(name) mutableCopy];
        query[(__bridge NSString *)kSecReturnData] = @YES;
        query[(__bridge NSString *)kSecMatchLimit] = (__bridge NSString *)kSecMatchLimitOne;
        CFTypeRef result = NULL;
        OSStatus status = SecItemCopyMatching((__bridge CFDictionaryRef)query, &result);
        if (status == errSecItemNotFound) return 0;
        if (status != errSecSuccess || !result) return (int32_t)status;
        NSData *value = CFBridgingRelease(result);
        *length = value.length;
        if (!out || capacity == 0) return 1;
        if (value.length > capacity) return -2;
        [value getBytes:out length:value.length];
        return 1;
    }
}

int32_t rs_keychain_store(const char *account, const uint8_t *data, size_t length) {
    @autoreleasepool {
        if (!account || (!data && length > 0)) return -1;
        NSString *name = [NSString stringWithUTF8String:account];
        if (!name) return -1;
        NSDictionary *query = keychain_attributes(name);
        NSDictionary *attributes = @{
            (__bridge NSString *)kSecValueData: [NSData dataWithBytes:data length:length],
            (__bridge NSString *)kSecAttrAccessible:
                (__bridge NSString *)kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
            (__bridge NSString *)kSecAttrSynchronizable: @NO,
        };
        OSStatus status = SecItemUpdate((__bridge CFDictionaryRef)query, (__bridge CFDictionaryRef)attributes);
        if (status == errSecItemNotFound) {
            NSMutableDictionary *item = [query mutableCopy];
            [item addEntriesFromDictionary:attributes];
            status = SecItemAdd((__bridge CFDictionaryRef)item, NULL);
        }
        return status == errSecSuccess ? 0 : (int32_t)status;
    }
}

int32_t rs_keychain_delete(const char *account) {
    @autoreleasepool {
        if (!account) return -1;
        NSString *name = [NSString stringWithUTF8String:account];
        if (!name) return -1;
        OSStatus status = SecItemDelete((__bridge CFDictionaryRef)keychain_attributes(name));
        return status == errSecSuccess || status == errSecItemNotFound ? 0 : (int32_t)status;
    }
}

// ---------------------------------------------------------------------------
// Platform API: independent SCK capture and VT encoder handles.

typedef struct {
    uint32_t id, width, height;
    int32_t x, y;
    float scale;
    uint32_t refresh_hz, primary, rotation;
} rs_display_record;

uint32_t rs_list_displays(rs_display_record *out, uint32_t capacity) {
    CGDirectDisplayID ids[32];
    uint32_t count = 0;
    if (CGGetActiveDisplayList(32, ids, &count) != kCGErrorSuccess) return 0;
    count = MIN(count, capacity);
    for (uint32_t i = 0; i < count; i++) {
        size_t w = 0, h = 0;
        display_pixels(ids[i], &w, &h);
        CGRect bounds = CGDisplayBounds(ids[i]);
        CGDisplayModeRef mode = CGDisplayCopyDisplayMode(ids[i]);
        double hz = mode ? CGDisplayModeGetRefreshRate(mode) : 0;
        if (mode) CGDisplayModeRelease(mode);
        out[i] = (rs_display_record) {
            .id = ids[i], .width = (uint32_t)w, .height = (uint32_t)h,
            .x = (int32_t)bounds.origin.x, .y = (int32_t)bounds.origin.y,
            .scale = bounds.size.width > 0 ? (float)(w / bounds.size.width) : 1,
            .refresh_hz = hz > 0 ? (uint32_t)llround(hz) : 60,
            .primary = ids[i] == CGMainDisplayID(), .rotation = (uint32_t)CGDisplayRotation(ids[i]),
        };
    }
    return count;
}

static void stop_frame_capture(RSCapture *cap) {
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    [cap->_stream stopCaptureWithCompletionHandler:^(NSError *error) {
        dispatch_semaphore_signal(sem);
    }];
    dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC));
    NSError *removeError = nil;
    [cap->_stream removeStreamOutput:cap type:SCStreamOutputTypeScreen error:&removeError];
    dispatch_sync(cap->_queue, ^{
        cap->_frameCb = NULL;
        cap->_cb = NULL;
    });
    cap->_stream = nil;
}

int32_t rs_frame_capture_start_region(uint32_t display_id, uint32_t fps, uint32_t width, uint32_t height,
                                      double src_x, double src_y, double src_w, double src_h, /* display-local points */
                                      void *ctx, rs_frame_cb cb, void **handle, char *err, size_t errlen) {
    @autoreleasepool {
        *handle = NULL;
        if (!rs_screen_capture_allowed(0)) {
            set_err(err, errlen, @"screen recording permission not granted");
            return 2;
        }
        wake_displays();
        __block SCShareableContent *content = nil;
        __block NSError *contentError = nil;
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        [SCShareableContent getShareableContentExcludingDesktopWindows:NO onScreenWindowsOnly:YES
            completionHandler:^(SCShareableContent *result, NSError *error) {
                content = result;
                contentError = error;
                dispatch_semaphore_signal(sem);
            }];
        if (dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) != 0) {
            set_err(err, errlen, @"timed out listing displays");
            return 1;
        }
        if (!content) {
            set_err(err, errlen, contentError.localizedDescription ?: @"cannot list displays");
            return contentError.code == SCStreamErrorUserDeclined ? 2 : 1;
        }
        SCDisplay *display = nil;
        for (SCDisplay *candidate in content.displays) {
            if (candidate.displayID == display_id) { display = candidate; break; }
        }
        if (!display) {
            set_err(err, errlen, @"requested display is unavailable");
            return 1;
        }
        RSCapture *cap = [RSCapture new];
        cap->_ctx = ctx;
        cap->_frameCb = cb;
        cap->_queue = dispatch_queue_create("rs.platform.capture", dispatch_queue_attr_make_with_qos_class(
            DISPATCH_QUEUE_SERIAL, QOS_CLASS_USER_INTERACTIVE, 0));
        SCContentFilter *filter = [[SCContentFilter alloc] initWithDisplay:display excludingWindows:@[]];
        SCStreamConfiguration *config = [SCStreamConfiguration new];
        config.width = width;
        config.height = height;
        if (src_w > 0 && src_h > 0) {
            config.sourceRect = CGRectMake(src_x, src_y, src_w, src_h);
        }
        config.minimumFrameInterval = CMTimeMake(1, (int32_t)fps);
        config.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange;
        config.colorMatrix = kCGDisplayStreamYCbCrMatrix_ITU_R_709_2;
        config.showsCursor = NO;
        config.queueDepth = 3;
        config.capturesAudio = NO;
        cap->_stream = [[SCStream alloc] initWithFilter:filter configuration:config delegate:cap];
        NSError *addError = nil;
        if (![cap->_stream addStreamOutput:cap type:SCStreamOutputTypeScreen sampleHandlerQueue:cap->_queue error:&addError]) {
            cap->_frameCb = NULL;
            set_err(err, errlen, addError.localizedDescription ?: @"cannot add stream output");
            return 1;
        }
        __block NSError *startError = nil;
        sem = dispatch_semaphore_create(0);
        [cap->_stream startCaptureWithCompletionHandler:^(NSError *error) {
            startError = error;
            dispatch_semaphore_signal(sem);
        }];
        BOOL timedOut = dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) != 0;
        if (timedOut || startError) {
            stop_frame_capture(cap);
            set_err(err, errlen, timedOut ? @"timed out starting capture" : startError.localizedDescription);
            return startError.code == SCStreamErrorUserDeclined ? 2 : 1;
        }
        hold_display_awake(true);
        *handle = (__bridge_retained void *)cap;
        return 0;
    }
}

int32_t rs_frame_capture_start(uint32_t display_id, uint32_t fps, uint32_t width, uint32_t height,
                               void *ctx, rs_frame_cb cb, void **handle, char *err, size_t errlen) {
    return rs_frame_capture_start_region(display_id, fps, width, height, 0, 0, 0, 0,
                                         ctx, cb, handle, err, errlen);
}

void rs_frame_capture_stop(void *handle) {
    @autoreleasepool {
        RSCapture *cap = (__bridge_transfer RSCapture *)handle;
        stop_frame_capture(cap);
        hold_display_awake(false);
    }
}

void rs_pixel_buffer_retain(void *pixel) { CVPixelBufferRetain((CVPixelBufferRef)pixel); }
void rs_pixel_buffer_release(void *pixel) { CVPixelBufferRelease((CVPixelBufferRef)pixel); }

static void fill_test_luma(uint8_t *yPlane, size_t yStride, uint32_t width, uint32_t height, uint32_t seed, uint32_t pattern) {
    uint32_t moveX = seed * 7u;
    uint32_t moveY = seed * 3u;
    uint32_t blockW = MAX(2u, width / 10u);
    uint32_t blockH = MAX(2u, height / 12u);
    uint32_t blockX = (moveX % (width - blockW + 1u)) & ~1u;
    uint32_t blockY = (moveY % (height - blockH + 1u)) & ~1u;
    uint32_t barW = MAX(2u, width / 5u);
    uint32_t barH = MAX(1u, height / 64u);
    uint32_t barX = ((seed * 3u) % (width - barW + 1u));
    uint32_t barY = height * 3u / 4u;
    if (pattern == 0) {
        // Draw the screen-like source in spans: no per-pixel hashing or
        // division on the default 60 fps path, including debug test builds.
        for (uint32_t y = 0; y < height; y++) memset(yPlane + (size_t)y * yStride, 48, width);
        for (uint32_t y = height / 8u; y < height * 7u / 8u; y++) {
            uint8_t gradient = (uint8_t)(64u + (uint64_t)y * 48u / height);
            memset(yPlane + (size_t)y * yStride + width / 4u, gradient, width - width / 4u);
        }
        for (uint32_t y = height / 4u; y < height * 2u / 3u; y++) {
            if (y % 24u >= 3u) continue;
            uint32_t right = width * 7u / 8u;
            for (uint32_t x = width / 3u; x < right; x += 12u)
                memset(yPlane + (size_t)y * yStride + x, 160, MIN(9u, right - x));
        }
        for (uint32_t y = blockY; y < blockY + blockH; y++)
            memset(yPlane + (size_t)y * yStride + blockX, 192, blockW);
        for (uint32_t y = barY; y < barY + barH; y++)
            memset(yPlane + (size_t)y * yStride + barX, 208, barW);
        return;
    }
    for (uint32_t y = 0; y < height; y++) {
        for (uint32_t x = 0; x < width; x++) {
            // Preserve the original stress source for comparison.
            uint32_t hash = (x * 0x9e3779b9u) ^ (y * 0x85ebca6bu) ^ (seed * 0xc2b2ae35u);
            hash ^= hash >> 16;
            hash *= 0x7feb352du;
            hash ^= hash >> 15;
            BOOL noisy = (((x + moveX) / 96u + (y + moveY) / 64u) % 3u) == 0;
            uint32_t gradient = (x + y * 3u + seed * 11u) % 200u;
            yPlane[(size_t)y * yStride + x] = (uint8_t)(16u + (noisy ? hash % 220u : gradient));
        }
    }
}

typedef struct {
    uint32_t key;
    uint32_t index;
} RSChurnKey;

static uint32_t churn_hash(uint32_t seed, uint32_t block) {
    uint32_t value = seed * 0x9e3779b9u + block * 0x85ebca6bu + 0xc2b2ae35u;
    value ^= value >> 16;
    value *= 0x7feb352du;
    value ^= value >> 15;
    return value;
}

static int churn_key_compare(const void *left, const void *right) {
    const RSChurnKey *a = left;
    const RSChurnKey *b = right;
    if (a->key < b->key) return -1;
    if (a->key > b->key) return 1;
    return a->index < b->index ? -1 : (a->index > b->index ? 1 : 0);
}

// Draw the same deterministic block set as encode::churn_blocks. The fixed
// background is generated with seed zero; only selected blocks depend on the
// per-frame seed, so churn=0 is a byte-for-byte static frame.
static void fill_churn_luma_texture(uint8_t *yPlane, size_t yStride, uint32_t width, uint32_t height,
                                    uint32_t seed, uint32_t churnPercent, uint32_t texture) {
    fill_test_luma(yPlane, yStride, width, height, 0, 0);
    if (churnPercent == 0) return;
    if (churnPercent > 100) churnPercent = 100;
    uint32_t columns = (width + 63u) / 64u;
    uint32_t rows = (height + 63u) / 64u;
    uint64_t total64 = (uint64_t)columns * rows;
    if (total64 == 0 || total64 > SIZE_MAX / sizeof(RSChurnKey)) return;
    uint32_t total = (uint32_t)total64;
    uint32_t selected = (uint32_t)(((uint64_t)total * churnPercent + 99u) / 100u);
    RSChurnKey *keys = calloc(total, sizeof(*keys));
    if (!keys) return;
    for (uint32_t index = 0; index < total; index++) {
        keys[index].key = churn_hash(seed, index);
        keys[index].index = index;
    }
    qsort(keys, total, sizeof(*keys), churn_key_compare);
    for (uint32_t rank = 0; rank < selected; rank++) {
        uint32_t index = keys[rank].index;
        uint32_t blockX = (index % columns) * 64u;
        uint32_t blockY = (index / columns) * 64u;
        uint32_t blockW = MIN(64u, width - blockX);
        uint32_t blockH = MIN(64u, height - blockY);
        for (uint32_t y = 0; y < blockH; y++) {
            for (uint32_t x = 0; x < blockW; x++) {
                uint32_t pixel = (blockY + y) * width + blockX + x;
                uint32_t value;
                if (texture == 0) {
                    // Smooth intra-block texture preserves spatial structure
                    // while changing deterministically from frame to frame.
                    value = (blockX + x) * 3u + (blockY + y) * 5u + seed * 11u + index * 17u;
                    value %= 220u;
                } else {
                    uint32_t hash = churn_hash(seed ^ (index * 0x27d4eb2du), pixel);
                    value = hash % 220u;
                }
                yPlane[(size_t)(blockY + y) * yStride + blockX + x] = (uint8_t)(16u + value);
            }
        }
    }
    free(keys);
}

static void fill_churn_luma(uint8_t *yPlane, size_t yStride, uint32_t width, uint32_t height,
                            uint32_t seed, uint32_t churnPercent) {
    fill_churn_luma_texture(yPlane, yStride, width, height, seed, churnPercent, 1);
}

// Exercise exactly the same luma generator without IOSurface or VT access.
int32_t rs_test_pixel_buffer_luma(uint32_t width, uint32_t height, uint32_t seed, uint32_t pattern, uint8_t *out, size_t len) {
    if (!out || pattern > 1 || width < 2 || height < 2 || width > INT32_MAX || height > INT32_MAX ||
        (width & 1) || (height & 1) || (uint64_t)width * height > len) return -1;
    fill_test_luma(out, width, width, height, seed, pattern);
    return 0;
}

int32_t rs_test_pixel_buffer_luma_churn(uint32_t width, uint32_t height, uint32_t seed,
                                        uint32_t churnPercent, uint8_t *out, size_t len) {
    if (!out || churnPercent > 100 || width < 2 || height < 2 || width > INT32_MAX || height > INT32_MAX ||
        (width & 1) || (height & 1) || (uint64_t)width * height > len)
        return -1;
    fill_churn_luma(out, width, width, height, seed, churnPercent);
    return 0;
}

int32_t rs_test_pixel_buffer_luma_churn_texture(uint32_t width, uint32_t height, uint32_t seed,
                                                uint32_t churnPercent, uint32_t texture,
                                                uint8_t *out, size_t len) {
    if (!out || churnPercent > 100 || texture > 1 || width < 2 || height < 2 || width > INT32_MAX ||
        height > INT32_MAX || (width & 1) || (height & 1) || (uint64_t)width * height > len)
        return -1;
    fill_churn_luma_texture(out, width, width, height, seed, churnPercent, texture);
    return 0;
}

// pattern 0: screen-like static background with small moving regions;
// pattern 1: deliberately hard-to-compress, temporally changing hash noise.
// Correctness tests must use smooth; noise exercises the hard rate limit.
void *rs_test_pixel_buffer_new_pattern(uint32_t width, uint32_t height, uint32_t seed, uint32_t pattern) {
    @autoreleasepool {
        if (pattern > 1 || width < 2 || height < 2 || width > INT32_MAX || height > INT32_MAX || (width & 1) || (height & 1)) return NULL;
        NSDictionary *attrs = @{
            (__bridge NSString *)kCVPixelBufferIOSurfacePropertiesKey : @{},
            (__bridge NSString *)kCVPixelBufferMetalCompatibilityKey : @YES,
        };
        CVPixelBufferRef pixel = NULL;
        CVReturn result = CVPixelBufferCreate(kCFAllocatorDefault, width, height,
                                              kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                                              (__bridge CFDictionaryRef)attrs, &pixel);
        if (result != kCVReturnSuccess || !pixel) return NULL;
        if (!CVPixelBufferGetIOSurface(pixel) || CVPixelBufferLockBaseAddress(pixel, 0) != kCVReturnSuccess) {
            CVPixelBufferRelease(pixel);
            return NULL;
        }
        uint8_t *yPlane = CVPixelBufferGetBaseAddressOfPlane(pixel, 0);
        uint8_t *uvPlane = CVPixelBufferGetBaseAddressOfPlane(pixel, 1);
        size_t yStride = CVPixelBufferGetBytesPerRowOfPlane(pixel, 0);
        size_t uvStride = CVPixelBufferGetBytesPerRowOfPlane(pixel, 1);
        memset(yPlane, 16, yStride * height);
        memset(uvPlane, 128, uvStride * (height / 2));
        fill_test_luma(yPlane, yStride, width, height, seed, pattern);
        uint32_t blockW = MAX(2u, width / 10u);
        uint32_t blockH = MAX(2u, height / 12u);
        uint32_t blockX = ((seed * 7u) % (width - blockW + 1u)) & ~1u;
        uint32_t blockY = ((seed * 3u) % (height - blockH + 1u)) & ~1u;
        if (pattern == 0) {
            for (uint32_t y = blockY / 2u; y < (blockY + blockH + 1u) / 2u; y++) {
                for (uint32_t x = blockX / 2u; x < (blockX + blockW + 1u) / 2u; x++) {
                    uvPlane[(size_t)y * uvStride + x * 2] = 96;
                    uvPlane[(size_t)y * uvStride + x * 2 + 1] = 176;
                }
            }
        } else {
            for (uint32_t y = 0; y < height / 2; y++) {
                for (uint32_t x = 0; x < width / 2; x++) {
                    uvPlane[(size_t)y * uvStride + x * 2] = (uint8_t)(80u + ((x * 3u + seed * 5u) & 0x7fu));
                    uvPlane[(size_t)y * uvStride + x * 2 + 1] = (uint8_t)(80u + ((y * 5u + seed * 7u) & 0x7fu));
                }
            }
        }
        CVPixelBufferUnlockBaseAddress(pixel, 0);
        return (void *)pixel;
    }
}

void *rs_test_pixel_buffer_new_churn_texture(uint32_t width, uint32_t height, uint32_t seed,
                                             uint32_t churnPercent, uint32_t texture);

void *rs_test_pixel_buffer_new_churn(uint32_t width, uint32_t height, uint32_t seed, uint32_t churnPercent) {
    return rs_test_pixel_buffer_new_churn_texture(width, height, seed, churnPercent, 1);
}

void *rs_test_pixel_buffer_new_churn_texture(uint32_t width, uint32_t height, uint32_t seed,
                                             uint32_t churnPercent, uint32_t texture) {
    @autoreleasepool {
        if (churnPercent > 100 || width < 2 || height < 2 || width > INT32_MAX || height > INT32_MAX ||
            (width & 1) || (height & 1) || texture > 1)
            return NULL;
        NSDictionary *attrs = @{
            (__bridge NSString *)kCVPixelBufferIOSurfacePropertiesKey : @{},
            (__bridge NSString *)kCVPixelBufferMetalCompatibilityKey : @YES,
        };
        CVPixelBufferRef pixel = NULL;
        CVReturn result = CVPixelBufferCreate(kCFAllocatorDefault, width, height,
                                              kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                                              (__bridge CFDictionaryRef)attrs, &pixel);
        if (result != kCVReturnSuccess || !pixel) return NULL;
        if (!CVPixelBufferGetIOSurface(pixel) || CVPixelBufferLockBaseAddress(pixel, 0) != kCVReturnSuccess) {
            CVPixelBufferRelease(pixel);
            return NULL;
        }
        uint8_t *yPlane = CVPixelBufferGetBaseAddressOfPlane(pixel, 0);
        uint8_t *uvPlane = CVPixelBufferGetBaseAddressOfPlane(pixel, 1);
        size_t yStride = CVPixelBufferGetBytesPerRowOfPlane(pixel, 0);
        size_t uvStride = CVPixelBufferGetBytesPerRowOfPlane(pixel, 1);
        memset(yPlane, 16, yStride * height);
        memset(uvPlane, 128, uvStride * (height / 2));
        fill_churn_luma_texture(yPlane, yStride, width, height, seed, churnPercent, texture);

        // Keep chroma deterministic and block-local as well. The luma plane
        // remains the quality reference used by the probe's PSNR report.
        uint32_t columns = (width + 63u) / 64u;
        uint32_t rows = (height + 63u) / 64u;
        uint32_t total = columns * rows;
        RSChurnKey *keys = total ? calloc(total, sizeof(*keys)) : NULL;
        if (keys) {
            uint32_t selected = (uint32_t)(((uint64_t)total * MIN(churnPercent, 100u) + 99u) / 100u);
            for (uint32_t index = 0; index < total; index++) {
                keys[index].key = churn_hash(seed, index);
                keys[index].index = index;
            }
            qsort(keys, total, sizeof(*keys), churn_key_compare);
            for (uint32_t rank = 0; rank < selected; rank++) {
                uint32_t index = keys[rank].index;
                uint32_t blockX = (index % columns) * 32u;
                uint32_t blockY = (index / columns) * 32u;
                uint32_t blockW = MIN(32u, width / 2u - blockX);
                uint32_t blockH = MIN(32u, height / 2u - blockY);
                for (uint32_t y = 0; y < blockH; y++) {
                    for (uint32_t x = 0; x < blockW; x++) {
                        uint32_t hash = churn_hash(seed ^ (index * 0x27d4eb2du),
                                                   (blockY + y) * (width / 2u) + blockX + x);
                        if (texture == 0) {
                            uint32_t smooth = blockX + x + (blockY + y) * 3u + seed * 5u + index * 7u;
                            uvPlane[(size_t)(blockY + y) * uvStride + (blockX + x) * 2] =
                                (uint8_t)(80u + (smooth % 48u));
                            uvPlane[(size_t)(blockY + y) * uvStride + (blockX + x) * 2 + 1] =
                                (uint8_t)(112u + ((smooth / 3u) % 32u));
                        } else {
                            uvPlane[(size_t)(blockY + y) * uvStride + (blockX + x) * 2] =
                                (uint8_t)(64u + (hash & 0x7fu));
                            uvPlane[(size_t)(blockY + y) * uvStride + (blockX + x) * 2 + 1] =
                                (uint8_t)(64u + ((hash >> 8) & 0x7fu));
                        }
                    }
                }
            }
            free(keys);
        }
        CVPixelBufferUnlockBaseAddress(pixel, 0);
        return (void *)pixel;
    }
}

void *rs_test_pixel_buffer_new(uint32_t width, uint32_t height, uint32_t seed) {
    return rs_test_pixel_buffer_new_pattern(width, height, seed, 0);
}

typedef struct {
    uint8_t *out;
    size_t outLen;
    uint32_t width;
    uint32_t height;
    int seen;
    OSStatus status;
} RSDecodeContext;

static const uint8_t *next_annexb_start(const uint8_t *data, size_t len, size_t from, size_t *codeLen) {
    if (!data || !codeLen) return NULL;
    for (size_t index = from; index + 2 < len; index++) {
        if (index + 4 <= len && data[index] == 0 && data[index + 1] == 0 && data[index + 2] == 0 && data[index + 3] == 1) {
            *codeLen = 4;
            return data + index;
        }
        if (data[index] == 0 && data[index + 1] == 0 && data[index + 2] == 1) {
            *codeLen = 3;
            return data + index;
        }
    }
    return NULL;
}

static size_t collect_annexb_nals(const uint8_t *data, size_t len, const uint8_t **nals,
                                  size_t *sizes, size_t capacity) {
    size_t count = 0;
    size_t search = 0;
    while (count < capacity) {
        size_t codeLen = 0;
        const uint8_t *start = next_annexb_start(data, len, search, &codeLen);
        if (!start) break;
        size_t startOffset = (size_t)(start - data);
        size_t nalOffset = startOffset + codeLen;
        size_t nextCodeLen = 0;
        const uint8_t *next = next_annexb_start(data, len, nalOffset, &nextCodeLen);
        size_t endOffset = next ? (size_t)(next - data) : len;
        if (endOffset > nalOffset) {
            nals[count] = data + nalOffset;
            sizes[count] = endOffset - nalOffset;
            count++;
        }
        search = endOffset;
    }
    return count;
}

static void rs_decode_output(void *decompressionOutputRefCon, void *sourceFrameRefCon,
                             OSStatus status, VTDecodeInfoFlags infoFlags, CVImageBufferRef imageBuffer,
                             CMTime presentationTimeStamp, CMTime presentationDuration) {
    (void)decompressionOutputRefCon;
    (void)sourceFrameRefCon;
    (void)infoFlags;
    (void)presentationTimeStamp;
    (void)presentationDuration;
    RSDecodeContext *context = sourceFrameRefCon;
    if (!context) return;
    if (status != noErr || !imageBuffer) {
        context->status = status != noErr ? status : kCMBlockBufferBadCustomBlockSourceErr;
        return;
    }
    CVPixelBufferRef pixel = (CVPixelBufferRef)imageBuffer;
    if (CVPixelBufferGetWidth(pixel) != context->width || CVPixelBufferGetHeight(pixel) != context->height ||
        CVPixelBufferGetPlaneCount(pixel) < 1 || CVPixelBufferLockBaseAddress(pixel, kCVPixelBufferLock_ReadOnly) != kCVReturnSuccess) {
        context->status = kCVReturnInvalidArgument;
        return;
    }
    uint8_t *plane = CVPixelBufferGetBaseAddressOfPlane(pixel, 0);
    size_t stride = CVPixelBufferGetBytesPerRowOfPlane(pixel, 0);
    if (!plane || context->outLen < (size_t)context->width * context->height) {
        CVPixelBufferUnlockBaseAddress(pixel, kCVPixelBufferLock_ReadOnly);
        context->status = kCVReturnInvalidArgument;
        return;
    }
    for (uint32_t y = 0; y < context->height; y++)
        memcpy(context->out + (size_t)y * context->width, plane + (size_t)y * stride, context->width);
    CVPixelBufferUnlockBaseAddress(pixel, kCVPixelBufferLock_ReadOnly);
    context->seen++;
}

static OSStatus make_decode_sample(const uint8_t *data, size_t len, CMVideoFormatDescriptionRef format,
                                   CMSampleBufferRef *sample) {
    const uint8_t *nals[128];
    size_t sizes[128];
    size_t count = collect_annexb_nals(data, len, nals, sizes, 128);
    if (!count) return -1;
    size_t packedLen = len + count * 4;
    uint8_t *packed = malloc(packedLen);
    if (!packed) return -1;
    size_t offset = 0;
    for (size_t index = 0; index < count; index++) {
        uint32_t n = (uint32_t)sizes[index];
        packed[offset++] = (uint8_t)(n >> 24);
        packed[offset++] = (uint8_t)(n >> 16);
        packed[offset++] = (uint8_t)(n >> 8);
        packed[offset++] = (uint8_t)n;
        memcpy(packed + offset, nals[index], sizes[index]);
        offset += sizes[index];
    }
    CMBlockBufferRef block = NULL;
    OSStatus status = CMBlockBufferCreateWithMemoryBlock(kCFAllocatorDefault, packed, offset,
                                                          kCFAllocatorMalloc, NULL, 0, offset, 0, &block);
    if (status != kCMBlockBufferNoErr || !block) {
        free(packed);
        return status;
    }
    CMSampleTimingInfo timing = {kCMTimeInvalid, CMTimeMake(0, 1000000), kCMTimeInvalid};
    size_t sampleSize = offset;
    status = CMSampleBufferCreateReady(kCFAllocatorDefault, block, format, 1, 1, &timing, 1,
                                       &sampleSize, sample);
    CFRelease(block);
    return status;
}

int32_t rs_decode_annexb_sequence_luma(const uint8_t *parameterData, size_t parameterLen,
                              const uint8_t *const *sampleData, const size_t *sampleLens,
                              size_t sampleCount, uint32_t codec,
                              uint32_t width, uint32_t height, uint8_t *out, size_t outLen) {
    if (!parameterData || !parameterLen || !sampleData || !sampleLens || !sampleCount || !out ||
        width < 2 || height < 2 || (width & 1) || (height & 1) ||
        (uint64_t)width * height > outLen || (codec != 1 && codec != 2))
        return -1;
    const uint8_t *nals[128];
    size_t sizes[128];
    size_t count = collect_annexb_nals(parameterData, parameterLen, nals, sizes, 128);
    const uint8_t *parameterSets[8];
    size_t parameterSizes[8];
    size_t parameterCount = 0;
    for (size_t index = 0; index < count && parameterCount < 8; index++) {
        uint8_t type = codec == 1 ? (uint8_t)((nals[index][0] >> 1) & 0x3f) : (nals[index][0] & 0x1f);
        BOOL parameter = codec == 1 ? (type >= 32 && type <= 34) : (type == 7 || type == 8);
        if (parameter) {
            parameterSets[parameterCount] = nals[index];
            parameterSizes[parameterCount++] = sizes[index];
        }
    }
    if (!parameterCount) return -1;
    CMVideoFormatDescriptionRef format = NULL;
    OSStatus status;
    if (codec == 1) {
        status = CMVideoFormatDescriptionCreateFromHEVCParameterSets(kCFAllocatorDefault, parameterCount,
                                                                      parameterSets, parameterSizes, 4, NULL, &format);
    } else {
        status = CMVideoFormatDescriptionCreateFromH264ParameterSets(kCFAllocatorDefault, parameterCount,
                                                                      parameterSets, parameterSizes, 4, &format);
    }
    if (status != noErr || !format) return -1;
    RSDecodeContext context = {.out = out, .outLen = outLen, .width = width, .height = height, .seen = 0, .status = noErr};
    VTDecompressionOutputCallbackRecord callback = {.decompressionOutputCallback = rs_decode_output,
                                                     .decompressionOutputRefCon = &context};
    VTDecompressionSessionRef session = NULL;
    status = VTDecompressionSessionCreate(kCFAllocatorDefault, format, NULL, NULL, &callback, &session);
    if (status == noErr && session) {
        CMSampleBufferRef keySample = NULL;
        status = make_decode_sample(parameterData, parameterLen, format, &keySample);
        if (status == noErr && keySample) {
            status = VTDecompressionSessionDecodeFrame(session, keySample, 0, &context, NULL);
            CFRelease(keySample);
        }
        if (status == noErr) {
            for (size_t index = 0; index < sampleCount; index++) {
                if (!sampleData[index] || !sampleLens[index]) {
                    status = -1;
                    break;
                }
                CMSampleBufferRef targetSample = NULL;
                status = make_decode_sample(sampleData[index], sampleLens[index], format, &targetSample);
                if (status != noErr || !targetSample) break;
                status = VTDecompressionSessionDecodeFrame(session, targetSample, 0, &context, NULL);
                CFRelease(targetSample);
                if (status != noErr) break;
            }
        }
        if (status == noErr) status = VTDecompressionSessionWaitForAsynchronousFrames(session);
        if (context.status != noErr) status = context.status;
        VTDecompressionSessionInvalidate(session);
        CFRelease(session);
    }
    CFRelease(format);
    return status == noErr && context.seen > 0 ? 0 : -1;
}

int32_t rs_decode_annexb_luma(const uint8_t *parameterData, size_t parameterLen,
                              const uint8_t *sampleData, size_t sampleLen, uint32_t codec,
                              uint32_t width, uint32_t height, uint8_t *out, size_t outLen) {
    const uint8_t *samples[] = {sampleData};
    const size_t lengths[] = {sampleLen};
    return rs_decode_annexb_sequence_luma(parameterData, parameterLen, samples, lengths, 1,
                                           codec, width, height, out, outLen);
}

void *rs_encoder_start(uint32_t codec, uint32_t width, uint32_t height, uint32_t fps, uint32_t kbps,
                        void *ctx, rs_encoded_cb cb, char *err, size_t errlen) {
    @autoreleasepool {
        RSCapture *encoder = [RSCapture new];
        encoder->_ctx = ctx;
        encoder->_cb = cb;
        encoder->_queue = dispatch_queue_create("rs.platform.encoder", dispatch_queue_attr_make_with_qos_class(
            DISPATCH_QUEUE_SERIAL, QOS_CLASS_USER_INTERACTIVE, 0));
        NSString *error = nil;
        if (![encoder createEncoderWidth:(int32_t)width height:(int32_t)height fps:fps kbps:kbps codec:codec error:&error]) {
            set_err(err, errlen, error);
            return NULL;
        }
        return (__bridge_retained void *)encoder;
    }
}

int32_t rs_encoder_encode(void *handle, void *pixel, uint64_t capture_us, int32_t force, int32_t boost) {
    @autoreleasepool {
        RSCapture *encoder = (__bridge RSCapture *)handle;
        __block OSStatus status = noErr;
        dispatch_sync(encoder->_queue, ^{
            if (encoder->_last) CVPixelBufferRelease(encoder->_last);
            encoder->_last = CVPixelBufferRetain((CVPixelBufferRef)pixel);
            if (force) request_keyframe(encoder);
            status = [encoder encode:(CVPixelBufferRef)pixel captureUs:capture_us boost:boost != 0];
        });
        return (int32_t)status;
    }
}

void rs_encoder_rate_window(void *handle, uint32_t kbps, uint32_t fps, double window_seconds);

void rs_encoder_rate(void *handle, uint32_t kbps, uint32_t fps) {
    rs_encoder_rate_window(handle, kbps, fps, 1.0);
}

void rs_encoder_rate_window(void *handle, uint32_t kbps, uint32_t fps, double window_seconds) {
    RSCapture *encoder = (__bridge RSCapture *)handle;
    dispatch_sync(encoder->_queue, ^{
        encoder->_baseKbps = kbps;
        encoder->_baseFps = fps;
        encoder->_rateWindow = window_seconds;
        encoder->_pendingRateRestore = 0;
        set_rate_props(encoder, kbps, fps, window_seconds);
    });
}

void rs_encoder_keyframe(void *handle) {
    RSCapture *encoder = (__bridge RSCapture *)handle;
    request_keyframe(encoder);
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 60 * NSEC_PER_MSEC), encoder->_queue, ^{
        if (keyframe_pending(encoder) && encoder->_last && encoder->_session)
            [encoder encode:encoder->_last captureUs:unix_now_us() boost:NO];
    });
}

void rs_encoder_quality(void *handle, int32_t on) {
    RSCapture *encoder = (__bridge RSCapture *)handle;
    dispatch_sync(encoder->_queue, ^{
        encoder->_qualityBoost = on != 0;
        if (!on) {
            if (encoder->_qualitySupported) {
                set_prop(encoder->_session, kVTCompressionPropertyKey_Quality,
                         (__bridge CFNumberRef)@(encoder->_qualityBaseline));
            }
            if (encoder->_pendingRateRestore) restore_rate(encoder);
        }
    });
}

void rs_encoder_info(void *handle, rs_encoder_info_t *out) {
    if (!handle || !out) return;
    RSCapture *encoder = (__bridge RSCapture *)handle;
    dispatch_sync(encoder->_queue, ^{
        out->codec = encoder->_codec;
        out->low_latency = encoder->_lowLatency ? 1 : 0;
        out->hardware = encoder->_hardware ? 1 : 0;
        out->quality_supported = encoder->_qualitySupported ? 1 : 0;
    });
}

void rs_encoder_stats(void *handle, rs_encoder_stats_t *out) {
    if (handle) copy_encoder_stats((__bridge RSCapture *)handle, out);
}

void rs_encoder_stop(void *handle, rs_encoder_stats_t *out) {
    @autoreleasepool {
        RSCapture *encoder = (__bridge_transfer RSCapture *)handle;
        dispatch_sync(encoder->_queue, ^{
            [encoder destroyEncoder];
            copy_encoder_stats(encoder, out);
            encoder->_cb = NULL;
            if (encoder->_last) { CVPixelBufferRelease(encoder->_last); encoder->_last = NULL; }
        });
    }
}

void rs_input_select_display(uint32_t display) { g_inputDisplay = display; }

void rs_input_release_all(void) {
    CGEventRef position = CGEventCreate(NULL);
    CGPoint point = position ? CGEventGetLocation(position) : CGPointZero;
    if (position) CFRelease(position);
    for (int i = 0; i < 5; i++) {
        if (!(g_buttons & (1 << i))) continue;
        CGEventType type = i == 0 ? kCGEventLeftMouseUp :
                           i == 1 ? kCGEventRightMouseUp : kCGEventOtherMouseUp;
        CGEventRef e = CGEventCreateMouseEvent(NULL, type, point, (CGMouseButton)i);
        if (e) CGEventSetIntegerValueField(e, kCGMouseEventButtonNumber, i);
        post(e);
    }
    g_buttons = 0;
}

// ---------------------------------------------------------------------------
// input v2. Keep M0 entry points and the shared tagged post() above intact.

#import <Carbon/Carbon.h>

int32_t rs_kb_profile(uint32_t *layout_type, uint32_t *keyboard_type) {
    if (!layout_type || !keyboard_type) return -1;
    // Carbon documents both APIs as not thread safe. Serialize profile probes
    // even when callers create more than one MacPlatform at the same time.
    static pthread_mutex_t profile_lock = PTHREAD_MUTEX_INITIALIZER;
    pthread_mutex_lock(&profile_lock);
    *keyboard_type = LMGetKbdType();
    *layout_type = *keyboard_type ? KBGetLayoutType((SInt16)*keyboard_type) : 0;
    pthread_mutex_unlock(&profile_lock);
    return *keyboard_type ? 0 : -1;
}

int32_t rs_input_key(uint16_t vk, int32_t kind, uint64_t flags,
                     int32_t autorepeat, uint32_t keyboard_type) {
    if (kind < 0 || kind > 2) return -1;
    // NULL preserves M0's event-source behavior. Hardware shortcut validation
    // must precede any switch to a shared HIDSystemState CGEventSource.
    CGEventRef e = CGEventCreateKeyboardEvent(NULL, (CGKeyCode)vk, kind == 0);
    if (!e) return -1;
    if (kind == 2) CGEventSetType(e, kCGEventFlagsChanged);
    CGEventSetFlags(e, (CGEventFlags)flags);
    CGEventSetIntegerValueField(e, kCGKeyboardEventAutorepeat, autorepeat != 0);
    CGEventSetIntegerValueField(e, kCGKeyboardEventKeyboardType, keyboard_type);
    post(e);
    return 0;
}

int32_t rs_input_unicode(const uint16_t *units, uint32_t len) {
    if (!len) return 0;
    if (!units) return -1;
    CGEventRef down = CGEventCreateKeyboardEvent(NULL, 0, true);
    CGEventRef up = CGEventCreateKeyboardEvent(NULL, 0, false);
    if (!down || !up) {
        if (down) CFRelease(down);
        if (up) CFRelease(up);
        return -1;
    }
    CGEventSetFlags(down, 0);
    CGEventSetFlags(up, 0);
    CGEventKeyboardSetUnicodeString(down, (UniCharCount)len, units);
    CGEventKeyboardSetUnicodeString(up, (UniCharCount)len, units);
    post(down);
    post(up);
    return 0;
}

int32_t rs_input_scroll2(int32_t dy, int32_t dx, uint32_t scroll_phase,
                         uint32_t momentum_phase, int32_t continuous) {
    CGEventRef e = CGEventCreateScrollWheelEvent2(NULL, kCGScrollEventUnitPixel, 2, dy, dx, 0);
    if (!e) return -1;
    CGEventSetIntegerValueField(e, kCGScrollWheelEventScrollPhase, scroll_phase);
    CGEventSetIntegerValueField(e, kCGScrollWheelEventMomentumPhase, momentum_phase);
    CGEventSetIntegerValueField(e, kCGScrollWheelEventIsContinuous, continuous != 0);
    post(e);
    return 0;
}

void rs_input_mouse_button2(int32_t button, int32_t down, double nx, double ny, int32_t clicks) {
    if (button < 0 || button > 4) return;
    if (button <= 2) {
        rs_input_mouse_button(button, down, nx, ny, clicks);
        return;
    }
    if (down) rs_input_mouse_move(nx, ny);
    CGEventRef e = CGEventCreateMouseEvent(NULL,
        down ? kCGEventOtherMouseDown : kCGEventOtherMouseUp,
        to_global(nx, ny), (CGMouseButton)button);
    if (!e) return;
    CGEventSetIntegerValueField(e, kCGMouseEventButtonNumber, button);
    CGEventSetIntegerValueField(e, kCGMouseEventClickState, clicks > 0 ? clicks : 1);
    if (down) {
        g_buttons |= 1 << button;
    } else {
        g_buttons &= ~(1 << button);
    }
    post(e);
}
