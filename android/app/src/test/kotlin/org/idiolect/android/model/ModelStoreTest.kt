package org.idiolect.android.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
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
}
