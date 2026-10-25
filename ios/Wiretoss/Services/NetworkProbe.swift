import Foundation

enum NetworkProbe {
    /// 连 WebView 之前先确认地址上真的跑着 Wiretoss。
    /// 直接让 WKWebView 去加载的话，失败时用户只能看到一个空白的错误页。
    static func check(_ url: URL) async throws {
        var request = URLRequest(url: url)
        request.timeoutInterval = 5
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.setValue("WiretossMobile", forHTTPHeaderField: "X-Wiretoss-Client")

        let (_, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw ProbeError.notHTTP
        }
        guard (200..<400).contains(http.statusCode) else {
            throw ProbeError.httpStatus(http.statusCode)
        }
    }
}

enum ProbeError: LocalizedError {
    case notHTTP
    case httpStatus(Int)

    var errorDescription: String? {
        switch self {
        case .notHTTP:
            return "这个地址没有返回网页。确认地址和端口是否正确。"
        case .httpStatus(let status):
            return "服务器返回 HTTP \(status)。确认电脑上的 Wiretoss 正在运行。"
        }
    }
}

extension Error {
    /// 把 URLSession 的底层错误翻译成给用户看的中文。
    var wiretossMessage: String {
        if let probe = self as? ProbeError {
            return probe.localizedDescription
        }
        if let urlError = self as? URLError {
            switch urlError.code {
            case .cannotConnectToHost, .cannotFindHost:
                return "连不上这台设备。确认电脑上的 Wiretoss 已启动，并且两台设备在同一个局域网。"
            case .timedOut:
                return "连接超时。检查 IP 地址、端口和防火墙设置。"
            case .networkConnectionLost, .notConnectedToInternet:
                return "网络连接不可用。确认手机连的是和电脑同一个 Wi-Fi。"
            case .appTransportSecurityRequiresSecureConnection:
                return "系统拦截了局域网 HTTP 连接。请更新到最新版本后再试。"
            default:
                return urlError.localizedDescription
            }
        }
        return localizedDescription
    }
}
