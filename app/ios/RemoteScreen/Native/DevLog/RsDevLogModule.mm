#import "RsDevLogModule.h"

#import <React/RCTBridgeModule.h>
#import <memory>

@implementation RsDevLogModule

+ (NSString *)moduleName
{
  return @"RsDevLog";
}

- (void)log:(NSString *)line
{
  if ([line hasPrefix:@"RS-"]) {
    NSLog(@"%@", line);
  }
}

- (std::shared_ptr<facebook::react::TurboModule>)getTurboModule:
    (const facebook::react::ObjCTurboModule::InitParams &)params
{
  return std::make_shared<facebook::react::NativeRsDevLogSpecJSI>(params);
}

@end
