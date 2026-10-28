// ScreenCaptureKit 窗口采集 shim。
#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>

#include <mach/mach_time.h>
#include <math.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <time.h>

typedef struct {
    uint64_t id;
    int32_t pid;
    uint32_t width;
    uint32_t height;
    int32_t on_screen;
    char app[256];
    char title[512];
} rsb_window;

typedef void (*rsb_frame_cb)(void *ctx, uint64_t t_us, uint32_t width, uint32_t height,
                             const uint8_t *y, size_t stride);
typedef void (*rsb_error_cb)(void *ctx, const char *msg);

static uint64_t unix_now_us(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (uint64_t)ts.tv_sec * 1000000ull + (uint64_t)ts.tv_nsec / 1000ull;
}

// SCStreamFrameInfoDisplayTime is in mach_absolute_time ticks, as in host-mac.
static uint64_t mach_to_unix_us(uint64_t mach) {
    static mach_timebase_info_data_t tb;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ mach_timebase_info(&tb); });
    uint64_t now_mach = mach_absolute_time();
    uint64_t now_unix = unix_now_us();
    if (mach >= now_mach) return now_unix;
    // The wider intermediate also handles a stale timestamp without overflow.
    uint64_t delta_us = (uint64_t)(((__uint128_t)(now_mach - mach) * tb.numer) / tb.denom / 1000);
    return delta_us > now_unix ? 0 : now_unix - delta_us;
}

uint64_t rsb_now_unix_us(void) {
    return mach_to_unix_us(mach_absolute_time());
}

static void copy_utf8(char *out, size_t capacity, NSString *text) {
    if (!out || capacity == 0) return;
    out[0] = '\0';
    if (!text) return;
    NSUInteger used = 0;
    // NSString writes only whole UTF-8 characters, even when the buffer fills.
    [text getBytes:out maxLength:capacity - 1 usedLength:&used encoding:NSUTF8StringEncoding
           options:0 range:NSMakeRange(0, text.length) remainingRange:NULL];
    out[used] = '\0';
}

static void set_error(char *err, int32_t err_len, NSString *message) {
    if (err_len > 0) copy_utf8(err, (size_t)err_len, message ?: @"unknown error");
}

int32_t rsb_permission(int32_t request) {
    if (@available(macOS 10.15, *)) {
        if (CGPreflightScreenCaptureAccess()) return 1;
        if (request) return CGRequestScreenCaptureAccess() ? 1 : 0;
    }
    return 0;
}

static NSString *capture_error(NSString *operation, NSError *error) API_AVAILABLE(macos(12.3));
static NSString *capture_error(NSString *operation, NSError *error) {
    NSString *description = error.localizedDescription ?: @"unknown error";
    if (error.code == SCStreamErrorUserDeclined || !rsb_permission(0)) {
        return [NSString stringWithFormat:@"%@: screen recording permission denied: %@", operation, description];
    }
    return [NSString stringWithFormat:@"%@: %@", operation, description];
}

static SCShareableContent *shareable_content(NSString **message) API_AVAILABLE(macos(12.3));
static SCShareableContent *shareable_content(NSString **message) {
    // Listing must not ask for TCC access; only rsb_permission(request) may do so.
    if (!rsb_permission(0)) {
        *message = @"screen recording permission not granted";
        return nil;
    }
    __block SCShareableContent *content = nil;
    __block NSError *content_error = nil;
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    [SCShareableContent getShareableContentExcludingDesktopWindows:YES onScreenWindowsOnly:NO
        completionHandler:^(SCShareableContent *result, NSError *error) {
            content = result;
            content_error = error;
            dispatch_semaphore_signal(sem);
        }];
    if (dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) != 0) {
        *message = @"timeout";
        return nil;
    }
    if (!content || content_error) {
        *message = capture_error(@"cannot list windows", content_error);
        return nil;
    }
    return content;
}

static CGFloat window_scale(CGRect frame) {
    NSArray<NSScreen *> *screens = NSScreen.screens;
    // firstObject is the primary screen, unlike mainScreen which follows focus.
    NSScreen *primary = screens.firstObject;
    CGFloat scale = primary ? primary.backingScaleFactor : 1.0;
    CGFloat primary_height = primary ? primary.frame.size.height : 0.0;
    CGPoint center = CGPointMake(CGRectGetMidX(frame), CGRectGetMidY(frame));
    for (NSScreen *screen in screens) {
        NSRect ns = screen.frame;
        CGRect cg = CGRectMake(ns.origin.x, primary_height - ns.origin.y - ns.size.height,
                               ns.size.width, ns.size.height);
        if (CGRectContainsPoint(cg, center)) {
            scale = screen.backingScaleFactor;
            break;
        }
    }
    return scale;
}

static uint32_t pixel_extent(CGFloat points, CGFloat scale) {
    double pixels = round((double)points * (double)scale);
    if (!isfinite(pixels) || pixels <= 0) return 0;
    return pixels > UINT32_MAX ? UINT32_MAX : (uint32_t)pixels;
}

