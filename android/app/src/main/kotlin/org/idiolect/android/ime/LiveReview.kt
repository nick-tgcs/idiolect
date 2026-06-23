package org.idiolect.android.ime

/**
 * The live-streaming sibling of [PendingInsert]. In review mode the core's partial transcript
 * must not be typed into the host field (it would land in the target app, then be clawed back);
 * instead each partial is pushed here and rendered on idiolect's own review surface as the user
 * speaks. Same process (the IME service and [ReviewActivity] share one), so a guarded singleton
 * with a single bound listener is enough — exactly the pattern [PendingInsert] already proves.
 *
 * Framework-free (`String` callbacks only) so it's unit-testable on the JVM. [push] is invoked
 * on the core's callback thread; the listener marshals to the main thread itself.
 */
object LiveReview {
    /** Receives live partials; an empty string means "clear the surface". */
    fun interface Listener {
        fun onLivePreedit(text: String)
    }

    @Volatile
    private var listener: Listener? = null

    /** Bind the surface that renders live partials, or `null` to unbind (no take in progress). */
    @Synchronized
    fun bind(listener: Listener?) {
        this.listener = listener
    }

    /** Push a live partial to the bound surface; a no-op when nothing is bound. */
    fun push(text: String) {
        listener?.onLivePreedit(text)
    }

    /** Clear the surface (take ended / preedit cancelled). */
    fun reset() = push("")
}
