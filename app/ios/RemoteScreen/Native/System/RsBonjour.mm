#import <RsVideoSpec/RsVideoSpec.h>
#import <React_RCTAppDelegate/RCTDefaultReactNativeFactoryDelegate.h>
#import "RemoteScreen-Swift.h"

#import <React/RCTBridgeModule.h>
#import <React/RCTInvalidating.h>
#include <memory>

@interface RsBonjourModule : NativeRsBonjourSpecBase <NativeRsBonjourSpec, RCTInvalidating>
@end

@implementation RsBonjourModule

+ (BOOL)requiresMainQueueSetup
{
  return YES;
}

+ (NSString *)moduleName
{
  return @"RsBonjour";
}

- (instancetype)init
{
  self = [super init];
  if (self) {
    __weak RsBonjourModule *weakSelf = self;
    [[RsBonjourService shared] registerWithCallback:^(NSDictionary *event) {
      RsBonjourModule *strongSelf = weakSelf;
      if (strongSelf != nil) {
        [strongSelf emitOnService:event];
      }
    }];
  }
  return self;
}

- (void)start:(NSString *)type
{
  [[RsBonjourService shared] start:type];
}

- (void)stop
{
  [[RsBonjourService shared] stop];
}

- (void)invalidate
{
  [[RsBonjourService shared] clearCallback];
  [[RsBonjourService shared] stop];
}

- (std::shared_ptr<facebook::react::TurboModule>)getTurboModule:
    (const facebook::react::ObjCTurboModule::InitParams &)params
{
  return std::make_shared<facebook::react::NativeRsBonjourSpecJSI>(params);
}

@end
