package org.idiolect.android.settings

import org.junit.Assert.assertEquals
import org.junit.Test
import java.io.File
import java.nio.file.Files

/**
 * The "Audio on device" figure: how many bytes of captured source audio sit under the core's
 * `filesDir/audio` store, formatted against the 1 GiB retention cap. The byte sum is a pure
 * recursive walk (the store nests blobs under date subdirs), host-tested with a temp tree; the
 * cap is enforced in the Rust core, this only *reports* the footprint, so no FFI is needed.
 */
class AudioUsageTest {
    private fun tempDir(): File = Files.createTempDirectory("audio-usage").toFile()

    @Test
    fun sums_every_blob_under_nested_date_dirs() {
        val audio = File(tempDir(), "audio")
        File(audio, "2026/06/22").mkdirs()
        File(audio, "2026/06/22/take-a.ogg").writeBytes(ByteArray(1000))
        File(audio, "2026/06/22/take-b.ogg").writeBytes(ByteArray(24))
        File(audio, "loose.ogg").writeBytes(ByteArray(48))
        assertEquals(1072L, AudioUsage.bytesOnDisk(audio))
    }

    @Test
    fun an_absent_store_is_zero_bytes() {
        // A device that has never dictated has no audio dir yet.
        assertEquals(0L, AudioUsage.bytesOnDisk(File(tempDir(), "audio")))
    }

    @Test
    fun formats_used_against_the_cap_in_human_units() {
        // 214 MiB of captured audio against the 1 GiB cap (the mockup's figure).
        val used = 214L * 1024 * 1024
        val cap = 1024L * 1024 * 1024
        assertEquals("214 MB of 1.0 GB", AudioUsage.format(used, cap))
    }

    @Test
    fun a_few_bytes_show_in_bytes_not_a_rounded_zero() {
        assertEquals("500 B of 1.0 GB", AudioUsage.format(500, 1024L * 1024 * 1024))
    }

    @Test
    fun kilobytes_round_to_kb() {
        assertEquals("2 KB of 1.0 GB", AudioUsage.format(2048, 1024L * 1024 * 1024))
    }
}
