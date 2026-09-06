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
    return [self demo_initWithFrame:frame configuration:config];
}
@end
__attribute__((constructor)) static void install(void) {
    @autoreleasepool {
        if (!NSProcessInfo.processInfo.environment[@"REPOMON_WEBVIEW_LOG"] ||
            ![NSProcessInfo.processInfo.processName isEqualToString:@"repomon-desktop"]) return;
        Method original = class_getInstanceMethod(WKWebView.class, @selector(initWithFrame:configuration:));
        Method replacement = class_getInstanceMethod(WKWebView.class, @selector(demo_initWithFrame:configuration:));
        method_exchangeImplementations(original, replacement);
        record(@{@"event": @"probe-loaded"});
    }
}
