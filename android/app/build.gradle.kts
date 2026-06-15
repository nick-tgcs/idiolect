// The idiolect Android IME app. Depends on the pure-JVM `:ffi` bindings module for
// the Rust core. JVM unit tests (`testDebugUnitTest`) run the IME logic on the host
// JVM (no emulator); Robolectric covers the thin Android-framework seams.
plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
}

android {
    namespace = "org.idiolect.android"
    compileSdk = 35

    defaultConfig {
        applicationId = "org.idiolect.android"
        // foregroundServiceType=microphone needs API 29; GrapheneOS runs current
        // Android, so a modern floor is fine.
        minSdk = 29
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
        unitTests.isIncludeAndroidResources = true
    }

    buildTypes {
        named("release") {
            isMinifyEnabled = false
        }
    }
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    implementation(project(":ffi"))
    implementation(libs.androidx.core.ktx)
    // JNA's Android AAR bundles the per-ABI jnidispatch the generated bindings load.
    implementation(libs.jna) {
        artifact { type = "aar" }
    }

    testImplementation(libs.junit)
    testImplementation(libs.robolectric)
    testImplementation(libs.androidx.test.ext.junit)
}

// Cross-compile the native core into src/main/jniLibs before the APK packs JNI libs.
// Only the APK path depends on this; unit tests (testDebugUnitTest) never trigger it.
val cargoNdkJniLibs by tasks.registering(Exec::class) {
    description = "Cross-compile idiolect-ffi for Android ABIs into src/main/jniLibs."
    workingDir = rootProject.projectDir
    val abis = (project.findProperty("idiolect.abis") as String?) ?: "x86_64 arm64-v8a"
    commandLine("bash", "build-jni.sh", abis, "release")
}

tasks.matching { it.name.contains("JniLibFolders") }.configureEach {
    dependsOn(cargoNdkJniLibs)
}
