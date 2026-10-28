#import "RsPipModule.h"

#import <React_RCTAppDelegate/RCTDefaultReactNativeFactoryDelegate.h>
#import "RemoteScreen-Swift.h"
#import <React/RCTBridgeModule.h>

#include <memory>

@implementation RsPipModule

- (instancetype)init
{
  self = [super init];
  if (self) {
    __weak RsPipModule *weakSelf = self;
    [[SessionPipCoordinator shared]
        registerWithChange:^(NSDictionary *event) {
          RsPipModule *strongSelf = weakSelf;
          if (strongSelf != nil) {
            [strongSelf emitOnPipChange:event];
          }
        }
        playback:^(NSDictionary *event) {
          RsPipModule *strongSelf = weakSelf;
          if (strongSelf != nil) {
            [strongSelf emitOnPipPlayback:event];
          }
        }];
  }
  return self;
}

+ (NSString *)moduleName
{
  return @"RsPip";
}

- (facebook::react::ModuleConstants<JS::NativeRsPip::Constants>)getConstants
{
  return facebook::react::typedConstants<JS::NativeRsPip::Constants>({
      .supported = [[SessionPipCoordinator shared] supported],
  });
}

- (facebook::react::ModuleConstants<JS::NativeRsPip::Constants>)constantsToExport
{
  return [self getConstants];
}

- (void)setEligibility:(NSString *)sessionId
              canStart:(BOOL)canStart
           canContinue:(BOOL)canContinue
{
  [[SessionPipCoordinator shared] setEligibility:sessionId
                                         canStart:canStart
                                      canContinue:canContinue];
}

- (void)start
{
  [[SessionPipCoordinator shared] requestStart];
}

- (void)stop
{
  [[SessionPipCoordinator shared] requestStop];
}

- (NSNumber *)isActive
{
  return @([[SessionPipCoordinator shared] isActiveSync]);
}

- (std::shared_ptr<facebook::react::TurboModule>)getTurboModule:
    (const facebook::react::ObjCTurboModule::InitParams &)params
{
  return std::make_shared<facebook::react::NativeRsPipSpecJSI>(params);
}

@end
