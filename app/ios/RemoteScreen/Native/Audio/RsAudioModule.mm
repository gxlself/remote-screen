#import "RsAudioModule.h"

#import <React_RCTAppDelegate/RCTDefaultReactNativeFactoryDelegate.h>
#import "RemoteScreen-Swift.h"

#import <React/RCTBridgeModule.h>
#import <React/RCTInvalidating.h>
#import <memory>

static void RsAudioOnMain(dispatch_block_t action)
{
  if ([NSThread isMainThread]) {
    action();
  } else {
    dispatch_async(dispatch_get_main_queue(), action);
  }
}

@interface RsAudioModule () <RCTInvalidating>
@property(nonatomic, strong) id stateObserver;
@end

@implementation RsAudioModule

+ (BOOL)requiresMainQueueSetup
{
  return YES;
}

+ (NSString *)moduleName
{
  return @"RsAudio";
}

- (instancetype)init
{
  self = [super init];
  if (self) {
    __weak RsAudioModule *weakSelf = self;
    self.stateObserver = [[NSNotificationCenter defaultCenter]
        addObserverForName:@"RsAudioStateDidChange"
                    object:nil
                     queue:[NSOperationQueue mainQueue]
                usingBlock:^(NSNotification *note) {
                  RsAudioModule *strongSelf = weakSelf;
                  if (strongSelf.stateObserver != nil && note.userInfo != nil) {
                    [strongSelf emitOnAudioState:note.userInfo];
                  }
                }];
  }
  return self;
}

- (void)dealloc
{
  if (_stateObserver != nil) {
    [[NSNotificationCenter defaultCenter] removeObserver:_stateObserver];
  }
}

- (void)invalidate
{
  if (self.stateObserver != nil) {
    [[NSNotificationCenter defaultCenter] removeObserver:self.stateObserver];
    self.stateObserver = nil;
  }
  RsAudioOnMain(^{
    [[RsAudioPlayer shared] stop];
  });
}

- (void)start:(NSString *)sessionId
      resolve:(RCTPromiseResolveBlock)resolve
       reject:(RCTPromiseRejectBlock)reject
{
  (void)reject;
  RsAudioOnMain(^{
    resolve(@([[RsAudioPlayer shared] start:sessionId]));
  });
}

- (void)stop
{
  RsAudioOnMain(^{
    [[RsAudioPlayer shared] stop];
  });
}

- (void)setMuted:(BOOL)muted
{
  RsAudioOnMain(^{
    [[RsAudioPlayer shared] setMuted:muted];
  });
}

- (void)getStats:(RCTPromiseResolveBlock)resolve
          reject:(RCTPromiseRejectBlock)reject
{
  (void)reject;
  RsAudioOnMain(^{
    resolve([[RsAudioPlayer shared] stats]);
  });
}

- (std::shared_ptr<facebook::react::TurboModule>)getTurboModule:
    (const facebook::react::ObjCTurboModule::InitParams &)params
{
  return std::make_shared<facebook::react::NativeRsAudioSpecJSI>(params);
}

@end
