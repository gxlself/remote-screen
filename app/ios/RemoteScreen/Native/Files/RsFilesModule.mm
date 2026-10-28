#import "RsFilesModule.h"

#import <React_RCTAppDelegate/RCTDefaultReactNativeFactoryDelegate.h>
#import "RemoteScreen-Swift.h"
#import <React/RCTBridgeModule.h>

#include <memory>

@implementation RsFilesModule

+ (BOOL)requiresMainQueueSetup
{
  return YES;
}

+ (NSString *)moduleName
{
  return @"RsFiles";
}

- (facebook::react::ModuleConstants<JS::NativeRsFiles::Constants>)getConstants
{
  NSDictionary *values = [[FilesService shared] getConstants];
  return facebook::react::typedConstants<JS::NativeRsFiles::Constants>({
      .filesDir = values[@"filesDir"],
      .inboxDir = values[@"inboxDir"],
  });
}

- (facebook::react::ModuleConstants<JS::NativeRsFiles::Constants>)constantsToExport
{
  return [self getConstants];
}

- (void)pickFiles:(RCTPromiseResolveBlock)resolve
           reject:(RCTPromiseRejectBlock)reject
{
  [[FilesService shared]
      pickFilesWithCompletion:^(NSArray *files, NSError *error) {
        if (error != nil) {
          reject(error.domain, error.localizedDescription, error);
        } else {
          resolve(files);
        }
      }];
}

- (void)saveToFiles:(NSString *)uri
               name:(NSString *)name
            resolve:(RCTPromiseResolveBlock)resolve
             reject:(RCTPromiseRejectBlock)reject
{
  [[FilesService shared]
      saveToFilesWithUri:uri
                    name:name
              completion:^(BOOL saved, NSError *error) {
                if (error != nil) {
                  reject(error.domain, error.localizedDescription, error);
                } else {
                  resolve(@(saved));
                }
              }];
}

- (void)importIncoming:(NSString *)url
                resolve:(RCTPromiseResolveBlock)resolve
                 reject:(RCTPromiseRejectBlock)reject
{
  [[FilesService shared]
      importIncomingWithUrl:url
                  completion:^(NSDictionary *file, NSError *error) {
                    if (error != nil) {
                      reject(error.domain, error.localizedDescription, error);
                    } else {
                      resolve(file);
                    }
                  }];
}

- (void)setTransfersActive:(BOOL)active
{
  [[FilesService shared] setTransfersActive:active];
}

- (std::shared_ptr<facebook::react::TurboModule>)getTurboModule:
    (const facebook::react::ObjCTurboModule::InitParams &)params
{
  return std::make_shared<facebook::react::NativeRsFilesSpecJSI>(params);
}

@end
