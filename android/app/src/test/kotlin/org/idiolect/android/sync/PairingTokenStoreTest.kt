package org.idiolect.android.sync

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Test
import java.io.File

/**
 * The wrap-on-save / unwrap-on-load logic of [PairingTokenStore], host-tested with the
 * reversible [FakeEnvelope] (the real AndroidKeyStore-backed envelope is a device seam).
 * This is the direct analogue of `crypto/HistoryKeyStoreTest`, but for the per-device
 * sync bearer token rather than a generated key: it proves the token round-trips and is
 * never written to disk in the clear.
 */
class PairingTokenStoreTest {
    private fun tempTokenFile(): File =
        File.createTempFile("sync-token", ".enc").apply { delete() }

    @Test
    fun a_saved_token_round_trips() {
        val file = tempTokenFile()
        PairingTokenStore(FakeEnvelope(), file).save("tok-secret")
        assertEquals("tok-secret", PairingTokenStore(FakeEnvelope(), file).load())
    }

    @Test
    fun load_is_null_before_anything_is_saved() {
        assertNull(PairingTokenStore(FakeEnvelope(), tempTokenFile()).load())
    }

    @Test
    fun the_token_is_stored_wrapped_never_in_plaintext() {
        val file = tempTokenFile()
        PairingTokenStore(FakeEnvelope(), file).save("tok-secret")
        assertFalse(
            "the raw token must not appear on disk",
            file.readBytes().contentEquals("tok-secret".toByteArray(Charsets.UTF_8)),
        )
    }

    @Test
    fun a_resaved_token_overwrites_the_previous_one() {
        val file = tempTokenFile()
        PairingTokenStore(FakeEnvelope(), file).save("old")
        PairingTokenStore(FakeEnvelope(), file).save("new")
        assertEquals("new", PairingTokenStore(FakeEnvelope(), file).load())
    }
}
