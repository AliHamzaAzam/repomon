#import <Cocoa/Cocoa.h>
#import <WebKit/WebKit.h>
#import <objc/runtime.h>

// Recorder-only diagnostics. Loaded into the copied executable, never installed or bundled.

static void record(NSDictionary *event) {
    NSString *path = NSProcessInfo.processInfo.environment[@"REPOMON_WEBVIEW_LOG"];
    if (!path) return;
    NSMutableDictionary *row = [event mutableCopy];
    row[@"pid"] = @(getpid());
    row[@"time"] = @([NSDate date].timeIntervalSince1970);
    NSData *data = [NSJSONSerialization dataWithJSONObject:row options:NSJSONWritingFragmentsAllowed error:nil];
    FILE *file = fopen(path.UTF8String, "a");
    if (file && data) { fwrite(data.bytes, 1, data.length, file); fputc('\n', file); }
    if (file) fclose(file);
}
@interface DemoProbe : NSObject <WKScriptMessageHandler>
@end
@implementation DemoProbe
- (void)userContentController:(WKUserContentController *)controller didReceiveScriptMessage:(WKScriptMessage *)message {
    record(@{@"event": @"javascript", @"body": message.body});
}
@end

@interface WKWebView (DemoProbe)
- (instancetype)demo_initWithFrame:(NSRect)frame configuration:(WKWebViewConfiguration *)config;
@end
@implementation WKWebView (DemoProbe)
- (instancetype)demo_initWithFrame:(NSRect)frame configuration:(WKWebViewConfiguration *)config {
    NSString *source = [NSString stringWithContentsOfFile:NSProcessInfo.processInfo.environment[@"REPOMON_WEBVIEW_SCRIPT"] encoding:NSUTF8StringEncoding error:nil];
    if (!source) {
        record(@{@"event": @"probe-error", @"message": @"Missing diagnostic script"});
        return [self demo_initWithFrame:frame configuration:config];
    }
    [config.userContentController addScriptMessageHandler:[DemoProbe new] name:@"demoProbe"];
    [config.userContentController addUserScript:[[WKUserScript alloc] initWithSource:source injectionTime:WKUserScriptInjectionTimeAtDocumentStart forMainFrameOnly:YES]];
    record(@{@"event": @"webview-init", @"home": NSHomeDirectory(), @"library": NSSearchPathForDirectoriesInDomains(NSLibraryDirectory, NSUserDomainMask, YES)});
    WKWebView *view = [self demo_initWithFrame:frame configuration:config];
    NSString *commandPath = NSProcessInfo.processInfo.environment[@"REPOMON_WEBVIEW_TOUR_COMMAND"];
    if (commandPath) {
        __block NSString *lastCommand = nil;
        __weak WKWebView *weakView = view;
        [NSTimer scheduledTimerWithTimeInterval:0.2 repeats:YES block:^(NSTimer *timer) {
            WKWebView *webview = weakView;
            if (!webview) { [timer invalidate]; return; }
            NSString *phase = [NSString stringWithContentsOfFile:commandPath encoding:NSUTF8StringEncoding error:nil];
            if (!phase || [phase isEqualToString:lastCommand]) return;
            NSSet *allowed = [NSSet setWithArray:@[@"opening", @"tour", @"workflow-start", @"workflow-answer", @"workflow-switch", @"workflow-resume", @"workflow-terminal", @"workflow-tui", @"workflow-quit"]];
            if (![allowed containsObject:phase]) return;
            lastCommand = phase;
            if ([phase isEqualToString:@"workflow-quit"]) {
                record(@{@"event": @"tour-complete", @"phase": phase});
                dispatch_after(dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC), dispatch_get_main_queue(), ^{ [NSApp terminate:nil]; });
                return;
            }
            if ([phase isEqualToString:@"opening"] || [phase isEqualToString:@"workflow-resume"]) {
                NSWindow *window = webview.window;
                NSRect visible = window.screen.visibleFrame;
                [window setFrame:NSMakeRect(visible.origin.x, NSMaxY(visible) - 900, 1440, 900) display:YES];
                [NSApp activateIgnoringOtherApps:YES];
                [window makeKeyAndOrderFront:nil];
            }
            NSString *js = [NSString stringWithFormat:@"window.repomonDemoTour.run('%@'); undefined", phase];
            [webview evaluateJavaScript:js completionHandler:^(id result, NSError *error) {
                if (error) record(@{@"event": @"tour-error", @"phase": phase, @"message": error.localizedDescription});
            }];
        }];
    }
    return view;
}
@end
__attribute__((constructor)) static void install(void) {
    @autoreleasepool {
        if (!NSProcessInfo.processInfo.environment[@"REPOMON_WEBVIEW_LOG"] ||
            ![NSProcessInfo.processInfo.processName hasPrefix:@"repomon-demo-"]) return;
        Method original = class_getInstanceMethod(WKWebView.class, @selector(initWithFrame:configuration:));
        Method replacement = class_getInstanceMethod(WKWebView.class, @selector(demo_initWithFrame:configuration:));
        method_exchangeImplementations(original, replacement);
        record(@{@"event": @"probe-loaded"});
    }
}
