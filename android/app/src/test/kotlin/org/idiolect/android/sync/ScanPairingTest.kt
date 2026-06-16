package org.idiolect.android.sync

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Before
import org.junit.Test
import java.io.File
import java.nio.file.Files

/**
 * Scanning a pairing QR end to end (minus the camera): parse the URI, then exchange the
 * one-time code for a per-device token via [PairingClient]. Host-tested with a fake
 * transport and a temp-dir config, exactly like [PairingClientTest] — the camera/Activity
 * glue that produces the scanned string is the manual emulator e2e.
 */
class ScanPairingTest {
    private lateinit var dir: File

    @Before
    fun setUp() {
        dir = Files.createTempDirectory("scan-pairing").toFile()
    }

    @After
    fun tearDown() {
        dir.deleteRecursively()
    }

    private fun config() = SecureSyncConfig(
        urlFile = File(dir, SecureSyncConfig.URL_FILE_NAME),
        tokenStore = PairingTokenStore(FakeEnvelope(), File(dir, PairingTokenStore.FILE_NAME)),
    )

    @Test
    fun a_scanned_qr_pairs_with_the_url_and_code_it_carries() {
        val transport = RecordingTransport(PairingResponse("tok-xyz", "pixel-7a", "default"))
        val config = config()
        val scan = ScanPairing(PairingClient(config, "pixel-7a") { transport })

        val endpoint =
            scan.pairFromScan("idiolect://pair?u=http%3A%2F%2F10%2E0%2E2%2E2%3A8765&c=GOODCODE")

        // The parsed code reached the transport, and the parsed URL is both the paired
        // endpoint and what got persisted for later syncs.
        assertEquals("GOODCODE", transport.requestedCode)
        assertEquals("http://10.0.2.2:8765", endpoint.baseUrl)
        assertEquals("tok-xyz", endpoint.token)
        val saved = config.load()!!
        assertEquals("http://10.0.2.2:8765", saved.baseUrl)
        assertEquals("tok-xyz", saved.token)
    }

    @Test
    fun a_malformed_qr_throws_before_pairing_and_persists_nothing() {
        val transport = RecordingTransport(PairingResponse("tok", "pixel", "default"))
        val client = PairingClient(config(), "pixel") { transport }

        assertThrows(IllegalArgumentException::class.java) {
            ScanPairing(client).pairFromScan("https://evil.example/login")
        }
        assertNull("a non-pairing QR never reaches the transport", transport.requestedCode)
        assertNull("nothing is persisted on a bad scan", config().load())
    }

    private class RecordingTransport(private val response: PairingResponse) : PairingTransport {
        var requestedCode: String? = null
        var requestedDeviceId: String? = null

        override fun requestToken(code: String, deviceId: String): PairingResponse {
            requestedCode = code
            requestedDeviceId = deviceId
            return response
        }
    }
}
