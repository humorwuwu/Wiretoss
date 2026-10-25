import SwiftUI
import WebKit

/// 把现有 Wiretoss Web UI 装进 WKWebView，并补上原生能力：
/// - 顶部注入「切换服务器」按钮（通过 WKScriptMessageHandler 回到服务器列表）
/// - 拦截 /api/download 与 /api/zip 的下载，交给系统分享/存储
/// - 主框架加载失败时回调给 SwiftUI 显示原生错误页
struct WebViewContainer: UIViewRepresentable {
    let url: URL
    let reloadToken: Int
    let onSwitchServer: () -> Void
    let onDownload: (URL) -> Void
    let onLoaded: () -> Void
    let onError: (String) -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(parent: self)
    }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = true
        configuration.websiteDataStore = .default()
        configuration.userContentController.add(context.coordinator, name: "wiretoss")
        configuration.userContentController.addUserScript(
            WKUserScript(
                source: Self.bridgeScript,
                injectionTime: .atDocumentEnd,
                forMainFrameOnly: true
            )
        )

        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.navigationDelegate = context.coordinator
        webView.uiDelegate = context.coordinator
        webView.allowsBackForwardNavigationGestures = true
        webView.isOpaque = false
        webView.backgroundColor = .clear
        webView.scrollView.backgroundColor = .clear
        #if DEBUG
        if #available(iOS 16.4, *) {
            webView.isInspectable = true
        }
        #endif
        context.coordinator.load(url, in: webView)
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        context.coordinator.parent = self
        if context.coordinator.lastReloadToken != reloadToken {
            context.coordinator.load(url, in: webView)
        }
    }

    static func dismantleUIView(_ webView: WKWebView, coordinator: Coordinator) {
        webView.configuration.userContentController.removeScriptMessageHandler(forName: "wiretoss")
        webView.stopLoading()
    }

    private static let bridgeScript = #"""
    (function () {
      var bridge = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.wiretoss;
      if (!bridge || window.__wiretossNativeInstalled) return;
      window.__wiretossNativeInstalled = true;

      document.documentElement.classList.add('in-wiretoss-app');

      var style = document.createElement('style');
      style.textContent = [
        '.in-wiretoss-app #upload-folder{display:none!important}',
        '.wiretoss-native-servers-float{position:fixed;top:calc(env(safe-area-inset-top) + 12px);right:12px;z-index:90;background:rgba(21,27,46,.84);border:1px solid rgba(255,255,255,.12);-webkit-backdrop-filter:blur(12px);backdrop-filter:blur(12px)}'
      ].join('');
      document.head.appendChild(style);

      function makeButton() {
        var button = document.createElement('button');
        button.type = 'button';
        button.className = 'icon-btn wiretoss-native-servers-inline';
        button.title = '切换服务器';
        button.setAttribute('aria-label', '切换服务器');
        button.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 6h16M4 12h16M4 18h16"/></svg>';
        button.addEventListener('click', function () {
          bridge.postMessage({ action: 'servers' });
        });
        return button;
      }

      function place() {
        var app = document.getElementById('app-view');
        var topbar = app && !app.hidden ? document.querySelector('.topbar-right') : null;
        var floating = document.getElementById('wiretoss-native-servers');

        if (topbar) {
          if (floating) floating.remove();
          if (!topbar.querySelector('.wiretoss-native-servers-inline')) {
            topbar.insertBefore(makeButton(), topbar.firstChild);
          }
          return;
        }

        var login = document.getElementById('login-view');
        if (login && !login.hidden && !floating) {
          var button = makeButton();
          button.id = 'wiretoss-native-servers';
          button.classList.remove('wiretoss-native-servers-inline');
          button.classList.add('wiretoss-native-servers-float');
          document.body.appendChild(button);
        }
      }

      place();
      new MutationObserver(place).observe(document.body, {
        childList: true,
        subtree: true,
        attributes: true,
        attributeFilter: ['hidden', 'class']
      });
    })();
    """#

    final class Coordinator: NSObject, WKNavigationDelegate, WKUIDelegate,
        WKScriptMessageHandler, WKDownloadDelegate
    {
        var parent: WebViewContainer
        var lastReloadToken: Int
        private var downloads: [ObjectIdentifier: URL] = [:]

        init(parent: WebViewContainer) {
            self.parent = parent
            self.lastReloadToken = parent.reloadToken
            super.init()
            Self.cleanupDownloads()
        }

        func load(_ url: URL, in webView: WKWebView) {
            lastReloadToken = parent.reloadToken
            webView.load(URLRequest(url: url))
        }

        // MARK: - WKScriptMessageHandler

        func userContentController(
            _ userContentController: WKUserContentController,
            didReceive message: WKScriptMessage
        ) {
            guard message.name == "wiretoss",
                  let body = message.body as? [String: Any],
                  body["action"] as? String == "servers"
            else {
                return
            }
            DispatchQueue.main.async { self.parent.onSwitchServer() }
        }

        // MARK: - WKNavigationDelegate

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            DispatchQueue.main.async { self.parent.onLoaded() }
        }

        func webView(
            _ webView: WKWebView,
            didFailProvisionalNavigation navigation: WKNavigation!,
            withError error: Error
        ) {
            report(error)
        }

        func webView(
            _ webView: WKWebView,
            didFail navigation: WKNavigation!,
            withError error: Error
        ) {
            report(error)
        }

        func webView(
            _ webView: WKWebView,
            decidePolicyFor navigationResponse: WKNavigationResponse,
            decisionHandler: @escaping (WKNavigationResponsePolicy) -> Void
        ) {
            if let url = navigationResponse.response.url, isDownload(url) {
                decisionHandler(.download)
            } else {
                decisionHandler(.allow)
            }
        }

        func webView(
            _ webView: WKWebView,
            navigationResponse: WKNavigationResponse,
            didBecome download: WKDownload
        ) {
            download.delegate = self
        }

        func webView(
            _ webView: WKWebView,
            navigationAction: WKNavigationAction,
            didBecome download: WKDownload
        ) {
            download.delegate = self
        }

        private func isDownload(_ url: URL) -> Bool {
            url.path.hasPrefix("/api/download") || url.path.hasPrefix("/api/zip/")
        }

        private func report(_ error: Error) {
            let nsError = error as NSError
            if nsError.domain == NSURLErrorDomain && nsError.code == NSURLErrorCancelled {
                return
            }
            DispatchQueue.main.async { self.parent.onError(error.wiretossMessage) }
        }

        // MARK: - WKDownloadDelegate

        func download(
            _ download: WKDownload,
            decideDestinationUsing response: URLResponse,
            suggestedFilename: String,
            completionHandler: @escaping (URL?) -> Void
        ) {
            let directory = Self.downloadsDirectory()
            try? FileManager.default.createDirectory(
                at: directory,
                withIntermediateDirectories: true
            )
            let safeName = suggestedFilename
                .replacingOccurrences(of: "/", with: "_")
                .replacingOccurrences(of: ":", with: "_")
            let name = safeName.isEmpty ? "wiretoss-download" : safeName
            let destination = directory.appendingPathComponent("\(UUID().uuidString)-\(name)")
            downloads[ObjectIdentifier(download)] = destination
            completionHandler(destination)
        }

        func downloadDidFinish(_ download: WKDownload) {
            guard let url = downloads.removeValue(forKey: ObjectIdentifier(download)) else {
                return
            }
            DispatchQueue.main.async { self.parent.onDownload(url) }
        }

        func download(
            _ download: WKDownload,
            didFailWithError error: Error,
            resumeData: Data?
        ) {
            downloads.removeValue(forKey: ObjectIdentifier(download))
            report(error)
        }

        private static func downloadsDirectory() -> URL {
            FileManager.default.temporaryDirectory
                .appendingPathComponent("WiretossDownloads", isDirectory: true)
        }

        private static func cleanupDownloads() {
            try? FileManager.default.removeItem(at: downloadsDirectory())
        }
    }
}
