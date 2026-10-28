#import "RsSecureKeyModule.h"

#import <React_RCTAppDelegate/RCTDefaultReactNativeFactoryDelegate.h>
#import <React/RCTBridgeModule.h>
#import "RemoteScreen-Swift.h"

#include <memory>

@implementation RsSecureKeyModule

+ (NSString *)moduleName
{
  return @"SecureKey";
}

- (void)info:(RCTPromiseResolveBlock)resolve
       reject:(RCTPromiseRejectBlock)reject
{
  (void)reject;
  dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
    resolve([RsSecureKeyBridge info]);
  });
}

- (void)benchmark:(double)count
           resolve:(RCTPromiseResolveBlock)resolve
            reject:(RCTPromiseRejectBlock)reject
{
  dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
    NSError *error = nil;
    NSDictionary *result = [RsSecureKeyBridge benchmark:@(count) error:&error];
    if (result != nil) {
      resolve(result);
    } else {
      NSString *message = error.localizedDescription ?: @"device key signing failed";
      reject(@"E_SIGN", message, error);
    }
  });
}

- (std::shared_ptr<facebook::react::TurboModule>)getTurboModule:
    (const facebook::react::ObjCTurboModule::InitParams &)params
{
  return std::make_shared<facebook::react::NativeSecureKeySpecJSI>(params);
}

@end
