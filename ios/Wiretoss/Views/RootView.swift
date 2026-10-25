import SwiftUI

struct RootView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        Group {
            if let server = model.activeServer {
                WebViewScreen(server: server) {
                    model.disconnect()
                }
                .transition(.opacity)
            } else {
                ServerSetupView()
                    .transition(.opacity)
            }
        }
        .animation(.easeInOut(duration: 0.2), value: model.activeServer)
        .preferredColorScheme(.dark)
        .onOpenURL { url in
            // 供测试和未来的深链使用：wiretoss://connect?url=http%3A%2F%2F...
            guard url.scheme?.lowercased() == "wiretoss" else { return }
            guard let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
                  let raw = components.queryItems?.first(where: { $0.name == "url" })?.value,
                  let normalized = ServerAddress.normalize(raw)
            else {
                return
            }
            model.connect(url: normalized)
        }
    }
}
