// Capture one frame through the existing ScreenCaptureKit shim, then encode a
// small JPEG with AppKit. No pixels or encoded images are written to disk.

#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>
#import <CoreVideo/CoreVideo.h>
#import <VideoToolbox/VideoToolbox.h>

#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

typedef void (*rs_thumbnail_frame_cb)(void *, void *, uint32_t, uint32_t, uint64_t);
extern int32_t rs_frame_capture_start(uint32_t display, uint32_t fps, uint32_t width,
                                     uint32_t height, void *ctx, rs_thumbnail_frame_cb cb,
                                     void **handle, char *err, size_t errlen);
extern void rs_frame_capture_stop(void *handle);

@interface RSThumbnailFrame : NSObject {
@public
    CVPixelBufferRef pixel;
    dispatch_semaphore_t ready;
}
@end

@implementation RSThumbnailFrame
- (instancetype)init {
    self = [super init];
    if (self) ready = dispatch_semaphore_create(0);
    return self;
}
- (void)dealloc {
    if (pixel) CVPixelBufferRelease(pixel);
}
@end

static void on_thumbnail_frame(void *ctx, void *pixel, uint32_t width, uint32_t height,
                               uint64_t capture_us) {
    RSThumbnailFrame *frame = (__bridge RSThumbnailFrame *)ctx;
    // The existing capture shim serializes callbacks. Its stop call drains
    // that queue before the caller reads this retained frame or frees ctx.
    if (!pixel || frame->pixel) return;
    frame->pixel = CVPixelBufferRetain((CVPixelBufferRef)pixel);
    dispatch_semaphore_signal(frame->ready);
}

void rs_thumbnail_free(void *data) {
    free(data);
}

int32_t rs_thumbnail_jpeg(uint32_t max_width, size_t max_bytes,
                          uint8_t **out_data, size_t *out_len) {
    if (out_data) *out_data = NULL;
    if (out_len) *out_len = 0;
    if (!out_data || !out_len || max_width == 0 || max_bytes == 0) return 0;

    @autoreleasepool {
        // Preflight only: never show a permission dialog from background work.
        if (!CGPreflightScreenCaptureAccess()) return 0;
        CGDirectDisplayID display = CGMainDisplayID();
        if (!CGDisplayIsActive(display)) return 0;
        size_t capture_width = CGDisplayPixelsWide(display);
        size_t capture_height = CGDisplayPixelsHigh(display);
        if (capture_width < 2 || capture_height < 2 ||
            capture_width > UINT32_MAX || capture_height > UINT32_MAX) return 0;

        RSThumbnailFrame *frame = [RSThumbnailFrame new];
        void *handle = NULL;
        char error[512] = {0};
        // Same ScreenCaptureKit stream as Platform::capture, for one frame.
        // The temporary stream is independent of every active session stream.
        int32_t status = rs_frame_capture_start(display, 1, (uint32_t)capture_width & ~1u,
                                                (uint32_t)capture_height & ~1u,
                                                (__bridge void *)frame, on_thumbnail_frame,
                                                &handle, error, sizeof(error));
        if (status != 0 || !handle) return 0;
        long wait = dispatch_semaphore_wait(frame->ready,
                                            dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC));
        rs_frame_capture_stop(handle);
        if (wait != 0 || !frame->pixel) return 0;

        CGImageRef source = NULL;
        if (VTCreateCGImageFromCVPixelBuffer(frame->pixel, NULL, &source) != noErr || !source) {
            return 0;
        }
        size_t source_width = CGImageGetWidth(source);
        size_t source_height = CGImageGetHeight(source);
        if (source_width == 0 || source_height == 0) {
            CGImageRelease(source);
            return 0;
        }
        size_t width = MIN(source_width, (size_t)max_width);
        size_t height = MAX(1, (size_t)llround((double)source_height * (double)width /
                                             (double)source_width));
        CGColorSpaceRef color_space = CGColorSpaceCreateDeviceRGB();
        if (!color_space) {
            CGImageRelease(source);
            return 0;
        }
        CGContextRef context = CGBitmapContextCreate(NULL, width, height, 8, width * 4,
                                                      color_space,
                                                      kCGImageAlphaNoneSkipLast |
                                                          kCGBitmapByteOrder32Big);
        CGColorSpaceRelease(color_space);
        if (!context) {
            CGImageRelease(source);
            return 0;
        }
        CGContextSetInterpolationQuality(context, kCGInterpolationHigh);
        CGContextDrawImage(context, CGRectMake(0, 0, width, height), source);
        CGImageRelease(source);
        CGImageRef scaled = CGBitmapContextCreateImage(context);
        CGContextRelease(context);
        if (!scaled) return 0;
        NSBitmapImageRep *bitmap = [[NSBitmapImageRep alloc] initWithCGImage:scaled];
        CGImageRelease(scaled);
        if (!bitmap) return 0;

        const double qualities[] = {0.6, 0.5, 0.4, 0.3};
        for (size_t i = 0; i < sizeof(qualities) / sizeof(qualities[0]); i++) {
            NSData *encoded = [bitmap representationUsingType:NSBitmapImageFileTypeJPEG
                properties:@{NSImageCompressionFactor: @(qualities[i])}];
            if (!encoded || encoded.length == 0 || encoded.length > max_bytes) continue;
            uint8_t *copy = malloc(encoded.length);
            if (!copy) return 0;
            memcpy(copy, encoded.bytes, encoded.length);
            *out_data = copy;
            *out_len = encoded.length;
            return 1;
        }
        return 0;
    }
}
