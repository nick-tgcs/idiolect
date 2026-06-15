package org.idiolect.android.setup

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The pure decision behind the model-download form: blank fields pick the zero-config
 * public model, both fields pick the user's PC (and that endpoint is remembered for sync),
 * and a half-filled form is a typo to be corrected — not a silent fall-through to either
 * path. Extracted from [SetupActivity] so this security-relevant routing is unit-tested,
 * mirroring how [ImeSetup.nextStep] is tested away from the framework.
 */
class ModelSourceChoiceTest {
    @Test
    fun both_blank_selects_the_public_model() {
        assertEquals(ModelSourceChoice.Public, ModelSourceChoice.from("", ""))
    }

    @Test
    fun blank_after_trimming_whitespace_still_selects_the_public_model() {
        assertEquals(ModelSourceChoice.Public, ModelSourceChoice.from("   ", "  "))
    }

    @Test
    fun both_filled_selects_the_pc_with_trimmed_values() {
        assertEquals(
            ModelSourceChoice.Pc("http://10.0.2.2:8765", "tok-123"),
            ModelSourceChoice.from("  http://10.0.2.2:8765 ", " tok-123 "),
        )
    }

    @Test
    fun a_url_without_a_token_needs_details() {
        assertEquals(ModelSourceChoice.NeedDetails, ModelSourceChoice.from("http://10.0.2.2:8765", ""))
    }

    @Test
    fun a_token_without_a_url_needs_details() {
        assertEquals(ModelSourceChoice.NeedDetails, ModelSourceChoice.from("", "tok-123"))
    }
}
