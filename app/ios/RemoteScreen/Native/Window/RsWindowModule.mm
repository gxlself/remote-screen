#import "RsWindowModule.h"

#import <React_RCTAppDelegate/RCTDefaultReactNativeFactoryDelegate.h>
#import <React/RCTBridgeModule.h>
#import "RemoteScreen-Swift.h"

#include <memory>

// the app's system windows for JS (NativeRsWindow.ts).
@implementation RsWindowModule

+ (NSString *)moduleName
{
  return @"RsWindow";
}

- (instancetype)init
{
  self = [super init];
  if (self) {
    __weak RsWindowModule *weakSelf = self;
    [[RsWindowCoordinator shared] registerWithStateCallback:^(NSDictionary *event) {
      RsWindowModule *strongSelf = weakSelf;
      if (strongSelf != nil) {
        [strongSelf emitOnWindowState:event];
      }
    }];
  }
  return self;
}

- (facebook::react::ModuleConstants<JS::NativeRsWindow::Constants>)constantsToExport
{
  return [self getConstants];
}

- (facebook::react::ModuleConstants<JS::NativeRsWindow::Constants>)getConstants
{
  return facebook::react::typedConstants<JS::NativeRsWindow::Constants>({
      .supported = [[RsWindowCoordinator shared] supported],
  });
}

- (void)open:(NSString *)windowId payload:(NSString *)payload title:(NSString *)title
{
  [[RsWindowCoordinator shared] open:windowId payload:payload title:title];
}

- (void)activate:(NSString *)windowId
{
  [[RsWindowCoordinator shared] activate:windowId];
}

- (void)close:(NSString *)windowId
{
  [[RsWindowCoordinator shared] close:windowId];
}

- (void)refresh
{
  [[RsWindowCoordinator shared] refresh];
}

- (std::shared_ptr<facebook::react::TurboModule>)getTurboModule:
    (const facebook::react::ObjCTurboModule::InitParams &)params
{
  return std::make_shared<facebook::react::NativeRsWindowSpecJSI>(params);
}

@end
