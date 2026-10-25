package dev.wiretoss.mobile

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Handler
import android.os.Looper

/** 用 Android NSD 找同一局域网里广播 `_wiretoss._tcp` 的服务端。 */
class ServerDiscovery(context: Context) {
    private val nsd = context.getSystemService(Context.NSD_SERVICE) as NsdManager
    private val main = Handler(Looper.getMainLooper())
    private val found = LinkedHashMap<String, DiscoveredServer>()
    private var listener: NsdManager.DiscoveryListener? = null

    var onUpdate: ((List<DiscoveredServer>) -> Unit)? = null
    var onSearching: ((Boolean) -> Unit)? = null
    var onError: ((String) -> Unit)? = null

    fun start() {
        if (listener != null) return
        found.clear()

        val discoveryListener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) {
                main.post { onSearching?.invoke(true) }
            }

            override fun onServiceFound(serviceInfo: NsdServiceInfo) {
                if (!serviceInfo.serviceType.contains("_wiretoss._tcp", ignoreCase = true)) return
                main.post { resolve(serviceInfo) }
            }

            override fun onServiceLost(serviceInfo: NsdServiceInfo) {
                main.post {
                    found.remove(serviceInfo.serviceName)
                    onUpdate?.invoke(found.values.toList())
                }
            }

            override fun onDiscoveryStopped(serviceType: String) {
                main.post { onSearching?.invoke(false) }
            }

            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                main.post {
                    onSearching?.invoke(false)
                    onError?.invoke("自动发现不可用（错误 $errorCode），可以手动输入地址。")
                }
                stop()
            }

            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {
                main.post { onSearching?.invoke(false) }
            }
        }

        listener = discoveryListener
        runCatching {
            nsd.discoverServices("_wiretoss._tcp.", NsdManager.PROTOCOL_DNS_SD, discoveryListener)
        }.onFailure {
            listener = null
            main.post {
                onSearching?.invoke(false)
                onError?.invoke("自动发现不可用，可以手动输入地址。")
            }
        }
    }

    fun stop() {
        val current = listener ?: return
        listener = null
        runCatching { nsd.stopServiceDiscovery(current) }
        main.post { onSearching?.invoke(false) }
    }

    private fun resolve(serviceInfo: NsdServiceInfo) {
        val resolveListener = object : NsdManager.ResolveListener {
            override fun onResolveFailed(serviceInfo: NsdServiceInfo, errorCode: Int) = Unit

            override fun onServiceResolved(serviceInfo: NsdServiceInfo) {
                val host = serviceInfo.host?.hostAddress ?: return
                val cleanHost = if (host.contains(":")) "[$host]" else host
                val url = "http://$cleanHost:${serviceInfo.port}/"
                val name = serviceInfo.serviceName.removePrefix("Wiretoss@").ifBlank { host }

                main.post {
                    found[serviceInfo.serviceName] = DiscoveredServer(
                        id = serviceInfo.serviceName,
                        name = name,
                        url = url,
                    )
                    onUpdate?.invoke(found.values.toList())
                }
            }
        }
        runCatching { nsd.resolveService(serviceInfo, resolveListener) }
    }
}