static void window_pixels(CGRect frame, uint32_t *width, uint32_t *height) {
    CGFloat scale = window_scale(frame);
    *width = pixel_extent(frame.size.width, scale);
    *height = pixel_extent(frame.size.height, scale);
}

int32_t rsb_list_windows(rsb_window *out, int32_t cap, char *err, int32_t err_len) {
    @autoreleasepool {
        set_error(err, err_len, @"");
        if (cap < 0 || (cap > 0 && !out)) {
            set_error(err, err_len, @"invalid window buffer");
            return -1;
        }
        if (@available(macOS 12.3, *)) {
            NSString *message = nil;
            SCShareableContent *content = shareable_content(&message);
            if (!content) {
                set_error(err, err_len, message);
                return -1;
            }
            if (content.windows.count > INT32_MAX) {
                set_error(err, err_len, @"too many windows");
                return -1;
            }
            int32_t count = (int32_t)content.windows.count;
            for (int32_t i = 0; i < count && i < cap; i++) {
                SCWindow *window = content.windows[(NSUInteger)i];
                rsb_window *record = &out[i];
                memset(record, 0, sizeof(*record));
                record->id = window.windowID;
                record->pid = window.owningApplication.processID;
                window_pixels(window.frame, &record->width, &record->height);
                record->on_screen = window.isOnScreen ? 1 : 0;
                NSString *app = window.owningApplication.bundleIdentifier;
                if (app.length == 0) app = window.owningApplication.applicationName;
                copy_utf8(record->app, sizeof(record->app), app);
                copy_utf8(record->title, sizeof(record->title), window.title);
            }
            return count;
        }
        set_error(err, err_len, @"ScreenCaptureKit requires macOS 12.3 or later");
        return -1;
    }
}

API_AVAILABLE(macos(12.3))
@interface RSBCapture : NSObject <SCStreamOutput, SCStreamDelegate>
@end

@implementation RSBCapture {
@public
    SCStream *_stream;
    dispatch_queue_t _queue;
    void *_ctx;
    rsb_frame_cb _on_frame;
    rsb_error_cb _on_error;
    atomic_bool _stopped;
    BOOL _has_output;
    BOOL _error_sent; // Only accessed on _queue, including delegate errors.
}

- (instancetype)init {
    self = [super init];
    if (self) atomic_init(&_stopped, false);
    return self;
}

- (void)stream:(SCStream *)stream didOutputSampleBuffer:(CMSampleBufferRef)sample ofType:(SCStreamOutputType)type {
    @autoreleasepool {
        if (atomic_load_explicit(&_stopped, memory_order_acquire)) return;
        if (type != SCStreamOutputTypeScreen || !CMSampleBufferIsValid(sample)) return;
        CFArrayRef attachments = CMSampleBufferGetSampleAttachmentsArray(sample, false);
        if (!attachments || CFArrayGetCount(attachments) == 0) return;
        NSDictionary *info = (__bridge NSDictionary *)CFArrayGetValueAtIndex(attachments, 0);
        NSNumber *status = info[SCStreamFrameInfoStatus];
        if (!status || status.integerValue != SCFrameStatusComplete) return;
        CVPixelBufferRef pixel = CMSampleBufferGetImageBuffer(sample);
        if (!pixel || CVPixelBufferGetPixelFormatType(pixel) != kCVPixelFormatType_420YpCbCr8BiPlanarFullRange ||
            CVPixelBufferGetPlaneCount(pixel) < 1) return;
        NSNumber *display_time = info[SCStreamFrameInfoDisplayTime];
        uint64_t t_us = display_time ? mach_to_unix_us(display_time.unsignedLongLongValue) : rsb_now_unix_us();
        if (CVPixelBufferLockBaseAddress(pixel, kCVPixelBufferLock_ReadOnly) != kCVReturnSuccess) return;
        const uint8_t *y = CVPixelBufferGetBaseAddressOfPlane(pixel, 0);
        size_t stride = CVPixelBufferGetBytesPerRowOfPlane(pixel, 0);
        size_t width = CVPixelBufferGetWidthOfPlane(pixel, 0);
        size_t height = CVPixelBufferGetHeightOfPlane(pixel, 0);
        if (y && width > 0 && height > 0 && width <= UINT32_MAX && height <= UINT32_MAX &&
            stride >= width && _on_frame && !atomic_load_explicit(&_stopped, memory_order_acquire)) {
            _on_frame(_ctx, t_us, (uint32_t)width, (uint32_t)height, y, stride);
        }
        CVPixelBufferUnlockBaseAddress(pixel, kCVPixelBufferLock_ReadOnly);
    }
}

- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
    if (atomic_load_explicit(&_stopped, memory_order_acquire)) return;
    // SCK delegate notifications need not arrive on the sample queue. Route
    // them through it so the same drain protects both Rust callbacks' context.
    dispatch_async(_queue, ^{
        if (atomic_load_explicit(&self->_stopped, memory_order_acquire) || self->_error_sent) return;
        self->_error_sent = YES;
        if (self->_on_error) {
            NSString *message = error.localizedDescription ?: @"capture stopped";
            self->_on_error(self->_ctx, message.UTF8String ?: "capture stopped");
        }
    });
}
@end

