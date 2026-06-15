package org.idiolect.android.ime

/** The core's recording state + edge toggle (`IdiolectCore.isRecording`/`toggle`). */
interface RecordingToggle {
    fun isRecording(): Boolean
    fun toggle()
}

/** Start/stop of a dictation take's capture (satisfied by [DictationController]). */
interface CaptureControl {
    fun start()
    fun stop()
}

/**
 * The one-tap mic key. The core is the authority on recording state; this just
 * sequences the capture lifecycle around the core's edge toggle so no audio is lost:
 *
 *  - start: toggle the core on (it begins accepting frames), *then* begin capture;
 *  - stop: stop + drain capture *first* (every captured frame is pushed while the core
 *    still accepts it), *then* toggle the core to finalize the take.
 */
class MicToggle(
    private val core: RecordingToggle,
    private val capture: CaptureControl,
) {
    fun onTap() {
        if (core.isRecording()) {
            capture.stop()
            core.toggle()
        } else {
            core.toggle()
            capture.start()
        }
    }
}
