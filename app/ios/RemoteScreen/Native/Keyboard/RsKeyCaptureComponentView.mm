#import "RsKeyCaptureComponentView.h"
#import <React_RCTAppDelegate/RCTDefaultReactNativeFactoryDelegate.h>
#import "RemoteScreen-Swift.h"

#import <react/renderer/components/RsVideoSpec/ComponentDescriptors.h>
#import <react/renderer/components/RsVideoSpec/EventEmitters.h>
#import <react/renderer/components/RsVideoSpec/Props.h>
#import <react/renderer/components/RsVideoSpec/RCTComponentViewHelpers.h>

using namespace facebook::react;

static std::string keyboardEventText(NSString *value)
{
  if (value == nil) return {};
  return std::string(value.UTF8String, [value lengthOfBytesUsingEncoding:NSUTF8StringEncoding]);
}

@interface RsKeyCaptureComponentView () <RCTRsKeyCaptureViewProtocol>
@end

@implementation RsKeyCaptureComponentView {
  RsKeyCaptureView *_capture;
  BOOL _needsConfiguration;
}

+ (ComponentDescriptorProvider)componentDescriptorProvider
{
  return concreteComponentDescriptorProvider<RsKeyCaptureComponentDescriptor>();
}

- (instancetype)initWithFrame:(CGRect)frame
{
  if (self = [super initWithFrame:frame]) {
    _props = std::make_shared<const RsKeyCaptureProps>();
    _needsConfiguration = YES;
    _capture = [[RsKeyCaptureView alloc] initWithFrame:self.bounds];
    self.contentView = _capture;
    __weak RsKeyCaptureComponentView *weakSelf = self;
    _capture.onKey = ^(NSDictionary *payload) {
      RsKeyCaptureComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RsKeyCaptureEventEmitter>(view->_eventEmitter);
      RsKeyCaptureEventEmitter::OnKey event = {};
      event.usage = [payload[@"usage"] intValue];
      event.down = [payload[@"down"] boolValue];
      event.mods = [payload[@"mods"] intValue];
      emitter->onKey(event);
    };
    _capture.onText = ^(NSDictionary *payload) {
      RsKeyCaptureComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RsKeyCaptureEventEmitter>(view->_eventEmitter);
      RsKeyCaptureEventEmitter::OnText event = {};
      event.text = keyboardEventText(payload[@"text"]);
      emitter->onText(event);
    };
    _capture.onComposition = ^(NSDictionary *payload) {
      RsKeyCaptureComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RsKeyCaptureEventEmitter>(view->_eventEmitter);
      RsKeyCaptureEventEmitter::OnComposition event = {};
      event.text = keyboardEventText(payload[@"text"]);
      emitter->onComposition(event);
    };
    _capture.onStickyConsumed = ^(NSDictionary *payload) {
      RsKeyCaptureComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RsKeyCaptureEventEmitter>(view->_eventEmitter);
      RsKeyCaptureEventEmitter::OnStickyConsumed event = {};
      event.mods = [payload[@"mods"] intValue];
      emitter->onStickyConsumed(event);
    };
    _capture.onKeyboardFrame = ^(NSDictionary *payload) {
      RsKeyCaptureComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RsKeyCaptureEventEmitter>(view->_eventEmitter);
      RsKeyCaptureEventEmitter::OnKeyboardFrame event = {};
      event.height = [payload[@"height"] floatValue];
      event.visible = [payload[@"visible"] boolValue];
      event.hardware = [payload[@"hardware"] boolValue];
      emitter->onKeyboardFrame(event);
    };
    _capture.onPaste = ^(NSDictionary *payload) {
      RsKeyCaptureComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RsKeyCaptureEventEmitter>(view->_eventEmitter);
      RsKeyCaptureEventEmitter::OnPaste event = {};
      event.mods = [payload[@"mods"] intValue];
      emitter->onPaste(event);
    };
    _capture.onShortcut = ^(NSDictionary *payload) {
      RsKeyCaptureComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RsKeyCaptureEventEmitter>(view->_eventEmitter);
      RsKeyCaptureEventEmitter::OnShortcut event = {};
      event.action = keyboardEventText(payload[@"action"]);
      event.index = [payload[@"index"] intValue];
      emitter->onShortcut(event);
    };
    _capture.onActiveChange = ^(NSDictionary *payload) {
      RsKeyCaptureComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RsKeyCaptureEventEmitter>(view->_eventEmitter);
      RsKeyCaptureEventEmitter::OnActiveChange event = {};
      event.active = [payload[@"active"] boolValue];
      emitter->onActiveChange(event);
    };
  }
  return self;
}

- (void)updateProps:(Props::Shared const &)props oldProps:(Props::Shared const &)oldProps
{
  const auto &next = *std::static_pointer_cast<const RsKeyCaptureProps>(props);
  const auto &previous = *std::static_pointer_cast<const RsKeyCaptureProps>(_props);
  if (_needsConfiguration || next.active != previous.active || next.inputMode != previous.inputMode ||
      next.stickyMods != previous.stickyMods || next.os != previous.os) {
    [_capture configureActive:next.active
                    inputMode:next.inputMode == RsKeyCaptureInputMode::Keys ? @"keys" : @"text"
                   stickyMods:next.stickyMods
                           os:next.os == RsKeyCaptureOs::Windows ? @"windows" : @"macos"];
    _needsConfiguration = NO;
  }
  _capture.syncPaste = next.syncPaste;
  [super updateProps:props oldProps:oldProps];
}

- (void)prepareForRecycle
{
  [_capture reset];
  _needsConfiguration = YES;
  [super prepareForRecycle];
}

@end
