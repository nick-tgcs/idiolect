package org.idiolect.android.ime

import android.text.InputType

/**
 * Decides, purely from a field's `EditorInfo.inputType`, whether the mic surface belongs there.
 *
 * idiolect renders no typing keyboard, so its only useful target is free text. A PIN pad, phone,
 * amount or date field defaulting to a microphone is pointless friction, and a *password* field is
 * worse than pointless — dictating a secret aloud is a security risk, and idiolect's correction
 * pipeline would otherwise capture the password's audio and plaintext and ship them to the paired
 * PC as training data. So those fields [handOff] to the user's own keyboard, and password/PIN
 * fields are additionally flagged [isSecure] so their content never enters the training pipeline
 * even if the user forces dictation there via the ⌨ override.
 *
 * Pure so it's host-JVM unit-testable; the framework `InputType` constants are compile-time
 * `int`s (inlined), so no Android runtime is needed.
 */
object InputFieldPolicy {
    /**
     * True when the mic should not default to this field — hand off to the user's keyboard.
     * Keeps the mic only for genuine free text (any text class that isn't a password); refuses
     * numeric, PIN, phone and date fields. Unknown/unspecified fields (`TYPE_NULL`, common for
     * WebViews and custom views over real text) keep the mic — we only refuse what we can name.
     */
    fun handOff(inputType: Int): Boolean =
        when (inputType and InputType.TYPE_MASK_CLASS) {
            InputType.TYPE_CLASS_NUMBER,
            InputType.TYPE_CLASS_PHONE,
            InputType.TYPE_CLASS_DATETIME,
            -> true
            InputType.TYPE_CLASS_TEXT -> isTextPassword(inputType)
            else -> false
        }

    /**
     * True for password and numeric-password (PIN) fields — content that must never be captured
     * for training, regardless of the [handOff] decision (defence in depth for the ⌨ override).
     */
    fun isSecure(inputType: Int): Boolean =
        when (inputType and InputType.TYPE_MASK_CLASS) {
            InputType.TYPE_CLASS_TEXT -> isTextPassword(inputType)
            InputType.TYPE_CLASS_NUMBER ->
                inputType and InputType.TYPE_MASK_VARIATION == InputType.TYPE_NUMBER_VARIATION_PASSWORD
            else -> false
        }

    private fun isTextPassword(inputType: Int): Boolean =
        when (inputType and InputType.TYPE_MASK_VARIATION) {
            InputType.TYPE_TEXT_VARIATION_PASSWORD,
            InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD,
            InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD,
            -> true
            else -> false
        }
}
