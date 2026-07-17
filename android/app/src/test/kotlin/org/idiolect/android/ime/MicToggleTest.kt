package org.idiolect.android.ime

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.Executor
import java.util.concurrent.RejectedExecutionException

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
        override fun cancel() {
            recording = false
            calls.add("core.cancel")
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

    @Test
    fun a_tap_is_refused_when_starting_is_blocked() {
        // A blocked start (a password/PIN field) must never toggle the core or begin capture —
        // the secret take must not reach the core at all (it would persist audio + a training row).
        val r = Recorder()
        MicToggle(r, r, direct, canStart = { false }).onTap()
        assertEquals(emptyList<String>(), r.calls)
    }

    @Test
    fun a_hold_is_refused_when_starting_is_blocked() {
        val r = Recorder()
        MicToggle(r, r, direct, canStart = { false }).startHold()
        assertEquals(emptyList<String>(), r.calls)
    }

    @Test
    fun a_continuous_take_is_refused_when_starting_is_blocked() {
        val r = Recorder()
        MicToggle(r, r, direct, canStart = { false }).startContinuous()
        assertEquals(emptyList<String>(), r.calls)
    }

    @Test
    fun stopping_is_allowed_even_when_starting_is_blocked() {
        // The gate blocks only *starting*; a take already running must still stop and finalize
        // cleanly (never leave the core recording).
        val r = Recorder()
        MicToggle(r, r, direct).onTap() // start a take (allowed)
        r.calls.clear()
        MicToggle(r, r, direct, canStart = { false }).stop() // a blocked toggle must still stop it
        assertEquals(listOf("capture.stop", "core.finalize"), r.calls)
    }

    @Test
    fun cancel_discards_a_running_take_without_finalizing() {
        // Abandoning a take (a learning-blocked field mid-take, a recognition caller's cancel)
        // must *discard* it — cancel in the core, then drain capture (never finalize, which
        // would persist audio/history and decode the whole take).
        val r = Recorder()
        val toggle = MicToggle(r, r, direct)
        toggle.onTap() // recording
        r.calls.clear()
        toggle.cancel()
        // Cancel the core *before* draining capture, so frames that drain after the discard are
        // rejected by the core (NoActiveTake) instead of being finalized/persisted.
        assertEquals(listOf("core.cancel", "capture.stop"), r.calls)
    }

    @Test
    fun cancel_when_idle_is_a_no_op() {
        val r = Recorder()
        MicToggle(r, r, direct).cancel()
        assertEquals(emptyList<String>(), r.calls)
    }

    @Test
    fun cancel_swallows_a_racing_core_error_and_still_stops_capture() {
        // The core is process-wide (the recognition service drives it too), so a stop on another
        // surface can leave core.cancel() throwing NoActiveTake after our isRecording() check. It
        // must not crash the executor thread, and capture must still be stopped.
        val capture = Recorder()
        val throwingCore = object : RecordingToggle {
            override fun isRecording() = true
            override fun toggle() {}
            override fun startContinuous() {}
            override fun cancel(): Unit = throw RuntimeException("NoActiveTake")
        }
        MicToggle(throwingCore, capture, direct).cancel() // must not throw
        assertEquals(listOf("capture.stop"), capture.calls)
    }

    @Test
    fun a_task_rejected_by_a_shut_down_executor_does_not_crash() {
        // During teardown toggleExecutor is shut down; a late gesture timer submitting work must
        // be a harmless no-op, not an uncaught RejectedExecutionException.
        val r = Recorder()
        val rejecting = Executor { throw RejectedExecutionException() }
        MicToggle(r, r, rejecting).cancel()
        MicToggle(r, r, rejecting).onTap()
        MicToggle(r, r, rejecting).startHold()
        MicToggle(r, r, rejecting).startContinuous()
        assertTrue(r.calls.isEmpty())
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
