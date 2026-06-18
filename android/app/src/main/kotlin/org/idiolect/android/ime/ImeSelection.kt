package org.idiolect.android.ime

/**
 * The pure rule for the value that selects idiolect as the active IME — written to
 * `Settings.Secure.DEFAULT_INPUT_METHOD` by [ReviewActivity] after Insert so the mic returns
 * without a manual keyboard switch (the auto-return path; needs a one-time WRITE_SECURE_SETTINGS
 * grant, see [ReviewActivity.returnToIdiolect]).
 *
 * It must match what the framework stores: the *fully-qualified* flattened component
 * `pkg/full.Class` (the same string `ComponentName(pkg, cls).flattenToString()` produces — note
 * it is NOT abbreviated to `pkg/.Class`). Kept here, free of Android types, so it can be
 * unit-tested ([ImeSelectionTest]) on the JVM.
 */
object ImeSelection {
    fun idiolectImeId(packageName: String, serviceClass: String): String =
        "$packageName/$serviceClass"
}
