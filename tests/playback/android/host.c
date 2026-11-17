// The check runs inside an Activity, with a VM and a process-lifetime Context.
#include <jni.h>
#include <android/log.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <unistd.h>

extern void hxn_audio_check_context(void *vm, void *context);
#ifdef HXN_CHECK_CPP
extern void hxcpp_main(void);
#else
extern int main(int argc, char **argv);
#endif

static void *forward_output(void *unused) {
    int descriptor = (int)(intptr_t)unused;
    char bytes[1024];
    ssize_t count;
    while ((count = read(descriptor, bytes, sizeof(bytes) - 1)) > 0) {
        bytes[count] = 0;
        __android_log_write(ANDROID_LOG_INFO, "hxn-audio-check", bytes);
    }
    close(descriptor);
    return NULL;
}
JNIEXPORT void JNICALL Java_hxn_audio_cli_CheckActivity_runCheck(JNIEnv *env, jclass activity, jobject application) {
    JavaVM *vm = NULL;
    if ((*env)->GetJavaVM(env, &vm) != JNI_OK) return;
    jobject context = (*env)->NewGlobalRef(env, application);
    if (!context) return;
    int descriptors[2];
    int has_forwarding = 0;
    pthread_t forwarding;
    if (pipe(descriptors) == 0) {
        dup2(descriptors[1], STDOUT_FILENO);
        dup2(descriptors[1], STDERR_FILENO);
        close(descriptors[1]);
        setvbuf(stdout, NULL, _IONBF, 0);
        setvbuf(stderr, NULL, _IONBF, 0);
        if (pthread_create(&forwarding, NULL, forward_output, (void *)(intptr_t)descriptors[0]) == 0) has_forwarding = 1;
        else close(descriptors[0]);
    }
    hxn_audio_check_context(vm, context);
#ifdef HXN_CHECK_CPP
    hxcpp_main();
#else
    main(0, NULL);
#endif
    fflush(stdout); fflush(stderr);
    close(STDOUT_FILENO); close(STDERR_FILENO);
    if (has_forwarding) pthread_join(forwarding, NULL);
    // The dedicated check process retains its Context until it is removed.
}
