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

    // The native core is cross-compiled into build/android-ffi/jniLibs by the
    // canonical scripts/android-ffi-build.sh (see cargoNdkJniLibs below), kept out of
    // the source tree.
    sourceSets["main"].jniLibs.srcDir(layout.buildDirectory.dir("android-ffi/jniLibs"))
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    implementation(project(":ffi"))
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity)
    implementation(libs.androidx.work.runtime.ktx)
    // Camera QR scanning for device pairing (FOSS, no Google Play Services).
    implementation(libs.zxing.android.embedded)
    // JNA's Android AAR bundles the per-ABI jnidispatch the generated bindings load.
    implementation(libs.jna) {
        artifact { type = "aar" }
    }

    testImplementation(libs.junit)
    testImplementation(libs.robolectric)
    testImplementation(libs.androidx.test.ext.junit)
    testImplementation(libs.androidx.work.testing)

    androidTestImplementation(libs.androidx.test.ext.junit)
    androidTestImplementation(libs.androidx.test.runner)
}

// Cross-compile the native core (arm64-v8a + x86_64) into build/android-ffi/jniLibs
// via the canonical repo script, which sets the full NDK env, bundles libc++_shared.so
// per ABI, and builds release. Only the APK path depends on this; unit tests
// (testDebugUnitTest) never trigger it.
val cargoNdkJniLibs by tasks.registering(Exec::class) {
    description = "Cross-compile idiolect-ffi for Android ABIs into build/android-ffi/jniLibs."
    // The cargo workspace root (android/ is the Gradle root; its parent is the repo).
    workingDir = rootProject.projectDir.parentFile
    val outDir = layout.buildDirectory.dir("android-ffi").get().asFile.absolutePath
    commandLine("bash", "scripts/android-ffi-build.sh", outDir, "--release")
}

tasks.matching { it.name.contains("JniLibFolders") }.configureEach {
    dependsOn(cargoNdkJniLibs)
}