static void stop_capture(RSBCapture *capture) API_AVAILABLE(macos(12.3));
static void stop_capture(RSBCapture *capture) {
    atomic_store_explicit(&capture->_stopped, true, memory_order_release);
    SCStream *stream = capture->_stream;
    if (stream) {
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        [stream stopCaptureWithCompletionHandler:^(NSError *error) {
            dispatch_semaphore_signal(sem);
        }];
        dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC));
        if (capture->_has_output) {
            [stream removeStreamOutput:capture type:SCStreamOutputTypeScreen error:NULL];
            capture->_has_output = NO;
        }
    }
    dispatch_sync(capture->_queue, ^{
        capture->_ctx = NULL;
        capture->_on_frame = NULL;
        capture->_on_error = NULL;
    });
    // Break ownership even if SCK retains the output during a delayed stop.
    // Late callbacks see _stopped before inspecting the reclaimed Rust context.
    capture->_stream = nil;
}

void *rsb_capture_start(uint64_t window_id, uint32_t fps, void *ctx,
                        rsb_frame_cb on_frame, rsb_error_cb on_error, char *err, int32_t err_len) {
    @autoreleasepool {
        set_error(err, err_len, @"");
        if (fps == 0 || fps > INT32_MAX || !ctx || !on_frame || !on_error) {
            set_error(err, err_len, @"invalid capture arguments");
            return NULL;
        }
        if (@available(macOS 12.3, *)) {
            NSString *message = nil;
            SCShareableContent *content = shareable_content(&message);
            if (!content) {
                set_error(err, err_len, message);
                return NULL;
            }
            SCWindow *window = nil;
            for (SCWindow *candidate in content.windows) {
                if ((uint64_t)candidate.windowID == window_id) {
                    window = candidate;
                    break;
                }
            }
            if (!window) {
                set_error(err, err_len, [NSString stringWithFormat:@"window not found: %llu", (unsigned long long)window_id]);
                return NULL;
            }
            uint32_t width = 0, height = 0;
            window_pixels(window.frame, &width, &height);
            if (width == 0 || height == 0) {
                set_error(err, err_len, @"window has no pixels");
                return NULL;
            }

            RSBCapture *capture = [RSBCapture new];
            dispatch_queue_attr_t attr =
                dispatch_queue_attr_make_with_qos_class(DISPATCH_QUEUE_SERIAL, QOS_CLASS_USER_INTERACTIVE, 0);
            capture->_queue = dispatch_queue_create("ai.gxlself.rs-bench.capture", attr);
            capture->_ctx = ctx;
            capture->_on_frame = on_frame;
            capture->_on_error = on_error;

            SCContentFilter *filter = [[SCContentFilter alloc] initWithDesktopIndependentWindow:window];
            SCStreamConfiguration *config = [SCStreamConfiguration new];
            config.width = width;
            config.height = height;
            config.minimumFrameInterval = CMTimeMake(1, (int32_t)fps);
            config.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarFullRange;
            config.showsCursor = NO;
            config.queueDepth = 5;
            if (@available(macOS 13.0, *)) config.capturesAudio = NO;
            if (@available(macOS 15.0, *)) config.captureMicrophone = NO;

            SCStream *stream = [[SCStream alloc] initWithFilter:filter configuration:config delegate:capture];
            capture->_stream = stream;
            NSError *add_error = nil;
            if (![stream addStreamOutput:capture type:SCStreamOutputTypeScreen sampleHandlerQueue:capture->_queue
                                  error:&add_error]) {
                set_error(err, err_len, capture_error(@"cannot add stream output", add_error));
                stop_capture(capture);
                return NULL;
            }
            capture->_has_output = YES;
            __block NSError *start_error = nil;
            dispatch_semaphore_t sem = dispatch_semaphore_create(0);
            __weak SCStream *pending_stream = stream;
            [stream startCaptureWithCompletionHandler:^(NSError *error) {
                start_error = error;
                dispatch_semaphore_signal(sem);
                // A timed-out start can finish after stop_capture returned.
                // Keep the stopped flag alive, without retaining the stream
                // through its own completion block. A live late start is stopped.
                if (atomic_load_explicit(&capture->_stopped, memory_order_acquire)) {
                    [pending_stream stopCaptureWithCompletionHandler:nil];
                }
            }];
            if (dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) != 0) {
                set_error(err, err_len, @"timeout");
                stop_capture(capture);
                return NULL;
            }
            if (start_error) {
                set_error(err, err_len, capture_error(@"cannot start capture", start_error));
                stop_capture(capture);
                return NULL;
            }
            return (__bridge_retained void *)capture;
        }
        set_error(err, err_len, @"ScreenCaptureKit requires macOS 12.3 or later");
        return NULL;
    }
}

void rsb_capture_stop(void *handle) {
    if (!handle) return;
    @autoreleasepool {
        if (@available(macOS 12.3, *)) {
            // Balance the retained bridge from a successful start, after drain.
            RSBCapture *capture = (__bridge_transfer RSBCapture *)handle;
            stop_capture(capture);
        }
    }
}
