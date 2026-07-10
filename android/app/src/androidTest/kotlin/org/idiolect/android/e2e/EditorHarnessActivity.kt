package org.idiolect.android.e2e

import android.app.Activity
import android.os.Bundle
import android.text.InputType
import android.view.WindowManager
import android.widget.EditText
import android.widget.LinearLayout

/**
 * A trivial host with two focusable fields — a free-text [EditText] and a numeric one — so the
 * e2e tests have real fields to summon idiolect into. Lives only in the androidTest APK (it is
 * not part of the shipped app). Focusing a field brings up whatever IME is selected; the tests
 * select idiolect. The numeric field exercises the field-type hand-off (idiolect refuses to
 * default its mic to numeric/PIN fields).
 */
class EditorHarnessActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val text = EditText(this).apply {
            contentDescription = FIELD_DESC
            hint = "dictate here"
            // Multiline (no declared IME action) so the ⏎ enter key's newline is observable
            // and deterministic in the edit-keys e2e — a plain single-line field would strip it.
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE
        }
        val number = EditText(this).apply {
            contentDescription = NUMBER_FIELD_DESC
            hint = "PIN / amount"
            inputType = InputType.TYPE_CLASS_NUMBER
        }
        setContentView(
            LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                val lp = LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.MATCH_PARENT,
                    LinearLayout.LayoutParams.WRAP_CONTENT,
                )
                addView(text, lp)
                addView(number, lp)
            },
        )
        text.requestFocus()
        window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE)
    }

    companion object {
        const val FIELD_DESC = "harness_field"
        const val NUMBER_FIELD_DESC = "harness_number_field"
    }
}
