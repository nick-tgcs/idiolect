package org.idiolect.android.core

import org.idiolect.ffi.IdiolectInputMethod

/**
 * The single [IdiolectInputMethod] callback handed to the core at construction. The core lives in
 * a process-wide [IdiolectCoreHost] (so it outlives any one IME instance), but its pushes — preedit,
 * commit, errors — must reach whichever consumer is *currently* active.
 *
 * Two layers, so the IME and a headless take don't clobber each other's callbacks:
 *  - **base** ([bind]/[unbind]) — the active IME binds itself; pushes between fields (no base, no
 *    override) are dropped, never crash.
 *  - **override** ([acquireOverride]/[releaseOverride]) — a headless take (the quick-launch button or
 *    the RECOGNIZE_SPEECH / RecognitionService voice provider) takes delivery *above* the base while
 *    it runs. This is load-bearing: the IME can be (re)created and `bind` itself mid-take, and an
 *    override keeps that from stealing the take's finalize callback (the "stuck on Transcribing…"
 *    hang). On [releaseOverride] the base resumes — whatever IME is bound by then.
 *
 * `@Volatile` because the core pushes on its callback thread while bind/override happen on other
 * threads.
 */
class CoreCallbackRouter : IdiolectInputMethod {
    @Volatile
    private var base: IdiolectInputMethod? = null

    @Volatile
    private var override: IdiolectInputMethod? = null

    /** The now-active IME takes over base delivery (replacing any previous base). */
    fun bind(sink: IdiolectInputMethod) {
        this.base = sink
    }

    /**
     * Stop base delivery to [sink]. A no-op if a newer base has already taken over (a late unbind
     * from a torn-down IME must not silence the current one).
     */
    fun unbind(sink: IdiolectInputMethod) {
        if (this.base === sink) this.base = null
    }

    /** A headless take takes delivery above the base while it runs (see the class note). */
    fun acquireOverride(sink: IdiolectInputMethod) {
        this.override = sink
    }

    /** Drop the take's override; the base (the live IME, if any) resumes. No-op if superseded. */
    fun releaseOverride(sink: IdiolectInputMethod) {
        if (this.override === sink) this.override = null
    }

    private fun active(): IdiolectInputMethod? = override ?: base

    override fun recordingStatus(recording: Boolean) {
        active()?.recordingStatus(recording)
    }

    override fun showPreedit(text: String) {
        active()?.showPreedit(text)
    }

    override fun updatePreedit(text: String) {
        active()?.updatePreedit(text)
    }

    override fun commitText(text: String) {
        active()?.commitText(text)
    }

    override fun cancelPreedit() {
        active()?.cancelPreedit()
    }

    override fun insertText(text: String) {
        active()?.insertText(text)
    }

    override fun editHistory(id: Long, text: String) {
        active()?.editHistory(id, text)
    }

    override fun dictationError(message: String) {
        active()?.dictationError(message)
    }
}
