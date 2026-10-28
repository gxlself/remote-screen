#import "RemoteSessionViewComponentView.h"
#import <React_RCTAppDelegate/RCTDefaultReactNativeFactoryDelegate.h>
#import "RemoteScreen-Swift.h"

#import <react/renderer/components/RsVideoSpec/ComponentDescriptors.h>
#import <react/renderer/components/RsVideoSpec/EventEmitters.h>
#import <react/renderer/components/RsVideoSpec/Props.h>
#import <react/renderer/components/RsVideoSpec/RCTComponentViewHelpers.h>

using namespace facebook::react;

@interface RemoteSessionViewComponentView () <RCTRemoteSessionViewViewProtocol>
@end

@implementation RemoteSessionViewComponentView {
  RemoteSessionSurface *_surface;
  BOOL _needsConfiguration;
}

+ (ComponentDescriptorProvider)componentDescriptorProvider
{
  return concreteComponentDescriptorProvider<RemoteSessionViewComponentDescriptor>();
}

- (instancetype)initWithFrame:(CGRect)frame
{
  if (self = [super initWithFrame:frame]) {
    _props = std::make_shared<const RemoteSessionViewProps>();
    _needsConfiguration = YES;
    _surface = [[RemoteSessionSurface alloc] initWithFrame:self.bounds];
    self.contentView = _surface;
    self.isAccessibilityElement = YES;
    self.accessibilityTraits = UIAccessibilityTraitImage;
    __weak RemoteSessionViewComponentView *weakSelf = self;
    _surface.onGesture = ^(NSDictionary *payload) {
      RemoteSessionViewComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RemoteSessionViewEventEmitter>(view->_eventEmitter);
      RemoteSessionViewEventEmitter::OnGesture event = {};
      event.kind = [payload[@"kind"] UTF8String] ?: "";
      emitter->onGesture(event);
    };
    _surface.onPointerLock = ^(NSDictionary *payload) {
      RemoteSessionViewComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RemoteSessionViewEventEmitter>(view->_eventEmitter);
      RemoteSessionViewEventEmitter::OnPointerLock event = {};
      event.mouse = [payload[@"mouse"] boolValue];
      event.locked = [payload[@"locked"] boolValue];
      event.reason = [payload[@"reason"] UTF8String] ?: "";
      emitter->onPointerLock(event);
    };
    _surface.onViewport = ^(NSDictionary *payload) {
      RemoteSessionViewComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RemoteSessionViewEventEmitter>(view->_eventEmitter);
      RemoteSessionViewEventEmitter::OnViewport event = {};
      event.zoom = [payload[@"zoom"] floatValue];
      event.x = [payload[@"x"] floatValue];
      event.y = [payload[@"y"] floatValue];
      emitter->onViewport(event);
    };
    _surface.onFirstFrame = ^(NSDictionary *payload) {
      RemoteSessionViewComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RemoteSessionViewEventEmitter>(view->_eventEmitter);
      RemoteSessionViewEventEmitter::OnFirstFrame event = {};
      event.epoch = [payload[@"epoch"] intValue];
      event.width = [payload[@"width"] intValue];
      event.height = [payload[@"height"] intValue];
      emitter->onFirstFrame(event);
    };
    _surface.onVideoStats = ^(NSDictionary *payload) {
      RemoteSessionViewComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RemoteSessionViewEventEmitter>(view->_eventEmitter);
      RemoteSessionViewEventEmitter::OnVideoStats event = {};
      event.fps = [payload[@"fps"] intValue];
      event.decodeP50Ms = [payload[@"decodeP50Ms"] floatValue];
      event.decodeP95Ms = [payload[@"decodeP95Ms"] floatValue];
      event.latencyP50Ms = payload[@"latencyP50Ms"] ? [payload[@"latencyP50Ms"] floatValue] : -1;
      event.queue = [payload[@"queue"] intValue];
      event.dropped = [payload[@"dropped"] intValue];
      event.keyframeRequests = [payload[@"keyframeRequests"] intValue];
      event.width = [payload[@"width"] intValue];
      event.height = [payload[@"height"] intValue];
      event.codec = [payload[@"codec"] UTF8String] ?: "";
      emitter->onVideoStats(event);
    };
    _surface.onSpaceSwipe = ^(NSDictionary *payload) {
      RemoteSessionViewComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RemoteSessionViewEventEmitter>(view->_eventEmitter);
      RemoteSessionViewEventEmitter::OnSpaceSwipe event = {};
      event.phase = [payload[@"phase"] UTF8String] ?: "";
      emitter->onSpaceSwipe(event);
    };
    _surface.onSpaceSwitch = ^(NSDictionary *payload) {
      RemoteSessionViewComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RemoteSessionViewEventEmitter>(view->_eventEmitter);
      RemoteSessionViewEventEmitter::OnSpaceSwitch event = {};
      event.sessionId = [payload[@"sessionId"] UTF8String] ?: "";
      emitter->onSpaceSwitch(event);
    };
  }
  return self;
}

