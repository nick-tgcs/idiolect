package org.idiolect.android.setup

import android.Manifest
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Typeface
import android.graphics.drawable.ColorDrawable
import android.graphics.drawable.GradientDrawable
import android.os.Bundle
import android.provider.Settings
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.view.inputmethod.InputMethodManager
import android.widget.Button
import android.widget.EditText
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import androidx.activity.ComponentActivity
import androidx.activity.result.ActivityResultLauncher
import androidx.core.content.ContextCompat
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import org.idiolect.android.R
import org.idiolect.android.ime.IdiolectImeService
import org.idiolect.android.model.DownloadProgress
import org.idiolect.android.model.HttpModelTransport
import org.idiolect.android.model.ModelDownloader
import org.idiolect.android.model.ModelStore
import org.idiolect.android.model.ModelTransport
import org.idiolect.android.model.PublicModelCatalog
import org.idiolect.android.model.PublicModelOption
import org.idiolect.android.settings.SettingsActivity
import org.idiolect.android.ui.EdgeToEdge
import org.idiolect.android.sync.DeviceId
import org.idiolect.android.sync.PairingClient
import org.idiolect.android.sync.PairingDeepLink
import org.idiolect.android.sync.ScanPairing
import org.idiolect.android.sync.SecureSyncConfig
import org.idiolect.android.sync.SyncSettings
import java.io.File
import kotlin.concurrent.thread

/**
 * The launcher / onboarding screen. It shows one CTA at a time — the next step from
 * [ImeSetup] — and re-evaluates on each resume, so returning from system settings or the IME
 * picker advances the flow. The look matches the rest of idiolect (the mic logo + dark
 * periwinkle palette of the voice keyboard / [SettingsActivity]), with an [OnboardingProgress]
 * step indicator so the four gates read as a guided flow rather than a bare line of text.
 * Pure framework glue around [ImeSetup] / [OnboardingProgress] (unit-tested separately);
 * validated end to end by the emulator e2e.
 *
 * On the model step the user can either type their PC's URL + token by hand or **scan the
 * PC's pairing QR** ([ScanPairing], host-tested): a scan exchanges the one-time code for a
 * per-device token and then pulls the model from the now-paired PC. The camera capture and
 * the activity-result plumbing have no headless seam, so they are covered by the manual
 * emulator e2e, not a unit test.
 */
class SetupActivity : ComponentActivity() {
    private lateinit var status: TextView
    private lateinit var cta: Button
    private lateinit var scanButton: Button
    private lateinit var urlField: EditText
    private lateinit var tokenField: EditText
    private lateinit var stepRow: LinearLayout
    private lateinit var stepLabel: TextView
    private val dots = mutableListOf<View>()

    /** The tiny/base model chooser (DownloadModel step). Default = the fast catalog default. */
    private lateinit var modelPicker: LinearLayout
    private val modelRows = mutableListOf<View>()
    private var selectedModel = PublicModelCatalog.default

    /** Bumped per download attempt so a cancelled or superseded download's late UI updates are ignored. */
    private var downloadToken = 0

