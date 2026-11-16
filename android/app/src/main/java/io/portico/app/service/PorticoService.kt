package io.portico.app.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.net.wifi.WifiManager
import android.os.Binder
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import io.portico.app.MainActivity
import io.portico.app.R
import io.portico.app.control.ControlChannelClient
import io.portico.app.control.ControlChannelParser
import io.portico.app.control.ControlReply
import io.portico.app.nativebridge.PorticoNative
import io.portico.app.usb.UsbDeviceManagerBridge
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import java.io.File
import java.util.concurrent.Executors

class PorticoService : Service() {
    inner class LocalBinder : Binder() {
        fun getService(): PorticoService = this@PorticoService
    }

    private val binder = LocalBinder()
    private val daemonExecutor = Executors.newSingleThreadExecutor { r ->
        Thread(r, "portico-daemon-jni").apply { isDaemon = true }
    }
    private val daemonDispatcher = daemonExecutor.asCoroutineDispatcher()
    private val serviceScope = CoroutineScope(SupervisorJob() + daemonDispatcher)

    private var usbBridge: UsbDeviceManagerBridge? = null
    private var eventSubJob: Job? = null
    private var wifiLock: WifiManager.WifiLock? = null
    private var multicastLock: WifiManager.MulticastLock? = null
    private var wakeLock: PowerManager.WakeLock? = null
    @Volatile
    private var daemonStarted = false

    val ctrlSocketPath: String by lazy {
        File(filesDir, "portico-ctrl.sock").absolutePath
    }

    val controlClient: ControlChannelClient by lazy {
        ControlChannelClient(ctrlSocketPath)
    }

    override fun onBind(intent: Intent?): IBinder = binder

