package com.easytier.jni

import android.app.Activity
import android.content.Intent
import android.os.Handler
import android.os.Looper
import android.util.Log
import com.squareup.moshi.Moshi
import com.squareup.wire.WireJsonAdapterFactory
import common.Ipv4Inet
import web.NetworkInstanceRunningInfoMap

fun parseIpv4InetToString(inet: Ipv4Inet?): String? {
    val addr = inet?.address?.addr ?: return null
    val networkLength = inet.network_length

    // 将 int32 转换为 IPv4 字符串
    val ip =
            String.format(
                    "%d.%d.%d.%d",
                    (addr shr 24) and 0xFF,
                    (addr shr 16) and 0xFF,
                    (addr shr 8) and 0xFF,
                    addr and 0xFF
            )

    return "$ip/$networkLength"
}

/** EasyTier 管理类 负责管理 EasyTier 实例的生命周期、监控网络状态变化、控制 VpnService */
class EasyTierManager(
        private val activity: Activity,
        private val instanceName: String,
        private val networkConfig: String
) {
    companion object {
        private const val TAG = "EasyTierManager"
        private const val MONITOR_INTERVAL = 3000L // 3秒监控间隔
    }

    private val handler = Handler(Looper.getMainLooper())
    private var isRunning = false
    private var currentIpv4: String? = null
    private var currentProxyCidrs: List<String> = emptyList()
    private var currentEnableMagicDns: Boolean = false
    private var vpnServiceIntent: Intent? = null
    private val allowPeerDefaultWithoutExit: Boolean
    private val exitNodes: List<String>
    private val enableMagicDns: Boolean

    init {
        val policy = parseRoutePolicy(networkConfig)
        allowPeerDefaultWithoutExit = policy.allowPeerDefaultWithoutExit
        exitNodes = policy.exitNodes
        enableMagicDns = policy.enableMagicDns
    }

    // JSON 解析器
    private val moshi = Moshi.Builder().add(WireJsonAdapterFactory()).build()
    private val adapter = moshi.adapter(NetworkInstanceRunningInfoMap::class.java)

    // 监控任务
    private val monitorRunnable =
            object : Runnable {
                override fun run() {
                    if (isRunning) {
                        monitorNetworkStatus()
                        handler.postDelayed(this, MONITOR_INTERVAL)
                    }
                }
            }

    /** 启动 EasyTier 实例和监控 */
    fun start() {
        if (isRunning) {
            Log.w(TAG, "EasyTier 实例已经在运行中")
            return
        }

        try {
            // 启动 EasyTier 实例
            val result = EasyTierJNI.runNetworkInstance(networkConfig)
            if (result == 0) {
                isRunning = true
                Log.i(TAG, "EasyTier 实例启动成功: $instanceName")

                // 开始监控网络状态
                handler.post(monitorRunnable)
            } else {
                Log.e(TAG, "EasyTier 实例启动失败: $result")
                val error = EasyTierJNI.getLastError()
                Log.e(TAG, "错误信息: $error")
            }
        } catch (e: Exception) {
            Log.e(TAG, "启动 EasyTier 实例时发生异常", e)
        }
    }

    /** 停止 EasyTier 实例和监控 */
    fun stop() {
        if (!isRunning) {
            Log.w(TAG, "EasyTier 实例未在运行")
            return
        }

        isRunning = false

        // 停止监控任务
        handler.removeCallbacks(monitorRunnable)

        try {
            // 停止 VpnService
            stopVpnService()

            // 停止 EasyTier 实例
            EasyTierJNI.stopAllInstances()
            Log.i(TAG, "EasyTier 实例已停止: $instanceName")

            // 重置状态
            currentIpv4 = null
            currentProxyCidrs = emptyList()
        } catch (e: Exception) {
            Log.e(TAG, "停止 EasyTier 实例时发生异常", e)
        }
    }

    /** 监控网络状态 */
    private fun monitorNetworkStatus() {
        try {
            val infosJson = EasyTierJNI.collectNetworkInfos(10)
            if (infosJson.isNullOrEmpty()) {
                Log.d(TAG, "未获取到网络信息")
                return
            }

            val networkInfoMap = parseNetworkInfo(infosJson)
            val networkInfo = networkInfoMap?.map?.get(instanceName)

            if (networkInfo == null) {
                Log.d(TAG, "未找到实例 $instanceName 的网络信息")
                // 实例可能已被远端（Web 控制台）删除：core 已无此实例，
                // 残留的 VpnService 会把 DNS/路由指向黑洞，必须跟随停掉。
                stopVpnServiceOnInstanceGone()
                return
            }

            Log.d(TAG, "网络信息: $networkInfo")

            // 检查实例是否正在运行
            if (!networkInfo.running) {
                Log.w(TAG, "EasyTier 实例未运行: ${networkInfo.error_msg}")
                // 同上：实例停了（本地/远端禁用），VPN 不能独自留着。
                stopVpnServiceOnInstanceGone()
                return
            }

            val newIpv4Inet = networkInfo.my_node_info?.virtual_ipv4

            if (newIpv4Inet == null) {
                Log.w(TAG, "EasyTier No Ipv4: $networkInfo")
                return
            }

            // 获取当前节点的 IPv4 地址
            val newIpv4 = parseIpv4InetToString(newIpv4Inet)

            // 获取所有节点的 proxy_cidrs（D+：仅 exit 可达或逃生阀时保留对端 0.0.0.0/0）
            val localExitDefault = hasReachableExit(networkInfo.routes)
            val newProxyCidrs = mutableListOf<String>()
            networkInfo.routes?.forEach { route ->
                route.proxy_cidrs?.forEach { cidr ->
                    if (shouldInstallProxyCidr(cidr, localExitDefault)) {
                        newProxyCidrs.add(cidr)
                    }
                }
            }
            if (localExitDefault) {
                newProxyCidrs.add("0.0.0.0/0")
            }
            // MagicDNS fake IP route (align with Tauri mobile_vpn.ts).
            if (enableMagicDns) {
                newProxyCidrs.add("10.10.10.10/32")
            }
            val dedupedProxyCidrs = newProxyCidrs.distinct().sorted()

            // 检查是否有变化
            val ipv4Changed = newIpv4 != currentIpv4
            val proxyCidrsChanged = dedupedProxyCidrs != currentProxyCidrs
            val magicDnsChanged = enableMagicDns != currentEnableMagicDns

            if (ipv4Changed || proxyCidrsChanged || magicDnsChanged) {
                Log.i(TAG, "网络状态发生变化:")
                Log.i(TAG, "  IPv4: $currentIpv4 -> $newIpv4")
                Log.i(TAG, "  Proxy CIDRs: $currentProxyCidrs -> $dedupedProxyCidrs")
                Log.i(TAG, "  MagicDNS: $currentEnableMagicDns -> $enableMagicDns")

                // 更新状态
                currentIpv4 = newIpv4
                currentProxyCidrs = dedupedProxyCidrs
                currentEnableMagicDns = enableMagicDns

                // 重启 VpnService
                if (newIpv4 != null) {
                    restartVpnService(newIpv4, dedupedProxyCidrs, enableMagicDns)
                }
            } else {
                Log.d(TAG, "网络状态无变化 - IPv4: $currentIpv4, Proxy CIDRs: ${currentProxyCidrs.size} 个")
            }
        } catch (e: Exception) {
            Log.e(TAG, "监控网络状态时发生异常", e)
        }
    }

    /** 解析网络信息 JSON */
    private fun parseNetworkInfo(jsonString: String): NetworkInstanceRunningInfoMap? {
        return try {
            adapter.fromJson(jsonString)
        } catch (e: Exception) {
            Log.e(TAG, "解析网络信息失败", e)
            null
        }
    }

    /** 重启 VpnService */
    private fun restartVpnService(ipv4: String, proxyCidrs: List<String>, enableMagicDns: Boolean) {
        try {
            // 先停止现有的 VpnService
            stopVpnService()

            // 启动新的 VpnService
            startVpnService(ipv4, proxyCidrs, enableMagicDns)
        } catch (e: Exception) {
            Log.e(TAG, "重启 VpnService 时发生异常", e)
        }
    }

    /** 启动 VpnService */
    private fun startVpnService(ipv4: String, proxyCidrs: List<String>, enableMagicDns: Boolean) {
        try {
            val intent = Intent(activity, EasyTierVpnService::class.java)
            intent.putExtra("ipv4_address", ipv4)
            intent.putStringArrayListExtra("proxy_cidrs", ArrayList(proxyCidrs))
            intent.putExtra("instance_name", instanceName)
            intent.putExtra("enable_magic_dns", enableMagicDns)

            activity.startService(intent)
            vpnServiceIntent = intent

            Log.i(
                    TAG,
                    "VpnService 已启动 - IPv4: $ipv4, Proxy CIDRs: $proxyCidrs, MagicDNS: $enableMagicDns"
            )
        } catch (e: Exception) {
            Log.e(TAG, "启动 VpnService 时发生异常", e)
        }
    }

    /** 停止 VpnService */
    private fun stopVpnService() {
        try {
            vpnServiceIntent?.let { intent ->
                activity.stopService(intent)
                Log.i(TAG, "VpnService 已停止")
            }
            vpnServiceIntent = null
        } catch (e: Exception) {
            Log.e(TAG, "停止 VpnService 时发生异常", e)
        }
    }

    /**
     * 实例消失/停止时的跟随清理：停掉残留 VPN 并重置已记录的网络状态，
     * 避免陈旧路由与 DNS 指向黑洞；实例恢复后监控会重新拉起 VPN。
     * 未启动过时 `stopVpnService` 是空操作，可安全调用。
     */
    private fun stopVpnServiceOnInstanceGone() {
        stopVpnService()
        currentIpv4 = null
        currentProxyCidrs = emptyList()
    }

    /** 获取当前状态信息 */
    fun getStatus(): EasyTierStatus {
        return EasyTierStatus(
                isRunning = isRunning,
                instanceName = instanceName,
                currentIpv4 = currentIpv4,
                currentProxyCidrs = currentProxyCidrs.toList()
        )
    }

    private fun shouldInstallProxyCidr(cidr: String, localExitDefault: Boolean): Boolean {
        if (localExitDefault || allowPeerDefaultWithoutExit) {
            return true
        }
        val normalized = if (cidr.contains('/')) cidr else "$cidr/32"
        return normalized != "0.0.0.0/0"
    }

    private fun hasReachableExit(routes: List<api.instance.Route>?): Boolean {
        if (routes.isNullOrEmpty() || exitNodes.isEmpty()) {
            return false
        }
        for (exit in exitNodes) {
            val exitIp = exit.trim().substringBefore('/')
            if (exitIp.isEmpty()) continue
            for (route in routes) {
                val vip = parseIpv4InetToString(route.ipv4_addr)?.substringBefore('/')
                if (vip == exitIp && route.next_hop_peer_id > 0) {
                    return true
                }
            }
        }
        return false
    }

    /** 状态数据类 */
    data class EasyTierStatus(
            val isRunning: Boolean,
            val instanceName: String,
            val currentIpv4: String?,
            val currentProxyCidrs: List<String>
    )
}

private data class RoutePolicy(
        val allowPeerDefaultWithoutExit: Boolean,
        val exitNodes: List<String>,
        val enableMagicDns: Boolean
)

private fun parseRoutePolicy(networkConfig: String): RoutePolicy {
    return try {
        val json = org.json.JSONObject(networkConfig)
        val allow = json.optBoolean("allow_peer_default_without_exit", false)
        val enableMagicDns = json.optBoolean("enable_magic_dns", false)
        val exitNodes = mutableListOf<String>()
        val arr = json.optJSONArray("exit_nodes")
        if (arr != null) {
            for (i in 0 until arr.length()) {
                val value = arr.optString(i)?.trim().orEmpty()
                if (value.isNotEmpty()) {
                    exitNodes.add(value)
                }
            }
        }
        RoutePolicy(allow, exitNodes, enableMagicDns)
    } catch (_: Exception) {
        RoutePolicy(false, emptyList(), false)
    }
}
