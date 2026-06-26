package org.idiolect.android.settings

import java.io.File

/**
 * The persisted toggle store behind the settings screen — three booleans written as a single
 * `key=value` file under the app's private `filesDir`, the same plain-file pattern as
 * [org.idiolect.android.sync.SecureSyncConfig] and [org.idiolect.android.model.ModelStore].
 * Keeping it a pure file (no `SharedPreferences`) means the IME and the settings screen read
 * the same store and it is host-tested with a temp dir, no Robolectric.
 *
 * Every flag's default is the pre-settings behaviour, so an un-written store (a fresh install,
 * or any flag the user never touched) behaves exactly as dictation did before this screen
 * existed — the screen only ever *records a deviation* from the established defaults.
 */
class SettingsStore(private val file: File) {
    /** 👁 review-before-insert as the default for each take. Was off (the strip toggle started unlit). */
    fun reviewByDefault(): Boolean = read(REVIEW, default = false)
    fun setReviewByDefault(on: Boolean) = write(REVIEW, on)

    /** Whether a double-tap on the mic enters continuous mode. Was always on. */
    fun continuousOnDoubleTap(): Boolean = read(CONTINUOUS, default = true)
    fun setContinuousOnDoubleTap(on: Boolean) = write(CONTINUOUS, on)

    /** Whether captured corrections are shipped to the paired PC (M6). Was always on. */
    fun shipCorrections(): Boolean = read(SHIP, default = true)
    fun setShipCorrections(on: Boolean) = write(SHIP, on)

    /**
     * Whether the floating accessibility ("quick-launch") button dictates into the focused field.
     * Defaults on so the button — which previously did nothing — now does something useful; turn
     * it off here to make idiolect ignore the button (see [IdiolectAccessibilityService]).
     */
    fun quickLaunchEnabled(): Boolean = read(QUICK_LAUNCH, default = true)
    fun setQuickLaunchEnabled(on: Boolean) = write(QUICK_LAUNCH, on)

    /** Parse the whole file into a map; an absent/garbled file yields an empty map (→ defaults). */
    private fun all(): MutableMap<String, String> {
        val map = LinkedHashMap<String, String>()
        if (file.exists()) {
            for (line in file.readLines()) {
                val eq = line.indexOf('=')
                if (eq > 0) map[line.substring(0, eq).trim()] = line.substring(eq + 1).trim()
            }
        }
        return map
    }

    private fun read(key: String, default: Boolean): Boolean = when (all()[key]) {
        "true" -> true
        "false" -> false
        else -> default
    }

    /** Read-modify-write the single flag so sibling flags keep their values. */
    private fun write(key: String, on: Boolean) {
        val map = all()
        map[key] = on.toString()
        file.parentFile?.mkdirs()
        file.writeText(map.entries.joinToString("\n") { "${it.key}=${it.value}" })
    }

    companion object {
        const val FILE_NAME = "settings.prefs"
        const val REVIEW = "review_by_default"
        const val CONTINUOUS = "continuous_on_double_tap"
        const val SHIP = "ship_corrections"
        const val QUICK_LAUNCH = "quick_launch_mic"

        /** The on-device store, under the app's private files directory. */
        fun under(filesDir: File) = SettingsStore(File(filesDir, FILE_NAME))
    }
}
