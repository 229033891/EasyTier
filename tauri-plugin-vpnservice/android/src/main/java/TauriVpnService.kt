package com.plugin.vpnservice

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Intent
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.VpnService
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.content.pm.ServiceInfo
import android.util.Log
import androidx.core.app.NotificationCompat

import app.tauri.plugin.JSObject

class TauriVpnService : VpnService() {
    companion object {
        private const val TAG = "TauriVpnService"
        private const val UNDERLAY_DEBOUNCE_MS = 400L

        @JvmField var triggerCallback: (String, JSObject) -> Unit = { _, _ -> }
        @JvmField var self: TauriVpnService? = null
        @JvmField var ipv4Addr: String? = null
        @JvmField var ipv6Addr: String? = null
        @JvmField var routes: Array<String> = emptyArray()
        @JvmField var dns: String? = null

        /** Bumped on each underlay (Wi-Fi/cellular) change so Rust can force reconnect (A9). */
        @JvmField @Volatile var underlayNetworkGeneration: Long = 0
        @JvmField @Volatile var underlayNetworkId: Long = 0

        const val IPV4_ADDR = "IPV4_ADDR"
        const val IPV6_ADDR = "IPV6_ADDR"
        const val ROUTES = "ROUTES"
        const val DNS = "DNS"
        const val DISALLOWED_APPLICATIONS = "DISALLOWED_APPLICATIONS"
        const val MTU = "MTU"

        private const val NOTIFICATION_CHANNEL_ID = "easytier_vpn_channel"
        private const val NOTIFICATION_ID = 1356

        /**
         * App-initiated stop (startVpn "replace" / stopVpn). Unlike [onRevoke],
         * this must not leave [self] null while the service instance is still
         * alive and about to receive another [onStartCommand].
         */
        @JvmStatic
        fun stopInternal() {
            self?.stopInternalLocked()
        }
    }

    private var vpnInterface: ParcelFileDescriptor? = null
    private var networkCallback: ConnectivityManager.NetworkCallback? = null
    private val mainHandler = Handler(Looper.getMainLooper())
    private var pendingUnderlayUpdate: Runnable? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // System restart after kill may deliver a null intent (START_STICKY).
        // Do not establish a default-parameter orphan TUN — stop cleanly.
        if (intent == null || intent.extras == null) {
            Log.w(TAG, "vpn on start command with null intent/extras; stopping")
            stopSelf()
            return START_NOT_STICKY
        }

        Log.i(TAG, "vpn on start command ${intent.extras}")
        // Re-bind self here: startVpn used to call onRevoke() which nulled self
        // while this same service instance kept running (A1).
        self = this

        try {
            startVpnForegroundService()
            val args = intent.extras
            // Close any previous PFD before establish to avoid leaking tun fds (A2).
            closeVpnInterface(emitStopEvent = false)

            vpnInterface = createVpnInterface(args)
            val fd = vpnInterface!!.fd
            Log.i(TAG, "vpn created fd=$fd")

            // Persist the values actually applied (not raw intent strings that
            // may differ from createVpnInterface defaults).
            ipv4Addr = args?.getString(IPV4_ADDR) ?: "10.126.126.1/24"
            ipv6Addr = args?.getString(IPV6_ADDR)
            routes = args?.getStringArray(ROUTES) ?: emptyArray()
            dns = args?.getString(DNS)

            registerUnderlayNetworkCallback()

            val eventData = JSObject()
            eventData.put("fd", fd)
            triggerCallback("vpn_service_start", eventData)
            EasyTierVpnTileService.requestStateUpdate(this)
        } catch (e: Exception) {
            Log.e(TAG, "vpn start failed", e)
            closeVpnInterface(emitStopEvent = false)
            clearStatus()
            // startVpnForegroundService() already stopped MainForegroundService, so
            // restore the no-TUN keepalive before dropping our own foreground state —
            // otherwise a failed start leaves the process with no FGS at all (R2).
            setMainForegroundServiceEnabled(true)
            stopForeground(STOP_FOREGROUND_REMOVE)
            self = null
            stopSelf()
            return START_NOT_STICKY
        }

