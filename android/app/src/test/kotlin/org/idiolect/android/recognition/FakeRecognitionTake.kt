package org.idiolect.android.recognition

import org.idiolect.android.model.InstalledModel

/** A headless [RecognitionTake] recording the order of lifecycle calls, so service teardown
 *  ordering (cancel a live capture BEFORE releasing the core) is assertable without the
 *  native core. */
class FakeRecognitionTake : RecognitionTake {
    val calls = mutableListOf<String>()

    override fun begin(model: InstalledModel, output: RecognitionOutput) {
        calls += "begin"
    }

    override fun stopListening() {
        calls += "stopListening"
    }

    override fun cancel() {
        calls += "cancel"
    }

    override fun release() {
        calls += "release"
    }
}
