package org.idiolect.android.ime

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.os.Bundle
import android.provider.Settings
import android.text.InputType
import android.view.Gravity
import android.view.View
import android.view.WindowManager
import android.widget.Button
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.TextView
import androidx.core.content.ContextCompat
import org.idiolect.android.R
import org.idiolect.android.accessibility.AccessibilityServices
import org.idiolect.android.accessibility.IdiolectAccessibilityService
import org.idiolect.android.accessibility.InjectQueue
import org.idiolect.android.core.IdiolectCoreHost
import java.io.File

/**
 * The centred "Review dictation" surface (the 👁 flow). A finished take opens this over the
 * host app; the transcript is editable with the user's **own** keyboard (the field is marked
 * so the IME hands off — see [IdiolectImeService]). On **Insert** the edit is recorded as a
 * raw→corrected training pair — **the whole point** — straight into the core, which lives in
 * [IdiolectCoreHost] (kept alive by this Activity's reference even though the IME was torn
 * down by the keyboard switch), and the approved text is written **straight into the original
 * field** by the [IdiolectAccessibilityService] (the only API that can type into another app
 * while a different keyboard is active). If that service isn't enabled, it falls back to
 * deferred insert ([PendingInsert]) and offers to turn it on. **Cancel** keeps the take as a
 * raw-only sample.
 *
 * A separate Activity — not the IME input view — because only one keyboard is active at a
 * time, so the edit field must live in a normal window to summon the user's real keyboard.
 */
class ReviewActivity : Activity() {
    // Hold the core alive for the capture, independent of the IME's lifecycle.
    private val host by lazy { IdiolectCoreHost.acquire(this) }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        host // acquire now
        val historyId = intent.getLongExtra(EXTRA_ID, -1L)
        val raw = intent.getStringExtra(EXTRA_RAW).orEmpty()

        val field = EditText(this).apply {
            setText(raw)
            setSelection(text.length)
            inputType = InputType.TYPE_CLASS_TEXT or
                InputType.TYPE_TEXT_FLAG_MULTI_LINE or
                InputType.TYPE_TEXT_FLAG_CAP_SENTENCES
            // Tell idiolect "this is my review field" so it hands off to the user's keyboard
            // (idiolect has no keyboard of its own) rather than drawing its mic over the card.
            privateImeOptions = REVIEW_FIELD_OPTION
            gravity = Gravity.TOP or Gravity.START
            minLines = 3
            background = ContextCompat.getDrawable(this@ReviewActivity, R.drawable.review_field_bg)
            setPadding(dp(12), dp(11), dp(12), dp(11))
            setTextColor(ContextCompat.getColor(this@ReviewActivity, R.color.idiolect_text))
            textSize = 15f
        }

        setContentView(buildScrim(buildCard(field, historyId, raw)))

