// Resolve private classes only at runtime, so removing this API does not stop
// dyld from loading the host. These protocols declare method ABIs, not classes.
#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>
#include <dispatch/dispatch.h>
#include <objc/runtime.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

@protocol RSVDDescriptor <NSObject>
- (void)setQueue:(dispatch_queue_t)queue;
- (void)setDispatchQueue:(dispatch_queue_t)queue;
- (void)setName:(NSString *)name;
- (void)setMaxPixelsWide:(unsigned int)width;
- (void)setMaxPixelsHigh:(unsigned int)height;
- (void)setSizeInMillimeters:(CGSize)size;
- (void)setVendorID:(unsigned int)vendor;
- (void)setProductID:(unsigned int)product;
- (void)setSerialNum:(unsigned int)serial;
- (void)setTerminationHandler:(void (^)(unsigned int, id))handler;
- (void)setRedPrimary:(CGPoint)point;
- (void)setGreenPrimary:(CGPoint)point;
- (void)setBluePrimary:(CGPoint)point;
- (void)setWhitePoint:(CGPoint)point;
@end

@protocol RSVDMode <NSObject>
- (instancetype)initWithWidth:(unsigned int)width height:(unsigned int)height refreshRate:(double)rate;
@end

@protocol RSVDSettings <NSObject>
- (void)setHiDPI:(unsigned int)hidpi;
- (void)setModes:(NSArray *)modes;
@end

@protocol RSVDDisplay <NSObject>
- (instancetype)initWithDescriptor:(id<RSVDDescriptor>)descriptor;
- (BOOL)applySettings:(id<RSVDSettings>)settings;
- (CGDirectDisplayID)displayID;
@end

// Variant values are intentionally kept in this private shim rather than in
// the public Rust contract. 0..3 are the baseline controls; 4..7 are the
// macOS-26/AppKit experiments from.
enum {
    RSVD_VARIANT_CURRENT = 0,
    RSVD_VARIANT_PIXELS = 1,
    RSVD_VARIANT_POINTS = 2,
    RSVD_VARIANT_PIXELS_AND_POINTS = 3,
    RSVD_VARIANT_APPKIT = 4,       // E
    RSVD_VARIANT_DESKPAD = 5,      // F
    RSVD_VARIANT_DELAYED = 6,      // G
    RSVD_VARIANT_RETAINED = 7,     // H
};

typedef struct {
    _Atomic(bool) cancelled;
} rs_vd_apply_context;

typedef struct {
    CGDirectDisplayID display_id;
    CFTypeRef display;
    CFTypeRef descriptor;
    CFTypeRef settings;
    CFTypeRef modes;
    CFTypeRef pending_token;
} rs_vd_handle;

// G retains only this cancellation token in the native handle. Its delayed
// block uses weak references to the display/settings so a queue that never
// runs cannot keep a virtual screen alive after destroy.
@interface RSVDApplyToken : NSObject {
@public
    rs_vd_apply_context context;
}
@end

@implementation RSVDApplyToken
@end

static BOOL rs_vd_variant_uses_appkit(uint32_t variant) {
    return variant >= RSVD_VARIANT_APPKIT;
}

static BOOL rs_vd_variant_uses_main_queue(uint32_t variant) {
    return rs_vd_variant_uses_appkit(variant);
}

static dispatch_queue_t rs_vd_queue(void) {
    static dispatch_once_t once;
    static dispatch_queue_t queue;
    dispatch_once(&once, ^{
        queue = dispatch_queue_create("ai.gxlself.remotehost.vdisplay", DISPATCH_QUEUE_SERIAL);
    });
    return queue;
}

// Keep system callbacks independent of synchronous creation/configuration.
// A CoreGraphics request must not occupy the queue needed to complete it.
static dispatch_queue_t rs_vd_callback_queue(void) {
    static dispatch_once_t once;
    static dispatch_queue_t queue;
    dispatch_once(&once, ^{
        queue = dispatch_queue_create("ai.gxlself.remotehost.vdisplay.callbacks", DISPATCH_QUEUE_SERIAL);
    });
    return queue;
}

// Probe both private spellings without sending an unsupported selector.
uint32_t rs_vd_queue_selectors(void) {
    Class descriptor = NSClassFromString(@"CGVirtualDisplayDescriptor");
    return ([descriptor instancesRespondToSelector:@selector(setQueue:)] ? 1u : 0u) |
           ([descriptor instancesRespondToSelector:@selector(setDispatchQueue:)] ? 2u : 0u);
}

