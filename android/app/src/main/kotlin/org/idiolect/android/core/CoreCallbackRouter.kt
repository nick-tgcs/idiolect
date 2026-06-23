package org.idiolect.android.core

import org.idiolect.ffi.IdiolectInputMethod

/**
 * The single [IdiolectInputMethod] callback handed to the core at construction. The core
 * lives in a process-wide [IdiolectCoreHost] (so it outlives any one IME instance), but its
 * pushes — preedit, commit, errors — must reach whichever IME is *currently* active. The
 * active IME [bind]s itself; pushes that arrive with no sink (between fields, or while the
 * user edits in their own keyboard during review) are dropped, never crash.
 *
 * `@Volatile` because the core pushes on its callback thread while bind/unbind happen on the
 * main thread.
 */
class CoreCallbackRouter : IdiolectInputMethod {
    @Volatile
    private var sink: IdiolectInputMethod? = null

    /** The now-active IME takes over delivery (replacing any previous sink). */
    fun bind(sink: IdiolectInputMethod) {
        this.sink = sink
    }

    /**
     * Stop delivering to [sink]. A no-op if a newer sink has already taken over (a late
     * unbind from a torn-down IME must not silence the current one).
     */
    fun unbind(sink: IdiolectInputMethod) {
        if (this.sink === sink) this.sink = null
    }

    override fun recordingStatus(recording: Boolean) {
        sink?.recordingStatus(recording)
    }

    override fun showPreedit(text: String) {
        sink?.showPreedit(text)
    }

    override fun updatePreedit(text: String) {
        sink?.updatePreedit(text)
    }

    override fun commitText(text: String) {
        sink?.commitText(text)
    }

    override fun cancelPreedit() {
        sink?.cancelPreedit()
    }

    override fun insertText(text: String) {
        sink?.insertText(text)
    }

    override fun editHistory(id: Long, text: String) {
        sink?.editHistory(id, text)
    }

    override fun dictationError(message: String) {
        sink?.dictationError(message)
    }
}
