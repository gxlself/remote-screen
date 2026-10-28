#import "RsLifecycleModule.h"

#import <React_RCTAppDelegate/RCTDefaultReactNativeFactoryDelegate.h>
#import <React/RCTBridgeModule.h>
#import "RemoteScreen-Swift.h"

#include <memory>

@implementation RsLifecycleModule

+ (NSString *)moduleName
{
  return @"RsLifecycle";
}

- (instancetype)init
{
  self = [super init];
  if (self) {
    [self ensureMonitorStarted];
  }
  return self;
}

- (facebook::react::ModuleConstants<JS::NativeRsLifecycle::Constants>)constantsToExport
{
  return [self getConstants];
}

- (facebook::react::ModuleConstants<JS::NativeRsLifecycle::Constants>)getConstants
{
  [self ensureMonitorStarted];
  NSDictionary *constants = [[RsLifecycleMonitor shared] constants];

  return facebook::react::typedConstants<JS::NativeRsLifecycle::Constants>({
      .previousRunEndedInBackground =
          [[constants objectForKey:@"previousRunEndedInBackground"] boolValue],
  });
}

- (void)setIdleTimerDisabled:(BOOL)disabled
{
  [self ensureMonitorStarted];
  [[RsLifecycleMonitor shared] setIdleTimerDisabled:disabled];
}

- (void)setBackgroundKeepAlive:(BOOL)enabled
{
  [self ensureMonitorStarted];
  [[RsLifecycleMonitor shared] setBackgroundKeepAlive:enabled];
}

- (void)getPowerState:(RCTPromiseResolveBlock)resolve
               reject:(RCTPromiseRejectBlock)reject
{
  [self ensureMonitorStarted];
  (void)reject;
  resolve([[RsLifecycleMonitor shared] powerState]);
}

- (void)ensureMonitorStarted
{
  RsLifecycleMonitor *monitor = [RsLifecycleMonitor shared];
  [monitor start];
  __weak RsLifecycleModule *weakSelf = self;
  [monitor registerWithPowerChangeCallback:^(NSDictionary *state) {
    RsLifecycleModule *strongSelf = weakSelf;
    if (strongSelf != nil) {
      [strongSelf emitOnPowerChange:state];
    }
  } backgroundTaskCallback:^(NSDictionary *event) {
    RsLifecycleModule *strongSelf = weakSelf;
    if (strongSelf != nil) {
      [strongSelf emitOnBackgroundTask:event];
    }
  }];
}

- (std::shared_ptr<facebook::react::TurboModule>)getTurboModule:
    (const facebook::react::ObjCTurboModule::InitParams &)params
{
  return std::make_shared<facebook::react::NativeRsLifecycleSpecJSI>(params);
}

@end