// macOS private frameworks have changed these classes without changing the
// public SDK headers. Keep this dump in the probe so a new initializer or
// settings property is visible before choosing a production combination.
static void rs_vd_dump_class(Class cls) {
    if (cls == Nil) {
        fprintf(stderr, "VD_INTROSPECT class=<missing>\n");
        return;
    }
    fprintf(stderr, "VD_INTROSPECT class=%s\n", class_getName(cls));
    for (Class owner = cls; owner != Nil; owner = class_getSuperclass(owner)) {
        unsigned int method_count = 0;
        Method *methods = class_copyMethodList(owner, &method_count);
        for (unsigned int i = 0; i < method_count; i++) {
            SEL selector = method_getName(methods[i]);
            const char *name = sel_getName(selector);
            const char *types = method_getTypeEncoding(methods[i]);
            fprintf(stderr, "VD_INTROSPECT method owner=%s name=%s types=%s\n",
                    class_getName(owner), name, types != NULL ? types : "<null>");
        }
        free(methods);

        unsigned int property_count = 0;
        objc_property_t *properties = class_copyPropertyList(owner, &property_count);
        for (unsigned int i = 0; i < property_count; i++) {
            const char *name = property_getName(properties[i]);
            const char *attributes = property_getAttributes(properties[i]);
            fprintf(stderr, "VD_INTROSPECT property owner=%s name=%s attributes=%s\n",
                    class_getName(owner), name != NULL ? name : "<null>",
                    attributes != NULL ? attributes : "<null>");
        }
        free(properties);
    }
}

void rs_vd_diag_introspect(void) {
    @autoreleasepool {
        const char *names[] = {
            "CGVirtualDisplay",
            "CGVirtualDisplayDescriptor",
            "CGVirtualDisplaySettings",
            "CGVirtualDisplayMode",
        };
        fprintf(stderr, "VD_INTROSPECT begin\n");
        for (size_t i = 0; i < sizeof(names) / sizeof(names[0]); i++) {
            Class cls = NSClassFromString([NSString stringWithUTF8String:names[i]]);
            if (cls == Nil) {
                fprintf(stderr, "VD_INTROSPECT class=%s found=0\n", names[i]);
            } else {
                rs_vd_dump_class(cls);
            }
        }
        fprintf(stderr, "VD_INTROSPECT end\n");
    }
}

// AppKit initialization is deliberately explicit. NSApplication is main
// thread-only, while Host's normal create path runs on spawn_blocking workers.
// The probe calls this before the E-H experiments; a production caller that
// adopts the main-queue variant must make the same one-time call at startup.
int rs_vd_appkit_initialize(void) {
    if (![NSThread isMainThread]) {
        return (int)kCGErrorIllegalArgument;
    }
    @autoreleasepool {
        @try {
            NSApplication *application = [NSApplication sharedApplication];
            if (![application setActivationPolicy:NSApplicationActivationPolicyAccessory]) {
                return (int)kCGErrorFailure;
            }
            [application finishLaunching];
            return (int)kCGErrorSuccess;
        } @catch (NSException *exception) {
            (void)exception;
            return (int)kCGErrorFailure;
        }
    }
}

// Host entry points that adopt the E-H descriptor queue can call this from
// their main-thread scheduler between Tokio turns. It does not register the
// diagnostic callback; it only services AppKit/CoreGraphics sources.
int rs_vd_run_main_loop(uint32_t timeout_ms) {
    if (![NSThread isMainThread]) {
        return 0;
    }
    CFRunLoopRunInMode(kCFRunLoopDefaultMode, MIN(timeout_ms, 50u) / 1000.0, false);
    return 1;
}

