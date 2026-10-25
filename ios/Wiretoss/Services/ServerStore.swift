import Foundation

/// 常用服务器列表。只存地址，不存账号密码——登录发生在 WebView 里，
/// 会话 cookie 由 WebKit 自己持久化。
@MainActor
final class ServerStore {
    private let key = "wiretoss.servers.v1"
    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    func load() -> [Server] {
        guard let data = defaults.data(forKey: key),
              let servers = try? JSONDecoder().decode([Server].self, from: data)
        else {
            return []
        }
        return servers.sorted { $0.lastUsed > $1.lastUsed }
    }

    func save(_ servers: [Server]) {
        guard let data = try? JSONEncoder().encode(servers) else { return }
        defaults.set(data, forKey: key)
    }

    func upsert(_ server: Server, into servers: [Server]) -> [Server] {
        var next = servers.filter { $0.url != server.url }
        next.insert(server, at: 0)
        save(next)
        return next
    }

    func delete(_ id: UUID, from servers: [Server]) -> [Server] {
        let next = servers.filter { $0.id != id }
        save(next)
        return next
    }
}
