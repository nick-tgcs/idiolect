package org.idiolect.android.ime

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The correction-capture orchestrator: it remembers the committed take, selects a
 * tapped word's range and flips to edit, and on capture reads the whole field back and
 * records a raw→corrected pair only when the text actually changed (the Android
 * ground-truth-from-the-field model; plan §1.4). Reading the field back is exactly the
 * desktop "read the field" capture — we never trust our own optimistic state.
 */
class CorrectionCaptureTest {
    private class FakeField(var text: String = "") : FieldEditor {
        val ops = mutableListOf<String>()
        override fun setComposingText(text: String) {}
        override fun commitText(text: String) {}
        override fun finishComposingText() {}
        override fun deleteBackward() {}
        override fun setSelection(start: Int, end: Int) { ops.add("select:$start:$end") }
        override fun fieldText(): String = text
    }

    private fun capture(
        field: FieldEditor?,
        reported: MutableList<String> = mutableListOf(),
        edits: MutableList<Unit> = mutableListOf(),
    ) = CorrectionCapture(
        editor = { field },
        reportCorrection = { reported.add(it) },
        onEnterEdit = { edits.add(Unit) },
    )

    @Test
    fun a_committed_take_becomes_word_chips() {
        val cc = capture(FakeField())
        assertEquals(listOf("send", "him", "the"), cc.onTakeCommitted("send him the").map { it.text })
        assertEquals(3, cc.currentChips().size)
    }

    @Test
    fun tapping_a_word_selects_its_range_and_enters_edit() {
        val field = FakeField("send him the")
        val edits = mutableListOf<Unit>()
        val cc = capture(field, edits = edits)
        cc.onTakeCommitted("send him the")
        cc.selectWord(1)
        assertEquals(listOf("select:5:8"), field.ops)
        assertEquals(1, edits.size)
    }

    @Test
    fun selecting_an_out_of_range_word_is_a_no_op() {
        val field = FakeField("send him the")
        val edits = mutableListOf<Unit>()
        val cc = capture(field, edits = edits)
        cc.onTakeCommitted("send him the")
        cc.selectWord(99)
        assertTrue(field.ops.isEmpty())
        assertTrue(edits.isEmpty())
    }

    @Test
    fun capture_records_the_pair_only_when_the_field_changed() {
        val field = FakeField()
        val reported = mutableListOf<String>()
        val cc = capture(field, reported = reported)
        cc.onTakeCommitted("send him the")

        field.text = "send him the"
        assertFalse(cc.capture())
        assertTrue(reported.isEmpty())

        field.text = "send them the"
        assertTrue(cc.capture())
        assertEquals(listOf("send them the"), reported)
        // Baseline advances: a second capture of the same text is a no-op...
        assertFalse(cc.capture())
        // ...and the chips now reflect the corrected take.
        assertEquals(listOf("send", "them", "the"), cc.currentChips().map { it.text })
    }

    @Test
    fun disarm_drops_the_pending_baseline_so_a_later_capture_reports_nothing() {
        // A secret take (dictated into a password/PIN field) must never linger: after disarm,
        // a following capture on an unrelated field records no raw→corrected pair — the secret
        // is never amended with foreign text, so nothing syncable is produced.
        val field = FakeField()
        val reported = mutableListOf<String>()
        val cc = capture(field, reported = reported)
        cc.onTakeCommitted("secret")

        cc.disarm()

        field.text = "an unrelated later field"
        assertFalse(cc.capture())
        assertTrue(reported.isEmpty())
        assertTrue(cc.currentChips().isEmpty())
    }

    @Test
    fun capture_before_any_take_or_without_a_field_is_a_no_op() {
        val reported = mutableListOf<String>()
        // No take committed yet.
        assertFalse(capture(FakeField("x"), reported).capture())
        // No focused field.
        val cc = capture(null, reported)
        cc.onTakeCommitted("hi")
        assertFalse(cc.capture())
        assertTrue(reported.isEmpty())
    }
}
