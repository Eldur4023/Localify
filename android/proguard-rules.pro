# Puente JS (addJavascriptInterface "AndroidBridge"): R8 no ve la reflexión del WebView.
-keep class dev.lux.desktop.MainActivity$AndroidBridge { *; }
-keepclassmembers class dev.lux.desktop.MainActivity$AndroidBridge {
    @android.webkit.JavascriptInterface <methods>;
}
# JNI: libluxapp.so exporta Java_dev_lux_desktop_LuxRuntime_{start,stop}; ni la clase ni los métodos se renombran.
-keep class dev.lux.desktop.LuxRuntime { *; }
-keepclasseswithmembernames class * { native <methods>; }
