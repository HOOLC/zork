// LaunchServices owns this app's TCC identity; the embedded child inherits it.
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <CommonCrypto/CommonDigest.h>
#include <sys/file.h>
#include <fcntl.h>
#include <signal.h>
#include <errno.h>
#include <sys/stat.h>
#include <unistd.h>

static NSPipe *lifetime;
static NSTask *driver;
static void finish(int signal) { _exit(128 + signal); } // closes lifetime pipe even on SIGKILL
static void writeJSON(NSDictionary *value, NSString *path) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:value options:0 error:nil];
    [data writeToFile:path options:NSDataWritingAtomic error:nil];
}
int main(int argc, const char **argv) {
    @autoreleasepool {
        NSBundle *bundle = NSBundle.mainBundle;
        NSString *bundlePath = bundle.bundlePath.stringByResolvingSymlinksInPath;
        NSData *bytes = [bundlePath dataUsingEncoding:NSUTF8StringEncoding];
        unsigned char hash[CC_SHA256_DIGEST_LENGTH];
        CC_SHA256(bytes.bytes, (CC_LONG)bytes.length, hash);
        NSString *root = [NSString stringWithFormat:@"/tmp/zork-cua-%u-%02x%02x%02x%02x%02x%02x%02x%02x",
                          getuid(), hash[0], hash[1], hash[2], hash[3], hash[4], hash[5], hash[6], hash[7]];
        NSString *socket = [root stringByAppendingPathComponent:@"driver.sock"];
        NSString *status = [root stringByAppendingPathComponent:@"status.json"];
        NSDictionary *endpoint = @{@"socket":socket, @"status":status, @"bundle_id":bundle.bundleIdentifier,
                                  @"driver_version":[bundle objectForInfoDictionaryKey:@"ZorkCuaVersion"]};
        if (argc == 2 && strcmp(argv[1], "--endpoint") == 0) {
            NSData *data = [NSJSONSerialization dataWithJSONObject:endpoint options:0 error:nil];
            fwrite(data.bytes, 1, data.length, stdout); return 0;
        }
        umask(0077);
        if (mkdir(root.fileSystemRepresentation, 0700) != 0 && errno != EEXIST) return 1;
        struct stat info;
        if (lstat(root.fileSystemRepresentation, &info) || !S_ISDIR(info.st_mode) ||
            info.st_uid != getuid() || (info.st_mode & 0077)) return 1;
        int lock = open([root stringByAppendingPathComponent:@"host.lock"].fileSystemRepresentation,
                        O_CREAT | O_RDWR | O_NOFOLLOW | O_CLOEXEC, 0600);
        if (lock < 0 || flock(lock, LOCK_EX | LOCK_NB)) return 1;
        // Only this owning host, while holding its lock, may remove a stale endpoint.
        unlink(socket.fileSystemRepresentation);
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
        [NSApp finishLaunching];
        BOOL request = argc == 2 && strcmp(argv[1], "--request-permissions") == 0;
        if (request) {
            AXIsProcessTrustedWithOptions((__bridge CFDictionaryRef)@{(__bridge NSString *)kAXTrustedCheckOptionPrompt:@YES});
            CGRequestScreenCaptureAccess();
        }
        BOOL accessibility = AXIsProcessTrusted();
        BOOL screen = CGPreflightScreenCaptureAccess();
        if (!accessibility || !screen) {
            writeJSON(@{@"state":@"permissions_required", @"accessibility":@(accessibility),
                        @"screen_recording":@(screen), @"bundle_id":bundle.bundleIdentifier}, status);
            return 77;
        }
        driver = [NSTask new];
        driver.executableURL = [NSURL fileURLWithPath:[bundlePath stringByAppendingPathComponent:@"Contents/MacOS/cua-driver"]];
        driver.arguments = @[@"serve", @"--embedded", @"--socket", socket];
        // Do not inherit any ambient cua profile, network listener, or standalone state.
        driver.environment = @{@"PATH":@"/usr/bin:/bin", @"HOME":root, @"TMPDIR":root,
                               @"CUA_DRIVER_PERMISSION_MODE":@"standard", @"CUA_DRIVER_EMBEDDED":@"1", @"CUA_DRIVER_HOST_BUNDLE_ID":bundle.bundleIdentifier,
                               @"CUA_DRIVER_PARENT_LIVENESS_STDIN":@"1",
                               @"CUA_DRIVER_RS_TELEMETRY_ENABLED":@"false", @"CUA_DRIVER_RS_UPDATE_CHECK":@"false"};
        lifetime = [NSPipe pipe];
        driver.standardInput = lifetime;
        NSFileHandle *log = [NSFileHandle fileHandleForWritingAtPath:@"/dev/null"];
        driver.standardOutput = log; driver.standardError = log;
        NSError *error = nil;
        if (![driver launchAndReturnError:&error]) {
            writeJSON(@{@"state":@"launch_failed", @"error":error.localizedDescription}, status); return 1;
        }
        [lifetime.fileHandleForReading closeFile];
        writeJSON(@{@"state":@"started", @"host_pid":@(getpid()), @"driver_pid":@(driver.processIdentifier),
                    @"accessibility":@YES, @"screen_recording":@YES, @"bundle_id":bundle.bundleIdentifier}, status);
        signal(SIGTERM, finish); signal(SIGINT, finish);
        driver.terminationHandler = ^(NSTask *task) { exit(task.terminationStatus); };
        [NSApp run];
        [lifetime.fileHandleForWriting closeFile];
        [driver waitUntilExit];
        unlink(socket.fileSystemRepresentation);
    }
    return 0;
}
