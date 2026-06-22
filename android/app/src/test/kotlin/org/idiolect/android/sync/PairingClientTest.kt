package org.idiolect.android.sync

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Before
import org.junit.Test
import java.io.File
import java.nio.file.Files

class PairingClientTest {
    private lateinit var dir: File

    @Before
    fun setUp() {
        // A fresh empty dir, so the device starts unconfigured (load() == null).
        dir = Files.createTempDirectory("pairing-client").toFile()
    }

    @After
    fun tearDown() {
        dir.deleteRecursively()
    }

    private fun config() = SecureSyncConfig(
        urlFile = File(dir, SecureSyncConfig.URL_FILE_NAME),
        tokenStore = PairingTokenStore(FakeEnvelope(), File(dir, PairingTokenStore.FILE_NAME)),
        pinFile = File(dir, SecureSyncConfig.PIN_FILE_NAME),
    )

    @Test
    fun a_successful_cleartext_pair_persists_the_endpoint_and_token() {
        val transport = RecordingTransport(PairingResponse("tok-xyz", "pixel-7a", "default"))
        val config = config()
        val client = PairingClient(config, "pixel-7a") { _, _ -> transport }

        val response = client.pair("http://pc.local:8765", "GOODCODE", pin = null)

        assertEquals("tok-xyz", response.token)
        assertEquals("the code reached the transport", "GOODCODE", transport.requestedCode)
        assertEquals("the device id reached the transport", "pixel-7a", transport.requestedDeviceId)
        val saved = config.load()!!
        assertEquals("http://pc.local:8765", saved.baseUrl)
        assertEquals("tok-xyz", saved.token)
        assertNull("a cleartext pair persists no pin", saved.pin)
    }

    @Test
    fun a_pinned_pair_hands_the_pin_to_the_transport_and_persists_it() {
        // TLS (the default): the pin from the QR must reach the transport (so it pins the
        // handshake) and be persisted (so later syncs re-pin the same cert).
        val transport = RecordingTransport(PairingResponse("tok-xyz", "pixel-7a", "default"))
        val config = config()
        var builtPin: String? = "UNSET"
        val client = PairingClient(config, "pixel-7a") { _, pin -> builtPin = pin; transport }
        val pin = "deadbeef".repeat(8)

        client.pair("https://10.0.2.2:8765", "GOODCODE", pin)

        assertEquals("the pin reached the transport factory", pin, builtPin)
        assertEquals("the pin is persisted for later syncs", pin, config.load()!!.pin)
    }

    @Test
    fun a_failed_pair_persists_nothing() {
        val client = PairingClient(config(), "pixel") { _, _ ->
            object : PairingTransport {
                override fun requestToken(code: String, deviceId: String): PairingResponse =
                    throw IllegalArgumentException("pairing failed: HTTP 401")
            }
        }

        assertThrows(IllegalArgumentException::class.java) {
            client.pair("http://pc.local:8765", "WRONGCOD", pin = null)
        }
        assertNull("a rejected code leaves the device unconfigured", config().load())
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
