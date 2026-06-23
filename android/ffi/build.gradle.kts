// The UniFFI bindings module: pure JVM (Kotlin + JNA), no Android SDK required.
// The Kotlin bindings are *generated* from the `idiolect-ffi` crate at build time
// (library mode, off the host cdylib), so they never go stale against the Rust
// contract and are never checked in. The IME app module depends on this module.
plugins {
    alias(libs.plugins.kotlin.jvm)
}

// `android/` is the Gradle root; its parent is the cargo workspace.
val workspaceRoot: java.io.File = rootProject.projectDir.parentFile
val hostCdylib: java.io.File = workspaceRoot.resolve("target/debug/libidiolect_ffi.so")
val hostLibDir: java.io.File = workspaceRoot.resolve("target/debug")

// Build the host cdylib so (a) bindgen can read its metadata and (b) the JVM unit
// tests can load it via JNA. CPU-only (no `cuda`): this is the phone's code path.
val cargoBuildFfiHost by tasks.registering(Exec::class) {
    description = "Build the idiolect-ffi host cdylib (for binding generation + JVM tests)."
    workingDir = workspaceRoot
    commandLine("cargo", "build", "-p", "idiolect-ffi")
    outputs.file(hostCdylib)
    // Always invoke cargo: it is itself incremental (a no-op when the Rust sources
    // are unchanged), and this keeps the generated bindings honest against the
    // crate without Gradle having to track every Rust source file as an input.
    outputs.upToDateWhen { false }
}

val generatedBindingsDir = layout.buildDirectory.dir("generated/uniffi")

// Generate the Kotlin bindings from the freshly built cdylib (UniFFI library mode).
val generateUniffiBindings by tasks.registering(Exec::class) {
    description = "Generate the Kotlin UniFFI bindings from the host cdylib."
    dependsOn(cargoBuildFfiHost)
    workingDir = workspaceRoot
    val outDir = generatedBindingsDir.get().asFile
    doFirst { outDir.mkdirs() }
    commandLine(
        "cargo", "run", "-q", "-p", "idiolect-ffi", "--bin", "uniffi-bindgen", "--",
        "generate",
        "--library", hostCdylib.absolutePath,
        "--language", "kotlin",
        "--out-dir", outDir.absolutePath,
        "--no-format",
    )
    inputs.file(hostCdylib)
    outputs.dir(outDir)
}

kotlin {
    jvmToolchain(17)
    sourceSets.named("main") {
        kotlin.srcDir(generateUniffiBindings)
    }
}

dependencies {
    // JNA backs the generated bindings. `compileOnly` here keeps the host JAR off the
    // app's Android runtime classpath — the app supplies the `@aar` (with per-ABI
    // jnidispatch) instead. The host JVM tests get the JAR via `testImplementation`.
    compileOnly(libs.jna)
    testImplementation(libs.junit)
    testImplementation(libs.jna)
}

// The unit tests load `libidiolect_ffi.so` from the cargo target dir via JNA.
tasks.test {
    dependsOn(cargoBuildFfiHost)
    systemProperty("jna.library.path", hostLibDir.absolutePath)
}
