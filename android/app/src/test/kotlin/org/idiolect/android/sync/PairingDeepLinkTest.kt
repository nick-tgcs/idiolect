package org.idiolect.android.sync

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * The [PairingDeepLink] gate: a fired/tapped `idiolect://pair?…` link is recognised and
 * handed on verbatim, while a plain launch (no data) or any other URL is ignored — so an
 * arbitrary VIEW intent can never be mistaken for a pairing. Pure-string, host-runnable
 * (no Robolectric), mirroring [PairingUriTest]; the recognised string must still satisfy
 * [PairingUri.parse], which the gate defers to for validation.
 */
class PairingDeepLinkTest {
    private val link = "idiolect://pair?u=http%3A%2F%2F10%2E0%2E2%2E2%3A8765&c=ABCD1234"

    @Test
    fun a_pairing_link_is_recognised_and_returned_verbatim() {
        assertEquals(link, PairingDeepLink.fromIntentData(link))
    }

    @Test
    fun a_launch_without_data_is_ignored() {
        assertNull(PairingDeepLink.fromIntentData(null))
        assertNull(PairingDeepLink.fromIntentData(""))
    }

    @Test
    fun a_non_pairing_link_is_ignored() {
        assertNull(PairingDeepLink.fromIntentData("https://example.com/pair?u=x&c=y"))
        assertNull(PairingDeepLink.fromIntentData("idiolect://other?c=ABCD1234"))
    }

    @Test
    fun a_recognised_link_parses_as_a_pairing_uri() {
        // The gate hands the raw string straight to PairingUri.parse — they must agree on
        // what a pairing link is, so a recognised link always parses to a usable endpoint.
        val scanned = PairingUri.parse(PairingDeepLink.fromIntentData(link)!!)
        assertEquals("http://10.0.2.2:8765", scanned.baseUrl)
        assertEquals("ABCD1234", scanned.code)
    }
}
