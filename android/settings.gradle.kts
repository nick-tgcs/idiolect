// The Android side of idiolect lives in its own Gradle build (the cargo workspace
// is the parent dir). The FFI bindings module is pure-JVM (UniFFI + JNA), so it
// builds and tests without the Android SDK; the IME app module (AGP) is added in a
// later increment and depends on `:ffi`.
pluginManagement {
    repositories {
        gradlePluginPortal()
        google()
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "idiolect-android"

include(":ffi")
