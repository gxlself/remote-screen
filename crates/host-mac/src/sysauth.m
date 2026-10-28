#import <Foundation/Foundation.h>
#import <OpenDirectory/OpenDirectory.h>
#import <SystemConfiguration/SystemConfiguration.h>

#include <stdint.h>
#include <stddef.h>
#include <string.h>

static void rs_sysauth_clear(char *buffer, size_t length) {
    if (buffer != NULL && length != 0) {
        buffer[0] = '\0';
    }
}

static void rs_sysauth_write_string(char *buffer, size_t length, NSString *value) {
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

static void rs_sysauth_write_error(char *buffer, size_t length, NSError *error) {
    if (error == nil) {
        rs_sysauth_write_string(buffer, length, @"OpenDirectory error (code 0)");
        return;
    }
    NSString *description = error.localizedDescription;
    if (description.length == 0) {
        description = @"OpenDirectory error";
    }
    NSString *message = [NSString stringWithFormat:@"%@ (code %ld)", description, (long)error.code];
    rs_sysauth_write_string(buffer, length, message);
}

// OpenDirectory's credential errors are documented in odconstants.h.  These
// are the rejection classes that are safe to collapse into WrongPassword.  In
// particular, method/server failures stay Unavailable so an outage is not
// counted as a password attempt.
static BOOL rs_sysauth_is_credential_error(NSInteger code) {
    return code == kODErrorCredentialsInvalid ||
           (code >= kODErrorCredentialsAccountNotFound &&
            code <= kODErrorCredentialsAccountLocked) ||
           (code >= kODErrorCredentialsPasswordExpired &&
            code <= kODErrorCredentialsPasswordUnrecoverable) ||
           (code >= kODErrorCredentialsInvalidLogonHours &&
            code <= kODErrorCredentialsInvalidComputer);
}

static ODNode *rs_sysauth_node(NSError **error) {
    return [ODNode nodeWithSession:[ODSession defaultSession]
                               type:kODNodeTypeAuthentication
                              error:error];
}

static NSString *rs_sysauth_string_attribute(ODRecord *record, ODAttributeType attribute) {
    NSArray *values = [record valuesForAttribute:attribute error:NULL];
    for (id value in values) {
        if ([value isKindOfClass:[NSString class]]) {
            return value;
        }
    }
    return nil;
}

int32_t rs_sysauth_lookup(const char *account,
                          char *short_name,
                          size_t short_len,
                          char *full_name,
                          size_t full_len,
                          int32_t *is_admin,
                          int32_t *is_console,
                          char *msg,
                          size_t msg_len) {
    @autoreleasepool {
        rs_sysauth_clear(short_name, short_len);
        rs_sysauth_clear(full_name, full_len);
        rs_sysauth_clear(msg, msg_len);
        if (is_admin != NULL) {
            *is_admin = 0;
        }
        if (is_console != NULL) {
            *is_console = 0;
        }
        if (account == NULL) {
            rs_sysauth_write_string(msg, msg_len, @"invalid account");
            return -1;
        }

        NSString *account_string = [[NSString alloc] initWithUTF8String:account];
        if (account_string == nil || account_string.length == 0) {
            return 0;
        }

        NSError *node_error = nil;
        ODNode *node = rs_sysauth_node(&node_error);
        if (node == nil) {
            rs_sysauth_write_error(msg, msg_len, node_error);
            return -1;
        }

        ODRecord *user = [node recordWithRecordType:kODRecordTypeUsers
                                               name:account_string
                                         attributes:@[ kODAttributeTypeRecordName, kODAttributeTypeFullName ]
                                              error:NULL];
        if (user == nil) {
            NSError *query_error = nil;
            ODQuery *query = [ODQuery queryWithNode:node
                                     forRecordTypes:kODRecordTypeUsers
                                          attribute:kODAttributeTypeFullName
                                          matchType:kODMatchEqualTo
                                        queryValues:account_string
                                     returnAttributes:@[ kODAttributeTypeFullName ]
                                      maximumResults:2
                                               error:&query_error];
            if (query == nil) {
                rs_sysauth_write_error(msg, msg_len, query_error);
                return -1;
            }
            NSArray *results = [query resultsAllowingPartial:NO error:&query_error];
            if (results == nil) {
                rs_sysauth_write_error(msg, msg_len, query_error);
                return -1;
            }
            // A full name is only unambiguous when exactly one record matches.
            if (results.count != 1) {
                return 0;
            }
            user = results.firstObject;
        }

        NSString *short_string = user.recordName;
        if (short_string.length == 0) {
            rs_sysauth_write_string(msg, msg_len, @"user record has no short name");
            return -1;
        }
        NSString *full_string = rs_sysauth_string_attribute(user, kODAttributeTypeFullName);
        if (full_string.length == 0) {
            full_string = short_string;
        }
        rs_sysauth_write_string(short_name, short_len, short_string);
        rs_sysauth_write_string(full_name, full_len, full_string);

        NSError *group_error = nil;
        ODRecord *admin_group = [node recordWithRecordType:kODRecordTypeGroups
                                                       name:@"admin"
                                                 attributes:nil
                                                      error:&group_error];
        if (group_error != nil && admin_group == nil) {
            rs_sysauth_write_error(msg, msg_len, group_error);
            return -1;
        }
        if (admin_group != nil) {
            NSError *membership_error = nil;
            BOOL member = [admin_group isMemberRecord:user error:&membership_error];
            if (membership_error != nil) {
                rs_sysauth_write_error(msg, msg_len, membership_error);
                return -1;
            }
            if (is_admin != NULL) {
                *is_admin = member ? 1 : 0;
            }
        }

        CFStringRef console_ref = SCDynamicStoreCopyConsoleUser(NULL, NULL, NULL);
        NSString *console_string = CFBridgingRelease(console_ref);
        if (is_console != NULL && console_string != nil &&
            ![console_string isEqualToString:@"loginwindow"] &&
            [console_string isEqualToString:short_string]) {
            *is_console = 1;
        }
        return 1;
    }
}

int32_t rs_sysauth_verify(const char *short_name,
                          const uint8_t *password,
                          size_t password_len,
                          char *msg,
                          size_t msg_len) {
    @autoreleasepool {
        rs_sysauth_clear(msg, msg_len);
        if (short_name == NULL || (password == NULL && password_len != 0)) {
            return 1;
        }

        NSString *short_string = [[NSString alloc] initWithUTF8String:short_name];
        NSString *password_string = [[NSString alloc] initWithBytes:password
                                                               length:password_len
                                                             encoding:NSUTF8StringEncoding];
        if (short_string == nil || password_string == nil) {
            password_string = nil;
            return 1;
        }

        NSError *node_error = nil;
        ODNode *node = rs_sysauth_node(&node_error);
        if (node == nil) {
            password_string = nil;
            rs_sysauth_write_error(msg, msg_len, node_error);
            return -1;
        }
        NSError *record_error = nil;
        ODRecord *user = [node recordWithRecordType:kODRecordTypeUsers
                                               name:short_string
                                         attributes:nil
                                              error:&record_error];
        if (user == nil) {
            password_string = nil;
            if (record_error == nil || rs_sysauth_is_credential_error(record_error.code)) {
                return 1;
            }
            rs_sysauth_write_error(msg, msg_len, record_error);
            return -1;
        }

        NSError *verify_error = nil;
        BOOL verified = [user verifyPassword:password_string error:&verify_error];
        // Explicitly release the Objective-C password object before handling
        // the result, so it cannot outlive this authentication attempt.
        password_string = nil;
        if (verified) {
            return 0;
        }
        if (verify_error == nil) {
            return 1;
        }
        if (verify_error != nil && rs_sysauth_is_credential_error(verify_error.code)) {
            return 1;
        }
        rs_sysauth_write_error(msg, msg_len, verify_error);
        return -1;
    }
}
