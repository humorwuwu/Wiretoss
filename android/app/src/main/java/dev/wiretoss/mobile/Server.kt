package dev.wiretoss.mobile

import android.net.Uri

data class Server(
    val id: String,
    val name: String,
    val url: String,
    val lastUsed: Long,
)

data class DiscoveredServer(
    val id: String,
    val name: String,
    val url: String,
)

object ServerAddress {
    /** 允许只输入 192.168.1.5 或 192.168.1.5:8080，统一补成根路径。 */
    fun normalize(raw: String): String? {
        var text = raw.trim()
        if (text.isEmpty()) return null
        if (!text.contains("://")) text = "http://$text"

        val uri = runCatching { Uri.parse(text) }.getOrNull() ?: return null
        val scheme = uri.scheme?.lowercase() ?: return null
        if (scheme != "http" && scheme != "https") return null
        val host = uri.host ?: return null
        if (host.isBlank()) return null

        val port = if (uri.port == -1) 8080 else uri.port
        val authority = if (host.contains(":")) "[$host]:$port" else "$host:$port"
        return Uri.Builder()
            .scheme(scheme)
            .encodedAuthority(authority)
            .path("/")
            .build()
            .toString()
    }

    fun displayName(url: String): String = Uri.parse(url).host ?: url
}
