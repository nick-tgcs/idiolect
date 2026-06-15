package org.idiolect.android.ime

/** The two in-place modes of the single IME surface (plan §1.2 / §1.3). */
enum class KeyboardMode { Voice, Edit }

/**
 * Tracks which mode the IME view shows. Voice is the default (voice is the hero); the
 * toggle is symmetric so either mode is always one tap from the other. Synchronized
 * because the mic key (main thread) and a correction-strip tap may both drive it.
 */
class ModePresenter {
    private var mode = KeyboardMode.Voice

    @Synchronized
    fun current(): KeyboardMode = mode

    @Synchronized
    fun toggle(): KeyboardMode =
        show(if (mode == KeyboardMode.Voice) KeyboardMode.Edit else KeyboardMode.Voice)

    @Synchronized
    fun show(mode: KeyboardMode): KeyboardMode {
        this.mode = mode
        return mode
    }
}
