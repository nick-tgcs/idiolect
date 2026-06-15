package org.idiolect.android.sync

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Test
import java.io.File
import java.nio.file.Files

/**
 * The persisted sync endpoint, with the bearer token wrapped by the AndroidKeyStore
 * ([PairingTokenStore]) and the non-secret base URL in the clear. Replaces the old
 * plaintext `SyncConfig`. Host-tested with a [FakeEnvelope]; the device wiring
 * ([SecureSyncConfig.keystoreBacked]) is the same seam as `crypto/HistoryKey`.
 */
class SecureSyncConfigTest {
    private fun newConfig(): SecureSyncConfig {
        val dir = Files.createTempDirectory("secure-sync").toFile()
        return config(dir)
    }

    private fun config(dir: File) = SecureSyncConfig(
        urlFile = File(dir, SecureSyncConfig.URL_FILE_NAME),
        tokenStore = PairingTokenStore(FakeEnvelope(), File(dir, PairingTokenStore.FILE_NAME)),
    )

    @Test
    fun saved_settings_round_trip() {
        val config = newConfig()
        config.save(SyncSettings("https://pc.local:8443", "secret-token"))
        assertEquals(SyncSettings("https://pc.local:8443", "secret-token"), config.load())
    }

    @Test
    fun load_is_null_before_anything_is_saved() {
        assertNull("no endpoint configured yet means nothing to sync to", newConfig().load())
    }

    @Test
    fun a_resaved_endpoint_overwrites_the_previous_one() {
        val config = newConfig()
        config.save(SyncSettings("https://old:1", "t1"))
        config.save(SyncSettings("https://new:2", "t2"))
        assertEquals(SyncSettings("https://new:2", "t2"), config.load())
    }

    @Test
    fun a_url_without_a_token_loads_as_null() {
        val dir = Files.createTempDirectory("secure-sync").toFile()
        // Only the URL was written (e.g. an interrupted save) — treat as unconfigured.
        File(dir, SecureSyncConfig.URL_FILE_NAME).writeText("https://pc.local:8443")
        assertNull("a half-written endpoint is not usable", config(dir).load())
    }

    @Test
    fun the_token_is_never_written_in_plaintext() {
        val dir = Files.createTempDirectory("secure-sync").toFile()
        config(dir).save(SyncSettings("https://pc.local:8443", "secret-token"))
        val tokenBytes = File(dir, PairingTokenStore.FILE_NAME).readBytes()
        assertFalse(
            "the token must be wrapped on disk, not in the clear",
            tokenBytes.contentEquals("secret-token".toByteArray(Charsets.UTF_8)),
        )
    }
}
