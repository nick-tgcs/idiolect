package org.idiolect.android.ime

import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.KeyEvent
import android.view.inputmethod.CompletionInfo
import android.view.inputmethod.CorrectionInfo
import android.view.inputmethod.ExtractedText
import android.view.inputmethod.ExtractedTextRequest
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputContentInfo
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf

/**
 * Robolectric cover for the one non-trivial bit of the framework seam: the ⌫ delete key must
 * remove a whole Unicode code point, not a single UTF-16 code unit — otherwise deleting after an
 * emoji (a surrogate pair) leaves half a character behind. We assert the seam calls
 * `deleteSurroundingTextInCodePoints`, which the framework resolves to a full code point.
 */
@RunWith(RobolectricTestRunner::class)
class InputConnectionFieldEditorTest {
    @Test
    fun delete_removes_a_whole_code_point_so_it_never_splits_an_emoji_surrogate_pair() {
        val connection = RecordingInputConnection()
        InputConnectionFieldEditor(connection).deleteBackward()
        shadowOf(Looper.getMainLooper()).idle()
        assertEquals(listOf("deleteCodePoints:1,0"), connection.deletes)
    }

    /** Records only the delete calls; everything else is an inert stub. */
    private class RecordingInputConnection : InputConnection {
        val deletes = mutableListOf<String>()

        override fun deleteSurroundingText(before: Int, after: Int): Boolean {
            deletes.add("deleteUnits:$before,$after")
            return true
        }

        override fun deleteSurroundingTextInCodePoints(before: Int, after: Int): Boolean {
            deletes.add("deleteCodePoints:$before,$after")
            return true
        }

        // --- unused InputConnection surface (inert stubs) ---
        override fun getTextBeforeCursor(n: Int, flags: Int): CharSequence = ""
        override fun getTextAfterCursor(n: Int, flags: Int): CharSequence = ""
        override fun getSelectedText(flags: Int): CharSequence? = null
        override fun getCursorCapsMode(reqModes: Int): Int = 0
        override fun getExtractedText(request: ExtractedTextRequest?, flags: Int): ExtractedText? = null
        override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean = true
        override fun setComposingRegion(start: Int, end: Int): Boolean = true
        override fun finishComposingText(): Boolean = true
        override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean = true
        override fun commitCompletion(text: CompletionInfo?): Boolean = true
        override fun commitCorrection(correctionInfo: CorrectionInfo?): Boolean = true
        override fun setSelection(start: Int, end: Int): Boolean = true
        override fun performEditorAction(editorAction: Int): Boolean = true
        override fun performContextMenuAction(id: Int): Boolean = true
        override fun beginBatchEdit(): Boolean = true
        override fun endBatchEdit(): Boolean = true
        override fun sendKeyEvent(event: KeyEvent?): Boolean = true
        override fun clearMetaKeyStates(states: Int): Boolean = true
        override fun reportFullscreenMode(enabled: Boolean): Boolean = true
        override fun performPrivateCommand(action: String?, data: Bundle?): Boolean = true
        override fun requestCursorUpdates(cursorUpdateMode: Int): Boolean = true
        override fun getHandler(): Handler? = null
        override fun closeConnection() {}
        override fun commitContent(inputContentInfo: InputContentInfo, flags: Int, opts: Bundle?): Boolean = true
    }
}