        // VPN must be started by an explicit user/app action; sticky restart
        // with a null intent used to create a wrong default TUN (A4).
        return START_NOT_STICKY
    }

    override fun onCreate() {
        super.onCreate()
        self = this
        Log.i(TAG, "vpn on create")
    }

    override fun onDestroy() {
        Log.i(TAG, "vpn on destroy")
        // External teardown (system kill / leftover iface). Emit before close so
        // JS can suppress TUN-error rebuild and disable the ET instance.
        disconnect(reason = "destroy", notifyBeforeClose = true)
        setMainForegroundServiceEnabled(true)
        stopForeground(STOP_FOREGROUND_REMOVE)
        self = null
        EasyTierVpnTileService.requestStateUpdate(this)
        super.onDestroy()
    }

    /** System revoked VPN permission / user disconnected from the VPN tile/settings. */
    override fun onRevoke() {
        Log.i(TAG, "vpn on revoke")
        // Emit stop with reason before closing the fd: otherwise core TunDeviceError
        // can race ahead and rebuild VpnService while the user asked to disconnect.
        disconnect(reason = "revoke", notifyBeforeClose = true)
        setMainForegroundServiceEnabled(true)
        stopForeground(STOP_FOREGROUND_REMOVE)
        self = null
        EasyTierVpnTileService.requestStateUpdate(this)
        stopSelf()
        super.onRevoke()
    }

    private fun stopInternalLocked() {
        Log.i(TAG, "vpn stop internal")
        // App-initiated stop/replace — must NOT disable the ET network instance.
        disconnect(reason = "app", notifyBeforeClose = false)
        setMainForegroundServiceEnabled(true)
        stopForeground(STOP_FOREGROUND_REMOVE)
        // Keep self until onDestroy / a fresh onStartCommand rebinds it.
        EasyTierVpnTileService.requestStateUpdate(this)
    }

    /**
     * @param reason `"app"` = startVpn replace / stopVpn; `"revoke"` / `"destroy"` =
     *   external teardown that should sync into the ET GUI (disable instance).
     * @param notifyBeforeClose when true, fire `vpn_service_stop` before closing
     *   the PFD so JS can arm the TUN-error grace window first.
     */
    private fun disconnect(reason: String, notifyBeforeClose: Boolean) {
        unregisterUnderlayNetworkCallback()
        if (notifyBeforeClose && vpnInterface != null) {
            val eventData = JSObject()
            eventData.put("reason", reason)
            triggerCallback("vpn_service_stop", eventData)
            closeVpnInterface(emitStopEvent = false)
        } else {
            closeVpnInterface(emitStopEvent = true, reason = reason)
        }
        clearStatus()
    }

    private fun closeVpnInterface(emitStopEvent: Boolean, reason: String = "app") {
        val iface = vpnInterface ?: return
        vpnInterface = null
        if (emitStopEvent) {
            val eventData = JSObject()
            eventData.put("reason", reason)
            triggerCallback("vpn_service_stop", eventData)
        }
        try {
            iface.close()
        } catch (e: Exception) {
            Log.w(TAG, "vpn interface close failed", e)
        }
    }

    private fun clearStatus() {
        ipv4Addr = null
        ipv6Addr = null
        routes = emptyArray()
        dns = null
        underlayNetworkGeneration = 0
        underlayNetworkId = 0
    }

    /**
     * Bind VpnService traffic to the current default underlay and notify Rust/JS
     * when Wi-Fi ↔ cellular (or similar) switches so peer conns can be rebuilt (A9).
     */
    private fun registerUnderlayNetworkCallback() {
        unregisterUnderlayNetworkCallback()
        // Fresh session: do not inherit generation/id from a previous VpnService life.
        underlayNetworkGeneration = 0
        underlayNetworkId = 0
        val cm = getSystemService(ConnectivityManager::class.java) ?: return
        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                scheduleUnderlayUpdate(network)
            }

            override fun onCapabilitiesChanged(network: Network, networkCapabilities: NetworkCapabilities) {
                scheduleUnderlayUpdate(network)
            }

            override fun onLost(network: Network) {
                // Debounce: the next default network's onAvailable usually follows.
                scheduleUnderlayUpdate(null)
            }
        }
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                cm.registerDefaultNetworkCallback(callback)
            } else {
                cm.registerNetworkCallback(NetworkRequest.Builder().build(), callback)
            }
            networkCallback = callback
            // Seed id without bumping generation / notifying — establish often fires
            // an immediate onAvailable for the same default network.
            cm.activeNetwork?.let { applyUnderlayNetwork(it, notifyChange = false) }
        } catch (e: Exception) {
            Log.w(TAG, "register underlay NetworkCallback failed", e)
        }
    }

    private fun unregisterUnderlayNetworkCallback() {
        pendingUnderlayUpdate?.let { mainHandler.removeCallbacks(it) }
        pendingUnderlayUpdate = null
        val callback = networkCallback ?: return
        networkCallback = null
        try {
            getSystemService(ConnectivityManager::class.java)?.unregisterNetworkCallback(callback)
        } catch (e: Exception) {
            Log.w(TAG, "unregister underlay NetworkCallback failed", e)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP_MR1) {
            try {
                setUnderlyingNetworks(null)
            } catch (e: Exception) {
                Log.w(TAG, "clear setUnderlyingNetworks failed", e)
            }
        }
    }

    private fun scheduleUnderlayUpdate(network: Network?) {
        pendingUnderlayUpdate?.let { mainHandler.removeCallbacks(it) }
        val task = Runnable { applyUnderlayNetwork(network, notifyChange = true) }
        pendingUnderlayUpdate = task
        mainHandler.postDelayed(task, UNDERLAY_DEBOUNCE_MS)
    }

    private fun applyUnderlayNetwork(network: Network?, notifyChange: Boolean) {
        val cm = getSystemService(ConnectivityManager::class.java)
        val net = network ?: cm?.activeNetwork
        val netId = when {
            net == null -> 0L
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.P -> net.networkHandle
            else -> net.hashCode().toLong()
        }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP_MR1) {
            try {
                setUnderlyingNetworks(if (net != null) arrayOf(net) else null)
            } catch (e: Exception) {
                Log.w(TAG, "setUnderlyingNetworks failed", e)
            }
        }

        val previousId = underlayNetworkId
        underlayNetworkId = netId
        if (!notifyChange) {
            return
        }
        // Same underlay networkHandle: ignore (capability jitter / establish echo).
        // Previously required generation > 0, so the first post-seed callback with
        // the same id still bumped 0→1 and tore down freshly built peer conns.
        if (netId == previousId) {
            return
        }

        underlayNetworkGeneration += 1
        Log.i(
            TAG,
            "underlay network changed generation=$underlayNetworkGeneration networkId=$netId",
        )
        val data = JSObject()
        data.put("generation", underlayNetworkGeneration)
        data.put("networkId", netId)
        triggerCallback("default_network_changed", data)
    }

    private fun startVpnForegroundService() {
        createNotificationChannel()

        val launchIntent = packageManager.getLaunchIntentForPackage(packageName)?.apply {
            addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        }
        val contentIntent = launchIntent?.let {
            PendingIntent.getActivity(
                this,
                0,
                it,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        }
        val notification = NotificationCompat.Builder(this, NOTIFICATION_CHANNEL_ID)
            .setSmallIcon(android.R.drawable.ic_menu_manage)
            .setContentTitle("EasyTier VPN is running")
            .setContentText("VPN connection is active")
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .apply { contentIntent?.let(::setContentIntent) }
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

        // A TUN-backed network is now protected by this foreground service, so the
        // no-TUN keepalive service is redundant and would show a second notification.
        setMainForegroundServiceEnabled(false)
    }

    private fun setMainForegroundServiceEnabled(enabled: Boolean) {
        val intent = Intent().setClassName(packageName, "$packageName.MainForegroundService")
        if (!enabled) {
            try {
                stopService(intent)
            } catch (e: Exception) {
                Log.w(TAG, "stop MainForegroundService failed", e)
            }
            return
        }

        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                startForegroundService(intent)
            } else {
                startService(intent)
            }
        } catch (e: Exception) {
            // API 31+ may reject background FGS starts from onDestroy/onRevoke.
            Log.w(TAG, "start MainForegroundService failed", e)
        }
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                NOTIFICATION_CHANNEL_ID,
                "EasyTier VPN",
                NotificationManager.IMPORTANCE_LOW,
            )
            getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
        }
    }

    private fun createVpnInterface(args: Bundle?): ParcelFileDescriptor {
        val builder = Builder()
            .setSession("TauriVpnService")
            .setBlocking(false)

        val mtu = args?.getInt(MTU)?.takeIf { it > 0 } ?: 1500
        val addr = args?.getString(IPV4_ADDR) ?: "10.126.126.1/24"
        val ipv6WithPrefix = args?.getString(IPV6_ADDR)?.trim()?.takeIf { it.isNotEmpty() }
        val dnsServer: String? = args?.getString(DNS)
        val routeList = args?.getStringArray(ROUTES) ?: emptyArray()
        val disallowedApplications = args?.getStringArray(DISALLOWED_APPLICATIONS) ?: emptyArray()

        Log.i(
            TAG,
            "vpn create interface mtu=$mtu ipv4Addr=$addr ipv6Addr=$ipv6WithPrefix dns=$dnsServer " +
                "routes=${routeList.contentToString()} " +
                "disallowed=${disallowedApplications.contentToString()}",
        )

        val ipParts = addr.split("/")
        if (ipParts.size != 2) throw IllegalArgumentException("Invalid IP addr string")
        builder.addAddress(ipParts[0], ipParts[1].toInt())

        ipv6WithPrefix?.let { ipv6 ->
            val v6Parts = ipv6.split("/")
            if (v6Parts.size != 2) throw IllegalArgumentException("Invalid IPv6 addr string: $ipv6")
            builder.addAddress(v6Parts[0], v6Parts[1].toInt())
        }

        builder.setMtu(mtu)
        dnsServer?.let { builder.addDnsServer(it) }

        for (route in routeList) {
            val parts = route.split("/")
            if (parts.size != 2) throw IllegalArgumentException("Invalid route cidr string: $route")
            builder.addRoute(parts[0], parts[1].toInt())
        }

        for (app in disallowedApplications) {
            try {
                builder.addDisallowedApplication(app)
            } catch (e: Exception) {
                Log.w(TAG, "addDisallowedApplication failed for $app", e)
            }
        }

        return builder.also {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                it.setMetered(false)
            }
        }
            .establish()
            ?: throw IllegalStateException("Failed to init VpnService")
    }
}
