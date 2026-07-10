package org.idiolect.android.ime

import android.text.InputType
import android.view.inputmethod.EditorInfo

/**
 * Decides, purely from a field's `EditorInfo`, how idiolect should treat it. Two orthogonal
 * questions:
 *
 *  - [handOff] — should the mic surface appear at all? idiolect renders no typing keyboard, so
 *    its only useful target is free text. Numeric, phone, date and password fields hand off to
 *    the user's own keyboard rather than defaulting to a pointless (or, for passwords, dangerous)
 *    microphone.
 *  - [blocksLearning] — may a take here be persisted and trained on? Password/PIN content is a
 *    secret; and a field flagged `IME_FLAG_NO_PERSONALIZED_LEARNING` (incognito browser/chat,
 *    promo-code boxes) is the app's explicit request not to update personalized data. Neither may
 *    reach idiolect's history/correction/training pipeline.
 *
 * Password/PIN and no-personalized-learning fields answer *yes* to both; plain numeric/phone/date
 * hand off but aren't secrets (a dictated phone number is fine to learn from); plain free text
 * answers *no* to both. Pure so it's host-JVM unit-testable; the framework `InputType`/`EditorInfo`
 * constants are compile-time `int`s (inlined), so no Android runtime is needed.
 */
object InputFieldPolicy {
    /**
     * True when the mic should not default to this field — hand off to the user's keyboard.
     * Every [blocksLearning] field hands off, plus numeric, phone and date fields. Unknown/
     * unspecified fields (`TYPE_NULL`, common for WebViews and custom views over real text) keep
     * the mic — we only refuse what we can positively name.
     */
    fun handOff(inputType: Int, imeOptions: Int): Boolean {
        if (blocksLearning(inputType, imeOptions)) return true
        return when (inputType and InputType.TYPE_MASK_CLASS) {
            InputType.TYPE_CLASS_NUMBER,
            InputType.TYPE_CLASS_PHONE,
            InputType.TYPE_CLASS_DATETIME,
            -> true
            else -> false
        }
    }

    /**
     * True for fields whose takes must never be persisted or trained on: password and
     * numeric-password (PIN) fields, and any field that sets `IME_FLAG_NO_PERSONALIZED_LEARNING`.
     * Drives the mic gate (no take reaches the core) and the correction-capture disarm.
     */
    fun blocksLearning(inputType: Int, imeOptions: Int): Boolean {
        if (imeOptions and EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING != 0) return true
        return when (inputType and InputType.TYPE_MASK_CLASS) {
            InputType.TYPE_CLASS_TEXT -> isTextPassword(inputType)
            InputType.TYPE_CLASS_NUMBER ->
                inputType and InputType.TYPE_MASK_VARIATION == InputType.TYPE_NUMBER_VARIATION_PASSWORD
            else -> false
        }
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
