package org.idiolect.android.accessibility

/**
 * The pure splice behind accessibility insertion: given a field's current text and selection,
 * compute the text + cursor after the reviewed correction is dropped in at the cursor (or over
 * the selection). Kept separate from the [IdiolectAccessibilityService] node calls so the
 * arithmetic — the part that can be wrong — is unit-tested without a real field.
 */
object TextInjection {
    data class Spliced(val text: String, val cursor: Int)

    /**
     * Splice [injected] into [existing] at [selStart]..[selEnd]. A collapsed selection inserts;
     * a range replaces. A negative index (a node with no selection) appends at the end. Indices
     * are clamped to the text, and a backwards selection (start > end) is normalised.
     */
    fun spliceAtSelection(existing: String, selStart: Int, selEnd: Int, injected: String): Spliced {
        val len = existing.length
        val (start, end) = if (selStart < 0 || selEnd < 0) {
            len to len
        } else {
            val a = selStart.coerceIn(0, len)
            val b = selEnd.coerceIn(0, len)
            minOf(a, b) to maxOf(a, b)
        }
        val text = existing.substring(0, start) + injected + existing.substring(end)
        return Spliced(text, start + injected.length)
    }
}
