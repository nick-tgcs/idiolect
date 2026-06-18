package org.idiolect.android.ime

import org.junit.Assert.assertEquals
import org.junit.Test
import java.util.concurrent.Executor

/**
 * Tests the one-tap mic ordering — the part that must be exactly right so no audio is
 * lost. Starting: toggle the core on, *then* begin capture. Stopping: stop+drain
 * capture *first* (so every captured frame is pushed while the core still accepts it),
 * *then* toggle the core to finalize. The core stays the authority on recording state.
 *
 * The sequence runs on an injected [Executor] (a single background thread in production)
 * because the finalize `toggle` re-transcribes the whole take on the calling thread —
 * seconds of whisper work that must never block the IME's UI thread (it ANRs). The
 * ordering tests below use a same-thread executor; [onTap_runs_off_the_calling_thread]
 * pins the off-thread dispatch.
 */
class MicToggleTest {
    /** Runs each task inline, so ordering assertions stay synchronous. */
    private val direct = Executor { it.run() }

    private class Recorder : RecordingToggle, CaptureControl {
        val calls = mutableListOf<String>()
        private var recording = false
        override fun isRecording() = recording
        override fun toggle() {
            recording = !recording
            calls.add(if (recording) "core.start" else "core.finalize")
        }
        override fun startContinuous() {
            recording = true
            calls.add("core.startContinuous")
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
        MicToggle(r, r, direct).onTap()
        assertEquals(listOf("core.start", "capture.start"), r.calls)
    }

    @Test
    fun second_tap_drains_capture_before_finalizing_the_core() {
        val r = Recorder()
        val toggle = MicToggle(r, r, direct)
        toggle.onTap() // start
        r.calls.clear()
        toggle.onTap() // stop
        assertEquals(listOf("capture.stop", "core.finalize"), r.calls)
    }

    @Test
    fun taps_alternate_start_and_stop_across_takes() {
        val r = Recorder()
        val toggle = MicToggle(r, r, direct)
        repeat(2) { toggle.onTap(); toggle.onTap() }
        assertEquals(
            listOf(
                "core.start", "capture.start", "capture.stop", "core.finalize",
                "core.start", "capture.start", "capture.stop", "core.finalize",
            ),
            r.calls,
        )
    }

    @Test
    fun hold_starts_a_take_then_release_drains_capture_before_finalizing() {
        val r = Recorder()
        val toggle = MicToggle(r, r, direct)
        toggle.startHold() // press-and-hold begins
        assertEquals(listOf("core.start", "capture.start"), r.calls)
        r.calls.clear()
        toggle.stop() // release
        assertEquals(listOf("capture.stop", "core.finalize"), r.calls)
    }

    @Test
    fun a_redundant_hold_start_while_recording_is_a_no_op() {
        val r = Recorder()
        val toggle = MicToggle(r, r, direct)
        toggle.startHold()
        r.calls.clear()
        toggle.startHold() // already recording — must not start a second take
        assertEquals(emptyList<String>(), r.calls)
    }

    @Test
    fun double_tap_begins_a_continuous_take() {
        val r = Recorder()
        val toggle = MicToggle(r, r, direct)
        toggle.startContinuous()
        assertEquals(listOf("core.startContinuous", "capture.start"), r.calls)
    }

    @Test
    fun stopping_a_continuous_take_drains_capture_before_finalizing() {
        val r = Recorder()
        val toggle = MicToggle(r, r, direct)
        toggle.startContinuous()
        r.calls.clear()
        toggle.stop()
        assertEquals(listOf("capture.stop", "core.finalize"), r.calls)
    }

    /**
     * The heavy work (the finalize `toggle` re-transcribes the whole take) must be
     * dispatched to the executor, never run on the caller's thread — otherwise the IME's
     * UI thread blocks for seconds and Android kills the app with an ANR. With an executor
     * that only *queues* tasks, [MicToggle.onTap] must return having done nothing itself.
     */
    @Test
    fun onTap_runs_off_the_calling_thread() {
        val r = Recorder()
        val queued = mutableListOf<Runnable>()
        val deferred = Executor { queued.add(it) } // capture the task, never run it inline
        MicToggle(r, r, deferred).onTap()

        assertEquals("onTap must not touch the core on the calling thread", emptyList<String>(), r.calls)
        assertEquals("the work must be queued on the executor", 1, queued.size)

        queued.single().run() // drain it: now the sequence runs
        assertEquals(listOf("core.start", "capture.start"), r.calls)
    }
}
