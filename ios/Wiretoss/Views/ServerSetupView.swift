import SwiftUI

struct ServerSetupView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.horizontalSizeClass) private var horizontalSizeClass
    @Environment(\.verticalSizeClass) private var verticalSizeClass
    @StateObject private var discovery = BonjourBrowser()

    @State private var address = ""
    @State private var isConnecting = false
    @State private var errorMessage: String?
    @State private var showScanner = false
    @FocusState private var addressFocused: Bool

    var body: some View {
        ZStack {
            WTGradientBackground()

            if horizontalSizeClass == .regular && verticalSizeClass == .regular {
                GeometryReader { proxy in
                    let panelHeight = min(600, proxy.size.height - 96)
                    let heroWidth = min(360, proxy.size.width * 0.34)
                    let contentWidth = min(560, proxy.size.width * 0.50)

                    HStack(alignment: .top, spacing: 36) {
                        tabletHero
                            .frame(width: heroWidth, alignment: .leading)
                            .padding(.top, 26)

                        ScrollView(showsIndicators: false) {
                            content
                                .frame(width: contentWidth)
                                .padding(.vertical, 26)
                        }
                        .frame(width: contentWidth, height: panelHeight, alignment: .top)
                        .scrollDismissesKeyboard(.interactively)
                    }
                    .frame(maxWidth: 1040, maxHeight: panelHeight, alignment: .top)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .padding(.horizontal, 40)
                }
            } else {
                ScrollView(showsIndicators: false) {
                    VStack(spacing: 18) {
                        header
                        content
                    }
                    .padding(.horizontal, 20)
                    .padding(.top, 26)
                    .padding(.bottom, 36)
                    .frame(maxWidth: 560)
                    .frame(maxWidth: .infinity)
                }
                .scrollDismissesKeyboard(.interactively)
            }
        }
        .preferredColorScheme(.dark)
        .onAppear { discovery.start() }
        .onDisappear { discovery.stop() }
        .fullScreenCover(isPresented: $showScanner) {
            ScannerSheet { code in
                guard let url = ServerAddress.normalize(code) else {
                    errorMessage = "二维码里不是一个有效的 Wiretoss 地址。"
                    return
                }
                address = url.absoluteString
                connect(url: url)
            }
        }
    }

    private var content: some View {
        VStack(spacing: 18) {
            connectCard
            if let errorMessage {
                errorBanner(errorMessage)
            }
            discoveryCard
            if !model.savedServers.isEmpty {
                savedCard
            }
            footer
        }
    }

    private var tabletHero: some View {
        VStack(alignment: .leading, spacing: 18) {
            WTLogo(size: 92)
            Text("Wiretoss")
                .font(.system(size: 40, weight: .bold, design: .rounded))
                .foregroundStyle(Color.wtText)
            Text("电脑上的文件，平板随手拿。")
                .font(.system(size: 17))
                .foregroundStyle(Color.wtTextDim)

            VStack(alignment: .leading, spacing: 14) {
                heroBullet("qrcode.viewfinder", "扫描电脑上的二维码即可连接")
                heroBullet("wifi", "自动发现同一局域网里的设备")
                heroBullet("arrow.down.circle", "下载交给系统，支持分享和保存")
            }
            .padding(.top, 4)
        }
    }

    private func heroBullet(_ icon: String, _ text: String) -> some View {
        HStack(spacing: 13) {
            Image(systemName: icon)
                .font(.system(size: 18, weight: .medium))
                .foregroundStyle(Color.wtAccent)
                .frame(width: 38, height: 38)
                .background(
                    Color.wtAccent.opacity(0.12),
                    in: RoundedRectangle(cornerRadius: 12, style: .continuous)
                )
            Text(text)
                .font(.system(size: 13.5))
                .foregroundStyle(Color.wtTextDim)
        }
    }

    private var header: some View {
        VStack(spacing: 14) {
            WTLogo(size: 68)

            VStack(spacing: 5) {
                Text("Wiretoss")
                    .font(.system(size: 32, weight: .bold, design: .rounded))
                    .foregroundStyle(Color.wtText)
                Text("连接电脑上的局域网文件快传")
                    .font(.system(size: 13.5))
                    .foregroundStyle(Color.wtTextDim)
            }
        }
        .padding(.top, 8)
        .padding(.bottom, 4)
    }

    private var connectCard: some View {
        VStack(alignment: .leading, spacing: 14) {
            sectionTitle("连接到电脑", icon: "desktopcomputer")

            WTAddressField(
                placeholder: "192.168.1.5:8080",
                text: $address,
                focused: $addressFocused
            )
            .onSubmit { connectCurrent() }

            Button {
                connectCurrent()
            } label: {
                HStack(spacing: 8) {
                    if isConnecting {
                        ProgressView().tint(.white)
                    } else {
                        Image(systemName: "arrow.right")
                    }
                    Text(isConnecting ? "正在连接…" : "连接")
                }
            }
            .buttonStyle(WTPrimaryButtonStyle())
            .disabled(isConnecting)

            Button {
                showScanner = true
            } label: {
                Label("扫描电脑上的二维码", systemImage: "qrcode.viewfinder")
            }
            .buttonStyle(WTSecondaryButtonStyle())

            Text("手机和电脑需要在同一个 Wi-Fi 或局域网。密码在电脑端 Wiretoss 启动时设置。")
                .font(.system(size: 11.5))
                .foregroundStyle(Color.wtTextFaint)
                .fixedSize(horizontal: false, vertical: true)
        }
        .wtCard()
    }

    private var discoveryCard: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                sectionTitle("自动发现", icon: "wifi")
                Spacer()
                if discovery.isSearching {
                    ProgressView()
                        .controlSize(.small)
                        .tint(Color.wtAccent)
                }
                Button {
                    discovery.stop()
                    discovery.start()
                } label: {
                    Image(systemName: "arrow.clockwise")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(Color.wtTextDim)
                        .frame(width: 30, height: 30)
                        .background(Color.white.opacity(0.05), in: Circle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("重新搜索")
            }

            if let message = discovery.errorMessage {
                Text(message)
                    .font(.system(size: 12))
                    .foregroundStyle(Color.wtDanger)
            }

            if discovery.servers.isEmpty {
                Text(
                    discovery.isSearching
                        ? "正在搜索同一局域网里的 Wiretoss…"
                        : "没有找到设备。确认电脑端的服务已经启动。"
                )
                .font(.system(size: 12.5))
                .foregroundStyle(Color.wtTextDim)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.vertical, 4)
            } else {
                VStack(spacing: 8) {
                    ForEach(discovery.servers) { server in
                        Button {
                            connect(urlString: server.url)
                        } label: {
                            rowContent(name: server.name, url: server.url)
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
        .wtCard()
    }

    private var savedCard: some View {
        VStack(alignment: .leading, spacing: 12) {
            sectionTitle("常用服务器", icon: "clock.arrow.circlepath")

            VStack(spacing: 8) {
                ForEach(model.savedServers) { server in
                    HStack(spacing: 8) {
                        Button {
                            connect(urlString: server.url)
                        } label: {
                            rowContent(name: server.name, url: server.url)
                        }
                        .buttonStyle(.plain)

                        Menu {
                            Button(role: .destructive) {
                                model.delete(server)
                            } label: {
                                Label("删除", systemImage: "trash")
                            }
                        } label: {
                            Image(systemName: "ellipsis")
                                .font(.system(size: 15, weight: .semibold))
                                .foregroundStyle(Color.wtTextDim)
                                .frame(width: 30, height: 36)
                        }
                    }
                }
            }
        }
        .wtCard()
    }

    private var footer: some View {
        Text("数据只在局域网内传输 · 不经过云端")
            .font(.system(size: 11))
            .foregroundStyle(Color.wtTextFaint)
            .padding(.top, 4)
    }

    private func sectionTitle(_ title: String, icon: String) -> some View {
        HStack(spacing: 8) {
            Image(systemName: icon)
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(Color.wtAccent)
            Text(title)
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(Color.wtText)
        }
    }

    private func rowContent(name: String, url: String) -> some View {
        HStack(spacing: 12) {
            Image(systemName: "desktopcomputer")
                .font(.system(size: 15, weight: .medium))
                .foregroundStyle(Color.wtAccent)
                .frame(width: 34, height: 34)
                .background(Color.wtAccent.opacity(0.13), in: RoundedRectangle(cornerRadius: 10, style: .continuous))

            VStack(alignment: .leading, spacing: 2) {
                Text(name)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundStyle(Color.wtText)
                    .lineLimit(1)
                Text(url)
                    .font(.system(size: 11.5, design: .monospaced))
                    .foregroundStyle(Color.wtTextDim)
                    .lineLimit(1)
            }

            Spacer(minLength: 6)

            Image(systemName: "chevron.right")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(Color.wtTextFaint)
        }
        .padding(11)
        .background(Color.white.opacity(0.045), in: RoundedRectangle(cornerRadius: 13, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 13, style: .continuous)
                .stroke(Color.white.opacity(0.07), lineWidth: 1)
        }
    }

    private func errorBanner(_ message: String) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(Color.wtDanger)
                .padding(.top, 1)
            Text(message)
                .font(.system(size: 12.5))
                .foregroundStyle(Color(red: 254 / 255, green: 205 / 255, blue: 211 / 255))
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.wtDanger.opacity(0.12), in: RoundedRectangle(cornerRadius: 13, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 13, style: .continuous)
                .stroke(Color.wtDanger.opacity(0.28), lineWidth: 1)
        }
    }

    private func connectCurrent() {
        guard let url = ServerAddress.normalize(address) else {
            errorMessage = "请输入有效的地址，例如 192.168.1.5:8080"
            return
        }
        connect(url: url)
    }

    private func connect(urlString: String) {
        guard let url = URL(string: urlString) else {
            errorMessage = "服务器地址无效。"
            return
        }
        address = url.absoluteString
        connect(url: url)
    }

    private func connect(url: URL) {
        guard !isConnecting else { return }
        addressFocused = false
        isConnecting = true
        errorMessage = nil

        Task { @MainActor in
            do {
                try await NetworkProbe.check(url)
                model.connect(url: url)
                isConnecting = false
            } catch {
                errorMessage = error.wiretossMessage
                isConnecting = false
            }
        }
    }
}
