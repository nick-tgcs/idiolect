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
        pinFile = File(dir, SecureSyncConfig.PIN_FILE_NAME),
    )

    @Test
    fun saved_settings_round_trip() {
        val config = newConfig()
        config.save(SyncSettings("https://pc.local:8443", "secret-token"))
        assertEquals(SyncSettings("https://pc.local:8443", "secret-token"), config.load())
    }

    @Test
    fun the_pin_round_trips_for_a_tls_endpoint() {
        val config = newConfig()
        val pin = "deadbeef".repeat(8) // 64 hex chars, like a real SPKI fingerprint
        config.save(SyncSettings("https://10.0.2.2:8765", "secret-token", pin))
        assertEquals(SyncSettings("https://10.0.2.2:8765", "secret-token", pin), config.load())
        assertEquals(pin, config.load()?.pin)
    }

    @Test
    fun a_cleartext_endpoint_loads_with_a_null_pin() {
        val config = newConfig()
        config.save(SyncSettings("http://10.0.2.2:8765", "secret-token")) // --no-tls, no pin
        assertNull("a cleartext endpoint has no pin", config.load()?.pin)
    }

    @Test
    fun resaving_without_a_pin_clears_a_previously_pinned_one() {
        // Re-pairing a now-cleartext endpoint must not keep pinning the old TLS cert.
        val config = newConfig()
        config.save(SyncSettings("https://pc:1", "t1", "aa".repeat(32)))
        config.save(SyncSettings("http://pc:2", "t2"))
        assertNull("the stale pin must be cleared on a cleartext re-save", config.load()?.pin)
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

    @Test
    fun clearing_unpairs_and_wipes_every_endpoint_file() {
        // Unpair from the settings screen: the URL, the pin, AND the wrapped token must all go,
        // so the device reads as unconfigured and no stale credential or pin survives.
        val dir = Files.createTempDirectory("secure-sync").toFile()
        val config = config(dir)
        config.save(SyncSettings("https://10.0.2.2:8765", "secret-token", "ab".repeat(32)))
        config.clear()
        assertNull("a cleared endpoint reads as unpaired", config.load())
        assertFalse("the URL file is gone", File(dir, SecureSyncConfig.URL_FILE_NAME).exists())
        assertFalse("the pin file is gone", File(dir, SecureSyncConfig.PIN_FILE_NAME).exists())
        assertFalse("the wrapped token file is gone", File(dir, PairingTokenStore.FILE_NAME).exists())
    }

    @Test
    fun clearing_an_already_unpaired_device_is_a_no_op() {
        // Tapping Unpair with nothing paired must not throw.
        newConfig().clear()
    }
}