// Called only on rs_vd_queue. This probes methods without allocating a display.
static BOOL rs_vd_classes_supported(void) {
    Class descriptor = NSClassFromString(@"CGVirtualDisplayDescriptor");
    Class display = NSClassFromString(@"CGVirtualDisplay");
    Class settings = NSClassFromString(@"CGVirtualDisplaySettings");
    Class mode = NSClassFromString(@"CGVirtualDisplayMode");
    if (descriptor == Nil || display == Nil || settings == Nil || mode == Nil ||
        rs_vd_queue_selectors() == 0) {
        return NO;
    }
    SEL descriptor_selectors[] = {
        @selector(setName:), @selector(setMaxPixelsWide:),
        @selector(setMaxPixelsHigh:), @selector(setSizeInMillimeters:),
        @selector(setVendorID:), @selector(setProductID:), @selector(setSerialNum:),
        @selector(setTerminationHandler:),
    };
    for (size_t i = 0; i < sizeof(descriptor_selectors) / sizeof(descriptor_selectors[0]); i++) {
        if (![descriptor instancesRespondToSelector:descriptor_selectors[i]]) {
            return NO;
        }
    }
    SEL display_selectors[] = {
        @selector(initWithDescriptor:), @selector(applySettings:), @selector(displayID),
    };
    for (size_t i = 0; i < sizeof(display_selectors) / sizeof(display_selectors[0]); i++) {
        if (![display instancesRespondToSelector:display_selectors[i]]) {
            return NO;
        }
    }
    return [settings instancesRespondToSelector:@selector(setHiDPI:)] &&
           [settings instancesRespondToSelector:@selector(setModes:)] &&
           [mode instancesRespondToSelector:@selector(initWithWidth:height:refreshRate:)];
}

static void rs_vd_set_error(char *buffer, size_t length, NSString *message) {
    if (buffer != NULL && length > 0) {
        strlcpy(buffer, message.UTF8String ?: "virtual display operation failed", length);
    }
}

static BOOL rs_vd_append_mode(NSMutableArray *modes, Class mode_class,
                               uint32_t width, uint32_t height, double refresh_hz,
                               char *err, size_t errlen) {
    id<RSVDMode> mode = [(id<RSVDMode>)[mode_class alloc]
        initWithWidth:width height:height refreshRate:refresh_hz];
    if (mode == nil) {
        rs_vd_set_error(err, errlen, @"could not create virtual display mode");
        return NO;
    }
    [modes addObject:mode];
    return YES;
}

static BOOL rs_vd_apply_settings(id display, id settings, BOOL main_queue) {
    __block BOOL applied = NO;
    void (^apply)(void) = ^{
        applied = [display applySettings:settings];
    };
    if (main_queue && ![NSThread isMainThread]) {
        dispatch_sync(dispatch_get_main_queue(), apply);
    } else {
        apply();
    }
    return applied;
}

static rs_vd_handle *rs_vd_retain_handle(id display, id descriptor, id settings,
                                         NSArray *modes, BOOL retain_configuration) {
    rs_vd_handle *handle = calloc(1, sizeof(*handle));
    if (handle == NULL) {
        return NULL;
    }
    handle->display_id = [(id<RSVDDisplay>)display displayID];
    handle->display = CFBridgingRetain(display);
    if (retain_configuration) {
        handle->descriptor = CFBridgingRetain(descriptor);
        handle->settings = CFBridgingRetain(settings);
        handle->modes = CFBridgingRetain(modes);
    }
    return handle;
}

static void rs_vd_release_handle(rs_vd_handle *handle) {
    if (handle == NULL) {
        return;
    }
    if (handle->pending_token != NULL) {
        RSVDApplyToken *token = (__bridge RSVDApplyToken *)handle->pending_token;
        atomic_store(&token->context.cancelled, true);
        id pending_token = CFBridgingRelease(handle->pending_token);
        (void)pending_token;
        handle->pending_token = NULL;
    }
    if (handle->modes != NULL) {
        id modes = CFBridgingRelease(handle->modes);
        (void)modes;
        handle->modes = NULL;
    }
    if (handle->settings != NULL) {
        id settings = CFBridgingRelease(handle->settings);
        (void)settings;
        handle->settings = NULL;
    }
    if (handle->descriptor != NULL) {
        id descriptor = CFBridgingRelease(handle->descriptor);
        (void)descriptor;
        handle->descriptor = NULL;
    }
    if (handle->display != NULL) {
        id display = CFBridgingRelease(handle->display);
        (void)display;
        handle->display = NULL;
    }
    free(handle);
}

int rs_vd_supported(void) {
    static dispatch_once_t once;
    static BOOL supported;
    dispatch_once(&once, ^{
        dispatch_sync(rs_vd_queue(), ^{
            @autoreleasepool {
                supported = rs_vd_classes_supported();
            }
        });
    });
    return supported ? 1 : 0;
}

