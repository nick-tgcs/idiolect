package org.idiolect.android.ime

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Tests the one-tap mic ordering — the part that must be exactly right so no audio is
 * lost. Starting: toggle the core on, *then* begin capture. Stopping: stop+drain
 * capture *first* (so every captured frame is pushed while the core still accepts it),
 * *then* toggle the core to finalize. The core stays the authority on recording state.
 */
class MicToggleTest {
    private class Recorder : RecordingToggle, CaptureControl {
        val calls = mutableListOf<String>()
        private var recording = false
        override fun isRecording() = recording
        override fun toggle() {
            recording = !recording
            calls.add(if (recording) "core.start" else "core.finalize")
        }
        override fun start() {
            calls.add("capture.start")
        }
        override fun stop() {
            calls.add("capture.stop")
        }
    }

    @Test
    fun first_tap_starts_the_core_then_begins_capture() {
        val r = Recorder()
        MicToggle(r, r).onTap()
        assertEquals(listOf("core.start", "capture.start"), r.calls)
    }

    @Test
    fun second_tap_drains_capture_before_finalizing_the_core() {
        val r = Recorder()
        val toggle = MicToggle(r, r)
        toggle.onTap() // start
        r.calls.clear()
        toggle.onTap() // stop
        assertEquals(listOf("capture.stop", "core.finalize"), r.calls)
    }

    @Test
    fun taps_alternate_start_and_stop_across_takes() {
        val r = Recorder()
        val toggle = MicToggle(r, r)
        repeat(2) { toggle.onTap(); toggle.onTap() }
        assertEquals(
            listOf(
                "core.start", "capture.start", "capture.stop", "core.finalize",
                "core.start", "capture.start", "capture.stop", "core.finalize",
            ),
            r.calls,
        )
    }
}
