package org.idiolect.android.sync

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Test

/**
 * The phone side of the pairing-QR contract: parsing the `idiolect://pair?u=..&c=..&f=..` URI
 * a PC's `--pair` QR carries. The literals here are byte-for-byte the ones the Rust
 * `pairing_qr` unit test asserts it *produces*, so the two stay in lockstep — a change to
 * one fails the other side's suite. `f` is the server cert's SPKI fingerprint, present under
 * TLS (the default) and absent under `--no-tls`.
 */
class PairingUriTest {
    @Test
    fun parses_the_cleartext_uri_with_no_pin() {
        // The `--no-tls` form: no `f`, so the pin is null (cleartext fallback).
        val parsed = PairingUri.parse("idiolect://pair?u=http%3A%2F%2F10%2E0%2E2%2E2%3A8765&c=ABCD1234")
        assertEquals("http://10.0.2.2:8765", parsed.baseUrl)
        assertEquals("ABCD1234", parsed.code)
        assertNull("no fingerprint means no pin", parsed.pin)
    }

    @Test
    fun parses_the_pinned_uri_the_pc_tls_qr_encodes() {
        // Byte-for-byte the URI the Rust `the_pairing_uri_appends_the_spki_fingerprint_when_pinned`
        // test asserts the server produces under TLS (the default). `f` is the SPKI pin the
        // phone will verify the presented cert against.
        val fingerprint = "0123456789abcdef".repeat(4)
        val parsed = PairingUri.parse(
            "idiolect://pair?u=https%3A%2F%2F10%2E0%2E2%2E2%3A8765&c=ABCD1234&f=$fingerprint",
        )
        assertEquals("https://10.0.2.2:8765", parsed.baseUrl)
        assertEquals("ABCD1234", parsed.code)
        assertEquals(fingerprint, parsed.pin)
    }

    @Test
    fun decodes_a_tailnet_https_base() {
        val parsed = PairingUri.parse("idiolect://pair?u=https%3A%2F%2Fpc%2Eexample%3A443&c=7K9MP2QW")
        assertEquals("https://pc.example:443", parsed.baseUrl)
        assertEquals("7K9MP2QW", parsed.code)
    }

    @Test
    fun rejects_a_uri_that_is_not_a_pairing_qr() {
        // A stray QR (a URL, a wifi config, anything) must not be mistaken for a pairing.
        assertThrows(IllegalArgumentException::class.java) {
            PairingUri.parse("https://evil.example/login")
        }
    }

    @Test
    fun rejects_a_pairing_uri_missing_the_code() {
        assertThrows(IllegalArgumentException::class.java) {
            PairingUri.parse("idiolect://pair?u=http%3A%2F%2F10%2E0%2E2%2E2%3A8765")
        }
    }

    @Test
    fun rejects_a_pairing_uri_missing_the_url() {
        assertThrows(IllegalArgumentException::class.java) {
            PairingUri.parse("idiolect://pair?c=ABCD1234")
        }
    }
}