void *rs_vd_create(const char *name, uint32_t width_px, uint32_t height_px, int hidpi,
                   uint32_t refresh_hz, uint32_t width_mm, uint32_t height_mm, uint32_t serial,
                   uint32_t mode_variant, uint32_t *out_display_id, char *err, size_t errlen) {
    if (out_display_id != NULL) {
        *out_display_id = 0;
    }
    if (err != NULL && errlen > 0) {
        err[0] = '\0';
    }
    if (name == NULL || out_display_id == NULL || width_px == 0 || height_px == 0 ||
        (hidpi && (width_px % 2 != 0 || height_px % 2 != 0)) ||
        (refresh_hz != 30 && refresh_hz != 60) || mode_variant > RSVD_VARIANT_RETAINED) {
        rs_vd_set_error(err, errlen, @"invalid virtual display arguments");
        return NULL;
    }

    __block void *handle = NULL;
    dispatch_sync(rs_vd_queue(), ^{
        @autoreleasepool {
            @try {
                if (!rs_vd_classes_supported()) {
                    rs_vd_set_error(err, errlen, @"CGVirtualDisplay is unavailable");
                    return;
                }
                NSString *display_name = [NSString stringWithUTF8String:name];
                if (display_name == nil) {
                    rs_vd_set_error(err, errlen, @"virtual display name is not valid UTF-8");
                    return;
                }
                Class descriptor_class = NSClassFromString(@"CGVirtualDisplayDescriptor");
                Class display_class = NSClassFromString(@"CGVirtualDisplay");
                Class settings_class = NSClassFromString(@"CGVirtualDisplaySettings");
                Class mode_class = NSClassFromString(@"CGVirtualDisplayMode");
                id<RSVDDescriptor> descriptor = [descriptor_class new];
                if (descriptor == nil) {
                    rs_vd_set_error(err, errlen, @"could not create virtual display descriptor");
                    return;
                }
                BOOL appkit_variant = rs_vd_variant_uses_appkit(mode_variant);
                if (appkit_variant) {
                    if (![descriptor respondsToSelector:@selector(setDispatchQueue:)]) {
                        rs_vd_set_error(err, errlen, @"CGVirtualDisplayDescriptor setDispatchQueue: unavailable");
                        return;
                    }
                    // E-H intentionally exercise the selector that DeskPad
                    // uses. The caller has initialized NSApplication and
                    // pumps the main run loop while this worker is pending.
                    [descriptor setDispatchQueue:dispatch_get_main_queue()];
                } else if ([descriptor respondsToSelector:@selector(setQueue:)]) {
                    // Preserve V1F2's queue for the baseline A-D controls.
                    [descriptor setQueue:rs_vd_callback_queue()];
                } else {
                    [descriptor setDispatchQueue:rs_vd_callback_queue()];
                }
                [descriptor setName:display_name];
                [descriptor setMaxPixelsWide:width_px];
                [descriptor setMaxPixelsHigh:height_px];
                [descriptor setSizeInMillimeters:CGSizeMake(width_mm, height_mm)];
                [descriptor setVendorID:0x5253];
                [descriptor setProductID:0x5644];
                [descriptor setSerialNum:serial];
                [descriptor setTerminationHandler:^(unsigned int reason, id terminated) {
                    (void)reason;
                    (void)terminated;
                    NSLog(@"[rs-host] virtual display terminated");
                }];
                if ([descriptor_class instancesRespondToSelector:@selector(setRedPrimary:)]) {
                    [descriptor setRedPrimary:CGPointMake(0.64, 0.33)];
                }
                if ([descriptor_class instancesRespondToSelector:@selector(setGreenPrimary:)]) {
                    [descriptor setGreenPrimary:CGPointMake(0.30, 0.60)];
                }
                if ([descriptor_class instancesRespondToSelector:@selector(setBluePrimary:)]) {
                    [descriptor setBluePrimary:CGPointMake(0.15, 0.06)];
                }
                if ([descriptor_class instancesRespondToSelector:@selector(setWhitePoint:)]) {
                    [descriptor setWhitePoint:CGPointMake(0.3127, 0.3290)];
                }

                id<RSVDDisplay> display = [(id<RSVDDisplay>)[display_class alloc]
                    initWithDescriptor:descriptor];
                if (display == nil) {
                    rs_vd_set_error(err, errlen, @"CGVirtualDisplay initWithDescriptor failed");
                    return;
                }
                id<RSVDSettings> settings = [settings_class new];
                if (settings == nil) {
                    rs_vd_set_error(err, errlen, @"could not create virtual display settings");
                    return;
                }
                BOOL deskpad_variant = mode_variant == RSVD_VARIANT_DESKPAD;
                BOOL retain_configuration = mode_variant == RSVD_VARIANT_RETAINED ||
                    mode_variant == RSVD_VARIANT_DELAYED;
                [settings setHiDPI:(hidpi || deskpad_variant) ? 1 : 0];
                NSMutableArray *modes = [NSMutableArray arrayWithCapacity:4];
                if (deskpad_variant) {
                    // DeskPad-style list: advertise backing pixels at several
                    // sizes, largest first. Keep every item within the
                    // descriptor's max so arbitrary probe dimensions remain
                    // valid.
                    uint32_t candidates[][2] = {
                        {2732, 2048}, {2048, 1536}, {1366, 1024},
                        {1280, 800}, {width_px, height_px},
                    };
                    for (size_t i = 0; i < sizeof(candidates) / sizeof(candidates[0]); i++) {
                        uint32_t candidate_width = candidates[i][0];
                        uint32_t candidate_height = candidates[i][1];
                        if (candidate_width > width_px || candidate_height > height_px) {
                            continue;
                        }
                        BOOL duplicate = i == 4 &&
                            ((width_px == 2732 && height_px == 2048) ||
                             (width_px == 2048 && height_px == 1536) ||
                             (width_px == 1366 && height_px == 1024) ||
                             (width_px == 1280 && height_px == 800));
                        if (!duplicate && !rs_vd_append_mode(modes, mode_class,
                                                              candidate_width, candidate_height,
                                                              refresh_hz, err, errlen)) {
                            return;
                        }
                    }
                    if ([modes count] == 0 && !rs_vd_append_mode(
                            modes, mode_class, width_px, height_px, refresh_hz, err, errlen)) {
                        return;
                    }
                } else {
                    // V1F2 A-D controls: current (native + point), backing
                    // pixels only, logical points only, or both.
                    if (!hidpi || mode_variant != RSVD_VARIANT_POINTS) {
                        if (!rs_vd_append_mode(modes, mode_class, width_px, height_px,
                                               refresh_hz, err, errlen)) {
                            return;
                        }
                    }
                    if (hidpi && mode_variant != RSVD_VARIANT_PIXELS) {
                        if (!rs_vd_append_mode(modes, mode_class,
                                               hidpi ? width_px / 2 : width_px,
                                               hidpi ? height_px / 2 : height_px,
                                               refresh_hz, err, errlen)) {
                            return;
                        }
                    }
                }
                [settings setModes:modes];
                if (!rs_vd_apply_settings(display, settings,
                                          rs_vd_variant_uses_main_queue(mode_variant))) {
                    rs_vd_set_error(err, errlen, @"CGVirtualDisplay applySettings failed");
                    return;
                }
                CGDirectDisplayID display_id = [display displayID];
                if (display_id == 0) {
                    rs_vd_set_error(err, errlen, @"CGVirtualDisplay returned display ID 0");
                    return;
                }
                *out_display_id = display_id;
                rs_vd_handle *owned = rs_vd_retain_handle(
                    display, descriptor, settings, modes, retain_configuration);
                if (owned == NULL) {
                    rs_vd_set_error(err, errlen, @"could not retain virtual display handle");
                    return;
                }
                if (mode_variant == RSVD_VARIANT_DELAYED) {
                    RSVDApplyToken *token = [RSVDApplyToken new];
                    atomic_init(&token->context.cancelled, false);
                    owned->pending_token = CFBridgingRetain(token);
                    __weak RSVDApplyToken *weak_token = token;
                    __weak id weak_display = display;
                    __weak id weak_settings = settings;
                    dispatch_after(dispatch_time(DISPATCH_TIME_NOW,
                                                  (int64_t)(0.5 * NSEC_PER_SEC)),
                                    dispatch_get_main_queue(), ^{
                        @autoreleasepool {
                            RSVDApplyToken *live_token = weak_token;
                            id live_display = weak_display;
                            id live_settings = weak_settings;
                            if (live_token != nil && live_display != nil &&
                                live_settings != nil &&
                                !atomic_load(&live_token->context.cancelled)) {
                                [live_display applySettings:live_settings];
                            }
                        }
                    });
                }
                handle = (void *)owned;
            } @catch (NSException *exception) {
                rs_vd_set_error(err, errlen, [NSString stringWithFormat:
                    @"CGVirtualDisplay exception: %@", exception.name]);
            }
        }
    });
    return handle;
}

