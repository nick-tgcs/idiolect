package org.idiolect.android.settings

import android.Manifest
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Typeface
import android.graphics.drawable.ColorDrawable
import android.os.Bundle
import android.provider.Settings
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.view.inputmethod.InputMethodManager
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.Switch
import android.widget.TextView
import androidx.activity.ComponentActivity
import androidx.activity.result.ActivityResultLauncher
import androidx.core.content.ContextCompat
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import org.idiolect.android.R
import org.idiolect.android.ime.IdiolectImeService
import org.idiolect.android.model.DownloadProgress
import org.idiolect.android.model.ModelDownloader
import org.idiolect.android.model.ModelStore
import org.idiolect.android.model.PublicModelCatalog
import org.idiolect.android.model.PublicModelOption
import org.idiolect.android.sync.DeviceId
import org.idiolect.android.sync.PairingClient
import org.idiolect.android.sync.PairingDeepLink
import org.idiolect.android.sync.ScanPairing
import org.idiolect.android.sync.SecureSyncConfig
import org.idiolect.android.sync.SyncSettings
import org.idiolect.android.ui.EdgeToEdge
import java.io.File
import kotlin.concurrent.thread

/**
 * The settings screen, reached via the ⚙ on the mic strip ([IdiolectImeService]). It is the
 * persistent home for configuration that used to be trapped in first-run onboarding — above
 * all, **managing the PC connection**: pair by scanning the QR, see the paired endpoint and
 * its verified cert pin, re-pair, or unpair. It also surfaces the speech model, the dictation
 * mode toggles ([SettingsStore]), the learning-sync switch, the on-device audio footprint
 * ([AudioUsage]), and the keyboard/mic system status.
 *
 * All display *logic* lives in the pure [SettingsView] (host-tested); this activity is glue —
 * it reads the device state off the main thread, hands it to [SettingsView], and renders the
 * result. The camera scan and the view plumbing have no headless seam, so they are covered by
 * the emulator e2e, not a unit test (the same split as [org.idiolect.android.setup.SetupActivity]).
 */
class SettingsActivity : ComponentActivity() {
    private lateinit var content: LinearLayout

    /** Whether the unpaired card's manual-entry sub-form is expanded (a session-local toggle). */
    private var manualExpanded = false

    /** Bumped per model download so a superseded switch's late UI updates are ignored. */
    private var modelDownloadToken = 0

