package org.idiolect.android.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.File
import java.nio.file.Files

class ModelStoreTest {
    private fun newStore(): Pair<ModelStore, File> {
        val root = Files.createTempDirectory("models").toFile()
        return ModelStore(root) to root
    }

    @Test
    fun install_atomically_places_the_model_and_records_it_active() {
        val (store, root) = newStore()
        val part = File(root, "base.en.bin.part").apply { writeBytes("model-bytes".toByteArray()) }

        val installed = store.install("base.en", "deadbeef", part)

        assertTrue(store.isInstalled("base.en"))
        assertFalse("the temp part is consumed", part.exists())
        assertEquals("model-bytes", store.modelFile("base.en").readText())
        assertEquals(InstalledModel("base.en", "deadbeef", store.modelFile("base.en").absolutePath), installed)
        assertEquals(installed, store.active())
    }

    @Test
    fun active_is_null_with_nothing_installed() {
        val (store, _) = newStore()
        assertNull(store.active())
    }

    @Test
    fun active_is_null_when_the_recorded_model_file_is_gone() {
        val (store, root) = newStore()
        store.install("base.en", "deadbeef", File(root, "p").apply { writeBytes(byteArrayOf(1)) })
        store.modelFile("base.en").delete()
        assertNull("a stale active record without its file is not active", store.active())
    }

    @Test
    fun install_keeps_the_existing_model_when_the_replacement_is_unavailable() {
        // Durability: installing over an existing model must not destroy it before the
        // replacement is in place. A re-install whose source never materialized (aborted
        // or partial download) must fail WITHOUT leaving `active` pointing at a missing
        // file. The crash-mid-rename window itself isn't reachable deterministically
        // on-device, so the durability contract is pinned here at the unit level via an
        // absent replacement source.
        val (store, root) = newStore()
        store.install("base.en", "old", File(root, "old.part").apply { writeBytes("OLD-MODEL".toByteArray()) })
        assertEquals("OLD-MODEL", store.modelFile("base.en").readText())

        val missing = File(root, "missing.part")
        assertFalse("precondition: the replacement source is absent", missing.exists())
        try {
            store.install("base.en", "new", missing)
            fail("install over an absent source must not silently succeed")
        } catch (expected: Exception) {
            // expected — the replacement could not be placed
        }

        assertTrue("the existing model survives a failed replace", store.isInstalled("base.en"))
        assertEquals("OLD-MODEL", store.modelFile("base.en").readText())
        assertEquals("base.en", store.active()?.id)
    }
}