// POD snapshots cross the Rust FFI; never expose a borrowed CGDisplayModeRef.
typedef struct {
    uint32_t mode_id;
    uint32_t width_px, height_px;
    uint32_t width_pt, height_pt;
    bool usable;
    double refresh_hz;
} rs_vd_mode_record;

static rs_vd_mode_record rs_vd_mode_record_from(CGDisplayModeRef mode) {
    return (rs_vd_mode_record) {
        .mode_id = (uint32_t)CGDisplayModeGetIODisplayModeID(mode),
        .width_px = (uint32_t)CGDisplayModeGetPixelWidth(mode),
        .height_px = (uint32_t)CGDisplayModeGetPixelHeight(mode),
        .width_pt = (uint32_t)CGDisplayModeGetWidth(mode),
        .height_pt = (uint32_t)CGDisplayModeGetHeight(mode),
        .usable = CGDisplayModeIsUsableForDesktopGUI(mode),
        // Preserve 0 (unknown) instead of inventing a 60 Hz measurement.
        .refresh_hz = CGDisplayModeGetRefreshRate(mode),
    };
}

// Public CoreGraphics queries run on the caller, just like rs_list_displays.
// Do not dispatch them to the private API queue or the descriptor callback
// queue. Include duplicates so a 1x mode cannot hide a 2x mode with the same
// logical dimensions, or vice versa.
static CFArrayRef rs_vd_copy_modes(uint32_t display_id) {
    NSDictionary *options = @{(__bridge NSString *)kCGDisplayShowDuplicateLowResolutionModes: @YES};
    return CGDisplayCopyAllDisplayModes(display_id, (__bridge CFDictionaryRef)options);
}

