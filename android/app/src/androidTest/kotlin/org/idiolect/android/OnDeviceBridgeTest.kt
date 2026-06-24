package org.idiolect.android

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.idiolect.android.crypto.HistoryKey
import org.idiolect.ffi.FfiException
import org.idiolect.ffi.HistoryItem
import org.idiolect.ffi.IdiolectCore
import org.idiolect.ffi.IdiolectInputMethod
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/**
 * On-device proof that the cross-compiled native core (whisper.cpp/ggml + opus +
 * sqlite, built under the NDK and packaged in jniLibs) actually loads and runs on
 * Android through the jna@aar bindings — the one thing the host-JVM BridgeTest cannot
 * establish. Mirrors BridgeTest's assertions over the same marshalling surface.
 */
@RunWith(AndroidJUnit4::class)
class OnDeviceBridgeTest {
    private class Recording : IdiolectInputMethod {
        val events = mutableListOf<String>()
        override fun recordingStatus(recording: Boolean) {
            events.add("recording:$recording")
        }

        override fun showPreedit(text: String) {}
        override fun updatePreedit(text: String) {}
        override fun commitText(text: String) {
            events.add("commit:$text")
        }

        override fun cancelPreedit() {}
        override fun insertText(text: String) {}
        override fun editHistory(id: Long, text: String) {}
        override fun dictationError(message: String) {}
    }

    private fun newCore(callback: IdiolectInputMethod): IdiolectCore {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val dir = File(context.filesDir, "bridge-${System.nanoTime()}").apply { mkdirs() }
        return IdiolectCore(dir.absolutePath, null, callback)
    }

    @Test
    fun the_native_core_loads_and_round_trips_the_recording_lifecycle() {
        val callback = Recording()
        newCore(callback).use { core ->
            assertFalse(core.isRecording())
            core.toggle()
            assertTrue(core.isRecording())
            val frame = List(480) { 0.toShort() }
            repeat(16) { core.pushPcmFrame(frame) }
            core.toggle()
            assertFalse(core.isRecording())
        }
        assertTrue(callback.events.contains("recording:true"))
        assertTrue(callback.events.contains("recording:false"))
        assertTrue(callback.events.none { it.startsWith("commit:") })
    }

    @Test
    fun an_empty_store_has_no_history() {
        newCore(Recording()).use { core ->
            val history: List<HistoryItem> = core.recentHistory(10u)
            assertEquals(0, history.size)
        }
    }

    @Test
    fun editing_a_missing_history_entry_throws_a_typed_error() {
        newCore(Recording()).use { core ->
            assertThrows(FfiException.HistoryEntryNotFound::class.java) {
                core.historyEdited(424_242L, "no such row")
            }
        }
    }

    @Test
    fun the_core_opens_with_a_keystore_backed_history_key() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val dir = File(context.filesDir, "keyed-${System.nanoTime()}").apply { mkdirs() }
        // Exercises the real AndroidKeyStore wrap/unwrap on the device.
        val key = HistoryKey.load(File(dir, HistoryKey.FILE_NAME))
        assertEquals(32, key.size)
        IdiolectCore(dir.absolutePath, key, Recording()).use { core ->
            assertTrue(core.recentHistory(10u).isEmpty())
        }
    }

    @Test
    fun load_model_verified_surfaces_a_typed_error_on_device() {
        // The M5 verify-at-load surface is reachable on the cross-compiled core: an
        // absent model file fails the integrity check as a typed FfiException, not a crash.
        newCore(Recording()).use { core ->
            assertThrows(FfiException::class.java) {
                core.loadModelVerified("/no/such/model.bin", "0".repeat(64))
            }
        }
    }
}
