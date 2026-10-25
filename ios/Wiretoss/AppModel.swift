import Foundation
import SwiftUI

@MainActor
final class AppModel: ObservableObject {
    @Published private(set) var savedServers: [Server] = []
    @Published var activeServer: Server?

    private let store = ServerStore()

    init() {
        savedServers = store.load()
    }

    func connect(_ server: Server) {
        var used = server
        used.lastUsed = .now
        savedServers = store.upsert(used, into: savedServers)
        activeServer = used
    }

    func connect(url: URL) {
        connect(Server(name: ServerAddress.displayName(for: url), url: url.absoluteString))
    }

    func delete(_ server: Server) {
        savedServers = store.delete(server.id, from: savedServers)
    }

    func disconnect() {
        activeServer = nil
    }
}
