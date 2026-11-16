package io.portico.app.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import androidx.core.content.ContextCompat

object BootPolicy {
    const val PREFS_NAME = "portico_boot"
    const val KEY_START_ON_BOOT = "start_on_boot"

    fun shouldStartOnAction(action: String?, bootEnabled: Boolean): Boolean {
        if (!bootEnabled) return false
        return action == Intent.ACTION_BOOT_COMPLETED ||
            action == Intent.ACTION_LOCKED_BOOT_COMPLETED
    }

    fun setStartOnBoot(context: Context, enabled: Boolean) {
        context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            .edit()
            .putBoolean(KEY_START_ON_BOOT, enabled)
            .apply()
    }

    fun isStartOnBoot(context: Context): Boolean {
        return context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            .getBoolean(KEY_START_ON_BOOT, false)
    }
}

class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val enabled = BootPolicy.isStartOnBoot(context)
        if (BootPolicy.shouldStartOnAction(intent.action, enabled)) {
            val svcIntent = Intent(context, PorticoService::class.java).apply {
                action = PorticoService.ACTION_START_AND_SHARE
            }
            ContextCompat.startForegroundService(context, svcIntent)
        }
    }
}
