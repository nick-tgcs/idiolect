package org.idiolect.android.sync

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class PairingResponseTest {
    @Test
    fun parses_a_well_formed_response() {
        val parsed = PairingResponse.parse(
            """{"token":"tok-abc","device_id":"pixel-7a","user_id":"default"}""",
        )
        assertEquals("tok-abc", parsed.token)
        assertEquals("pixel-7a", parsed.deviceId)
        assertEquals("default", parsed.userId)
    }

    @Test
    fun tolerates_reordered_fields_and_whitespace() {
        val parsed = PairingResponse.parse(
            """{ "user_id": "default", "device_id": "pixel", "token": "t" }""",
        )
        assertEquals("t", parsed.token)
        assertEquals("pixel", parsed.deviceId)
        assertEquals("default", parsed.userId)
    }

    @Test
    fun a_missing_field_is_rejected() {
        assertThrows(IllegalArgumentException::class.java) {
            PairingResponse.parse("""{"token":"t","device_id":"pixel"}""")
        }
    }
}
