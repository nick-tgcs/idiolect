// Root build: every plugin is declared here once (resolved onto the build classpath)
// and applied per-module. Declaring them all (even the Kotlin JVM/Android pair, which
// share one artifact) avoids version-conflict errors when a module requests a plugin
// that another module has already put on the classpath.
plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.kotlin.android) apply false
    alias(libs.plugins.kotlin.jvm) apply false
}
