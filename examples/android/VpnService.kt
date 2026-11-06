package com.example.tokiotun

import android.content.Intent
import android.net.VpnService
import android.os.ParcelFileDescriptor

/**
 * Minimal Android VpnService example.
 *
 * Start this service only after the containing Activity has obtained consent
 * with VpnService.prepare(). The service owns the VpnService fd until it is
 * detached and transferred to Rust.
 */
class TokioTunService : VpnService() {
    private var tunnel: ParcelFileDescriptor? = null
    private var started = false

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (started) {
            return START_STICKY
        }

        val builder = Builder()
            .setSession("tokio-tun")
            .setMtu(1500)
            .addAddress("10.0.0.1", 24)
            .addRoute("0.0.0.0", 0)
            .addDnsServer("1.1.1.1")

        val pfd = builder.establish() ?: return START_NOT_STICKY
        tunnel = pfd

        // detachFd() transfers ownership of the raw descriptor to the caller.
        // Rust owns it after nativeStart() and closes it when TunQueue is dropped.
        val rawFd = pfd.detachFd()
        tunnel = null

        if (!nativeStart(rawFd)) {
            stopSelf()
            return START_NOT_STICKY
        }
        started = true
        return START_STICKY
    }

    override fun onDestroy() {
        if (started) {
            nativeStop()
            started = false
        }
        tunnel?.close()
        super.onDestroy()
    }

    private external fun nativeStart(fd: Int): Boolean
    private external fun nativeStop()

    companion object {
        init {
            System.loadLibrary("tokio_tun_bridge")
        }
    }
}
