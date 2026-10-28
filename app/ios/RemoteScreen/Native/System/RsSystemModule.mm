#import "RsSystemModule.h"

#import <React_RCTAppDelegate/RCTDefaultReactNativeFactoryDelegate.h>
#import "RemoteScreen-Swift.h"
#import <React/RCTBridgeModule.h>
#import <memory>

@implementation RsSystemModule

+ (BOOL)requiresMainQueueSetup
{
  return YES;
}

+ (NSString *)moduleName
{
  return @"RsSystem";
}

- (instancetype)init
{
  self = [super init];
  if (self) {
    [[SystemServices shared] prepareConstants];
  }
  return self;
}

- (facebook::react::ModuleConstants<JS::NativeRsSystem::Constants>)getConstants
{
  NSDictionary *values = [[SystemServices shared] getConstants];
  return facebook::react::typedConstants<JS::NativeRsSystem::Constants>({
    .appVersion = values[@"appVersion"],
    .buildNumber = values[@"buildNumber"],
    .bundleId = values[@"bundleId"],
    .modelId = values[@"modelId"],
    .maxFps = [values[@"maxFps"] doubleValue],
  });
}

- (facebook::react::ModuleConstants<JS::NativeRsSystem::Constants>)constantsToExport
{
  return [self getConstants];
}

- (void)copyText:(NSString *)text
{
  [[SystemServices shared] copyText:text];
}

- (NSNumber *)hasText
{
  return @([[SystemServices shared] hasText]);
}

- (void)readText:(RCTPromiseResolveBlock)resolve
          reject:(RCTPromiseRejectBlock)reject
{
  (void)reject;
  [[SystemServices shared] readTextWithCompletion:^(NSString * _Nullable value) {
    resolve(value);
  }];
}

- (void)haptic:(NSString *)kind
{
  [[SystemServices shared] haptic:kind];
}

- (void)notificationStatus:(RCTPromiseResolveBlock)resolve
                     reject:(RCTPromiseRejectBlock)reject
{
  (void)reject;
  [[SystemServices shared] notificationStatusWithCompletion:^(NSString *value) {
    resolve(value);
  }];
}

- (void)requestNotifications:(RCTPromiseResolveBlock)resolve
                       reject:(RCTPromiseRejectBlock)reject
{
  (void)reject;
  [[SystemServices shared] requestNotificationsWithCompletion:^(NSString *value) {
    resolve(value);
  }];
}

- (void)requestLocalNetwork:(double)timeoutMs
                    resolve:(RCTPromiseResolveBlock)resolve
                     reject:(RCTPromiseRejectBlock)reject
{
  (void)reject;
  [[SystemServices shared]
      requestLocalNetworkWithTimeoutMs:timeoutMs
                             completion:^(NSString *value) {
                               resolve(value);
                             }];
}

- (std::shared_ptr<facebook::react::TurboModule>)getTurboModule:
    (const facebook::react::ObjCTurboModule::InitParams &)params
{
  return std::make_shared<facebook::react::NativeRsSystemSpecJSI>(params);
}

@end
