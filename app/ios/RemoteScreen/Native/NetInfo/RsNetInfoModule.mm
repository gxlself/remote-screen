#import "RsNetInfoModule.h"

#import <React_RCTAppDelegate/RCTDefaultReactNativeFactoryDelegate.h>
#import "RemoteScreen-Swift.h"

#import <React/RCTBridgeModule.h>
#import <memory>

@interface RsNetInfoModule ()
@property(nonatomic, assign) BOOL monitorStarted;
@end

@implementation RsNetInfoModule

+ (NSString *)moduleName
{
  return @"NetInfoPlus";
}

- (instancetype)init
{
  self = [super init];
  if (self) {
    [self ensureMonitorStarted];
  }
  return self;
}

- (void)setVpnPools:(NSArray *)pools
{
  [self ensureMonitorStarted];
  NSMutableArray<NSString *> *values = [NSMutableArray arrayWithCapacity:pools.count];
  for (id value in pools) {
    if ([value isKindOfClass:[NSString class]]) {
      [values addObject:value];
    }
  }
  [[NetInfoMonitor shared] setVpnPools:values];
}

- (void)getStatus:(RCTPromiseResolveBlock)resolve
           reject:(RCTPromiseRejectBlock)reject
{
  [self ensureMonitorStarted];
  (void)reject;
  resolve([[NetInfoMonitor shared] status]);
}

- (void)ensureMonitorStarted
{
  if (self.monitorStarted) {
    return;
  }
  self.monitorStarted = YES;
  __weak RsNetInfoModule *weakSelf = self;
  [[NetInfoMonitor shared] startWithCallback:^(NSDictionary *status) {
    RsNetInfoModule *strongSelf = weakSelf;
    if (strongSelf != nil) {
      [strongSelf emitOnChange:status];
    }
  }];
}

- (std::shared_ptr<facebook::react::TurboModule>)getTurboModule:
    (const facebook::react::ObjCTurboModule::InitParams &)params
{
  return std::make_shared<facebook::react::NativeNetInfoPlusSpecJSI>(params);
}

@end
