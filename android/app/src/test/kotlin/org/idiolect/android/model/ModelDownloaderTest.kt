package org.idiolect.android.model

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
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

    /** Ignores the requested offset and always streams the WHOLE file — a CDN that drops Range. */
    private class RangeIgnoringTransport(val manifest: ModelManifest, val bytes: ByteArray) : ModelTransport {
        val offsets = mutableListOf<Long>()
        override fun fetchManifest() = manifest
        override fun download(offset: Long, sink: OutputStream, onBytes: (Long) -> Unit) {
            offsets.add(offset)
            sink.write(bytes)
            onBytes(bytes.size.toLong())
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
    fun a_dropped_range_on_resume_self_heals_in_one_download() {
        val bytes = ByteArray(5000) { (it % 256).toByte() }
        val store = newStore()
        // A stale partial would be resumed, but the server ignores the Range and restreams
        // from 0; appending that onto the partial corrupts the file, so the downloader must
        // discard and retry cleanly rather than surface an integrity error.
        store.partFile("base.en").apply { parentFile?.mkdirs() }.writeBytes(bytes.copyOfRange(0, 2000))
        val transport = RangeIgnoringTransport(ModelManifest("base.en", sha256(bytes), bytes.size.toLong()), bytes)

        val installed = ModelDownloader(transport, store).download()

        assertEquals("base.en", installed.id)
        assertArrayEquals(bytes, store.modelFile("base.en").readBytes())
        assertEquals("resumed first, then retried clean from 0", listOf(2000L, 0L), transport.offsets)
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
    fun a_cancelled_download_installs_nothing() {
        val bytes = ByteArray(5000) { (it % 256).toByte() }
        val store = newStore()
        // The user cancelled (or a newer download superseded) this one while it was streaming.
        // Even though the bytes finish and verify, the model must NOT be installed — otherwise
        // onboarding would advance to Ready with a model the user explicitly abandoned.
        assertThrows(ModelDownloadCancelledException::class.java) {
            ModelDownloader(
                FakeTransport(ModelManifest("base.en", sha256(bytes), bytes.size.toLong()), bytes),
                store,
            ).download(isCancelled = { true })
        }
        assertFalse("a cancelled download installs nothing", store.isInstalled("base.en"))
        assertNull("no model becomes active", store.active())
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
