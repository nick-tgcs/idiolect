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
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView
import org.idiolect.android.R
import org.idiolect.android.ime.IdiolectImeService
import org.idiolect.android.model.HttpModelTransport
import org.idiolect.android.model.ModelDownloader
import org.idiolect.android.model.ModelStore
import org.idiolect.android.model.ModelTransport
import org.idiolect.android.model.PublicModelTransport
import org.idiolect.android.sync.SecureSyncConfig
import org.idiolect.android.sync.SyncSettings
import java.io.File
import kotlin.concurrent.thread

/**
 * The launcher / onboarding screen. It shows one CTA at a time — the next step from
 * [ImeSetup] — and re-evaluates on each resume, so returning from system settings or
 * the IME picker advances the flow. Pure framework glue around [ImeSetup] (unit-tested
 * separately); validated end to end by the emulator e2e.
 */
class SetupActivity : Activity() {
    private lateinit var status: TextView
    private lateinit var cta: Button
    private lateinit var urlField: EditText
    private lateinit var tokenField: EditText

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        status = TextView(this).apply { textSize = 18f }
        urlField = EditText(this).apply {
            setHint(R.string.setup_model_url_hint)
            visibility = ViewGroup.GONE
        }
        tokenField = EditText(this).apply {
            setHint(R.string.setup_model_token_hint)
            visibility = ViewGroup.GONE
        }
        cta = Button(this)
        val pad = (24 * resources.displayMetrics.density).toInt()
        setContentView(
            LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                gravity = Gravity.CENTER
                setPadding(pad, pad, pad, pad)
                addView(status, lp())
                addView(urlField, lp())
                addView(tokenField, lp())
                addView(cta, lp())
            },
        )
    }

    override fun onResume() {
        super.onResume()
        render()
    }

    private fun render() {
        when (ImeSetup.nextStep(hasMicPermission(), isEnabled(), isSelected(), hasModel())) {
            ImeSetupStep.EnableKeyboard -> bind(R.string.setup_enable, R.string.setup_enable_cta) {
                startActivity(Intent(Settings.ACTION_INPUT_METHOD_SETTINGS))
            }
            ImeSetupStep.SelectKeyboard -> bind(R.string.setup_select, R.string.setup_select_cta) {
                imm().showInputMethodPicker()
            }
            ImeSetupStep.GrantMicrophone -> bind(R.string.setup_mic, R.string.setup_mic_cta) {
                requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), REQ_MIC)
            }
            ImeSetupStep.DownloadModel -> {
                status.setText(R.string.setup_model)
                urlField.visibility = ViewGroup.VISIBLE
                tokenField.visibility = ViewGroup.VISIBLE
                cta.visibility = ViewGroup.VISIBLE
                cta.isEnabled = true
                cta.setText(R.string.setup_model_cta)
                cta.setOnClickListener { onDownloadTapped() }
            }
            ImeSetupStep.Ready -> {
                status.setText(R.string.setup_ready)
                urlField.visibility = ViewGroup.GONE
                tokenField.visibility = ViewGroup.GONE
                cta.visibility = ViewGroup.GONE
            }
        }
    }

    private fun bind(statusRes: Int, ctaRes: Int, action: () -> Unit) {
        status.setText(statusRes)
        urlField.visibility = ViewGroup.GONE
        tokenField.visibility = ViewGroup.GONE
        cta.visibility = ViewGroup.VISIBLE
        cta.isEnabled = true
        cta.setText(ctaRes)
        cta.setOnClickListener { action() }
    }

    /** Route the form's two fields to a model source ([ModelSourceChoice]), then download it. */
    private fun onDownloadTapped() {
        when (val choice = ModelSourceChoice.from(urlField.text.toString(), tokenField.text.toString())) {
            ModelSourceChoice.NeedDetails -> status.setText(R.string.setup_model_need_details)
            ModelSourceChoice.Public -> startDownload(PublicModelTransport.recommended(), pcEndpoint = null)
            is ModelSourceChoice.Pc ->
                startDownload(HttpModelTransport(choice.url, choice.token), pcEndpoint = choice)
        }
    }

    /**
     * Download [transport]'s model with progress, then go Ready. Only a PC pull carries a
     * [pcEndpoint] to remember (so the sync worker can ship learnings back, M6; the token
     * is wrapped at rest by the AndroidKeyStore). The public path saves nothing: a phone
     * with no prior pairing stays unpaired, and a previously-paired phone keeps its
     * existing endpoint untouched.
     */
    private fun startDownload(transport: ModelTransport, pcEndpoint: ModelSourceChoice.Pc?) {
        cta.isEnabled = false
        val downloader = ModelDownloader(transport, modelStore())
        thread(isDaemon = true, name = "idiolect-model-download") {
            runCatching {
                downloader.download { downloaded, total ->
                    val pct = if (total > 0) (downloaded * 100 / total).toInt() else 0
                    runOnUiThread { status.text = getString(R.string.setup_model_progress, pct) }
                }
            }.onSuccess {
                pcEndpoint?.let {
                    SecureSyncConfig.keystoreBacked(filesDir).save(SyncSettings(it.url, it.token))
                }
                runOnUiThread { render() } // a model is installed now → Ready
            }.onFailure { error ->
                runOnUiThread {
                    status.text = getString(R.string.setup_model_error, error.message ?: "")
                    cta.isEnabled = true
                }
            }
        }
    }

    private fun hasModel(): Boolean = modelStore().active() != null

    private fun modelStore() = ModelStore(File(filesDir, "models/whisper"))

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
