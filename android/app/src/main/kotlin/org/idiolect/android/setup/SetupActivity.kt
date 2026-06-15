package org.idiolect.android.setup

import android.Manifest
import android.app.Activity
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import android.provider.Settings
import android.view.Gravity
import android.view.ViewGroup
import android.view.inputmethod.InputMethodManager
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import org.idiolect.android.R
import org.idiolect.android.ime.IdiolectImeService

/**
 * The launcher / onboarding screen. It shows one CTA at a time — the next step from
 * [ImeSetup] — and re-evaluates on each resume, so returning from system settings or
 * the IME picker advances the flow. Pure framework glue around [ImeSetup] (unit-tested
 * separately); validated end to end by the emulator e2e.
 */
class SetupActivity : Activity() {
    private lateinit var status: TextView
    private lateinit var cta: Button

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        status = TextView(this).apply { textSize = 18f }
        cta = Button(this)
        val pad = (24 * resources.displayMetrics.density).toInt()
        setContentView(
            LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                gravity = Gravity.CENTER
                setPadding(pad, pad, pad, pad)
                addView(status, lp())
                addView(cta, lp())
            },
        )
    }

    override fun onResume() {
        super.onResume()
        render()
    }

    private fun render() {
        when (ImeSetup.nextStep(hasMicPermission(), isEnabled(), isSelected())) {
            ImeSetupStep.EnableKeyboard -> bind(R.string.setup_enable, R.string.setup_enable_cta) {
                startActivity(Intent(Settings.ACTION_INPUT_METHOD_SETTINGS))
            }
            ImeSetupStep.SelectKeyboard -> bind(R.string.setup_select, R.string.setup_select_cta) {
                imm().showInputMethodPicker()
            }
            ImeSetupStep.GrantMicrophone -> bind(R.string.setup_mic, R.string.setup_mic_cta) {
                requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), REQ_MIC)
            }
            ImeSetupStep.Ready -> {
                status.setText(R.string.setup_ready)
                cta.visibility = ViewGroup.GONE
            }
        }
    }

    private fun bind(statusRes: Int, ctaRes: Int, action: () -> Unit) {
        status.setText(statusRes)
        cta.visibility = ViewGroup.VISIBLE
        cta.setText(ctaRes)
        cta.setOnClickListener { action() }
    }

    private fun hasMicPermission(): Boolean =
        checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED

    private fun isEnabled(): Boolean =
        imm().enabledInputMethodList.any { ComponentName.unflattenFromString(it.id) == component() }

    private fun isSelected(): Boolean {
        val selected = Settings.Secure.getString(contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD)
        return ComponentName.unflattenFromString(selected ?: "") == component()
    }

    private fun component() = ComponentName(this, IdiolectImeService::class.java)

    private fun imm() = getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager

    private fun lp() = LinearLayout.LayoutParams(
        ViewGroup.LayoutParams.WRAP_CONTENT,
        ViewGroup.LayoutParams.WRAP_CONTENT,
    ).apply { gravity = Gravity.CENTER; topMargin = 24 }

    private companion object {
        const val REQ_MIC = 1
    }
}
