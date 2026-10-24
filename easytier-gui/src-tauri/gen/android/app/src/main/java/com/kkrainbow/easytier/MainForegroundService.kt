package com.kkrainbow.easytier

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat

/**
 * No-TUN process keepalive while VPN is down.
 *
 * Uses [NotificationManager.IMPORTANCE_MIN] so this does **not** occupy the
 * status bar; the visible top-bar icon is only shown by [TauriVpnService]
 * while the VPN is connected.
 */
class MainForegroundService : Service() {
    companion object {
        // New id: prior DEFAULT channel would keep showing a status-bar icon.
        const val CHANNEL_ID = "easytier_keepalive_min_v3"
        const val NOTIFICATION_ID = 1355
    }

    override fun onCreate() {
        super.onCreate()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        createNotificationChannel()
        val notification = NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("EasyTier")
            .setContentText("Background keepalive (VPN inactive)")
            .setSmallIcon(R.drawable.ic_stat_easytier)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setPriority(NotificationCompat.PRIORITY_MIN)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setVisibility(NotificationCompat.VISIBILITY_SECRET)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .build()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE,
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
        return START_STICKY
    }

    override fun onDestroy() {
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            try {
                val channel = NotificationChannel(
                    CHANNEL_ID,
                    "EasyTier keepalive",
                    // MIN: shade-only / below the fold — no status-bar glyph.
                    NotificationManager.IMPORTANCE_MIN,
                ).apply {
                    description = "Keeps EasyTier alive when VPN is off; hidden from status bar"
                    setShowBadge(false)
                    setSound(null, null)
                    enableVibration(false)
                }
                val manager = getSystemService(NotificationManager::class.java)
                manager?.createNotificationChannel(channel)
            } catch (e: Exception) {
                Log.e("MainForegroundService", "Failed to create notification channel", e)
            }
        }
    }
}
