import Foundation

/// 一台可以连接的 Wiretoss 服务端。只保存地址和显示名，不保存密码。
struct Server: Identifiable, Codable, Hashable {
    var id: UUID
    var name: String
    var url: String
    var lastUsed: Date

    init(id: UUID = UUID(), name: String, url: String, lastUsed: Date = .now) {
        self.id = id
        self.name = name
        self.url = url
        self.lastUsed = lastUsed
    }

    var webURL: URL? { URL(string: url) }
}

/// Bonjour 自动发现的结果。id 用服务实例名，删除/去重时稳定。
struct DiscoveredServer: Identifiable, Hashable {
    let id: String
    let name: String
    let url: String
}

enum ServerAddress {
    /// 允许用户只输入 `192.168.1.5` 或 `192.168.1.5:8080`。
    /// 统一补成 `http://host:8080/`，Wiretoss 服务端只提供根路径下的 UI。
    static func normalize(_ raw: String) -> URL? {
        var text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return nil }
        if !text.contains("://") {
            text = "http://" + text
        }
        guard var components = URLComponents(string: text) else { return nil }
        let scheme = components.scheme?.lowercased()
        guard scheme == "http" || scheme == "https" else { return nil }
        guard let host = components.host, !host.isEmpty else { return nil }

        if components.port == nil {
            components.port = 8080
        }
        components.path = "/"
        components.query = nil
        components.fragment = nil
        return components.url
    }

    static func displayName(for url: URL) -> String {
        url.host ?? url.absoluteString
    }
}
