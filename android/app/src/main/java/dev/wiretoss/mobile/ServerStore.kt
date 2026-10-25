package dev.wiretoss.mobile

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject

/** 只收藏服务器地址，不保存账号密码。登录会话在 WebView 的 Cookie 里。 */
class ServerStore(context: Context) {
    private val prefs = context.getSharedPreferences("wiretoss", Context.MODE_PRIVATE)

    fun load(): List<Server> {
        val raw = prefs.getString(KEY, null) ?: return emptyList()
        return runCatching {
            val array = JSONArray(raw)
            buildList {
                for (index in 0 until array.length()) {
                    val item = array.getJSONObject(index)
                    add(
                        Server(
                            id = item.optString("id"),
                            name = item.optString("name"),
                            url = item.optString("url"),
                            lastUsed = item.optLong("lastUsed"),
                        )
                    )
                }
            }
                .filter { it.url.isNotBlank() }
                .sortedByDescending { it.lastUsed }
        }.getOrDefault(emptyList())
    }

    fun save(servers: List<Server>) {
        val array = JSONArray()
        servers.forEach { server ->
            array.put(
                JSONObject().apply {
                    put("id", server.id)
                    put("name", server.name)
                    put("url", server.url)
                    put("lastUsed", server.lastUsed)
                }
            )
        }
        prefs.edit().putString(KEY, array.toString()).apply()
    }

    fun upsert(server: Server): List<Server> {
        val next = load().filterNot { it.url == server.url }.toMutableList()
        next.add(0, server)
        save(next)
        return next
    }

    fun delete(id: String): List<Server> {
        val next = load().filterNot { it.id == id }
        save(next)
        return next
    }

    private companion object {
        const val KEY = "servers.v1"
    }
}
