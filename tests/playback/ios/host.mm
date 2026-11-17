#import <UIKit/UIKit.h>
#import <AVFAudio/AVFAudio.h>
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#if HXN_CHECK_CPP
extern "C" const char *hxRunLibrary();
extern "C" void hxcpp_set_top_of_stack();
#endif

@interface HxnAudioCheckDelegate : UIResponder <UIApplicationDelegate>
@property(strong, nonatomic) UIWindow *window;
@property(nonatomic) BOOL started;
@end
@implementation HxnAudioCheckDelegate
- (BOOL)application:(UIApplication *)app didFinishLaunchingWithOptions:(NSDictionary *)options {
    self.window = [[UIWindow alloc] initWithFrame:UIScreen.mainScreen.bounds];
    UIViewController *controller = [UIViewController new];
    controller.view.backgroundColor = UIColor.systemBackgroundColor;
    UILabel *label = [[UILabel alloc] initWithFrame:CGRectMake(20, 100, 350, 100)];
    label.text = @"hxn audio device check";
    label.textColor = UIColor.labelColor;
    [controller.view addSubview:label];
    self.window.rootViewController = controller;
    [self.window makeKeyAndVisible];
    return YES;
}
- (void)applicationDidBecomeActive:(UIApplication *)app {
    if (self.started) return;
    self.started = YES;
    NSError *error = nil;
    AVAudioSession *session = AVAudioSession.sharedInstance;
    if (![session setCategory:AVAudioSessionCategoryPlayback error:&error] || ![session setActive:YES error:&error]) {
        fprintf(stderr, "audio playback failed: %s\n", error.localizedDescription.UTF8String);
        exit(1);
    }
    printf("audio media volume: %.3f\n", session.outputVolume);
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
#if HXN_CHECK_CPP
        hxcpp_set_top_of_stack();
        const char *failure = hxRunLibrary();
        if (failure) { fprintf(stderr, "audio playback failed: %s\n", failure); exit(1); }
#else
        NSString *path = [NSBundle.mainBundle.privateFrameworksPath stringByAppendingPathComponent:@"AudioAsh.framework/AudioAsh"];
        void *image = dlopen(path.UTF8String, RTLD_NOW | RTLD_LOCAL);
        int (*entry)(int, char **) = image ? (int (*)(int, char **))dlsym(image, "main") : nullptr;
        if (!entry) { fprintf(stderr, "audio playback failed: %s\n", dlerror()); exit(1); }
        char *arguments[] = {(char *)NSBundle.mainBundle.executablePath.UTF8String, nullptr};
        int status = entry(1, arguments);
        if (status) exit(status);
#endif
        fflush(stdout);
        fflush(stderr);
    });
}
@end
int main(int argc, char **argv) {
    setvbuf(stdout, nullptr, _IONBF, 0);
    setvbuf(stderr, nullptr, _IONBF, 0);
    @autoreleasepool { return UIApplicationMain(argc, argv, nil, NSStringFromClass(HxnAudioCheckDelegate.class)); }
}
