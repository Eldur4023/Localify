// The Android shell: same Lux server as the desktop one (src/boot.cpp),
// but the "window" is a plain android.webkit.WebView in the Kotlin
// Activity (android/), which this library only hands a loopback port to.
// There is no `window` hook wired here -- menu/tray/minimize are
// meaningless on a phone, and the `window` module already no-ops (or
// reports "no native window running") for any hook left unset.
#include "boot.hpp"

#include <android/log.h>
#include <jni.h>

#include <memory>

namespace {
std::unique_ptr<LuxServer> g_server;
}

// Called once from LuxRuntime.start(filesDir). Returns the loopback port the
// WebView should open, or -1 if the app failed to compile (details in logcat).
extern "C" JNIEXPORT jint JNICALL
Java_dev_lux_desktop_LuxRuntime_start(JNIEnv* env, jclass, jstring data_dir) {
    if (g_server) return -1;
    const char* dir = env->GetStringUTFChars(data_dir, nullptr);
    std::filesystem::path work_dir(dir);
    env->ReleaseStringUTFChars(data_dir, dir);
    // La app (app/movil.lux, shell/web-audio.js) adapta su reproducción a
    // este flag: sin motor nativo, quien suena es el <audio> del WebView.
    setenv("LUX_LOCALIFY_MOBILE", "1", 1);
    try {
        g_server = std::make_unique<LuxServer>(work_dir);
        return g_server->start();
    } catch (const std::exception& e) {
        __android_log_print(ANDROID_LOG_ERROR, "luxdesktop", "%s", e.what());
        g_server.reset();
        return -1;
    }
}

extern "C" JNIEXPORT void JNICALL
Java_dev_lux_desktop_LuxRuntime_stop(JNIEnv*, jclass) {
    if (g_server) g_server->stop();
    g_server.reset();
}