    /** The FOSS QR scanner; a decoded pairing QR drives [onScanned], a cancel is ignored. */
    private val scanLauncher: ActivityResultLauncher<ScanOptions> =
        registerForActivityResult(ScanContract()) { result ->
            result.contents?.let { onScanned(it) }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val bg = color(R.color.idiolect_bg)
        window.setBackgroundDrawable(ColorDrawable(bg))
        window.statusBarColor = bg

        status = TextView(this).apply {
            textSize = 15f
            gravity = Gravity.CENTER
            setTextColor(color(R.color.idiolect_grey))
        }
        scanButton = secondary(R.string.setup_scan_cta) { onScanTapped() }.apply { visibility = View.GONE }
        urlField = field(R.string.setup_model_url_hint).apply { visibility = View.GONE }
        tokenField = field(R.string.setup_model_token_hint).apply { visibility = View.GONE }
        cta = primary("") {}

        buildModelPicker()
        buildDots()
        stepLabel = TextView(this).apply {
            textSize = 12f
            gravity = Gravity.CENTER
            setTextColor(color(R.color.idiolect_muted))
        }

        val card = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            background = ContextCompat.getDrawable(this@SetupActivity, R.drawable.review_card_bg)
            setPadding(dp(18), dp(18), dp(18), dp(18))
            addView(status, LinearLayout.LayoutParams(MATCH, WRAP))
            addView(modelPicker, inCardLp())
            addView(scanButton, inCardLp())
            addView(urlField, inCardLp())
            addView(tokenField, inCardLp())
            addView(cta, inCardLp())
        }

        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
            setBackgroundColor(bg)
            setPadding(dp(24), dp(40), dp(24), dp(28))
            addView(logo(), LinearLayout.LayoutParams(dp(92), dp(92)))
            addView(wordmark(), topMargin(dp(16)))
            addView(tagline(), topMargin(dp(4)))
            addView(stepRow, topMargin(dp(22)))
            addView(stepLabel, topMargin(dp(8)))
            addView(card, LinearLayout.LayoutParams(MATCH, WRAP).apply { topMargin = dp(22) })
        }
        val scroll = ScrollView(this).apply { setBackgroundColor(bg); addView(root) }
        setContentView(scroll)
        // targetSdk 35 draws under the status/nav bars; inset the scroll root so the logo and
        // CTA are not hidden behind the clock (the "top gets cut off" bug).
        EdgeToEdge.enable(this, scroll)

        // A pairing link the activity was launched with enrols straight away (camera-free).
        handleDeepLink(intent)
    }

    /** A new `idiolect://pair` link arriving while setup is already open (singleTop). */
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleDeepLink(intent)
    }

    /**
     * Route an `idiolect://pair?u=…&c=…` link (the same URI the PC's QR encodes; the normal
     * MAIN/LAUNCHER launch or any other URL is ignored). A first enrolment is handled here
     * ([onScanned]) so it also pulls the model; but on an **already-paired** device a re-pair
     * belongs on the ⚙ settings screen — a lean endpoint/token/pin swap shown in context, with
     * no redundant model re-download — so we forward the same link there ([PairingRouter]).
     */
    private fun handleDeepLink(intent: Intent?) {
        val scanned = PairingDeepLink.fromIntentData(intent?.dataString)
        val alreadyPaired = SecureSyncConfig.keystoreBacked(filesDir).load() != null
        when (PairingRouter.route(isPairingLink = scanned != null, alreadyPaired = alreadyPaired)) {
            PairingLinkRoute.Settings ->
                startActivity(
                    Intent(this, SettingsActivity::class.java)
                        .setData(intent?.data)
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                )
            PairingLinkRoute.Onboarding -> onScanned(scanned!!)
            PairingLinkRoute.Ignore -> Unit
        }
    }

    override fun onResume() {
        super.onResume()
        render()
    }

    private fun render() {
        val step = ImeSetup.nextStep(hasMicPermission(), isEnabled(), isSelected(), hasModel())
        renderProgress(step)
        when (step) {
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
                modelPicker.visibility = View.VISIBLE
                modelPickerEnabled(true)
                scanButton.visibility = View.VISIBLE
                scanButton.isEnabled = true
                urlField.visibility = View.VISIBLE
                tokenField.visibility = View.VISIBLE
                cta.visibility = View.VISIBLE
                cta.isEnabled = true
                cta.setText(R.string.setup_model_cta)
                cta.setOnClickListener { onDownloadTapped() }
            }
            ImeSetupStep.Ready -> {
                status.setText(R.string.setup_ready)
                modelPicker.visibility = View.GONE
                scanButton.visibility = View.GONE
                urlField.visibility = View.GONE
                tokenField.visibility = View.GONE
                cta.visibility = View.GONE
            }
        }
    }

    /** Light up the step dots and label for [step]; hidden once everything's set up. */
    private fun renderProgress(step: ImeSetupStep) {
        if (step == ImeSetupStep.Ready) {
            stepRow.visibility = View.GONE
            stepLabel.visibility = View.GONE
            return
        }
        val (done, total) = OnboardingProgress.of(step)
        stepRow.visibility = View.VISIBLE
        stepLabel.visibility = View.VISIBLE
        dots.forEachIndexed { index, dot ->
            dot.background = oval(if (index < done) R.color.idiolect_accent else R.color.idiolect_slate)
        }
        stepLabel.text = getString(R.string.setup_step, done + 1, total)
    }

    private fun bind(statusRes: Int, ctaRes: Int, action: () -> Unit) {
        status.setText(statusRes)
        modelPicker.visibility = View.GONE
        scanButton.visibility = View.GONE
        urlField.visibility = View.GONE
        tokenField.visibility = View.GONE
        cta.visibility = View.VISIBLE
        cta.isEnabled = true
        cta.setText(ctaRes)
        cta.setOnClickListener { action() }
    }

    /** Route the form's two fields to a model source ([ModelSourceChoice]), then download it. */
    private fun onDownloadTapped() {
        when (val choice = ModelSourceChoice.from(urlField.text.toString(), tokenField.text.toString())) {
            ModelSourceChoice.NeedDetails -> status.setText(R.string.setup_model_need_details)
            ModelSourceChoice.Public -> startDownload(selectedModel.transport(), pcEndpoint = null)
            is ModelSourceChoice.Pc ->
                startDownload(
                    HttpModelTransport(choice.url, choice.token, choice.pin),
                    pcEndpoint = choice,
                )
        }
    }

    /** Launch the QR scanner; zxing handles the camera-permission prompt and the preview. */
    private fun onScanTapped() {
        scanLauncher.launch(
            ScanOptions()
                .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                .setPrompt(getString(R.string.setup_scan_prompt))
                .setBeepEnabled(false)
                .setOrientationLocked(false),
        )
    }

    /**
     * A pairing QR was decoded: exchange its one-time code for a per-device token off the UI
     * thread ([ScanPairing] persists the endpoint), then pull the model from the now-paired
     * PC by reusing the PC download path. A malformed QR or a rejected code surfaces as an
     * error and leaves the form (and the scan button) usable.
     */
    private fun onScanned(contents: String) {
        cta.isEnabled = false
        scanButton.isEnabled = false
        status.setText(R.string.setup_pairing)
        thread(isDaemon = true, name = "idiolect-pair") {
            runCatching { ScanPairing(pairingClient()).pairFromScan(contents) }
                .onSuccess { endpoint ->
                    runOnUiThread {
                        startDownload(
                            HttpModelTransport(endpoint.baseUrl, endpoint.token, endpoint.pin),
                            ModelSourceChoice.Pc(endpoint.baseUrl, endpoint.token, endpoint.pin),
                        )
                    }
                }
                .onFailure { error ->
                    runOnUiThread {
                        status.text = getString(R.string.setup_pairing_error, error.message ?: "")
                        cta.isEnabled = true
                        scanButton.isEnabled = true
                    }
                }
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
        val token = ++downloadToken
        scanButton.isEnabled = false
        modelPickerEnabled(false)
        cta.isEnabled = true
        cta.setText(R.string.setup_model_cancel)
        cta.setOnClickListener { cancelDownload() }
        val downloader = ModelDownloader(transport, modelStore())
        thread(isDaemon = true, name = "idiolect-model-download") {
            runCatching {
                // Gate the install itself, not just the UI: a cancel/supersede that lands after
                // the bytes verify must leave nothing installed (throws, swallowed below).
                downloader.download(isCancelled = { token != downloadToken }) { downloaded, total ->
                    if (token != downloadToken) return@download // cancelled / superseded
                    val line = DownloadProgress.label(downloaded, total)
                    runOnUiThread { if (token == downloadToken) status.text = getString(R.string.setup_model_progress, line) }
                }
            }.onSuccess {
                if (token != downloadToken) return@onSuccess
                pcEndpoint?.let {
                    SecureSyncConfig.keystoreBacked(filesDir).save(SyncSettings(it.url, it.token, it.pin))
                }
                runOnUiThread { if (token == downloadToken) render() } // a model is installed now → Ready
            }.onFailure { error ->
                if (token != downloadToken) return@onFailure
                runOnUiThread {
                    if (token == downloadToken) {
                        status.text = getString(R.string.setup_model_error, error.message ?: "")
                        restoreDownloadCta()
                    }
                }
            }
        }
    }

    /**
     * Abandon the in-flight download's UI immediately so the screen never feels frozen. The
     * daemon thread finishes streaming the (small) file and is then discarded by the token check
     * — nothing is installed and the form is usable again to retry or switch model.
     */
    private fun cancelDownload() {
        downloadToken++
        status.setText(R.string.setup_model_cancelled)
        restoreDownloadCta()
    }

    /** Put the model step's CTA back to "Download" (after an error or a cancel). */
    private fun restoreDownloadCta() {
        scanButton.isEnabled = true
        modelPickerEnabled(true)
        cta.isEnabled = true
        cta.setText(R.string.setup_model_cta)
        cta.setOnClickListener { onDownloadTapped() }
    }

    /** The PC pairing client: the keystore-backed sync config and this install's device id. */
    private fun pairingClient() = PairingClient(
        SecureSyncConfig.keystoreBacked(filesDir),
        DeviceId(File(filesDir, DeviceId.FILE_NAME)).get(),
    )

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

    // --- View builders (the idiolect look: mic logo + periwinkle accent on dark slate) -------

    /** The idiolect mark: the mic glyph, white on an accent disc. */
    private fun logo(): View = ImageView(this).apply {
        setImageResource(R.drawable.ic_mic)
        imageTintList = ContextCompat.getColorStateList(this@SetupActivity, R.color.mic_glyph_active)
        background = oval(R.color.idiolect_accent)
        scaleType = ImageView.ScaleType.CENTER_INSIDE
        setPadding(dp(22), dp(22), dp(22), dp(22))
    }

    private fun wordmark(): View = TextView(this).apply {
        text = getString(R.string.app_name)
        textSize = 28f
        setTypeface(typeface, Typeface.BOLD)
        setTextColor(color(R.color.idiolect_text))
        gravity = Gravity.CENTER
    }

    private fun tagline(): View = TextView(this).apply {
        setText(R.string.setup_tagline)
        textSize = 14f
        setTextColor(color(R.color.idiolect_muted))
        gravity = Gravity.CENTER
    }

    /** Build the (initially hidden) tiny/base model chooser shown on the DownloadModel step. */
    private fun buildModelPicker() {
        modelPicker = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            visibility = View.GONE
            addView(
                TextView(this@SetupActivity).apply {
                    setText(R.string.setup_model_picker_title)
                    textSize = 12f
                    letterSpacing = 0.06f
                    setTypeface(typeface, Typeface.BOLD)
                    setTextColor(color(R.color.idiolect_accent_bright))
                    setPadding(0, 0, 0, dp(6))
                },
            )
        }
        PublicModelCatalog.options.forEach { option ->
            val row = modelRow(option)
            modelRows.add(row)
            modelPicker.addView(row, LinearLayout.LayoutParams(MATCH, WRAP).apply { topMargin = dp(6) })
        }
        renderModelSelection()
    }

    /** One selectable model row: a radio dot + the catalog label/size/blurb; tap selects it. */
    private fun modelRow(option: PublicModelOption): View {
        val dot = View(this).apply {
            layoutParams = LinearLayout.LayoutParams(dp(14), dp(14)).apply { marginEnd = dp(12); topMargin = dp(3) }
        }
        val texts = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            addView(
                TextView(this@SetupActivity).apply {
                    text = "${option.label} · ${option.sizeLabel}"
                    textSize = 15f
                    setTextColor(color(R.color.idiolect_text))
                },
            )
            addView(
                TextView(this@SetupActivity).apply {
                    text = option.blurb
                    textSize = 12f
                    setTextColor(color(R.color.idiolect_muted))
                },
            )
        }
        return LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            background = ContextCompat.getDrawable(this@SetupActivity, R.drawable.review_field_bg)
            setPadding(dp(12), dp(10), dp(12), dp(10))
            tag = option
            addView(dot)
            addView(texts)
            setOnClickListener {
                if (isEnabled) {
                    selectedModel = option
                    renderModelSelection()
                }
            }
        }
    }

    /** Light the selected row's dot accent, the others slate. */
    private fun renderModelSelection() {
        modelRows.forEach { row ->
            val option = row.tag as PublicModelOption
            (row as LinearLayout).getChildAt(0).background =
                oval(if (option == selectedModel) R.color.idiolect_accent else R.color.idiolect_slate)
        }
    }

    /** Dim + lock the picker while a download is running (re-enabled on success/cancel/error). */
    private fun modelPickerEnabled(enabled: Boolean) {
        modelRows.forEach { row ->
            row.isEnabled = enabled
            row.alpha = if (enabled) 1f else 0.5f
        }
    }

    private fun buildDots() {
        stepRow = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER
        }
        repeat(OnboardingProgress.TOTAL_GATES) {
            val dot = View(this).apply {
                layoutParams = LinearLayout.LayoutParams(dp(9), dp(9)).apply {
                    marginStart = dp(5); marginEnd = dp(5)
                }
            }
            dots.add(dot)
            stepRow.addView(dot)
        }
    }

    private fun primary(text: String, onClick: () -> Unit): Button = Button(this).apply {
        this.text = text
        isAllCaps = false
        setTextColor(color(R.color.mic_glyph_active))
        background = ContextCompat.getDrawable(this@SetupActivity, R.drawable.review_btn_insert)
        setOnClickListener { onClick() }
    }

    private fun secondary(textRes: Int, onClick: () -> Unit): Button = Button(this).apply {
        setText(textRes)
        isAllCaps = false
        setTextColor(color(R.color.idiolect_text))
        background = ContextCompat.getDrawable(this@SetupActivity, R.drawable.strip_pill)
        setOnClickListener { onClick() }
    }

    private fun field(hintRes: Int): EditText = EditText(this).apply {
        setHint(hintRes)
        textSize = 14f
        setTextColor(color(R.color.idiolect_text))
        setHintTextColor(color(R.color.idiolect_muted))
        background = ContextCompat.getDrawable(this@SetupActivity, R.drawable.review_field_bg)
        setPadding(dp(10), dp(10), dp(10), dp(10))
    }

    /** An oval drawable filled with [colorRes] — the logo disc and the step dots. */
    private fun oval(colorRes: Int) = GradientDrawable().apply {
        shape = GradientDrawable.OVAL
        setColor(color(colorRes))
    }

    private fun color(res: Int) = ContextCompat.getColor(this, res)

    private fun inCardLp() = LinearLayout.LayoutParams(MATCH, WRAP).apply { topMargin = dp(12) }

    private fun topMargin(margin: Int) =
        LinearLayout.LayoutParams(WRAP, WRAP).apply { gravity = Gravity.CENTER_HORIZONTAL; topMargin = margin }

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    private companion object {
        const val REQ_MIC = 1
        const val MATCH = LinearLayout.LayoutParams.MATCH_PARENT
        const val WRAP = LinearLayout.LayoutParams.WRAP_CONTENT
    }
}
