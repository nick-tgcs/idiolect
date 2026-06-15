package org.idiolect.android.sync

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.nio.file.Files
import java.util.UUID

/**
 * The stable per-install device identifier the PC keys ingest idempotency on
 * (`(device_id, audio_digest)`). Generated once, then persisted under `filesDir` so it
 * survives across app/worker process restarts. Host-tested file IO.
 */
class DeviceIdTest {
    private fun newDeviceId(): Pair<DeviceId, File> {
        val dir = Files.createTempDirectory("device-id").toFile()
        val file = File(dir, DeviceId.FILE_NAME)
        return DeviceId(file) to file
    }

    @Test
    fun get_is_stable_across_calls() {
        val (deviceId, _) = newDeviceId()
        assertEquals(deviceId.get(), deviceId.get())
    }

    @Test
    fun the_id_persists_across_instances() {
        val (first, file) = newDeviceId()
        val id = first.get()
        assertEquals("a fresh instance over the same file reads the saved id", id, DeviceId(file).get())
    }

    @Test
    fun a_fresh_install_mints_a_uuid() {
        val (deviceId, _) = newDeviceId()
        // Parsing as a UUID both checks the shape and rejects empty/garbage.
        assertEquals(deviceId.get(), UUID.fromString(deviceId.get()).toString())
        assertTrue("a real id is non-blank", deviceId.get().isNotBlank())
    }
}
