package com.plugin.vpnservice

import android.app.Activity
import android.content.Intent
import android.net.VpnService
import android.util.Log
import androidx.activity.result.ActivityResult
import app.tauri.annotation.Command
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import android.webkit.WebView

@InvokeArg
class PingArgs {
    var value: String? = null
}

@InvokeArg
class StartVpnArgs {
    var ipv4Addr: String? = null
    var ipv6Addr: String? = null
    var routes: Array<String> = emptyArray()
    var dns: String? = null
    var disallowedApplications: Array<String> = emptyArray()
    var mtu: Int? = null
}

@TauriPlugin
class VpnServicePlugin(private val activity: Activity) : Plugin(activity) {
    companion object {
        private const val TAG = "VpnServicePlugin"

        @Volatile
        private var tileActionReady = false

        @Volatile
        private var tileActionCallback: (String) -> Boolean = { false }

        /**
         * Returns false when the WebView/plugin is not ready so the tile can
         * fall back to opening the App (A14).
         */
        fun dispatchTileAction(action: String): Boolean {
            if (!tileActionReady) {
                return false
            }
            return tileActionCallback(action)
        }
    }

    private val implementation = Example()
    private val tileActionHandler: (String) -> Boolean = { action ->
        val data = JSObject()
        data.put("action", action)
        trigger("vpn_tile_action", data)
        true
    }

    override fun load(webView: WebView) {
        Log.i(TAG, "load vpn service plugin")
        TauriVpnService.triggerCallback = { event, data ->
            Log.i(TAG, "vpn triggerCallback $event $data")
            trigger(event, data)
        }
        tileActionCallback = tileActionHandler
        tileActionReady = true
    }

    override fun onDestroy() {
        tileActionReady = false
        if (tileActionCallback === tileActionHandler) {
            tileActionCallback = { false }
        }
        super.onDestroy()
    }

    @Command
    fun ping(invoke: Invoke) {
        val args = invoke.parseArgs(PingArgs::class.java)

        val ret = JSObject()
        ret.put("value", implementation.pong(args.value ?: "default value :("))
        invoke.resolve(ret)
    }

    @Command
    fun prepareVpn(invoke: Invoke) {
        activity.runOnUiThread {
            Log.i(TAG, "prepare vpn")
            val it = VpnService.prepare(activity)
            if (it != null) {
                startActivityForResult(invoke, it, "onPrepareVpnResult")
                return@runOnUiThread
            }
            val ret = JSObject()
            ret.put("granted", true)
            invoke.resolve(ret)
        }
    }

    @ActivityCallback
    fun onPrepareVpnResult(invoke: Invoke, result: ActivityResult) {
        val ret = JSObject()
        ret.put("granted", result.resultCode == Activity.RESULT_OK)
        invoke.resolve(ret)
    }

    @Command
    fun startVpn(invoke: Invoke) {
        val args = invoke.parseArgs(StartVpnArgs::class.java)
        activity.runOnUiThread {
            Log.i(TAG, "start vpn args=$args")

            val ret = JSObject()
            // Check consent before replacing anything: if the permission was
            // revoked, prepare() is non-null and we must not tear down the VPN
            // that is still running (R5).
            if (VpnService.prepare(activity) != null) {
                ret.put("errorMsg", "need_prepare")
                invoke.resolve(ret)
                return@runOnUiThread
            }

            // App-initiated replace: close previous PFD without poisoning self (A1).
            TauriVpnService.stopInternal()

            val intent = Intent(activity, TauriVpnService::class.java)
            intent.putExtra(TauriVpnService.IPV4_ADDR, args.ipv4Addr)
            intent.putExtra(TauriVpnService.IPV6_ADDR, args.ipv6Addr)
            intent.putExtra(TauriVpnService.ROUTES, args.routes)
            intent.putExtra(TauriVpnService.DNS, args.dns)
            intent.putExtra(TauriVpnService.DISALLOWED_APPLICATIONS, args.disallowedApplications)
            intent.putExtra(TauriVpnService.MTU, args.mtu)

            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) {
                activity.startForegroundService(intent)
            } else {
                activity.startService(intent)
            }
            invoke.resolve(ret)
        }
    }

    @Command
    fun stopVpn(invoke: Invoke) {
        activity.runOnUiThread {
            Log.i(TAG, "stop vpn")
            TauriVpnService.stopInternal()
            activity.stopService(Intent(activity, TauriVpnService::class.java))
            Log.i(TAG, "stop vpn end")
            invoke.resolve(JSObject())
        }
    }

    @Command
    fun getVpnStatus(invoke: Invoke) {
        val ret = JSObject()
        ret.put("running", TauriVpnService.self != null)
        ret.put("ipv4Addr", TauriVpnService.ipv4Addr)
        // JSObject.put(Array) ends up as Java Array.toString() ("[Ljava.lang.String;@…")
        // and the frontend spreads that string into one-character "routes".
        val routes = JSArray()
        for (route in TauriVpnService.routes) {
            routes.put(route)
        }
        ret.put("routes", routes)
        ret.put("dns", TauriVpnService.dns)
        ret.put("underlayNetworkGeneration", TauriVpnService.underlayNetworkGeneration)
        ret.put("underlayNetworkId", TauriVpnService.underlayNetworkId)
        invoke.resolve(ret)
    }

    @Command
    fun consumeVpnTileAction(invoke: Invoke) {
        val ret = JSObject()
        ret.put("action", EasyTierVpnTileService.consumePendingAction(activity))
        invoke.resolve(ret)
    }
}
