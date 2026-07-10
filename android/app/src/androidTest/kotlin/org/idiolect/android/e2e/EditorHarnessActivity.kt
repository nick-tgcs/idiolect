package org.idiolect.android.e2e

import android.app.Activity
import android.os.Bundle
import android.text.InputType
import android.view.WindowManager
import android.widget.EditText
import android.widget.LinearLayout

/**
 * A trivial host with a single focusable [EditText], so the e2e tests have a real field to
 * summon idiolect into. Lives only in the androidTest APK (it is not part of the shipped
 * app). Focusing the field brings up whatever IME is selected — the tests select idiolect.
 */
class EditorHarnessActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val field = EditText(this).apply {
            contentDescription = FIELD_DESC
            hint = "dictate here"
            // Multiline (no declared IME action) so the ⏎ enter key's newline is observable
            // and deterministic in the edit-keys e2e — a plain single-line field would strip it.
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE
        }
        setContentView(
            LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                addView(
                    field,
                    LinearLayout.LayoutParams(
                        LinearLayout.LayoutParams.MATCH_PARENT,
                        LinearLayout.LayoutParams.WRAP_CONTENT,
                    ),
                )
            },
        )
        field.requestFocus()
        window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE)
    }

    companion object {
        const val FIELD_DESC = "harness_field"
    }
}
