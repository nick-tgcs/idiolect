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
 *  - **override** ([tryAcquireOverride]/[releaseOverride]) — a headless take (the quick-launch
 *    button or the RECOGNIZE_SPEECH / RecognitionService voice provider) takes delivery *above* the
 *    base while it runs. This is load-bearing: the IME can be (re)created and `bind` itself
 *    mid-take, and an override keeps that from stealing the take's finalize callback (the "stuck on
 *    Transcribing…" hang). The slot is single and FIRST-holder-wins: a second headless take must be
 *    refused (and busy-fail), because replacing the slot would reroute the live take's commit and
 *    finalize to the newcomer — the original caller hangs and the wrong surface gets the
 *    transcript. On [releaseOverride] the base resumes — whatever IME is bound by then.
 *
 * `@Volatile` because the core pushes on its callback thread while bind/override happen on other
 * threads; acquire/release are `@Synchronized` so two takes racing for the slot resolve atomically.
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

    /**
     * Claim delivery above the base for a headless take (see the class note). Returns `false` —
     * claiming NOTHING — while a different take holds the slot; the caller must busy-fail its
     * session rather than run a take whose callbacks would collide with the holder's. Reclaiming
     * the slot already held by [sink] is an idempotent `true`.
     */
    @Synchronized
    fun tryAcquireOverride(sink: IdiolectInputMethod): Boolean {
        if (override != null && override !== sink) return false
        override = sink
        return true
    }

    /** Drop the take's override; the base (the live IME, if any) resumes. No-op unless [sink] is
     *  the holder (a late duplicate release must not evict the next take's claim). */
    @Synchronized
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
