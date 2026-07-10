package org.idiolect.android.ime

import android.text.InputType
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Unit cover for [InputFieldPolicy] — the pure decision, read straight from a field's
 * `EditorInfo.inputType`, of whether the mic surface has any business there. Free text is the
 * only thing worth dictating into; numeric/phone/date and — above all — password fields hand
 * off to the user's own keyboard, and password/PIN fields are never fed to the training
 * pipeline. The framework `InputType` constants are compile-time `int`s (inlined), so no Android
 * runtime is needed.
 */
class InputFieldPolicyTest {
    private fun text(variation: Int = 0) = InputType.TYPE_CLASS_TEXT or variation
    private fun number(variation: Int = 0) = InputType.TYPE_CLASS_NUMBER or variation

    // --- handOff: keep the mic only for genuine free text ---

    @Test
    fun plain_text_keeps_the_mic() {
        assertFalse(InputFieldPolicy.handOff(text()))
    }

    @Test
    fun an_email_or_uri_text_field_keeps_the_mic() {
        // Still free text you might reasonably dictate — don't hand these off.
        assertFalse(InputFieldPolicy.handOff(text(InputType.TYPE_TEXT_VARIATION_EMAIL_ADDRESS)))
        assertFalse(InputFieldPolicy.handOff(text(InputType.TYPE_TEXT_VARIATION_URI)))
    }

    @Test
    fun a_text_password_field_hands_off() {
        assertTrue(InputFieldPolicy.handOff(text(InputType.TYPE_TEXT_VARIATION_PASSWORD)))
        assertTrue(InputFieldPolicy.handOff(text(InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD)))
        assertTrue(InputFieldPolicy.handOff(text(InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD)))
    }

    @Test
    fun a_plain_numeric_field_hands_off() {
        // "Secure + all numeric": a PIN/amount/OTP box shouldn't default to the microphone.
        assertTrue(InputFieldPolicy.handOff(number()))
    }

    @Test
    fun a_numeric_password_pin_hands_off() {
        assertTrue(InputFieldPolicy.handOff(number(InputType.TYPE_NUMBER_VARIATION_PASSWORD)))
    }

    @Test
    fun phone_and_datetime_fields_hand_off() {
        assertTrue(InputFieldPolicy.handOff(InputType.TYPE_CLASS_PHONE))
        assertTrue(InputFieldPolicy.handOff(InputType.TYPE_CLASS_DATETIME))
    }

    @Test
    fun an_unspecified_field_keeps_the_mic() {
        // TYPE_NULL (0) — WebViews and custom views often report this for real text fields;
        // only refuse when we've positively identified a numeric/secure field.
        assertFalse(InputFieldPolicy.handOff(InputType.TYPE_NULL))
    }

    // --- isSecure: never capture these for training, even if dictated into ---

    @Test
    fun password_and_pin_fields_are_secure() {
        assertTrue(InputFieldPolicy.isSecure(text(InputType.TYPE_TEXT_VARIATION_PASSWORD)))
        assertTrue(InputFieldPolicy.isSecure(text(InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD)))
        assertTrue(InputFieldPolicy.isSecure(text(InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD)))
        assertTrue(InputFieldPolicy.isSecure(number(InputType.TYPE_NUMBER_VARIATION_PASSWORD)))
    }

    @Test
    fun non_password_fields_are_not_secure() {
        // Plain numeric/phone hand off from the mic, but their content isn't a secret to
        // withhold from training if it ever were captured.
        assertFalse(InputFieldPolicy.isSecure(text()))
        assertFalse(InputFieldPolicy.isSecure(number()))
        assertFalse(InputFieldPolicy.isSecure(InputType.TYPE_CLASS_PHONE))
    }
}
