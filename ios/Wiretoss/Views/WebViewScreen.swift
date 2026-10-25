import SwiftUI

struct ShareItem: Identifiable {
    let id = UUID()
    let url: URL
}

struct WebViewScreen: View {
    let server: Server
    let onSwitchServer: () -> Void

    @State private var reloadToken = 0
    @State private var loadError: String?
    @State private var shareItem: ShareItem?

    var body: some View {
        ZStack {
            Color.wtBackgroundDeep.ignoresSafeArea()

            if let url = server.webURL {
                WebViewContainer(
                    url: url,
                    reloadToken: reloadToken,
                    onSwitchServer: onSwitchServer,
                    onDownload: { shareItem = ShareItem(url: $0) },
                    onLoaded: { loadError = nil },
                    onError: { loadError = $0 }
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                errorOverlay(message: "服务器地址无效，请返回服务器列表重新添加。")
            }

            if let loadError, server.webURL != nil {
                errorOverlay(message: loadError)
            }
        }
        .sheet(item: $shareItem) { item in
            ShareSheet(items: [item.url])
        }
        .preferredColorScheme(.dark)
    }

    private func errorOverlay(message: String) -> some View {
        ZStack {
            Color.wtBackgroundDeep.opacity(0.97).ignoresSafeArea()

            VStack(spacing: 16) {
                WTLogo(size: 56)
                Text("无法连接服务器")
                    .font(.system(size: 20, weight: .bold))
                    .foregroundStyle(Color.wtText)
                Text(message)
                    .font(.system(size: 13))
                    .foregroundStyle(Color.wtTextDim)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)

                Button("重试") {
                    loadError = nil
                    reloadToken += 1
                }
                .buttonStyle(WTPrimaryButtonStyle())
                .padding(.top, 4)

                Button("返回服务器列表") {
                    onSwitchServer()
                }
                .buttonStyle(WTSecondaryButtonStyle())
            }
            .padding(24)
            .wtCard()
            .padding(24)
        }
    }
}
