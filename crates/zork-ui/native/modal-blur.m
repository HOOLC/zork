#import <AppKit/AppKit.h>
#import <QuartzCore/QuartzCore.h>
#import <dlfcn.h>
#import <math.h>

// A window-level compositor scrim, above the native titlebar. GPUI owns input.
@interface ZorkModalBlurView : NSView
@property(nonatomic, strong) CAShapeLayer *exclusionMask;
@property(nonatomic, weak) NSView *sourceView;
@property(nonatomic) CGRect previousCard;
@property(nonatomic) BOOL retiring;
@property(nonatomic) BOOL needsArrival;
@property(nonatomic) NSUInteger animationGeneration;
@end
@implementation ZorkModalBlurView
- (BOOL)isFlipped { return YES; }
- (NSView *)hitTest:(NSPoint)point { return nil; }
- (BOOL)acceptsFirstResponder { return NO; }
@end

// Read the displayed opacity before replacing an animation, including rapid reversals.
static void animateScrim(ZorkModalBlurView *view, float target) {
    float current = view.layer.presentationLayer ? view.layer.presentationLayer.opacity : view.layer.opacity;
    NSUInteger generation = ++view.animationGeneration;
    BOOL reduced = NSWorkspace.sharedWorkspace.accessibilityDisplayShouldReduceMotion;
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    view.layer.opacity = target;
    [view.layer removeAnimationForKey:@"zork-modal-opacity"];
    if (!reduced && fabsf(target - current) > 0.001f) {
        [CATransaction setCompletionBlock:^{
            if (view.retiring && view.animationGeneration == generation) [view removeFromSuperview];
        }];
        CABasicAnimation *animation = [CABasicAnimation animationWithKeyPath:@"opacity"];
        animation.fromValue = @(current);
        animation.toValue = @(target);
        animation.duration = (target > current ? 0.22 : 0.18) * fabsf(target - current);
        animation.timingFunction = [CAMediaTimingFunction functionWithControlPoints:0.2 :0 :0 :1];
        [view.layer addAnimation:animation forKey:@"zork-modal-opacity"];
    } else if (view.retiring) {
        [view removeFromSuperview];
    }
    [CATransaction commit];
}

static NSView *scrimHost(NSView *source) {
    // The titlebar is a sibling of the full-size content view, not its child.
    return source.window.contentView.superview ?: source.superview;
}

int zork_modal_blur_available(void *rawView) {
    if (![NSThread isMainThread] || NSWorkspace.sharedWorkspace.accessibilityDisplayShouldReduceTransparency) return 0;
    NSView *source = (__bridge NSView *)rawView;
    return scrimHost(source) != nil;
}
void *zork_modal_blur_create(void *rawView) {
    @autoreleasepool {
        if (![NSThread isMainThread] || NSWorkspace.sharedWorkspace.accessibilityDisplayShouldReduceTransparency) return NULL;
        NSView *source = (__bridge NSView *)rawView;
        NSView *host = scrimHost(source);
        if (!host) return NULL;
        for (NSView *sibling in host.subviews) {
            if ([sibling isKindOfClass:ZorkModalBlurView.class]) {
                ZorkModalBlurView *previous = (ZorkModalBlurView *)sibling;
                if (previous.sourceView == source && previous.retiring) {
                    previous.retiring = NO;
                    previous.needsArrival = YES;
                    previous.previousCard = CGRectNull;
                    ++previous.animationGeneration;
                    [host addSubview:previous positioned:NSWindowAbove relativeTo:nil];
                    return (__bridge_retained void *)previous;
                }
            }
        }
        ZorkModalBlurView *view = [[ZorkModalBlurView alloc] initWithFrame:host.bounds];
        view.sourceView = source;
        view.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
        view.wantsLayer = YES;
        view.layer.masksToBounds = YES;
        view.layer.opacity = 0;
        view.needsArrival = YES;
        view.layer.backgroundColor = [NSColor colorWithWhite:0 alpha:0.35].CGColor;
        view.exclusionMask = [CAShapeLayer layer];
        view.exclusionMask.fillRule = kCAFillRuleEvenOdd;
        view.layer.mask = view.exclusionMask;
        view.previousCard = CGRectNull;
        [host addSubview:view positioned:NSWindowAbove relativeTo:nil];
#if defined(ZORK_MODAL_TESTING)
        const char *capturePath = getenv("ZORK_BLUR_CAPTURE");
        if (capturePath) {
            NSString *path = [NSString stringWithUTF8String:capturePath];
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC), dispatch_get_main_queue(), ^{
                typedef CGImageRef (*Capture)(CGRect, CGWindowListOption, CGWindowID, CGWindowImageOption);
                Capture capture = (Capture)dlsym(RTLD_DEFAULT, "CGWindowListCreateImage");
                CGImageRef image = capture ? capture(CGRectNull, kCGWindowListOptionIncludingWindow, (CGWindowID)view.window.windowNumber, kCGWindowImageBoundsIgnoreFraming) : NULL;
                if (image) {
                    NSBitmapImageRep *bitmap = [[NSBitmapImageRep alloc] initWithCGImage:image];
                    [[bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}] writeToFile:path atomically:YES];
                    CGImageRelease(image);
                    NSLog(@"Zork blur capture saved");
                } else { NSLog(@"Zork own-window capture unavailable"); }
            });
        }
