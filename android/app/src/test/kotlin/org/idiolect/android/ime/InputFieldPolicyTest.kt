package org.idiolect.android.ime

import android.text.InputType
import android.view.inputmethod.EditorInfo
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Unit cover for [InputFieldPolicy] — the two pure decisions read from a field's `EditorInfo`:
 * whether the mic surface belongs there at all ([InputFieldPolicy.handOff]), and whether a take
 * may be persisted/learned from ([InputFieldPolicy.blocksLearning]). Free text is the only thing
 * worth dictating into and learning from; numeric/phone/date hand off (pointless mic) but aren't
 * secrets; password/PIN and any field flagged `IME_FLAG_NO_PERSONALIZED_LEARNING` (incognito,
 * promo codes) both hand off AND must never be persisted or trained on. The framework `InputType`
 * / `EditorInfo` constants are compile-time `int`s (inlined), so no Android runtime is needed.
 */
class InputFieldPolicyTest {
    private val noLearn = EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING

    private fun text(variation: Int = 0) = InputType.TYPE_CLASS_TEXT or variation
    private fun number(variation: Int = 0) = InputType.TYPE_CLASS_NUMBER or variation

    // --- handOff: keep the mic only for genuine, learnable free text ---

    @Test
    fun plain_text_keeps_the_mic() {
        assertFalse(InputFieldPolicy.handOff(text(), imeOptions = 0))
    }

    @Test
    fun an_email_or_uri_text_field_keeps_the_mic() {
        // Still free text you might reasonably dictate — don't hand these off.
        assertFalse(InputFieldPolicy.handOff(text(InputType.TYPE_TEXT_VARIATION_EMAIL_ADDRESS), 0))
        assertFalse(InputFieldPolicy.handOff(text(InputType.TYPE_TEXT_VARIATION_URI), 0))
    }

    @Test
    fun a_text_password_field_hands_off() {
        assertTrue(InputFieldPolicy.handOff(text(InputType.TYPE_TEXT_VARIATION_PASSWORD), 0))
        assertTrue(InputFieldPolicy.handOff(text(InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD), 0))
        assertTrue(InputFieldPolicy.handOff(text(InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD), 0))
    }

    @Test
    fun a_plain_numeric_field_hands_off() {
        // "Secure + all numeric": a PIN/amount/OTP box shouldn't default to the microphone.
        assertTrue(InputFieldPolicy.handOff(number(), 0))
    }

    @Test
    fun a_numeric_password_pin_hands_off() {
        assertTrue(InputFieldPolicy.handOff(number(InputType.TYPE_NUMBER_VARIATION_PASSWORD), 0))
    }

    @Test
    fun phone_and_datetime_fields_hand_off() {
        assertTrue(InputFieldPolicy.handOff(InputType.TYPE_CLASS_PHONE, 0))
        assertTrue(InputFieldPolicy.handOff(InputType.TYPE_CLASS_DATETIME, 0))
    }

    @Test
    fun an_unspecified_field_keeps_the_mic() {
        // TYPE_NULL (0) — WebViews and custom views often report this for real text fields;
        // only refuse when we've positively identified a numeric/secure/no-learn field.
        assertFalse(InputFieldPolicy.handOff(InputType.TYPE_NULL, 0))
    }

    @Test
    fun a_no_personalized_learning_text_field_hands_off() {
        // Incognito browser/chat or promo-code fields: free text, but the app asked us not to
        // personalize — so hand off rather than dictate-and-learn.
        assertTrue(InputFieldPolicy.handOff(text(), imeOptions = noLearn))
    }

    // --- blocksLearning: never persist/train these takes ---

    @Test
    fun password_and_pin_fields_block_learning() {
        assertTrue(InputFieldPolicy.blocksLearning(text(InputType.TYPE_TEXT_VARIATION_PASSWORD), 0))
        assertTrue(InputFieldPolicy.blocksLearning(text(InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD), 0))
        assertTrue(InputFieldPolicy.blocksLearning(text(InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD), 0))
        assertTrue(InputFieldPolicy.blocksLearning(number(InputType.TYPE_NUMBER_VARIATION_PASSWORD), 0))
    }

    @Test
    fun a_no_personalized_learning_field_blocks_learning() {
        // The flag is the app's explicit request not to update personalized data — honour it
        // even on otherwise free text.
        assertTrue(InputFieldPolicy.blocksLearning(text(), imeOptions = noLearn))
        // And it composes with other flags in imeOptions.
        assertTrue(
            InputFieldPolicy.blocksLearning(text(), noLearn or EditorInfo.IME_ACTION_SEARCH),
        )
    }

    @Test
    fun plain_and_numeric_fields_do_not_block_learning() {
        // Plain numeric/phone hand off from the mic, but a phone number isn't a secret to
        // withhold from training if it ever were dictated (e.g. a failed hand-off).
        assertFalse(InputFieldPolicy.blocksLearning(text(), 0))
        assertFalse(InputFieldPolicy.blocksLearning(number(), 0))
        assertFalse(InputFieldPolicy.blocksLearning(InputType.TYPE_CLASS_PHONE, 0))
    }
}
