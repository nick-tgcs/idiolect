package org.idiolect.android.audio

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import org.idiolect.android.R

/**
 * A `foregroundServiceType=microphone` service held for the duration of a dictation
 * take. It posts an ongoing, low-importance notification so the microphone use is
 * always visible to the user (the privacy story), and gives capture a proper
 * foreground footing rather than relying solely on the IME window being shown.
 *
 * Started/stopped from [org.idiolect.android.ime.IdiolectImeService] in lock-step with
 * the core's authoritative recording state. Framework boundary (notification +
 * startForeground); the start/stop wiring is exercised by the emulator e2e.
 */
class MicForegroundService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        ServiceCompat.startForeground(
            this,
            NOTIFICATION_ID,
            buildNotification(),
            ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE,
        )
        return START_NOT_STICKY
    }

    private fun buildNotification(): Notification {
        ensureChannel()
        return Notification.Builder(this, CHANNEL_ID)
            .setContentTitle(getString(R.string.fgs_listening_title))
            .setContentText(getString(R.string.fgs_listening_text))
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setOngoing(true)
            .build()
    }

    private fun ensureChannel() {
        val manager = getSystemService(NotificationManager::class.java)
        if (manager.getNotificationChannel(CHANNEL_ID) == null) {
            manager.createNotificationChannel(
                NotificationChannel(
                    CHANNEL_ID,
                    getString(R.string.fgs_channel),
                    NotificationManager.IMPORTANCE_LOW,
                ),
            )
        }
    }

    companion object {
        private const val CHANNEL_ID = "idiolect.dictation"
        private const val NOTIFICATION_ID = 1

        /** Bring the mic foreground service up (idempotent for the system). */
        fun start(context: Context) {
            ContextCompat.startForegroundService(
                context,
                Intent(context, MicForegroundService::class.java),
            )
        }

        /** Tear the mic foreground service down. */
        fun stop(context: Context) {
            context.stopService(Intent(context, MicForegroundService::class.java))
        }
    }
}
