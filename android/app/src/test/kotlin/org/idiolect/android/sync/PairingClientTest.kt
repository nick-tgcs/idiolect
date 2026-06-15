package org.idiolect.android.sync

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Before
import org.junit.Test
import java.io.File

class PairingClientTest {
    private lateinit var configFile: File

    @Before
    fun setUp() {
        // Created then deleted, so the device starts unconfigured (load() == null).
        configFile = File.createTempFile("sync", ".config").also { it.delete() }
    }

    @After
    fun tearDown() {
        configFile.delete()
    }

    private fun config() = SyncConfig(configFile)

    @Test
    fun a_successful_pair_persists_the_endpoint_and_token() {
        val transport = RecordingTransport(PairingResponse("tok-xyz", "pixel-7a", "default"))
        val config = config()
        val client = PairingClient(config, "pixel-7a") { transport }

        val response = client.pair("http://pc.local:8765", "GOODCODE")

        assertEquals("tok-xyz", response.token)
        assertEquals("the code reached the transport", "GOODCODE", transport.requestedCode)
        assertEquals("the device id reached the transport", "pixel-7a", transport.requestedDeviceId)
        val saved = config.load()!!
        assertEquals("http://pc.local:8765", saved.baseUrl)
        assertEquals("tok-xyz", saved.token)
    }

    @Test
    fun a_failed_pair_persists_nothing() {
        val client = PairingClient(config(), "pixel") {
            object : PairingTransport {
                override fun requestToken(code: String, deviceId: String): PairingResponse =
                    throw IllegalArgumentException("pairing failed: HTTP 401")
            }
        }

        assertThrows(IllegalArgumentException::class.java) {
            client.pair("http://pc.local:8765", "WRONGCOD")
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
