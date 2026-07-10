package org.idiolect.android.ime

import org.idiolect.ffi.IdiolectCore

/**
 * Adapts [IdiolectCore] to the [RecordingToggle] that the mic key — and the headless recognition
 * take (`ACTION_RECOGNIZE_SPEECH` / the system speech service) — drive. A pure forwarding adapter:
 * the core is the authority on recording state, so there is nothing here to unit-test (the
 * sequencing that uses it lives in [MicToggle], covered with a fake toggle).
 */
class CoreRecordingToggle(private val core: IdiolectCore) : RecordingToggle {
    override fun isRecording(): Boolean = core.isRecording()

    override fun toggle() {
        core.toggle()
    }

    override fun startContinuous() {
        core.startContinuous()
    }

    override fun cancel() {
        core.cancel()
    }
}
