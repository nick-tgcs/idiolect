package org.idiolect.android.ime

/**
 * The system IME-switch primitives, behind an interface so the fallback order is pure and
 * unit-testable while the actual `InputMethodService` calls stay a thin boundary on the
 * service. idiolect is a voice surface only — it never types — so "edit this" always means
 * handing the field to the user's own keyboard.
 */
interface KeyboardHandoff {
    /** Hand off to the user's last-used keyboard (`switchToPreviousInputMethod`). */
    fun toPreviousKeyboard(): Boolean

    /** Cycle to the next enabled keyboard (`switchToNextInputMethod`). */
    fun toNextKeyboard(): Boolean

    /** Last resort: open the system keyboard picker. */
    fun openPicker()
}

/**
 * "Switch to your keyboard": prefer the user's last-used keyboard, fall back to the next
 * enabled one, and only pop the system picker when there is nothing to switch to.
 */
object SwitchToYourKeyboard {
    fun run(handoff: KeyboardHandoff) {
        if (handoff.toPreviousKeyboard()) return
        if (handoff.toNextKeyboard()) return
        handoff.openPicker()
    }
}

/** An enabled input method, as much of it as the target-picker needs. */
data class EnabledKeyboard(val id: String, val packageName: String)

/**
 * Picking which keyboard to hand off to when there's no switch *history* to fall back on
 * (the common case — the user selected idiolect from Settings, so `switchToNextInputMethod`
 * has nothing to rotate to). Deterministically choose another enabled keyboard by id and
 * switch to it directly, rather than relying on the unreliable system picker.
 */
object KeyboardTargets {
    /** The first enabled keyboard that isn't idiolect's own, or null if there is none. */
    fun pickOther(enabled: List<EnabledKeyboard>, ownPackage: String): String? =
        enabled.firstOrNull { it.packageName != ownPackage }?.id
}
