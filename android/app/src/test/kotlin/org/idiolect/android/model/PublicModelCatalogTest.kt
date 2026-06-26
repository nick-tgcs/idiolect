package org.idiolect.android.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The selectable on-device model catalog for the PC-less path. Each entry pins a public
 * download URL + integrity manifest (the digest is the trust anchor) and carries the display
 * metadata the onboarding / settings pickers show. The default is **tiny.en** — several times
 * faster on a phone CPU than base.en and a fraction of the download — with base.en kept as the
 * higher-accuracy opt-in. These tests lock the pins (a wrong digest silently bricks the
 * download) and the "fast model is the default" contract.
 */
class PublicModelCatalogTest {
    @Test
    fun the_default_is_the_fast_tiny_english_model() {
        assertSame(
            "the zero-config default must be the fastest model so transcription is quick out of the box",
            PublicModelCatalog.TINY_EN_Q5_1,
            PublicModelCatalog.default,
        )
        assertEquals("ggml-tiny.en-q5_1", PublicModelCatalog.default.id)
    }

    @Test
    fun the_default_is_listed_first_so_the_picker_preselects_it() {
        assertSame(PublicModelCatalog.default, PublicModelCatalog.options.first())
    }

    @Test
    fun both_the_tiny_and_base_options_are_offered() {
        val ids = PublicModelCatalog.options.map { it.id }
        assertEquals(listOf("ggml-tiny.en-q5_1", "ggml-base.en-q5_1"), ids)
    }

    @Test
    fun the_tiny_pin_matches_the_published_q5_1_file() {
        val tiny = PublicModelCatalog.TINY_EN_Q5_1
        assertEquals("c77c5766f1cef09b6b7d47f21b546cbddd4157886b3b5d6d4f709e91e66c7c2b", tiny.sha256)
        assertEquals(32_166_155L, tiny.size)
    }

    @Test
    fun the_base_pin_matches_the_published_q5_1_file() {
        val base = PublicModelCatalog.BASE_EN_Q5_1
        assertEquals("4baf70dd0d7c4247ba2b81fafd9c01005ac77c2f9ef064e00dcf195d0e2fdd2f", base.sha256)
        assertEquals(59_721_011L, base.size)
    }

    @Test
    fun every_option_is_an_https_source_with_a_full_digest_and_display_metadata() {
        for (option in PublicModelCatalog.options) {
            assertTrue("${option.id} must download over https", option.url.startsWith("https://"))
            assertEquals("${option.id} pins a full SHA-256", 64, option.sha256.length)
            assertTrue("${option.id} pins a positive size", option.size > 0)
            assertTrue("${option.id} has a label", option.label.isNotBlank())
            assertTrue("${option.id} has a size hint", option.sizeLabel.isNotBlank())
            assertTrue("${option.id} has a blurb", option.blurb.isNotBlank())
        }
    }

    @Test
    fun an_option_builds_a_pinned_https_transport() {
        val transport = PublicModelCatalog.TINY_EN_Q5_1.transport()
        val manifest = transport.fetchManifest()
        assertEquals("ggml-tiny.en-q5_1", manifest.id)
        assertEquals(PublicModelCatalog.TINY_EN_Q5_1.sha256, manifest.sha256)
        assertEquals(PublicModelCatalog.TINY_EN_Q5_1.size, manifest.size)
    }

    @Test
    fun by_id_resolves_an_installed_model_back_to_its_catalog_entry() {
        assertSame(PublicModelCatalog.BASE_EN_Q5_1, PublicModelCatalog.byId("ggml-base.en-q5_1"))
        assertNotNull(PublicModelCatalog.byId("ggml-tiny.en-q5_1"))
        assertNull("an unknown / PC-served id is not in the public catalog", PublicModelCatalog.byId("ggml-small.en"))
    }
}