        field.requestFocus()
        // STATE_VISIBLE alone *replaces* the manifest's adjustResize, so the window stops
        // shrinking above the keyboard and the Insert/Cancel row hides behind it. Keep both:
        // pop the keyboard AND resize the window so the card stays fully reachable.
        window.setSoftInputMode(
            WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE or
                WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE,
        )
    }

    override fun onDestroy() {
        IdiolectCoreHost.release()
        super.onDestroy()
    }

    /** Full-screen dim scrim; tapping outside the card cancels (leaves the take as raw). */
    private fun buildScrim(card: View): View = FrameLayout(this).apply {
        setBackgroundColor(Color.parseColor("#cc05060a"))
        setOnClickListener { finish() }
        addView(
            card,
            // Anchor near the top, not centred: with adjustResize the window shrinks above the
            // keyboard, and a top-anchored card keeps the whole thing — including the buttons —
            // clear of it, however tall the keyboard is.
            FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.WRAP_CONTENT,
                Gravity.TOP,
            ).apply { leftMargin = dp(20); rightMargin = dp(20); topMargin = dp(36) },
        )
    }

    private fun buildCard(field: EditText, historyId: Long, raw: String): View {
        val title = TextView(this).apply {
            text = getString(R.string.review_title)
            setTextColor(ContextCompat.getColor(this@ReviewActivity, R.color.idiolect_text))
            textSize = 16f
            setTypeface(typeface, android.graphics.Typeface.BOLD)
        }
        val hint = TextView(this).apply {
            text = getString(R.string.review_hint)
            setTextColor(ContextCompat.getColor(this@ReviewActivity, R.color.idiolect_muted))
            textSize = 12f
        }
        val cancel = Button(this).apply {
            text = getString(R.string.review_cancel)
            setTextColor(ContextCompat.getColor(this@ReviewActivity, R.color.idiolect_muted))
            background = null
            setOnClickListener { finish() }
        }
        val insert = Button(this).apply {
            text = getString(R.string.review_insert)
            setTextColor(Color.WHITE)
            background = ContextCompat.getDrawable(this@ReviewActivity, R.drawable.review_btn_insert)
            setOnClickListener { onInsert(historyId, raw, field.text.toString()) }
        }
        val actions = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.END
            addView(cancel)
            addView(insert, LinearLayout.LayoutParams(WRAP, WRAP).apply { leftMargin = dp(8) })
        }
        return LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            background = ContextCompat.getDrawable(this@ReviewActivity, R.drawable.review_card_bg)
            setPadding(dp(16), dp(15), dp(16), dp(13))
            isClickable = true // swallow taps so they don't reach the cancel-on-scrim handler
            addView(title, lp())
            addView(hint, lp().apply { topMargin = dp(3) })
            addView(field, lp().apply { topMargin = dp(11) })
            // Instant insert off → Insert can't auto-land in the app yet; nudge the user to
            // enable it (one-time, in Accessibility settings). Shown only while it's missing.
            if (!instantInsertEnabled()) {
                addView(buildEnableBanner(), lp().apply { topMargin = dp(11) })
            }
            addView(actions, lp().apply { topMargin = dp(12) })
        }
    }

    /** The "turn on instant insert" prompt, shown in the card when the service isn't enabled. */
    private fun buildEnableBanner(): View {
        val note = TextView(this).apply {
            text = getString(R.string.review_enable_insert)
            setTextColor(ContextCompat.getColor(this@ReviewActivity, R.color.idiolect_muted))
            textSize = 12f
        }
        val enable = Button(this).apply {
            text = getString(R.string.review_enable_cta)
            setTextColor(Color.WHITE)
            background = ContextCompat.getDrawable(this@ReviewActivity, R.drawable.review_btn_insert)
            setOnClickListener {
                startActivity(
                    Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                )
            }
        }
        return LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            addView(note, lp())
            addView(enable, lp().apply { topMargin = dp(8); gravity = Gravity.END })
        }
    }

    /**
     * Record the correction (if the user actually changed the text), then land the approved
     * text in the original field. Capture is the priority — it goes straight to the on-disk
     * store via the core, so it survives whatever happens next. Insertion prefers the
     * accessibility service (lands immediately, no keyboard switch); if it isn't enabled or the
     * field is gone, it defers to [PendingInsert] so idiolect types it on its next focus.
     */
    private fun onInsert(historyId: Long, raw: String, edited: String) {
        if (historyId >= 0 && ReviewDecision.isCorrection(raw, edited)) {
            // Amend the persisted take with the edit → records the raw→corrected training pair.
            runCatching { host.core.historyEdited(historyId, edited) }
        }
        ReviewDecision.textToInsert(edited)?.let { toInsert ->
            if (instantInsertEnabled()) {
                // Hand the text to the accessibility service, which types it into the field the
                // moment it regains focus after this dialog closes — no keyboard switch needed.
                InjectQueue(File(filesDir, IdiolectAccessibilityService.PENDING_FILE)).put(toInsert)
            } else {
                // Service off: fall back to insert-on-return (idiolect types it on next focus).
                PendingInsert.set(toInsert)
            }
        }
        returnToIdiolect()
        finish()
    }

    /**
     * Pull the active IME back to idiolect's mic, so the user can dictate again without manually
     * switching keyboards (the auto-return they asked for). Android forbids an app from selecting
     * an IME without [Manifest.permission.WRITE_SECURE_SETTINGS] — a deliberate anti-hijack rule
     * — so this is a no-op unless that permission was granted once via `adb pm grant`; without it
     * the user returns to idiolect with a single tap on the system IME switcher (the fallback).
     * With instant insert on, the accessibility service still injects on the field's focus event
     * regardless of which IME is active, so the text lands either way.
     */
    private fun returnToIdiolect() {
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.WRITE_SECURE_SETTINGS) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        val ime = ImeSelection.idiolectImeId(packageName, IdiolectImeService::class.java.name)
        runCatching {
            Settings.Secure.putString(contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD, ime)
        }
    }

    /** Whether idiolect's instant-insert accessibility service is enabled right now. */
    private fun instantInsertEnabled(): Boolean = AccessibilityServices.isListed(
        Settings.Secure.getString(contentResolver, Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES),
        "$packageName/${IdiolectAccessibilityService::class.java.name}",
    )

    private fun lp() = LinearLayout.LayoutParams(MATCH, WRAP)

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    companion object {
        private const val EXTRA_ID = "org.idiolect.android.ime.REVIEW_ID"
        private const val EXTRA_RAW = "org.idiolect.android.ime.REVIEW_RAW"
        private const val MATCH = LinearLayout.LayoutParams.MATCH_PARENT
        private const val WRAP = LinearLayout.LayoutParams.WRAP_CONTENT

        /** Marks the review field so [IdiolectImeService] hands off to the user's keyboard. */
        const val REVIEW_FIELD_OPTION = "org.idiolect.android.review_field"

        /** Launch the review surface for a finished take (its history [id] + [rawText]). */
        fun launch(context: Context, id: Long, rawText: String) {
            val intent = Intent(context, ReviewActivity::class.java)
                .putExtra(EXTRA_ID, id)
                .putExtra(EXTRA_RAW, rawText)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_NO_HISTORY)
            context.startActivity(intent)
        }
    }
}
