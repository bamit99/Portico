package io.portico.app.usb

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.hardware.usb.UsbDevice
import android.hardware.usb.UsbDeviceConnection
import android.hardware.usb.UsbManager
import android.os.Build
import io.portico.app.nativebridge.PorticoNative
import java.util.concurrent.ConcurrentHashMap

class UsbDeviceManagerBridge(
    private val context: Context,
    private val onDevicesChanged: () -> Unit,
) {
    private val usbManager = context.getSystemService(Context.USB_SERVICE) as UsbManager
    private val openConnections = ConcurrentHashMap<String, UsbDeviceConnection>()
    private var registered = false

    private val receiver = object : BroadcastReceiver() {
        override fun onReceive(ctx: Context, intent: Intent) {
            when (intent.action) {
                ACTION_USB_PERMISSION -> {
                    val device = extractUsbDevice(intent) ?: return
                    val granted = intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)
                    if (granted) {
                        openAndAttach(device)
                    }
                    onDevicesChanged()
                }
                UsbManager.ACTION_USB_DEVICE_ATTACHED -> {
                    val device = extractUsbDevice(intent) ?: return
                    handleDeviceAttached(device)
                    onDevicesChanged()
                }
                UsbManager.ACTION_USB_DEVICE_DETACHED -> {
                    val device = extractUsbDevice(intent) ?: return
                    detachDevice(device)
                    onDevicesChanged()
                }
            }
        }
    }

    fun start() {
        if (registered) return
        val filter = IntentFilter().apply {
            addAction(ACTION_USB_PERMISSION)
            addAction(UsbManager.ACTION_USB_DEVICE_ATTACHED)
            addAction(UsbManager.ACTION_USB_DEVICE_DETACHED)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            @Suppress("DEPRECATION")
            context.registerReceiver(receiver, filter)
        }
        registered = true
        scanAttachedDevices()
    }

    fun stop() {
        if (registered) {
            runCatching { context.unregisterReceiver(receiver) }
            registered = false
        }
        for ((busId, conn) in openConnections.entries.toList()) {
            PorticoNative.nativeDetachUsb(busId)
            runCatching { conn.close() }
            openConnections.remove(busId)
        }
    }

    fun scanAttachedDevices() {
        val current = usbManager.deviceList.values.toList()
        val currentBusIds = current.map { busIdFor(it) }.toSet()

        // Detach any disappeared devices cleanly before closing fds
        for (busId in openConnections.keys.toList()) {
            if (busId !in currentBusIds) {
                PorticoNative.nativeDetachUsb(busId)
                openConnections.remove(busId)?.let { runCatching { it.close() } }
            }
        }

        for (dev in current) {
            handleDeviceAttached(dev)
        }
    }

    fun requestPermissionForBus(busId: String) {
        val dev = usbManager.deviceList.values.firstOrNull { busIdFor(it) == busId } ?: return
        if (usbManager.hasPermission(dev)) {
            openAndAttach(dev)
            onDevicesChanged()
        } else {
            requestPermission(dev)
        }
    }

    private fun handleDeviceAttached(device: UsbDevice) {
        val busId = busIdFor(device)
        if (openConnections.containsKey(busId)) return
        if (usbManager.hasPermission(device)) {
            openAndAttach(device)
        } else {
            val product = device.productName ?: "USB Device"
            PorticoNative.nativeRegisterPendingUsb(busId, device.vendorId, device.productId, product)
            requestPermission(device)
        }
    }

    private fun requestPermission(device: UsbDevice) {
        val flags = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        } else {
            PendingIntent.FLAG_UPDATE_CURRENT
        }
        val pi = PendingIntent.getBroadcast(
            context,
            device.deviceId,
            Intent(ACTION_USB_PERMISSION).setPackage(context.packageName),
            flags,
        )
        usbManager.requestPermission(device, pi)
    }

    private fun openAndAttach(device: UsbDevice) {
        val busId = busIdFor(device)
        if (openConnections.containsKey(busId)) return
        val conn = usbManager.openDevice(device) ?: return
        val (busNum, devNum) = parseBusAndDev(device)
        val fallbackProduct = device.productName ?: "USB Device"
        // Keep `conn` alive in `openConnections` until AFTER `nativeDetachUsb(busId)` completes!
        openConnections[busId] = conn
        val ok = PorticoNative.nativeAttachUsbFd(
            conn.fileDescriptor,
            busId,
            busNum,
            devNum,
            fallbackProduct,
        )
        if (!ok) {
            openConnections.remove(busId)
            runCatching { conn.close() }
        }
    }

    private fun detachDevice(device: UsbDevice) {
        val busId = busIdFor(device)
        PorticoNative.nativeDetachUsb(busId)
        openConnections.remove(busId)?.let { runCatching { it.close() } }
    }

    companion object {
        const val ACTION_USB_PERMISSION = "io.portico.app.USB_PERMISSION"

        fun busIdFor(device: UsbDevice): String {
            val (b, d) = parseBusAndDev(device)
            return "$b-$d"
        }

        private fun parseBusAndDev(device: UsbDevice): Pair<Int, Int> {
            val parts = device.deviceName.trim('/').split('/')
            if (parts.size >= 2) {
                val bus = parts[parts.size - 2].toIntOrNull()
                val dev = parts[parts.size - 1].toIntOrNull()
                if (bus != null && dev != null) {
                    return bus to dev
                }
            }
            return 1 to (device.deviceId and 0xFF)
        }

        @Suppress("DEPRECATION")
        private fun extractUsbDevice(intent: Intent): UsbDevice? {
            return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent.getParcelableExtra(UsbManager.EXTRA_DEVICE, UsbDevice::class.java)
            } else {
                intent.getParcelableExtra(UsbManager.EXTRA_DEVICE)
            }
        }
    }
}