// Synchronous snapshots; out lives for this call only. Return the total count
// even when capacity is smaller, allowing the caller to retry a growing list.
size_t rs_vd_modes(uint32_t display_id, rs_vd_mode_record *out, size_t capacity) {
    @autoreleasepool {
        CFArrayRef modes = rs_vd_copy_modes(display_id);
        if (modes == NULL) {
            return 0;
        }
        size_t count = (size_t)CFArrayGetCount(modes);
        for (size_t i = 0; out != NULL && i < MIN(capacity, count); i++) {
            CGDisplayModeRef mode = (CGDisplayModeRef)CFArrayGetValueAtIndex(modes, (CFIndex)i);
            out[i] = rs_vd_mode_record_from(mode);
        }
        CFRelease(modes);
        return count;
    }
}

int rs_vd_current_mode(uint32_t display_id, rs_vd_mode_record *out) {
    if (out == NULL) {
        return 0;
    }
    CGDisplayModeRef mode = CGDisplayCopyDisplayMode(display_id);
    if (mode == NULL) {
        return 0;
    }
    *out = rs_vd_mode_record_from(mode);
    CGDisplayModeRelease(mode);
    return 1;
}

int rs_vd_set_mode(uint32_t display_id, uint32_t mode_id) {
    @autoreleasepool {
        CFArrayRef modes = rs_vd_copy_modes(display_id);
        if (modes == NULL) {
            return (int)kCGErrorIllegalArgument;
        }
        CGError result = kCGErrorIllegalArgument;
        for (CFIndex i = 0; i < CFArrayGetCount(modes); i++) {
            CGDisplayModeRef mode = (CGDisplayModeRef)CFArrayGetValueAtIndex(modes, i);
            if ((uint32_t)CGDisplayModeGetIODisplayModeID(mode) == mode_id) {
                result = CGDisplaySetDisplayMode(display_id, mode, NULL);
                break;
            }
        }
        CFRelease(modes);
        return (int)result;
    }
}

