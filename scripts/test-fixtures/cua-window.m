// An isolated blank window: the only actionable control writes a test marker.
#import <AppKit/AppKit.h>
#include <unistd.h>
@interface Target : NSObject
@property NSString *marker;
- (void)clicked:(NSButton *)sender;
@end
@implementation Target
- (void)clicked:(NSButton *)sender {
    [@"clicked" writeToFile:self.marker atomically:YES encoding:NSUTF8StringEncoding error:nil];
    sender.title = @"Clicked";
}
@end
int main(int argc, const char **argv) {
    @autoreleasepool {
        if (argc != 3) return 64;
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
        NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(100, 100, 420, 260)
            styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable backing:NSBackingStoreBuffered defer:NO];
        window.title = @"Zork Cua Isolated Test";
        window.releasedWhenClosed = NO;
        Target *target = [Target new]; target.marker = @(argv[1]);
        NSButton *button = [NSButton buttonWithTitle:@"Cua fixture click" target:target action:@selector(clicked:)];
        button.frame = NSMakeRect(100, 100, 220, 44);
        button.accessibilityLabel = @"Cua fixture click";
        [window.contentView addSubview:button];
        [NSApp finishLaunching];
        [window makeKeyAndOrderFront:nil];
        [NSApp activateIgnoringOtherApps:YES];
        NSDictionary *info = @{@"pid":@(getpid()), @"window_id":@(window.windowNumber)};
        [[NSJSONSerialization dataWithJSONObject:info options:0 error:nil] writeToFile:@(argv[2]) atomically:YES];
        [NSApp run];
    }
}
