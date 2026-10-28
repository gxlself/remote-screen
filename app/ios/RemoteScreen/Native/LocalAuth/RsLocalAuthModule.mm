#import "RsLocalAuthModule.h"

#import <React_RCTAppDelegate/RCTDefaultReactNativeFactoryDelegate.h>
#import "RemoteScreen-Swift.h"
#import <React/RCTBridgeModule.h>
#import <memory>

@implementation RsLocalAuthModule

+ (NSString *)moduleName
{
  return @"LocalAuth";
}

- (void)authenticate:(NSString *)reason
       fallbackTitle:(NSString *)fallbackTitle
             resolve:(RCTPromiseResolveBlock)resolve
              reject:(RCTPromiseRejectBlock)reject
{
  (void)reject;
  [[LocalAuthService shared]
      authenticateWithReason:reason
                fallbackTitle:fallbackTitle
                  completion:^(NSString *result) {
                    resolve(result);
                  }];
}

- (NSString *)getBiometryType
{
  return [[LocalAuthService shared] getBiometryType];
}

- (NSDictionary *)canAuthenticate
{
  return [[LocalAuthService shared] canAuthenticate];
}

- (void)setPrivacyCover:(BOOL)enabled
{
  [[PrivacyCover shared] setEnabled:enabled];
}

- (std::shared_ptr<facebook::react::TurboModule>)getTurboModule:
    (const facebook::react::ObjCTurboModule::InitParams &)params
{
  return std::make_shared<facebook::react::NativeLocalAuthSpecJSI>(params);
}

@end