typedef struct {
    uint32_t id;
    uint32_t vendor;
    uint32_t model;
    uint32_t serial;
} rs_vd_display_identity;

size_t rs_vd_online_identities(rs_vd_display_identity *out, size_t capacity) {
    CGDirectDisplayID displays[32];
    uint32_t count = 0;
    if (CGGetOnlineDisplayList(32, displays, &count) != kCGErrorSuccess) {
        return 0;
    }
    size_t total = MIN((size_t)count, 32u);
    for (size_t i = 0; out != NULL && i < MIN(capacity, total); i++) {
        CGDirectDisplayID display_id = displays[i];
        out[i] = (rs_vd_display_identity) {
            .id = display_id,
            .vendor = CGDisplayVendorNumber(display_id),
            .model = CGDisplayModelNumber(display_id),
            .serial = CGDisplaySerialNumber(display_id),
        };
    }
    return total;
}

static int rs_vd_online_state(CGDirectDisplayID display_id) {
    CGDirectDisplayID displays[32];
    uint32_t count = 0;
    if (CGGetOnlineDisplayList(32, displays, &count) != kCGErrorSuccess) {
        return -1;
    }
    for (uint32_t i = 0; i < count && i < 32; i++) {
        if (displays[i] == display_id) {
            return 1;
        }
    }
    return 0;
}

static BOOL rs_vd_wait_offline(CGDirectDisplayID display_id, uint32_t timeout_ms) {
    uint64_t started = clock_gettime_nsec_np(CLOCK_MONOTONIC_RAW);
    uint64_t timeout_ns = (uint64_t)timeout_ms * 1000000;
    for (;;) {
        if (rs_vd_online_state(display_id) == 0) {
            return YES;
        }
        uint64_t elapsed_ns = clock_gettime_nsec_np(CLOCK_MONOTONIC_RAW) - started;
        if (elapsed_ns >= timeout_ns) {
            return NO;
        }
        uint64_t delay_us = (timeout_ns - elapsed_ns) / 1000;
        usleep((useconds_t)MIN(delay_us, UINT64_C(50000)));
    }
}

int rs_vd_destroy(void *handle) {
    if (handle == NULL) {
        return (int)kCGErrorSuccess;
    }
    __block CGDirectDisplayID display_id = 0;
    dispatch_sync(rs_vd_queue(), ^{
        @autoreleasepool {
            rs_vd_handle *owned = (rs_vd_handle *)handle;
            display_id = owned->display_id;
            rs_vd_release_handle(owned);
        }
    });
    return rs_vd_wait_offline(display_id, 3000) ?
        (int)kCGErrorSuccess : (int)kCGErrorFailure;
}

int rs_vd_set_origin(uint32_t display_id, int32_t x, int32_t y) {
    __block CGError result = kCGErrorFailure;
    dispatch_sync(rs_vd_queue(), ^{
        CGDisplayConfigRef config = NULL;
        result = CGBeginDisplayConfiguration(&config);
        if (result != kCGErrorSuccess) {
            if (config != NULL) {
                CGCancelDisplayConfiguration(config);
            }
            return;
        }
        if (config == NULL) {
            result = kCGErrorFailure;
            return;
        }
        result = CGConfigureDisplayMirrorOfDisplay(config, display_id, kCGNullDirectDisplay);
        if (result == kCGErrorSuccess) {
            result = CGConfigureDisplayOrigin(config, display_id, x, y);
        }
        if (result == kCGErrorSuccess) {
            result = CGCompleteDisplayConfiguration(config, kCGConfigureForSession);
        }
        if (result != kCGErrorSuccess) {
            CGCancelDisplayConfiguration(config);
        }
    });
    return (int)result;
}

int rs_vd_wait_online(uint32_t display_id, uint32_t timeout_ms) {
    uint64_t started = clock_gettime_nsec_np(CLOCK_MONOTONIC_RAW);
    uint64_t timeout_ns = (uint64_t)timeout_ms * 1000000;
    for (;;) {
        // Creation has returned and callbacks have a separate queue. This
        // short snapshot never holds either queue while polling or sleeping.
        if (rs_vd_online_state(display_id) == 1) {
            return 1;
        }
        uint64_t elapsed_ns = clock_gettime_nsec_np(CLOCK_MONOTONIC_RAW) - started;
        if (elapsed_ns >= timeout_ns) {
            return 0;
        }
        uint64_t delay_us = (timeout_ns - elapsed_ns) / 1000;
        usleep((useconds_t)MIN(delay_us, UINT64_C(50000)));
    }
}

