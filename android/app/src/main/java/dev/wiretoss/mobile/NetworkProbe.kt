package dev.wiretoss.mobile

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.net.ConnectException
import java.net.HttpURLConnection
import java.net.SocketTimeoutException
import java.net.URL
import java.net.UnknownHostException

object NetworkProbe {
    suspend fun check(url: String): Result<Unit> = withContext(Dispatchers.IO) {
        runCatching {
            val connection = URL(url).openConnection() as HttpURLConnection
            connection.connectTimeout = 5_000
            connection.readTimeout = 5_000
            connection.requestMethod = "GET"
            connection.instanceFollowRedirects = true
            connection.setRequestProperty("X-Wiretoss-Client", "WiretossAndroid")
            try {
                val code = connection.responseCode
                if (code !in 200..399) error("服务器返回 HTTP $code")
                connection.inputStream?.use { it.readBytes() }
                Unit
            } finally {
                connection.disconnect()
            }
        }
    }

    fun friendlyMessage(error: Throwable): String = when (error) {
        is SocketTimeoutException -> "连接超时。检查 IP 地址、端口和防火墙设置。"
        is ConnectException -> "连不上这台设备。确认电脑上的 Wiretoss 已启动，并且两台设备在同一个局域网。"
        is UnknownHostException -> "找不到这个地址。检查 IP 或主机名是否正确。"
        else -> {
            val message = error.message.orEmpty()
            if (message.contains("Cleartext", ignoreCase = true)) {
                "系统拦截了局域网 HTTP 连接。请更新到最新版本后再试。"
            } else {
                message.ifBlank { "连接失败，请重试。" }
            }
        }
    }
}