- (void)updateProps:(Props::Shared const &)props oldProps:(Props::Shared const &)oldProps
{
  const auto &next = *std::static_pointer_cast<const RemoteSessionViewProps>(props);
  const auto &previous = *std::static_pointer_cast<const RemoteSessionViewProps>(_props);
  if (_needsConfiguration || next.mode != previous.mode) {
    _surface.mode = [NSString stringWithUTF8String:toString(next.mode).c_str()];
  }
  if (_needsConfiguration || next.keyboardInset != previous.keyboardInset) {
    _surface.keyboardInset = next.keyboardInset;
  }
  if (_needsConfiguration || next.reduceMotion != previous.reduceMotion) {
    _surface.reduceMotion = next.reduceMotion;
  }
  if (_needsConfiguration || next.toolbarCollapsed != previous.toolbarCollapsed) {
    _surface.toolbarCollapsed = next.toolbarCollapsed;
  }
  if (_needsConfiguration || next.cursorSpeed != previous.cursorSpeed) {
    _surface.cursorSpeed = next.cursorSpeed;
  }
  if (_needsConfiguration || next.naturalScroll != previous.naturalScroll) {
    _surface.naturalScroll = next.naturalScroll;
  }
  if (_needsConfiguration || next.haptics != previous.haptics) {
    _surface.haptics = next.haptics;
  }
  if (_needsConfiguration || next.spaceIds != previous.spaceIds || next.spaceNames != previous.spaceNames) {
    NSMutableArray<NSString *> *ids = [NSMutableArray arrayWithCapacity:next.spaceIds.size()];
    for (const auto &value : next.spaceIds) {
      [ids addObject:[NSString stringWithUTF8String:value.c_str()]];
    }
    NSMutableArray<NSString *> *names = [NSMutableArray arrayWithCapacity:next.spaceNames.size()];
    for (const auto &value : next.spaceNames) {
      [names addObject:[NSString stringWithUTF8String:value.c_str()]];
    }
    [_surface setSpacesWithIds:ids names:names];
  }
  // Before the session id, so a switched-to session opens on its snapshot.
  if (_needsConfiguration || next.showSnapshot != previous.showSnapshot) {
    _surface.showSnapshot = next.showSnapshot;
  }
  if (_needsConfiguration || next.sessionId != previous.sessionId) {
    [_surface setSessionId:[NSString stringWithUTF8String:next.sessionId.c_str()]];
  }
  _needsConfiguration = NO;
  [super updateProps:props oldProps:oldProps];
  // RN owns the label; the native picture and future cursor layers form one
  // image element rather than additional VoiceOver targets.
  self.isAccessibilityElement = YES;
  self.accessibilityTraits = UIAccessibilityTraitImage;
}

- (void)handleCommand:(NSString const *)commandName args:(NSArray const *)args
{
  RCTRemoteSessionViewHandleCommand(self, commandName, args);
}

- (void)zoomReset { [_surface zoomReset]; }
- (void)centerCursor { [_surface centerCursor]; }
- (void)showMagnifier:(BOOL)on { [_surface showMagnifier:on]; }
- (void)animateSpace:(NSString *)sessionId direction:(NSString *)direction
{
  [_surface animateSpaceTo:sessionId direction:direction];
}

- (void)prepareForRecycle
{
  [_surface detach];
  [_surface reset];
  _needsConfiguration = YES;
  [super prepareForRecycle];
}

@end