#endif
        return (__bridge_retained void *)view;
    }
}
void zork_modal_blur_update(void *handle, double x, double y, double width, double height, const double *curves, size_t curveCount) {
    @autoreleasepool {
        ZorkModalBlurView *view = (__bridge ZorkModalBlurView *)handle;
        if (!view.sourceView || !curves || !curveCount || width <= 0 || height <= 0) return;
        NSView *source = view.sourceView;
        NSView *host = view.superview;
        NSRect frame = host.bounds;
        if (host.subviews.lastObject != view) [host addSubview:view positioned:NSWindowAbove relativeTo:nil];
        // GPUI reports top-left coordinates; AppKit parents can be unflipped.
        NSRect localCard = NSMakeRect(NSMinX(source.bounds) + x,
            source.isFlipped ? NSMinY(source.bounds) + y : NSMaxY(source.bounds) - y - height,
            width, height);
        NSRect hostCard = [host convertRect:localCard fromView:source];
        CGRect card = CGRectMake(NSMinX(hostCard) - NSMinX(frame),
            host.isFlipped ? NSMinY(hostCard) - NSMinY(frame) : NSMaxY(frame) - NSMaxY(hostCard),
            NSWidth(hostCard), NSHeight(hostCard));
        if (NSEqualRects(view.frame, frame) && NSEqualRects(view.exclusionMask.frame, view.bounds)
            && CGRectEqualToRect(view.previousCard, card)) return;
        [CATransaction begin];
        [CATransaction setDisableActions:YES];
        view.frame = frame;
        view.layer.mask = view.exclusionMask;
        view.exclusionMask.frame = view.bounds;
        CGMutablePathRef mask = CGPathCreateMutable();
        CGPathAddRect(mask, NULL, NSRectToCGRect(view.bounds));
        // Leave the foreground card outside the dark scrim without an extra outline.
        // Rust supplies the exact shared smooth contour; AppKit only maps it
        // into compositor coordinates. No platform-specific corner formula.
        double sx = card.size.width / width, sy = card.size.height / height;
        CGPathMoveToPoint(mask, NULL, card.origin.x + curves[0] * sx, card.origin.y + curves[1] * sy);
        for (size_t i = 0; i < curveCount; ++i) {
            const double *c = curves + i * 8;
            CGPathAddCurveToPoint(mask, NULL, card.origin.x + c[2] * sx, card.origin.y + c[3] * sy,
                card.origin.x + c[4] * sx, card.origin.y + c[5] * sy,
                card.origin.x + c[6] * sx, card.origin.y + c[7] * sy);
        }
        CGPathCloseSubpath(mask);
        view.exclusionMask.path = mask;
        CGPathRelease(mask);
        view.previousCard = card;
        [CATransaction commit];
        if (view.needsArrival) { view.needsArrival = NO; animateScrim(view, 1); }
    }
}
void zork_modal_blur_destroy(void *handle) {
    @autoreleasepool {
        ZorkModalBlurView *view = (__bridge_transfer ZorkModalBlurView *)handle;
        view.retiring = YES;
        // The GPUI card has gone; fade a uniform scrim without leaving a bright hole.
        [CATransaction begin];
        [CATransaction setDisableActions:YES];
        view.layer.mask = nil;
        [CATransaction commit];
        animateScrim(view, 0);
    }
}
