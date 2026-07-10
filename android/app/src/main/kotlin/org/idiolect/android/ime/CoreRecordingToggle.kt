package org.idiolect.android.ime

import org.idiolect.ffi.IdiolectCore

/**
 * Adapts [IdiolectCore] to the [RecordingToggle] that the mic key — and the headless recognition
 * take (`ACTION_RECOGNIZE_SPEECH` / the system speech service) — drive. A pure forwarding adapter:
 * the core is the authority on recording state, so there is nothing here to unit-test (the
 * sequencing that uses it lives in [MicToggle], covered with a fake toggle).
 *
 * [ephemeral] routes the take to the core's transcription-only path: the recognition surface has
 * no `EditorInfo`, so it can't tell a password/PIN field from any other — an ephemeral take
 * commits the transcript but persists nothing (no history, audio, or training pair). The IME
 * (which *does* see the field) uses the normal, persisting path.
 */
class CoreRecordingToggle(
    private val core: IdiolectCore,
    private val ephemeral: Boolean = false,
) : RecordingToggle {
    override fun isRecording(): Boolean = core.isRecording()

    override fun toggle() {
        if (ephemeral) core.toggleEphemeral() else core.toggle()
    }

    override fun startContinuous() {
        core.startContinuous()
    }

    override fun cancel() {
        core.cancel()
    }
}
