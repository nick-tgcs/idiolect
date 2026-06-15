package org.idiolect.android.sync

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.io.File
import java.nio.file.Files

/**
 * The persisted server endpoint the sync worker ships to: written once at setup (when
 * the user enters their PC's URL + token for the model download), read later by the
 * background [SyncWorker]. Pure file IO, host-tested. The token is plaintext on disk for
 * now; S3's pairing flow moves it behind the AndroidKeyStore.
 */
class SyncConfigTest {
    private fun newConfig(): Pair<SyncConfig, File> {
        val dir = Files.createTempDirectory("sync-config").toFile()
        val file = File(dir, SyncConfig.FILE_NAME)
        return SyncConfig(file) to file
    }

    @Test
    fun saved_settings_round_trip() {
        val (config, _) = newConfig()
        config.save(SyncSettings("https://pc.local:8443", "secret-token"))
        assertEquals(SyncSettings("https://pc.local:8443", "secret-token"), config.load())
    }

    @Test
    fun load_is_null_before_anything_is_saved() {
        val (config, _) = newConfig()
        assertNull("no endpoint configured yet means nothing to sync to", config.load())
    }

    @Test
    fun a_resaved_endpoint_overwrites_the_previous_one() {
        val (config, _) = newConfig()
        config.save(SyncSettings("https://old:1", "t1"))
        config.save(SyncSettings("https://new:2", "t2"))
        assertEquals(SyncSettings("https://new:2", "t2"), config.load())
    }

    @Test
    fun a_malformed_file_loads_as_null_rather_than_crashing() {
        val (config, file) = newConfig()
        file.parentFile?.mkdirs()
        file.writeText("only-one-line")
        assertNull("a truncated config is treated as unconfigured", config.load())
    }
}
