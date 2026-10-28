#import "RsTerminalViewComponentView.h"
#import <React_RCTAppDelegate/RCTDefaultReactNativeFactoryDelegate.h>
#import "RemoteScreen-Swift.h"

#import <react/renderer/components/RsVideoSpec/ComponentDescriptors.h>
#import <react/renderer/components/RsVideoSpec/EventEmitters.h>
#import <react/renderer/components/RsVideoSpec/Props.h>
#import <react/renderer/components/RsVideoSpec/RCTComponentViewHelpers.h>

using namespace facebook::react;

@interface RsTerminalViewComponentView () <RCTRsTerminalViewViewProtocol>
@end

@implementation RsTerminalViewComponentView {
  RsTerminalSurface *_surface;
  BOOL _needsConfiguration;
}

+ (ComponentDescriptorProvider)componentDescriptorProvider
{
  return concreteComponentDescriptorProvider<RsTerminalViewComponentDescriptor>();
}

- (instancetype)initWithFrame:(CGRect)frame
{
  if (self = [super initWithFrame:frame]) {
    _props = std::make_shared<const RsTerminalViewProps>();
    _needsConfiguration = YES;
    _surface = [[RsTerminalSurface alloc] initWithFrame:self.bounds];
    self.contentView = _surface;
    self.isAccessibilityElement = YES;
    __weak RsTerminalViewComponentView *weakSelf = self;
    _surface.onModifiersConsumed = ^(NSDictionary *payload) {
      RsTerminalViewComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RsTerminalViewEventEmitter>(view->_eventEmitter);
      RsTerminalViewEventEmitter::OnModifiersConsumed event = {};
      event.mods = [payload[@"mods"] intValue];
      emitter->onModifiersConsumed(event);
    };
    _surface.onSizeChange = ^(NSDictionary *payload) {
      RsTerminalViewComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RsTerminalViewEventEmitter>(view->_eventEmitter);
      RsTerminalViewEventEmitter::OnSizeChange event = {};
      event.cols = [payload[@"cols"] intValue];
      event.rows = [payload[@"rows"] intValue];
      emitter->onSizeChange(event);
    };
    _surface.onKeyboardFrame = ^(NSDictionary *payload) {
      RsTerminalViewComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RsTerminalViewEventEmitter>(view->_eventEmitter);
      RsTerminalViewEventEmitter::OnKeyboardFrame event = {};
      event.height = [payload[@"height"] floatValue];
      event.visible = [payload[@"visible"] boolValue];
      event.hardware = [payload[@"hardware"] boolValue];
      emitter->onKeyboardFrame(event);
    };
    _surface.onActiveChange = ^(NSDictionary *payload) {
      RsTerminalViewComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RsTerminalViewEventEmitter>(view->_eventEmitter);
      RsTerminalViewEventEmitter::OnActiveChange event = {};
      event.active = [payload[@"active"] boolValue];
      emitter->onActiveChange(event);
    };
    _surface.onCopied = ^(NSDictionary *payload) {
      RsTerminalViewComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RsTerminalViewEventEmitter>(view->_eventEmitter);
      RsTerminalViewEventEmitter::OnCopied event = {};
      event.length = [payload[@"length"] intValue];
      emitter->onCopied(event);
    };
  }
  return self;
}

- (void)updateProps:(Props::Shared const &)props oldProps:(Props::Shared const &)oldProps
{
  const auto &next = *std::static_pointer_cast<const RsTerminalViewProps>(props);
  const auto &previous = *std::static_pointer_cast<const RsTerminalViewProps>(_props);
  if (_needsConfiguration || next.sshId != previous.sshId || next.fontSize != previous.fontSize ||
      next.theme != previous.theme || next.modifiers != previous.modifiers ||
      next.keyboardActive != previous.keyboardActive || next.debugEcho != previous.debugEcho) {
    // Preserve the full string (including NUL) for strict decimal-id validation.
    NSString *sshId = [[NSString alloc] initWithBytes:next.sshId.data()
                                            length:next.sshId.size()
                                          encoding:NSUTF8StringEncoding] ?: @"";
    [_surface configureSshId:sshId
                    fontSize:next.fontSize
                       theme:[NSString stringWithUTF8String:next.theme.c_str()] ?: @"night"
                    modifiers:next.modifiers
               keyboardActive:next.keyboardActive
                   debugEcho:next.debugEcho];
    _needsConfiguration = NO;
  }
  [super updateProps:props oldProps:oldProps];
}

- (void)handleCommand:(NSString const *)commandName args:(NSArray const *)args
{
  RCTRsTerminalViewHandleCommand(self, commandName, args);
}

- (void)sendSequence:(NSString *)text { [_surface sendSequence:text ?: @""]; }
- (void)sendKey:(NSString *)name { [_surface sendKey:name ?: @""]; }
- (void)copySelection { [_surface copySelection]; }
- (void)paste { [_surface paste]; }

- (void)prepareForRecycle
{
  [_surface reset];
  _needsConfiguration = YES;
  [super prepareForRecycle];
}

@end
