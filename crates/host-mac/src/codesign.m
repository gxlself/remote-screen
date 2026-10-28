#import <Foundation/Foundation.h>
#import <Security/Security.h>

#include <stddef.h>
#include <stdint.h>
#include <string.h>

static void rs_codesign_clear(char *buffer, size_t length) {
    if (buffer != NULL && length != 0) {
        buffer[0] = '\0';
    }
}

static void rs_codesign_write_string(char *buffer, size_t length, NSString *value) {
    if (buffer == NULL || length == 0) {
        return;
    }
    const char *utf8 = value.UTF8String;
    if (utf8 == NULL) {
        buffer[0] = '\0';
        return;
    }
    size_t copy_length = strnlen(utf8, length - 1);
    memcpy(buffer, utf8, copy_length);
    buffer[copy_length] = '\0';
}

static void rs_codesign_write_status(char *buffer,
                                     size_t length,
                                     OSStatus status,
                                     CFErrorRef error) {
    NSString *description = nil;
    if (error != NULL) {
        CFStringRef copied = CFErrorCopyDescription(error);
        if (copied != NULL) {
            description = [(__bridge NSString *)copied copy];
            CFRelease(copied);
        }
    }
    if (description.length == 0) {
        CFStringRef copied = SecCopyErrorMessageString(status, NULL);
        if (copied != NULL) {
            description = [(__bridge NSString *)copied copy];
            CFRelease(copied);
        }
    }
    if (description.length == 0) {
        description = @"unknown code-signing error";
    }
    NSString *message = [NSString stringWithFormat:@"code signing error (%d): %@",
                                                   (int)status,
                                                   description];
    rs_codesign_write_string(buffer, length, message);
}

static int rs_codesign_team_for_code(SecStaticCodeRef code,
                                     char *out,
                                     size_t out_len,
                                     char *err,
                                     size_t err_len) {
    CFDictionaryRef information = NULL;
    OSStatus status = SecCodeCopySigningInformation(code,
                                                    kSecCSSigningInformation,
                                                    &information);
    if (status == errSecCSUnsigned) {
        if (information != NULL) {
            CFRelease(information);
        }
        return 0;
    }
    if (status != errSecSuccess) {
        rs_codesign_write_status(err, err_len, status, NULL);
        if (information != NULL) {
            CFRelease(information);
        }
        return -1;
    }

    CFTypeRef team = information == NULL
                         ? NULL
                         : CFDictionaryGetValue(information, kSecCodeInfoTeamIdentifier);
    int result = 0;
    if (team != NULL && CFGetTypeID(team) == CFStringGetTypeID()) {
        NSString *team_string = (__bridge NSString *)team;
        if (team_string.length != 0) {
            rs_codesign_write_string(out, out_len, team_string);
            result = 1;
        }
    }
    if (information != NULL) {
        CFRelease(information);
    }
    return result;
}

int rs_codesign_team_self(char *out,
                          size_t out_len,
                          char *err,
                          size_t err_len) {
    @autoreleasepool {
        rs_codesign_clear(out, out_len);
        rs_codesign_clear(err, err_len);

        SecCodeRef code = NULL;
        OSStatus status = SecCodeCopySelf(kSecCSDefaultFlags, &code);
        if (status == errSecCSUnsigned) {
            if (code != NULL) {
                CFRelease(code);
            }
            return 0;
        }
        if (status != errSecSuccess || code == NULL) {
            rs_codesign_write_status(err, err_len, status, NULL);
            if (code != NULL) {
                CFRelease(code);
            }
            return -1;
        }
        int result = rs_codesign_team_for_code(code, out, out_len, err, err_len);
        CFRelease(code);
        return result;
    }
}

int rs_codesign_team_path(const char *path,
                          char *out,
                          size_t out_len,
                          char *err,
                          size_t err_len) {
    @autoreleasepool {
        rs_codesign_clear(out, out_len);
        rs_codesign_clear(err, err_len);
        if (path == NULL) {
            rs_codesign_write_string(err, err_len, @"code path is null");
            return -1;
        }

        CFURLRef url = CFURLCreateFromFileSystemRepresentation(kCFAllocatorDefault,
                                                                 (const UInt8 *)path,
                                                                 strlen(path),
                                                                 false);
        if (url == NULL) {
            rs_codesign_write_string(err, err_len, @"could not create code URL");
            return -1;
        }

        SecStaticCodeRef code = NULL;
        OSStatus status = SecStaticCodeCreateWithPath(url, kSecCSDefaultFlags, &code);
        CFRelease(url);
        if (status == errSecCSUnsigned) {
            if (code != NULL) {
                CFRelease(code);
            }
            return 0;
        }
        if (status != errSecSuccess || code == NULL) {
            rs_codesign_write_status(err, err_len, status, NULL);
            if (code != NULL) {
                CFRelease(code);
            }
            return -1;
        }
        int result = rs_codesign_team_for_code(code, out, out_len, err, err_len);
        CFRelease(code);
        return result;
    }
}

int rs_codesign_check(const char *path,
                      const char *requirement,
                      char *err,
                      size_t err_len) {
    @autoreleasepool {
        rs_codesign_clear(err, err_len);
        if (path == NULL) {
            rs_codesign_write_string(err, err_len, @"code path is null");
            return -1;
        }

        CFURLRef url = CFURLCreateFromFileSystemRepresentation(kCFAllocatorDefault,
                                                                 (const UInt8 *)path,
                                                                 strlen(path),
                                                                 false);
        if (url == NULL) {
            rs_codesign_write_string(err, err_len, @"could not create code URL");
            return -1;
        }

        SecStaticCodeRef code = NULL;
        OSStatus status = SecStaticCodeCreateWithPath(url, kSecCSDefaultFlags, &code);
        CFRelease(url);
        if (status != errSecSuccess || code == NULL) {
            rs_codesign_write_status(err, err_len, status, NULL);
            if (code != NULL) {
                CFRelease(code);
            }
            return -1;
        }

        SecRequirementRef requirement_ref = NULL;
        if (requirement != NULL) {
            CFStringRef requirement_string = CFStringCreateWithCString(kCFAllocatorDefault,
                                                                         requirement,
                                                                         kCFStringEncodingUTF8);
            if (requirement_string == NULL) {
                rs_codesign_write_string(err, err_len, @"requirement is not valid UTF-8");
                CFRelease(code);
                return -1;
            }
            status = SecRequirementCreateWithString(requirement_string,
                                                    kSecCSDefaultFlags,
                                                    &requirement_ref);
            CFRelease(requirement_string);
            if (status != errSecSuccess || requirement_ref == NULL) {
                rs_codesign_write_status(err, err_len, status, NULL);
                if (requirement_ref != NULL) {
                    CFRelease(requirement_ref);
                }
                CFRelease(code);
                return -1;
            }
        }

        CFErrorRef error = NULL;
        status = SecStaticCodeCheckValidityWithErrors(code,
                                                      kSecCSCheckAllArchitectures |
                                                          kSecCSStrictValidate |
                                                          kSecCSCheckNestedCode |
                                                          kSecCSRestrictSymlinks |
                                                          kSecCSRestrictToAppLike,
                                                      requirement_ref,
                                                      &error);
        if (requirement_ref != NULL) {
            CFRelease(requirement_ref);
        }
        CFRelease(code);
        if (status != errSecSuccess) {
            rs_codesign_write_status(err, err_len, status, error);
            if (error != NULL) {
                CFRelease(error);
            }
            return -1;
        }
        if (error != NULL) {
            CFRelease(error);
        }
        return 0;
    }
}
