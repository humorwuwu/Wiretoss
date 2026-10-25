import Foundation

/// 用 Bonjour 找同一局域网里的 Wiretoss 服务。
/// 服务端广播的是 `_wiretoss._tcp.local.`，实例名形如 `Wiretoss@MacBook`。
final class BonjourBrowser: NSObject, ObservableObject {
    @Published private(set) var servers: [DiscoveredServer] = []
    @Published private(set) var isSearching = false
    @Published private(set) var errorMessage: String?

    private let browser = NetServiceBrowser()
    private var resolving: [NetService] = []

    override init() {
        super.init()
        browser.delegate = self
    }

    func start() {
        guard !isSearching else { return }
        servers = []
        errorMessage = nil
        isSearching = true
        browser.searchForServices(ofType: "_wiretoss._tcp.", inDomain: "local.")
    }

    func stop() {
        browser.stop()
        resolving.forEach { $0.stop() }
        resolving.removeAll()
        isSearching = false
    }

    private func add(_ service: NetService) {
        guard let hostName = service.hostName, service.port > 0 else { return }
        let host = hostName.hasSuffix(".") ? String(hostName.dropLast()) : hostName
        var components = URLComponents()
        components.scheme = "http"
        components.host = host
        components.port = service.port
        components.path = "/"
        guard let url = components.url else { return }

        let display = service.name
            .replacingOccurrences(of: "Wiretoss@", with: "")
            .trimmingCharacters(in: .whitespaces)
        let entry = DiscoveredServer(
            id: service.name,
            name: display.isEmpty ? host : display,
            url: url.absoluteString
        )
        servers.removeAll { $0.id == entry.id }
        servers.append(entry)
    }
}

extension BonjourBrowser: NetServiceBrowserDelegate, NetServiceDelegate {
    func netServiceBrowser(
        _ browser: NetServiceBrowser,
        didFind service: NetService,
        moreComing: Bool
    ) {
        service.delegate = self
        resolving.append(service)
        service.resolve(withTimeout: 4)
    }

    func netServiceBrowser(
        _ browser: NetServiceBrowser,
        didRemove service: NetService,
        moreComing: Bool
    ) {
        DispatchQueue.main.async {
            self.servers.removeAll { $0.id == service.name }
        }
    }

    func netServiceBrowserDidStopSearch(_ browser: NetServiceBrowser) {
        DispatchQueue.main.async { self.isSearching = false }
    }

    func netServiceBrowser(
        _ browser: NetServiceBrowser,
        didNotSearch errorDict: [String: NSNumber]
    ) {
        DispatchQueue.main.async {
            self.isSearching = false
            self.errorMessage = "自动发现不可用，可以手动输入地址。"
        }
    }

    func netServiceDidResolveAddress(_ sender: NetService) {
        DispatchQueue.main.async {
            self.add(sender)
            self.resolving.removeAll { $0 === sender }
        }
    }

    func netService(_ sender: NetService, didNotResolve errorDict: [String: NSNumber]) {
        DispatchQueue.main.async {
            self.resolving.removeAll { $0 === sender }
        }
    }
}
