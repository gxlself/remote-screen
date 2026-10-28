#import "RsQrScannerComponentView.h"
#import <React_RCTAppDelegate/RCTDefaultReactNativeFactoryDelegate.h>
#import "RemoteScreen-Swift.h"

#import <react/renderer/components/RsVideoSpec/ComponentDescriptors.h>
#import <react/renderer/components/RsVideoSpec/EventEmitters.h>
#import <react/renderer/components/RsVideoSpec/Props.h>
#import <react/renderer/components/RsVideoSpec/RCTComponentViewHelpers.h>

using namespace facebook::react;

@interface RsQrScannerComponentView () <RCTRsQrScannerViewViewProtocol>
@end

@implementation RsQrScannerComponentView {
  UIView *_surface;
  QrScannerController *_scanner;
  BOOL _needsConfiguration;
}

+ (ComponentDescriptorProvider)componentDescriptorProvider
{
  return concreteComponentDescriptorProvider<RsQrScannerViewComponentDescriptor>();
}

- (instancetype)initWithFrame:(CGRect)frame
{
  if (self = [super initWithFrame:frame]) {
    _props = std::make_shared<const RsQrScannerViewProps>();
    _needsConfiguration = YES;
    _surface = [[UIView alloc] initWithFrame:self.bounds];
    _surface.clipsToBounds = YES;
    self.contentView = _surface;
    _scanner = [[QrScannerController alloc] initWithView:_surface];

    __weak RsQrScannerComponentView *weakSelf = self;
    _scanner.onCode = ^(NSString *text) {
      RsQrScannerComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RsQrScannerViewEventEmitter>(view->_eventEmitter);
      RsQrScannerViewEventEmitter::OnCode event = {};
      event.text = text.UTF8String;
      emitter->onCode(event);
    };
    _scanner.onScannerState = ^(NSString *state, NSString *message, BOOL hasTorch) {
      RsQrScannerComponentView *view = weakSelf;
      if (!view || !view->_eventEmitter) return;
      const auto emitter = std::static_pointer_cast<const RsQrScannerViewEventEmitter>(view->_eventEmitter);
      RsQrScannerViewEventEmitter::OnScannerState event = {};
      event.state = state.UTF8String;
      event.message = message.UTF8String;
      event.hasTorch = hasTorch;
      emitter->onScannerState(event);
    };
    [_scanner layout];
  }
  return self;
}

- (void)updateProps:(Props::Shared const &)props oldProps:(Props::Shared const &)oldProps
{
  const auto &next = *std::static_pointer_cast<const RsQrScannerViewProps>(props);
  const auto &previous = *std::static_pointer_cast<const RsQrScannerViewProps>(_props);
  if (_needsConfiguration || next.active != previous.active) {
    [_scanner setActive:next.active];
  }
  if (_needsConfiguration || next.torch != previous.torch) {
    [_scanner setTorch:next.torch];
  }
  _needsConfiguration = NO;
  [super updateProps:props oldProps:oldProps];
}

- (void)layoutSubviews
{
  [super layoutSubviews];
  _surface.frame = self.bounds;
  [_scanner layout];
}

- (void)didMoveToWindow
{
  [super didMoveToWindow];
  [_scanner setAttached:self.window != nil];
  [_scanner layout];
}

- (void)prepareForRecycle
{
  [_scanner setAttached:NO];
  [_scanner setTorch:NO];
  [_scanner stop];
  _needsConfiguration = YES;
  [super prepareForRecycle];
}

@end
