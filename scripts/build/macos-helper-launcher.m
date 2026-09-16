// Register the background app's identity before replacing this process with
// its runtime. exec preserves PID, arguments, descriptors and signal handling.
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <limits.h>
#include <mach-o/dyld.h>
#include <unistd.h>
#include <fcntl.h>
#include <time.h>

static void startup_mark(const char *name) {
    const char *directory = getenv("ZORK_STARTUP_TRACE");
    if (!directory) return;
    struct timespec clock;
    if (clock_gettime(CLOCK_MONOTONIC, &clock) != 0) return;
    char path[PATH_MAX];
    int length = snprintf(path, sizeof(path), "%s/startup-%d.jsonl", directory, getpid());
    if (length < 0 || (size_t)length >= sizeof(path)) return;
    int fd = open(path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0600);
    if (fd < 0) return;
    dprintf(fd, "{\"pid\":%d,\"mark\":\"%s\",\"ns\":%llu}\n", getpid(), name,
            (unsigned long long)clock.tv_sec * 1000000000ULL + clock.tv_nsec);
    close(fd);
}

int main(int argc, char **argv) {
    startup_mark("helper.main");
    // A CLI compatibility symlink otherwise makes NSBundle select the outer
    // desktop app. Re-exec the canonical launcher before AppKit caches it.
    char executable[PATH_MAX], resolved[PATH_MAX];
    uint32_t size = sizeof(executable);
    if (_NSGetExecutablePath(executable, &size) != 0 || !realpath(executable, resolved)) {
        fputs("Cannot resolve Zork helper executable\n", stderr);
        return 1;
    }
    if (strcmp(executable, resolved) != 0) {
        argv[0] = resolved;
        execv(resolved, argv);
        perror("Unable to enter Zork helper bundle");
        return 1;
    }
    @autoreleasepool {
        startup_mark("helper.canonical");
        NSBundle *bundle = NSBundle.mainBundle;
        NSString *name = [bundle objectForInfoDictionaryKey:@"ZorkRuntimeExecutable"];
        if (![name isKindOfClass:NSString.class] || name.length == 0 ||
            ![name.lastPathComponent isEqualToString:name]) {
            fputs("Missing Zork runtime executable in helper bundle\n", stderr);
            return 1;
        }
        NSString *runtime = [[bundle.bundlePath stringByAppendingPathComponent:@"Contents/MacOS"]
                             stringByAppendingPathComponent:name];
        startup_mark("helper.before_registration");
        BOOL registered = NO;
        if ([[bundle objectForInfoDictionaryKey:@"LSBackgroundOnly"] boolValue]) {
            // Background runtimes need a process identity, but no Cocoa app or
            // window system. Keep the declared activation policy and bundle.
            ProcessSerialNumber current;
            // Check in using the bundle's LSBackgroundOnly policy. Transforming
            // an already-background process returns paramErr on current macOS.
            registered = GetCurrentProcess(&current) == noErr;
        }
        if (!registered) {
            startup_mark("helper.appkit_fallback");
            // Browser helpers retain their existing AppKit lifecycle. This is
            // also a compatibility fallback if process registration is unavailable.
            [NSApplication sharedApplication];
            [NSApp finishLaunching];
        }
        startup_mark("helper.identity_ready");
        argv[0] = (char *)runtime.fileSystemRepresentation;
        execv(runtime.fileSystemRepresentation, argv);
        perror("Unable to launch Zork runtime");
        return 1;
    }
}