    override fun onCreate() {
        super.onCreate()
        ensureNotificationChannel()
        startForeground(NOTIF_ID, buildNotification("Starting Portico USB server…"))
        serviceScope.launch {
            ensureDaemonStartedBlocking()
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        ensureNotificationChannel()
        startForeground(NOTIF_ID, buildNotification("Portico USB server active"))

        val action = intent?.action
        serviceScope.launch {
            ensureDaemonStartedBlocking()
            when (action) {
                ACTION_START_AND_SHARE -> {
                    controlClient.sendCommand("START_SERVER")
                    syncLocksFromDaemon()
                }
                ACTION_STOP_SERVER -> {
                    controlClient.sendCommand("STOP_SERVER")
                    syncLocksFromDaemon()
                }
                ACTION_SHUTDOWN_EXIT -> {
                    runCatching { controlClient.sendCommand("SHUTDOWN") }
                    stopSelf()
                }
            }
        }
        return START_STICKY
    }

    override fun onDestroy() {
        eventSubJob?.cancel()
        usbBridge?.unregisterReceiverOnly()
        releaseAllLocks()
        // Stop daemon & wait for all active USB/IP tunnels BEFORE closing Android USB fds!
        runBlocking(daemonDispatcher) {
            if (daemonStarted) {
                PorticoNative.nativeStopDaemon()
                daemonStarted = false
            }
            usbBridge?.closeConnectionsBlocking()
            usbBridge = null
        }
        serviceScope.cancel()
        daemonExecutor.shutdown()
        super.onDestroy()
    }

    fun requestUsbPermission(busId: String) {
        usbBridge?.requestPermissionForBus(busId)
    }

    fun rescanUsb() {
        usbBridge?.rescanAsync()
    }

    fun applyLocks(netLock: Boolean, wakeLockEnabled: Boolean, serverRunning: Boolean) {
        val wifiMgr = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        if (netLock) {
            if (wifiLock == null) {
                @Suppress("DEPRECATION")
                val mode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    WifiManager.WIFI_MODE_FULL_LOW_LATENCY
                } else {
                    WifiManager.WIFI_MODE_FULL_HIGH_PERF
                }
                wifiLock = wifiMgr.createWifiLock(mode, "Portico:NetLock").apply {
                    setReferenceCounted(false)
                }
            }
            if (wifiLock?.isHeld == false) wifiLock?.acquire()
        } else {
            if (wifiLock?.isHeld == true) wifiLock?.release()
        }

        if (serverRunning) {
            if (multicastLock == null) {
                multicastLock = wifiMgr.createMulticastLock("Portico:Discovery").apply {
                    setReferenceCounted(false)
                }
            }
            if (multicastLock?.isHeld == false) multicastLock?.acquire()
        } else {
            if (multicastLock?.isHeld == true) multicastLock?.release()
        }

        val pm = applicationContext.getSystemService(Context.POWER_SERVICE) as PowerManager
        if (wakeLockEnabled) {
            if (wakeLock == null) {
                wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "Portico:WakeLock").apply {
                    setReferenceCounted(false)
                }
            }
            if (wakeLock?.isHeld == false) wakeLock?.acquire(24 * 60 * 60 * 1000L)
        } else {
            if (wakeLock?.isHeld == true) wakeLock?.release()
        }
    }

    private suspend fun ensureDaemonStartedBlocking() {
        if (daemonStarted) return
        val ok = PorticoNative.nativeStartDaemon(filesDir.absolutePath, ctrlSocketPath)
        daemonStarted = ok
        if (ok) {
            val bridge = UsbDeviceManagerBridge(this, serviceScope, daemonDispatcher) {}
            usbBridge = bridge
            bridge.start()
            controlClient.sendCommand("REGISTER_CLIENT android-service")
            startHeadlessEventObserver()
            syncLocksFromDaemon()
        }
    }

    private fun startHeadlessEventObserver() {
        eventSubJob?.cancel()
        eventSubJob = serviceScope.launch {
            controlClient.subscribeEvents().collect { ev ->
                when (ev.type) {
                    "RECONNECTED", "LOCK_CHANGED", "SERVER_STATE" -> syncLocksFromDaemon()
                }
            }
        }
    }

    private suspend fun syncLocksFromDaemon() {
        val reply = controlClient.sendCommand("GET_STATE")
        if (reply is ControlReply.Ok) {
            val st = ControlChannelParser.parseState(reply.line)
            BootPolicy.setStartOnBoot(this, st.startOnBoot)
            applyLocks(st.netLock, st.wakeLock, st.running)
            updateNotification(
                if (st.running) {
                    "Sharing USB devices on port ${st.port}"
                } else {
                    "USB sharing paused"
                },
            )
        }
    }

    private fun releaseAllLocks() {
        if (wifiLock?.isHeld == true) wifiLock?.release()
        if (multicastLock?.isHeld == true) multicastLock?.release()
        if (wakeLock?.isHeld == true) wakeLock?.release()
    }

    private fun ensureNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val nm = getSystemService(NotificationManager::class.java)
            val chan = NotificationChannel(
                CHANNEL_ID,
                "Portico USB Sharing",
                NotificationManager.IMPORTANCE_LOW,
            )
            nm.createNotificationChannel(chan)
        }
    }

    private fun updateNotification(text: String) {
        val nm = getSystemService(NotificationManager::class.java)
        nm.notify(NOTIF_ID, buildNotification(text))
    }

    private fun buildNotification(text: String): Notification {
        val openIntent = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_portico_usb)
            .setContentTitle("Portico")
            .setContentText(text)
            .setOngoing(true)
            .setContentIntent(openIntent)
            .build()
    }

    companion object {
        private const val CHANNEL_ID = "portico_service"
        private const val NOTIF_ID = 3742
        const val ACTION_START_AND_SHARE = "io.portico.app.action.START_AND_SHARE"
        const val ACTION_STOP_SERVER = "io.portico.app.action.STOP_SERVER"
        const val ACTION_SHUTDOWN_EXIT = "io.portico.app.action.SHUTDOWN_EXIT"
    }
}
