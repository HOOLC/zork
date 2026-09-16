#!/usr/bin/env python3
"""Exercise the packaged GUI's signing contract against macOS UserNotifications.

Uses unique temporary app identities and provisional authorization (no prompt).
A future notification is accepted and then removed before it can be displayed.
No installed Zork app, real notification permissions or client data are changed.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/lib'))
from test_app_slot import stop_app, unregister_app


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


PROBE = r'''
#import <AppKit/AppKit.h>
#import <Security/Security.h>
#import <UserNotifications/UserNotifications.h>

int main(int argc, const char **argv) {
    @autoreleasepool {
        NSString *output = [NSString stringWithUTF8String:argv[1]];
        [NSApplication sharedApplication];
        SecCodeRef code = NULL;
        CFDictionaryRef information = NULL;
        if (SecCodeCopySelf(kSecCSDefaultFlags, &code) != errSecSuccess ||
            SecCodeCopySigningInformation(code, kSecCSSigningInformation, &information) != errSecSuccess) return 2;
        NSMutableDictionary *result = [@{
            @"bundle": NSBundle.mainBundle.bundleIdentifier,
            @"signed": ((__bridge NSDictionary *)information)[(__bridge NSString *)kSecCodeInfoIdentifier],
            @"granted": @NO, @"accepted": @NO
        } mutableCopy];
        CFRelease(information);
        CFRelease(code);
        void (^finish)(NSError *) = ^(NSError *error) {
            result[@"error_code"] = @(error.code);
            result[@"error_domain"] = error.domain ?: @"";
            [[NSJSONSerialization dataWithJSONObject:result options:NSJSONWritingPrettyPrinted error:nil]
                writeToFile:output atomically:YES];
            exit(0);
        };
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 12 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{ exit(3); });
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 200 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
            UNUserNotificationCenter *center = UNUserNotificationCenter.currentNotificationCenter;
            [center requestAuthorizationWithOptions:UNAuthorizationOptionProvisional completionHandler:^(BOOL granted, NSError *error) {
                result[@"granted"] = @(granted);
                if (error || !granted) { finish(error); return; }
                UNMutableNotificationContent *content = [UNMutableNotificationContent new];
                content.title = @"Zork notification identity test";
                content.body = @"Temporary notification; removed before delivery.";
                UNNotificationRequest *request = [UNNotificationRequest requestWithIdentifier:@"identity-test" content:content
                    trigger:[UNTimeIntervalNotificationTrigger triggerWithTimeInterval:60 repeats:NO]];
                [UNUserNotificationCenter.currentNotificationCenter addNotificationRequest:request withCompletionHandler:^(NSError *error) {
                    if (error) { finish(error); return; }
                    result[@"accepted"] = @YES;
                    UNUserNotificationCenter *cleanup = UNUserNotificationCenter.currentNotificationCenter;
                    [cleanup removePendingNotificationRequestsWithIdentifiers:@[@"identity-test"]];
                    [cleanup getPendingNotificationRequestsWithCompletionHandler:^(NSArray<UNNotificationRequest *> *pending) {
                        result[@"pending_after_cleanup"] = @(pending.count);
                        finish(nil);
                    }];
                }];
            }];
        });
        [NSApp run];
        return 0;
    }
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'artifacts/notification-identity')
    args = parser.parse_args()
    if sys.platform != 'darwin':
        raise SystemExit('Requires macOS with a logged-in desktop session')
    args.output.mkdir(parents=True, exist_ok=True)
    packager = load('notification_packager', ROOT / 'scripts/package-macos-client.py')
    runtime = load('notification_signer', ROOT / 'scripts/lib/browser-runtime.py')
    scratch = ROOT / '.tmp'
    scratch.mkdir(exist_ok=True)
    report = {}
    with tempfile.TemporaryDirectory(prefix='notification-identity-', dir=scratch) as directory:
        root = Path(directory)
        source = root / 'probe.m'
        source.write_text(PROBE)
        binary = root / 'probe'
        subprocess.run(['clang', '-fobjc-arc', '-fblocks', str(source), '-framework', 'AppKit',
                        '-framework', 'Security', '-framework', 'UserNotifications', '-o', str(binary)], check=True)
        for mode in ('old_script_entry', 'native_entry'):
            app = root / (mode + '.app')
            mac = app / 'Contents/MacOS'
            mac.mkdir(parents=True)
            resources = app / 'Contents/Resources'
            resources.mkdir()
            shutil.copyfile(ROOT / 'crates/zork-ui/assets/app/Zork.icns', resources / 'Zork.icns')
            shutil.copy2(binary, mac / 'zork-gui')
            info = packager.app_info('1.0')
            info['CFBundleIdentifier'] = 'surf.zork.notification-test.' + uuid.uuid4().hex
            info['CFBundleName'] = info['CFBundleDisplayName'] = 'Zork notification identity test'
            if mode == 'old_script_entry':
                info['CFBundleExecutable'] = 'ZorkLauncher'
                launcher = mac / 'ZorkLauncher'
                launcher.write_text('#!/bin/sh\nexec "$(dirname "$0")/zork-gui" "$@"\n')
                launcher.chmod(0o755)
            (app / 'Contents/Info.plist').write_bytes(plistlib.dumps(info))
            try:
                if mode == 'old_script_entry':
                    runtime.sign(mac / 'zork-gui', '-')
                    runtime.sign(app, '-')
                    # The old package passed this check despite being unable to notify.
                    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True)
                    try:
                        packager.verify_app(app)
                    except RuntimeError:
                        pass
                    else:
                        raise AssertionError('Packaging accepted a script as the GUI main executable')
                else:
                    packager.sign_app(app, runtime.sign, '-')
                tool = '/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister'
                subprocess.run([tool, '-f', str(app)], check=True)
                result_file = root / (mode + '.json')
                subprocess.run(['open', '-n', '-W', '-g', str(app), '--args', str(result_file)], check=True, timeout=20)
                result = json.loads(result_file.read_text())
                report[mode] = result
                if mode == 'old_script_entry':
                    assert result['signed'] != result['bundle'], result
                    assert result['error_domain'] == 'UNErrorDomain' and result['error_code'] == 1, result
                    assert not result['granted'] and not result['accepted'], result
                else:
                    assert result['signed'] == result['bundle'], result
                    assert result['granted'] and result['accepted'] and result['error_code'] == 0, result
                    assert result['pending_after_cleanup'] == 0, result
            finally:
                stop_app(app)
                unregister_app(app)
                (args.output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print('PASS old entry reproduces UNErrorDomain 1; native entry authorizes, accepts and removes the test notification')


if __name__ == '__main__':
    main()
