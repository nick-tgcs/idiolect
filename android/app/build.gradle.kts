// The idiolect Android IME app. Depends on the pure-JVM `:ffi` bindings module for
// the Rust core. JVM unit tests (`testDebugUnitTest`) run the IME logic on the host
// JVM (no emulator); Robolectric covers the thin Android-framework seams.
import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
}

// Release signing. CI passes credentials via environment (KEYSTORE_PATH / KEYSTORE_PASSWORD /
// KEY_ALIAS / KEY_PASSWORD) — mirroring the WhisperVault release workflow; local sideload builds
// read android/keystore.properties (git-ignored, see .gitignore). When neither is present the
// release build is left unsigned rather than failing, so the workspace stays buildable everywhere.
val keystorePropsFile = rootProject.file("keystore.properties")
val keystoreProps = Properties().apply {
    if (keystorePropsFile.exists()) keystorePropsFile.inputStream().use { load(it) }
}
val envStoreFile: String? = System.getenv("KEYSTORE_PATH")
val releaseStoreFile: java.io.File? = when {
    envStoreFile != null -> file(envStoreFile)
    keystorePropsFile.exists() -> rootProject.file(keystoreProps.getProperty("storeFile"))
    else -> null
}
val releaseStorePassword: String? = System.getenv("KEYSTORE_PASSWORD") ?: keystoreProps.getProperty("storePassword")
val releaseKeyAlias: String? = System.getenv("KEY_ALIAS") ?: keystoreProps.getProperty("keyAlias")
val releaseKeyPassword: String? = System.getenv("KEY_PASSWORD") ?: keystoreProps.getProperty("keyPassword")

// ABIs packaged into the APK and cross-compiled for the native core. Default: both the device
// (arm64-v8a) and the emulator (x86_64), so the x86_64 e2e keeps working locally. The release
// workflow ships ARM only via -PandroidAbis=arm64-v8a (the only supported device target).
val androidAbis: List<String> = (project.findProperty("androidAbis") as String? ?: "arm64-v8a,x86_64")
    .split(',', ' ').map { it.trim() }.filter { it.isNotEmpty() }

android {
    namespace = "org.idiolect.android"
    compileSdk = 35

    defaultConfig {
        applicationId = "org.idiolect.android"
        // foregroundServiceType=microphone needs API 29; GrapheneOS runs current
        // Android, so a modern floor is fine.
        minSdk = 29
        targetSdk = 35
        // Stamped by the release workflow from the CalVer tag (-PappVersionName / -PappVersionCode);
        // these literals are the local-build fallback.
        versionCode = (project.findProperty("appVersionCode") as String?)?.toIntOrNull() ?: 1
        versionName = (project.findProperty("appVersionName") as String?) ?: "0.1.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"

        // Package only the selected ABIs (see androidAbis) — filters the jniLibs the
        // cargoNdkJniLibs task produces down to what this build actually ships.
        ndk { abiFilters.addAll(androidAbis) }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
        unitTests.isIncludeAndroidResources = true
        // HttpPairingTransportTlsTest stands up a real SSLServerSocket; the JDK's server-side
        // TLS handshake reflects into java.net, which JDK 17+ strong encapsulation blocks
        // ("does not opens java.net to unnamed module") unless we open the package to the test
        // JVM. Without this the pinned-TLS host test fails intermittently on a cold fork.
        unitTests.all {
            it.jvmArgs("--add-opens=java.base/java.net=ALL-UNNAMED")
        }
    }

    signingConfigs {
        if (releaseStoreFile != null) {
            create("release") {
                storeFile = releaseStoreFile
                storePassword = releaseStorePassword
                keyAlias = releaseKeyAlias
                keyPassword = releaseKeyPassword
            }
        }
    }

    buildTypes {
        named("release") {
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("release")
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
    androidTestImplementation(libs.androidx.test.uiautomator)
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
    // Cross-compile only the ABIs this build ships (see androidAbis) — ARM only in the release
    // workflow, both locally so the x86_64 emulator e2e still has a native library to load.
    environment("ANDROID_ABIS", androidAbis.joinToString(" "))
    commandLine("bash", "scripts/android-ffi-build.sh", outDir, "--release")
}

tasks.matching { it.name.contains("JniLibFolders") }.configureEach {
    dependsOn(cargoNdkJniLibs)
}
