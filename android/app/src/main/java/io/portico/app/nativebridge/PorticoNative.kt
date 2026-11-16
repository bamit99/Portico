package io.portico.app.nativebridge

object PorticoNative {
    init {
        System.loadLibrary("portico_jni")
    }

    external fun nativeStartDaemon(dataDir: String, ctrlSockPath: String): Boolean
    external fun nativeStopDaemon()
    external fun nativeAttachUsbFd(
        fd: Int,
        busId: String,
        busNum: Int,
        devNum: Int,
        fallbackProduct: String
    ): Boolean

    external fun nativeRegisterPendingUsb(
        busId: String,
        vid: Int,
        pid: Int,
        product: String
    ): Boolean

    external fun nativeDetachUsb(busId: String): Boolean
    external fun nativeExecControl(cmd: String): String
}