    /** The FOSS QR scanner; a decoded pairing QR drives [onScanned], a cancel is ignored. */
    private val scanLauncher: ActivityResultLauncher<ScanOptions> =
        registerForActivityResult(ScanContract()) { result ->
            result.contents?.let { onScanned(it) }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        title = getString(R.string.settings_title)
        // Dark window so there is no light flash before the first (off-thread) render lands.
        val bg = ContextCompat.getColor(this, R.color.idiolect_bg)
        window.setBackgroundDrawable(ColorDrawable(bg))
        window.statusBarColor = bg
        content = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(16), dp(16), dp(16), dp(28))
        }
        val scroll = ScrollView(this).apply {
            setBackgroundColor(bg)
            addView(content, ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT)
        }
        setContentView(scroll)
        // targetSdk 35 draws under the status/nav bars; inset the scroll root so the header
        // and the first card are not hidden behind the clock (the "top gets cut off" bug).
        EdgeToEdge.enable(this, scroll)
        // A re-pair `idiolect://pair` link (the same one the PC's QR encodes), forwarded here by
        // [SetupActivity] once the device is already paired, enrols straight away — the camera-free
        // path that makes the ⚙ re-pair testable on an emulator and lets a tapped link land in
        // Settings on a real device.
        handlePairingLink(intent)
    }

    /** A new re-pair link arriving while Settings is already open (singleTop). */
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handlePairingLink(intent)
    }

    private fun handlePairingLink(intent: Intent?) {
        PairingDeepLink.fromIntentData(intent?.dataString)?.let { onScanned(it) }
    }

    override fun onResume() {
        super.onResume()
        refresh()
    }

    /** Gather device state off the main thread (keystore unwrap + an audio-dir walk), then render. */
    private fun refresh() {
        thread(isDaemon = true, name = "idiolect-settings-load") {
            val paired = config().load()
            val model = modelStore().active()
            val store = settingsStore()
            val prefs = PrefsSnapshot(
                reviewByDefault = store.reviewByDefault(),
                continuousOnDoubleTap = store.continuousOnDoubleTap(),
                shipCorrections = store.shipCorrections(),
                quickLaunchMic = store.quickLaunchEnabled(),
            )
            val system = SystemStatus(isEnabled(), isSelected(), hasMicPermission())
            val audioUsed = AudioUsage.bytesOnDisk(File(filesDir, "audio"))
            val state = SettingsView.from(paired, model, prefs, system, audioUsed, AUDIO_CAP_BYTES)
            runOnUiThread { renderState(state) }
        }
    }

    private fun renderState(state: SettingsViewState) {
        content.removeAllViews()
        content.addView(screenTitle())
        content.addView(connectionCard(state.connection))
        content.addView(modelCard(state))
        content.addView(dictationCard(state))
        content.addView(learningCard(state))
        content.addView(audioCard(state.audioLabel))
        content.addView(systemCard(state.system))
    }

    // --- Sections -----------------------------------------------------------------------------

    private fun connectionCard(connection: ConnectionView): View {
        val card = card(R.string.settings_section_connection)
        when (connection) {
            ConnectionView.Unpaired -> {
                card.addView(pill(getString(R.string.settings_not_connected), R.color.idiolect_muted))
                card.addView(primaryButton(R.string.settings_scan_cta) { onScanTapped() })
                card.addView(bodyText(getString(R.string.settings_scan_hint), R.color.idiolect_muted))
                card.addView(manualEntry())
            }
            is ConnectionView.Paired -> {
                card.addView(pill(getString(R.string.settings_connected), R.color.idiolect_accent_bright))
                card.addView(keyValue(getString(R.string.settings_endpoint), connection.endpoint))
                card.addView(pinView(connection.pin))
                card.addView(
                    rowOf(
                        secondaryButton(R.string.settings_repair) { onScanTapped() },
                        secondaryButton(R.string.settings_unpair) { onUnpair() },
                    ),
                )
            }
        }
        return card
    }

    private fun pinView(pin: PinView): View = when (pin) {
        is PinView.Pinned -> LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            addView(bodyText(getString(R.string.settings_pin_verified), R.color.idiolect_accent_bright))
            addView(monospace(pin.fingerprintGrouped))
        }
        PinView.Cleartext -> bodyText(getString(R.string.settings_pin_cleartext), R.color.idiolect_live)
    }

    /** The demoted manual-entry path: a cleartext (`--no-tls`) URL + token, hidden behind a link. */
    private fun manualEntry(): View {
        val url = field(R.string.settings_manual_url_hint)
        val token = field(R.string.settings_manual_token_hint)
        val connect = secondaryButton(R.string.settings_manual_connect) {
            onManualConnect(url.text.toString(), token.text.toString())
        }
        val form = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            visibility = if (manualExpanded) View.VISIBLE else View.GONE
            addView(url)
            addView(token)
            addView(connect)
        }
        val link = bodyText(getString(R.string.settings_manual_link), R.color.idiolect_accent).apply {
            setOnClickListener {
                manualExpanded = !manualExpanded
                form.visibility = if (manualExpanded) View.VISIBLE else View.GONE
            }
        }
        return LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            addView(link)
            addView(form)
        }
    }

    /**
     * The speech-model card: the active model, plus a one-tap switch/download for each other
     * catalog option (tiny ⇄ base). Switching re-downloads from the public CDN with inline
     * progress and relabels on success — the runtime half of "let the user pick their model".
     */
    private fun modelCard(state: SettingsViewState): View {
        val card = card(R.string.settings_section_model)
        card.addView(bodyText(state.modelLabel, R.color.idiolect_text))
        val status = bodyText("", R.color.idiolect_muted).apply { visibility = View.GONE }
        PublicModelCatalog.options
            .filter { it.id != state.modelId }
            .forEach { option ->
                val textRes = if (state.modelId == null) R.string.settings_model_get else R.string.settings_model_switch
                card.addView(
                    modelButton(getString(textRes, option.label, option.sizeLabel)) {
                        downloadModel(option, status)
                    },
                )
            }
        card.addView(status)
        return card
    }

    /**
     * Download (and switch to) [option], reporting progress inline in [status]. A newer request
     * supersedes any in-flight one (the token guard) so rapid taps don't cross streams; on
     * success the screen re-reads device state, which relabels the now-active model.
     */
    private fun downloadModel(option: PublicModelOption, status: TextView) {
        val token = ++modelDownloadToken
        status.visibility = View.VISIBLE
        status.text = getString(R.string.settings_model_downloading, DownloadProgress.label(0, option.size))
        thread(isDaemon = true, name = "idiolect-settings-model") {
            runCatching {
                ModelDownloader(option.transport(), modelStore()).download { downloaded, total ->
                    if (token != modelDownloadToken) return@download
                    val line = DownloadProgress.label(downloaded, total)
                    runOnUiThread { if (token == modelDownloadToken) status.text = getString(R.string.settings_model_downloading, line) }
                }
            }.onSuccess {
                if (token == modelDownloadToken) refresh()
            }.onFailure { error ->
                if (token == modelDownloadToken) {
                    runOnUiThread { status.text = getString(R.string.settings_model_error, error.message ?: "") }
                }
            }
        }
    }

    /** A full-width pill button used by the model card's switch/download actions. */
    private fun modelButton(text: String, onClick: () -> Unit): Button =
        Button(this).apply {
            this.text = text
            isAllCaps = false
            setTextColor(ContextCompat.getColor(this@SettingsActivity, R.color.idiolect_text))
            background = ContextCompat.getDrawable(this@SettingsActivity, R.drawable.strip_pill)
            setOnClickListener { onClick() }
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ).apply { topMargin = dp(8) }
        }

    private fun dictationCard(state: SettingsViewState): View {
        val card = card(R.string.settings_section_dictation)
        card.addView(
            toggleRow(R.string.settings_review_label, R.string.settings_review_sub, state.reviewOn) { on ->
                settingsStore().setReviewByDefault(on)
            },
        )
        card.addView(
            toggleRow(R.string.settings_continuous_label, R.string.settings_continuous_sub, state.continuousOn) { on ->
                settingsStore().setContinuousOnDoubleTap(on)
            },
        )
        card.addView(
            toggleRow(R.string.settings_quicklaunch_label, R.string.settings_quicklaunch_sub, state.quickLaunchOn) { on ->
                settingsStore().setQuickLaunchEnabled(on)
            },
        )
        return card
    }

    private fun learningCard(state: SettingsViewState): View =
        card(R.string.settings_section_learning).apply {
            addView(
                toggleRow(R.string.settings_ship_label, R.string.settings_ship_sub, state.shipOn) { on ->
                    settingsStore().setShipCorrections(on)
                },
            )
        }

    private fun audioCard(label: String): View =
        card(R.string.settings_section_audio).apply {
            addView(bodyText(label, R.color.idiolect_text))
            addView(bodyText(getString(R.string.settings_audio_sub), R.color.idiolect_muted))
        }

    private fun systemCard(system: SystemStatus): View {
        val card = card(R.string.settings_section_system)
        val keyboardOk = system.keyboardEnabled && system.keyboardSelected
        card.addView(
            statusRow(getString(R.string.settings_system_keyboard), keyboardOk) {
                if (!system.keyboardEnabled) {
                    startActivity(Intent(Settings.ACTION_INPUT_METHOD_SETTINGS))
                } else {
                    imm().showInputMethodPicker()
                }
            },
        )
        card.addView(
            statusRow(getString(R.string.settings_system_mic), system.micGranted) {
                requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), REQ_MIC)
            },
        )
        return card
    }

    // --- Actions ------------------------------------------------------------------------------

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
     * thread ([ScanPairing] persists the endpoint + pin on success), then re-render to the paired
     * state. Unlike onboarding this pulls no model — a settings re-pair only refreshes the sync
     * endpoint/token/pin; the installed model is untouched.
     */
    private fun onScanned(contents: String) {
        toast(getString(R.string.settings_pairing))
        thread(isDaemon = true, name = "idiolect-settings-pair") {
            runCatching { ScanPairing(pairingClient()).pairFromScan(contents) }
                .onSuccess { refresh() }
                .onFailure { error ->
                    runOnUiThread { toast(getString(R.string.settings_pairing_error, error.message ?: "")) }
                }
        }
    }

    /** Save a cleartext (`--no-tls`) endpoint typed by hand, then re-render. */
    private fun onManualConnect(url: String, token: String) {
        if (url.isBlank() || token.isBlank()) {
            toast(getString(R.string.settings_manual_incomplete))
            return
        }
        thread(isDaemon = true, name = "idiolect-settings-manual") {
            config().save(SyncSettings(url.trim(), token.trim(), pin = null))
            manualExpanded = false
            refresh()
        }
    }

    /** Unpair: wipe the endpoint, pin, and token, then re-render to the unpaired state. */
    private fun onUnpair() {
        thread(isDaemon = true, name = "idiolect-settings-unpair") {
            config().clear()
            refresh()
        }
    }

    // --- View builders ------------------------------------------------------------------------

    private fun screenTitle(): View = TextView(this).apply {
        text = getString(R.string.settings_title)
        textSize = 24f
        setTypeface(typeface, Typeface.BOLD)
        setTextColor(ContextCompat.getColor(this@SettingsActivity, R.color.idiolect_text))
        setPadding(dp(4), dp(4), dp(4), dp(12))
    }

    /** A rounded panel card with a small accent header; callers add their own rows. */
    private fun card(titleRes: Int): LinearLayout {
        val box = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            background = ContextCompat.getDrawable(this@SettingsActivity, R.drawable.review_card_bg)
            setPadding(dp(16), dp(14), dp(16), dp(16))
        }
        box.addView(
            TextView(this).apply {
                text = getString(titleRes).uppercase()
                textSize = 12f
                letterSpacing = 0.08f
                setTypeface(typeface, Typeface.BOLD)
                setTextColor(ContextCompat.getColor(this@SettingsActivity, R.color.idiolect_accent_bright))
                setPadding(0, 0, 0, dp(10))
            },
        )
        return box.also { it.layoutParams = cardLp() }
    }

    private fun cardLp() = LinearLayout.LayoutParams(
        ViewGroup.LayoutParams.MATCH_PARENT,
        ViewGroup.LayoutParams.WRAP_CONTENT,
    ).apply { bottomMargin = dp(14) }

    private fun pill(text: String, colorRes: Int): View = TextView(this).apply {
        this.text = text
        textSize = 13f
        setTypeface(typeface, Typeface.BOLD)
        setTextColor(ContextCompat.getColor(this@SettingsActivity, colorRes))
        setPadding(0, 0, 0, dp(8))
    }

    private fun bodyText(text: String, colorRes: Int): TextView = TextView(this).apply {
        this.text = text
        textSize = 14f
        setTextColor(ContextCompat.getColor(this@SettingsActivity, colorRes))
        setPadding(0, dp(4), 0, dp(4))
    }

    private fun monospace(text: String): View = TextView(this).apply {
        this.text = text
        textSize = 13f
        typeface = Typeface.MONOSPACE
        setTextColor(ContextCompat.getColor(this@SettingsActivity, R.color.idiolect_grey))
        background = ContextCompat.getDrawable(this@SettingsActivity, R.drawable.review_field_bg)
        setPadding(dp(10), dp(8), dp(10), dp(8))
    }

    private fun keyValue(key: String, value: String): View = LinearLayout(this).apply {
        orientation = LinearLayout.VERTICAL
        setPadding(0, dp(2), 0, dp(6))
        addView(
            TextView(this@SettingsActivity).apply {
                text = key
                textSize = 12f
                setTextColor(ContextCompat.getColor(this@SettingsActivity, R.color.idiolect_muted))
            },
        )
        addView(
            TextView(this@SettingsActivity).apply {
                text = value
                textSize = 15f
                setTextColor(ContextCompat.getColor(this@SettingsActivity, R.color.idiolect_text))
            },
        )
    }

    private fun primaryButton(textRes: Int, onClick: () -> Unit): Button =
        Button(this).apply {
            setText(textRes)
            isAllCaps = false // sentence case, matching the mockup (the platform Button forces caps)
            setTextColor(ContextCompat.getColor(this@SettingsActivity, R.color.mic_glyph_active))
            background = ContextCompat.getDrawable(this@SettingsActivity, R.drawable.review_btn_insert)
            setOnClickListener { onClick() }
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ).apply { topMargin = dp(4); bottomMargin = dp(6) }
        }

    private fun secondaryButton(textRes: Int, onClick: () -> Unit): Button =
        Button(this).apply {
            setText(textRes)
            isAllCaps = false
            setTextColor(ContextCompat.getColor(this@SettingsActivity, R.color.idiolect_text))
            background = ContextCompat.getDrawable(this@SettingsActivity, R.drawable.strip_pill)
            setOnClickListener { onClick() }
            layoutParams = LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f)
                .apply { marginEnd = dp(6); topMargin = dp(6) }
        }

    private fun rowOf(vararg views: View): View = LinearLayout(this).apply {
        orientation = LinearLayout.HORIZONTAL
        views.forEach { addView(it) }
    }

    @Suppress("DEPRECATION") // android.widget.Switch — the app has no AppCompat theme dependency.
    private fun toggleRow(labelRes: Int, subRes: Int, checked: Boolean, onChange: (Boolean) -> Unit): View {
        val labels = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            layoutParams = LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f)
            addView(
                TextView(this@SettingsActivity).apply {
                    setText(labelRes)
                    textSize = 15f
                    setTextColor(ContextCompat.getColor(this@SettingsActivity, R.color.idiolect_text))
                },
            )
            addView(
                TextView(this@SettingsActivity).apply {
                    setText(subRes)
                    textSize = 12f
                    setTextColor(ContextCompat.getColor(this@SettingsActivity, R.color.idiolect_muted))
                },
            )
        }
        val toggle = Switch(this).apply {
            isChecked = checked
            setOnCheckedChangeListener { _, isOn -> onChange(isOn) }
        }
        return LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(0, dp(8), 0, dp(8))
            addView(labels)
            addView(toggle)
        }
    }

    private fun statusRow(label: String, ok: Boolean, onFix: () -> Unit): View {
        val valueRes = if (ok) R.string.settings_enabled else R.string.settings_not_enabled
        val grantedRes = if (label == getString(R.string.settings_system_mic) && ok) {
            R.string.settings_granted
        } else if (label == getString(R.string.settings_system_mic)) {
            R.string.settings_not_granted
        } else {
            valueRes
        }
        return LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(0, dp(8), 0, dp(8))
            if (!ok) setOnClickListener { onFix() }
            addView(
                TextView(this@SettingsActivity).apply {
                    text = label
                    textSize = 15f
                    setTextColor(ContextCompat.getColor(this@SettingsActivity, R.color.idiolect_text))
                    layoutParams = LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f)
                },
            )
            addView(
                TextView(this@SettingsActivity).apply {
                    setText(grantedRes)
                    textSize = 13f
                    setTextColor(
                        ContextCompat.getColor(
                            this@SettingsActivity,
                            if (ok) R.color.idiolect_accent_bright else R.color.idiolect_live,
                        ),
                    )
                },
            )
        }
    }

    private fun field(hintRes: Int): EditText = EditText(this).apply {
        setHint(hintRes)
        textSize = 14f
        setTextColor(ContextCompat.getColor(this@SettingsActivity, R.color.idiolect_text))
        setHintTextColor(ContextCompat.getColor(this@SettingsActivity, R.color.idiolect_muted))
        setPadding(dp(8), dp(8), dp(8), dp(8))
    }

    private fun toast(message: String) =
        android.widget.Toast.makeText(this, message, android.widget.Toast.LENGTH_SHORT).show()

    // --- Device state seams (shared with SetupActivity) ---------------------------------------

    private fun config() = SecureSyncConfig.keystoreBacked(filesDir)
    private fun settingsStore() = SettingsStore.under(filesDir)
    private fun modelStore() = ModelStore(File(filesDir, "models/whisper"))
    private fun pairingClient() = PairingClient(config(), DeviceId(File(filesDir, DeviceId.FILE_NAME)).get())

    private fun hasMicPermission(): Boolean =
        ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO) ==
            PackageManager.PERMISSION_GRANTED

    private fun isEnabled(): Boolean =
        imm().enabledInputMethodList.any { ComponentName.unflattenFromString(it.id) == component() }

    private fun isSelected(): Boolean {
        val selected = Settings.Secure.getString(contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD)
        return ComponentName.unflattenFromString(selected ?: "") == component()
    }

    private fun component() = ComponentName(this, IdiolectImeService::class.java)
    private fun imm() = getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager
    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    companion object {
        private const val REQ_MIC = 1

        /** Mirrors the core's `DEFAULT_AUDIO_STORAGE_CAP_BYTES` (1 GiB); the cap is enforced in Rust. */
        private const val AUDIO_CAP_BYTES = 1_073_741_824L

        /** Open the settings screen from the IME service (a Service → Activity needs NEW_TASK). */
        fun launch(context: Context) {
            context.startActivity(
                Intent(context, SettingsActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        }
    }
}
