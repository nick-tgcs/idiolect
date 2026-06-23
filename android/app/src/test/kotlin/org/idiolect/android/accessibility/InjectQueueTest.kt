package org.idiolect.android.accessibility

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

/**
 * Unit cover for [InjectQueue] — the one-slot hand-off the review dialog writes and the
 * [IdiolectAccessibilityService] drains. It's a **file** (not an in-memory singleton) on
 * purpose: the dialog and the bound accessibility service can run in different processes, so
 * the pending correction must survive the process boundary. Exercised here over a temp file.
 */
class InjectQueueTest {
    @get:Rule
    val tmp = TemporaryFolder()

    private fun queue(): InjectQueue = InjectQueue(File(tmp.root, "pending_insert"))

    @Test
    fun take_returns_what_was_put_then_clears() {
        val q = queue()
        q.put("restart the traffic service")
        assertEquals("restart the traffic service", q.take())
        assertNull("a second take must be empty", q.take())
    }

    @Test
    fun take_on_an_empty_queue_is_null() {
        assertNull(queue().take())
    }

    @Test
    fun put_overwrites_a_pending_value() {
        val q = queue()
        q.put("first")
        q.put("second")
        assertEquals("second", q.take())
    }

    @Test
    fun a_separate_instance_over_the_same_file_sees_the_value() {
        // The cross-process case: the service constructs its own InjectQueue on the same path.
        val file = File(tmp.root, "pending_insert")
        InjectQueue(file).put("from the dialog")
        assertEquals("from the dialog", InjectQueue(file).take())
    }

    @Test
    fun preserves_newlines_and_unicode() {
        val q = queue()
        q.put("line one\nlíne twö 🚦")
        assertEquals("line one\nlíne twö 🚦", q.take())
    }
}