// Diagnostic-only main-loop observer. Static callback storage stays valid even
// if a notification was already in flight when the observer was removed.
typedef struct {
    uint64_t callbacks, main_callbacks;
    uint32_t last_display_id, flags;
} rs_vd_diag_events;

static _Atomic(uint64_t) rs_vd_diag_callbacks;
static _Atomic(uint64_t) rs_vd_diag_main_callbacks;
static _Atomic(uint32_t) rs_vd_diag_last_display;
static _Atomic(uint32_t) rs_vd_diag_flags;
// Accessed only on the process main thread.
static CFRunLoopSourceRef rs_vd_diag_source;

static void rs_vd_diag_reconfigured(CGDirectDisplayID display_id,
                                   CGDisplayChangeSummaryFlags flags, void *context) {
    (void)context;
    atomic_fetch_add(&rs_vd_diag_callbacks, 1);
    if ([NSThread isMainThread]) {
        atomic_fetch_add(&rs_vd_diag_main_callbacks, 1);
    }
    atomic_store(&rs_vd_diag_last_display, display_id);
    atomic_fetch_or(&rs_vd_diag_flags, (uint32_t)flags);
}

static void rs_vd_diag_keepalive(void *context) {
    (void)context;
}

void *rs_vd_diag_begin(int *out_error) {
    if (out_error == NULL) {
        return NULL;
    }
    *out_error = (int)kCGErrorIllegalArgument;
    if (![NSThread isMainThread] || rs_vd_diag_source != NULL) {
        return NULL;
    }
    // A source keeps RunInMode from returning kCFRunLoopRunFinished when CG
    // hasn't installed a source yet. It has no work and is never signalled.
    CFRunLoopSourceContext context = {0};
    context.perform = rs_vd_diag_keepalive;
    CFRunLoopSourceRef source = CFRunLoopSourceCreate(kCFAllocatorDefault, 0, &context);
    if (source == NULL) {
        *out_error = (int)kCGErrorFailure;
        return NULL;
    }
    *out_error = (int)CGDisplayRegisterReconfigurationCallback(rs_vd_diag_reconfigured, NULL);
    if (*out_error != (int)kCGErrorSuccess) {
        CFRelease(source);
        return NULL;
    }
    CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopDefaultMode);
    rs_vd_diag_source = source;
    return (void *)source;
}

int rs_vd_diag_end(void *handle) {
    if (![NSThread isMainThread] || handle == NULL || handle != (void *)rs_vd_diag_source) {
        return (int)kCGErrorIllegalArgument;
    }
    CGError result = CGDisplayRemoveReconfigurationCallback(rs_vd_diag_reconfigured, NULL);
    CFRunLoopRemoveSource(CFRunLoopGetMain(), rs_vd_diag_source, kCFRunLoopDefaultMode);
    CFRelease(rs_vd_diag_source);
    rs_vd_diag_source = NULL;
    return (int)result;
}

int rs_vd_diag_pump(uint32_t timeout_ms) {
    if (![NSThread isMainThread] || rs_vd_diag_source == NULL) {
        return 0;
    }
    // Only --diag's original process main thread pumps this loop. Production
    // create/configure workers and descriptor queues do not run a CFRunLoop.
    // rs-host-agent main currently block_on's Tokio; a confirmed main-loop
    // dependency requires a separate Host entry-point change outside this task.
    CFRunLoopRunInMode(kCFRunLoopDefaultMode, MIN(timeout_ms, 50u) / 1000.0, false);
    return 1;
}

void rs_vd_diag_snapshot(rs_vd_diag_events *out) {
    if (out != NULL) {
        *out = (rs_vd_diag_events) {
            .callbacks = atomic_load(&rs_vd_diag_callbacks),
            .main_callbacks = atomic_load(&rs_vd_diag_main_callbacks),
            .last_display_id = atomic_load(&rs_vd_diag_last_display),
            // Each before/after pair consumes only that case's flag union;
            // callback counters remain monotonic for delta calculation.
            .flags = atomic_exchange(&rs_vd_diag_flags, 0),
        };
    }
}
