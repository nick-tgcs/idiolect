package org.idiolect.android.ime

/**
 * Picks idiolect's IME id — exactly as the framework registered it — from the enabled-IME list,
 * so the value written to `Settings.Secure.DEFAULT_INPUT_METHOD` (to pull the active keyboard
 * back to idiolect after a reviewed Insert) is one the framework recognises.
 *
 * The id MUST be the framework's own: its `flattenToShortString` form, which abbreviates a class
 * under its own package to a leading dot (`pkg/.ime.Class`). Reconstructing the *long* form
 * `pkg/pkg.ime.Class` — the original mistake here — makes InputMethodManagerService reject it as
 * "Unknown id" and the IME switch silently fails (so the mic never comes back). Taking the id
 * straight from `InputMethodManager.enabledInputMethodList` sidesteps the format question
 * entirely. Pure of Android types so it's unit-tested ([ImeSelectionTest]); the framework call
 * that supplies the list is the thin boundary on [org.idiolect.android.accessibility]'s service.
 */
object ImeSelection {
    /** idiolect's enabled-IME id (the framework's own short-form string), or null if not enabled. */
    fun idiolectImeId(enabled: List<EnabledKeyboard>, ownPackage: String): String? =
        enabled.firstOrNull { it.packageName == ownPackage }?.id
}
