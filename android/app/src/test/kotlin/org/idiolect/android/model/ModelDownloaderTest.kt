package org.idiolect.android.model

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Test
import java.io.OutputStream
import java.nio.file.Files
import java.security.MessageDigest

class ModelDownloaderTest {
    private fun sha256(bytes: ByteArray): String =
        MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it.toInt() and 0xFF) }

    /** Serves [bytes] from the requested offset; records the offset it was asked for. */
    private class FakeTransport(val manifest: ModelManifest, val bytes: ByteArray) : ModelTransport {
        var requestedOffset: Long = -1
        override fun fetchManifest() = manifest
        override fun download(offset: Long, sink: OutputStream, onBytes: (Long) -> Unit) {
            requestedOffset = offset
            val slice = bytes.copyOfRange(offset.toInt(), bytes.size)
            sink.write(slice)
            onBytes(slice.size.toLong())
        }
    }

    private fun newStore() = ModelStore(Files.createTempDirectory("dl").toFile())

    @Test
    fun a_fresh_download_verifies_and_installs() {
        val bytes = ByteArray(5000) { (it % 256).toByte() }
        val store = newStore()
        val installed = ModelDownloader(
            FakeTransport(ModelManifest("base.en", sha256(bytes), bytes.size.toLong()), bytes),
            store,
        ).download()

        assertEquals("base.en", installed.id)
        assertArrayEquals(bytes, store.modelFile("base.en").readBytes())
        assertEquals(installed, store.active())
    }

    @Test
    fun it_resumes_from_an_existing_partial() {
        val bytes = ByteArray(5000) { (it % 256).toByte() }
        val store = newStore()
        store.partFile("base.en").apply { parentFile?.mkdirs() }.writeBytes(bytes.copyOfRange(0, 2000))
        val transport = FakeTransport(ModelManifest("base.en", sha256(bytes), bytes.size.toLong()), bytes)

        ModelDownloader(transport, store).download()

        assertEquals("resumes from the partial's length", 2000L, transport.requestedOffset)
        assertArrayEquals(bytes, store.modelFile("base.en").readBytes())
    }

    @Test
    fun a_digest_mismatch_throws_and_discards_the_partial() {
        val bytes = ByteArray(100) { 1 }
        val store = newStore()
        val wrongDigest = "00".repeat(32)

        assertThrows(ModelIntegrityException::class.java) {
            ModelDownloader(FakeTransport(ModelManifest("base.en", wrongDigest, 100), bytes), store).download()
        }
        assertFalse(store.isInstalled("base.en"))
        assertFalse("the corrupt partial is discarded", store.partFile("base.en").exists())
    }

    @Test
    fun it_reports_progress_to_the_total() {
        val bytes = ByteArray(3000) { 7 }
        var last = -1L to -1L
        ModelDownloader(
            FakeTransport(ModelManifest("base.en", sha256(bytes), bytes.size.toLong()), bytes),
            newStore(),
        ).download { downloaded, total -> last = downloaded to total }

        assertEquals(3000L to 3000L, last)
    }
}
