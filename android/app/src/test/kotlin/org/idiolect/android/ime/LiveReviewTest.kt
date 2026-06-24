package org.idiolect.android.ime

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The same-process streaming channel that carries live partials from the core's callback
 * thread to the IME's review surface (sibling to [PendingInsert]). Pure JVM — just `String`
 * callbacks, no Android — so the guard semantics are unit-tested directly.
 */
class LiveReviewTest {
    private class Sink : LiveReview.Listener {
        val ops = mutableListOf<String>()
        override fun onLivePreedit(text: String) { ops.add(text) }
    }

    @After
    fun unbind() {
        // Process-wide singleton: don't leak a listener into the next test.
        LiveReview.bind(null)
    }

    @Test
    fun push_forwards_to_the_bound_listener() {
        val sink = Sink()
        LiveReview.bind(sink)
        LiveReview.push("the quick")
        LiveReview.push("the quick brown")
        assertEquals(listOf("the quick", "the quick brown"), sink.ops)
    }

    @Test
    fun reset_clears_the_surface_with_an_empty_push() {
        val sink = Sink()
        LiveReview.bind(sink)
        LiveReview.reset()
        assertEquals(listOf(""), sink.ops)
    }

    @Test
    fun push_after_unbind_is_a_no_op() {
        val sink = Sink()
        LiveReview.bind(sink)
        LiveReview.bind(null)
        LiveReview.push("ignored")
        assertEquals(emptyList<String>(), sink.ops)
    }

    @Test
    fun binding_a_new_listener_replaces_the_old_one() {
        val first = Sink()
        val second = Sink()
        LiveReview.bind(first)
        LiveReview.bind(second)
        LiveReview.push("hi")
        assertEquals(emptyList<String>(), first.ops)
        assertEquals(listOf("hi"), second.ops)
    }
}
