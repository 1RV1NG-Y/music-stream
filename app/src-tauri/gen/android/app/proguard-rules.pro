# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# If your project uses WebView with JS, uncomment the following
# and specify the fully qualified class name to the JavaScript interface
# class:
#-keepclassmembers class fqcn.of.javascript.interface.for.webview {
#   public *;
#}

# Uncomment this to preserve the line number information for
# debugging stack traces.
#-keepattributes SourceFile,LineNumberTable

# If you keep the line number information, uncomment this to
# hide the original source file name.
#-renamesourcefileattribute SourceFile# reflection. R8 strips them in release builds, which crashes scanning on the first camera
# frame (NullPointerException in mlkit_vision_common). Keep ML Kit and its GMS internals.
-keep class com.google.mlkit.** { *; }
-keep class com.google.android.gms.internal.mlkit_** { *; }
-keep class com.google.android.gms.common.** { *; }
-keep class com.google.firebase.components.** { *; }
-dontwarn com.google.mlkit.**

# Tauri loads the plugin and calls commands by reflection; Rust resolves JNI by class name.
-keep class app.musicstream.LanSyncPlugin { *; }
-keep class app.musicstream.LanSyncRuntime { *; }
