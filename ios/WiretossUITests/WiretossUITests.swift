import XCTest
import UIKit

final class WiretossUITests: XCTestCase {
    /// 连接本机的 Wiretoss 服务，必要时在 WKWebView 里登录。
    /// 跑之前需要在本机 8080 端口启动一个 test/test 账号的 headless 服务。
    private func connectAndLogin(_ app: XCUIApplication) -> XCUIElement {
        app.launch()

        let address = app.textFields.firstMatch
        XCTAssertTrue(address.waitForExistence(timeout: 8), "设置页的地址输入框没有出现")
        address.tap()
        address.typeText("127.0.0.1:8080")
        app.buttons["连接"].tap()

        let webView = app.webViews.firstMatch
        XCTAssertTrue(webView.waitForExistence(timeout: 20), "WebView 没有加载")

        let username = webView.textFields.firstMatch
        if username.waitForExistence(timeout: 3) {
            username.tap()
            username.typeText("test")

            let password = webView.secureTextFields.firstMatch
            XCTAssertTrue(password.exists)
            password.tap()
            password.typeText("test\n")
        }

        // 文件列表加载后会出现行内的「下载」按钮。
        let download = webView.buttons["下载"].firstMatch
        XCTAssertTrue(download.waitForExistence(timeout: 20), "登录后文件列表没有加载")
        return webView
    }

    func testConnectLoginAndReturnToServers() throws {
        let app = XCUIApplication()
        let webView = connectAndLogin(app)

        let serversButton = webView.buttons["切换服务器"]
        XCTAssertTrue(
            serversButton.waitForExistence(timeout: 10),
            "登录后没有看到原生的切换服务器按钮"
        )
        serversButton.tap()

        XCTAssertTrue(
            app.textFields.firstMatch.waitForExistence(timeout: 8),
            "点切换服务器后没有回到原生设置页"
        )
    }

    /// iPad 使用左右双栏设置页；这条测试在 iPhone 上自动跳过。
    func testTabletLayoutShowsHero() throws {
        try XCTSkipUnless(UIDevice.current.userInterfaceIdiom == .pad, "仅在 iPad 上运行")
        let app = XCUIApplication()
        app.launch()
        XCTAssertTrue(
            app.staticTexts["电脑上的文件，平板随手拿。"].waitForExistence(timeout: 8),
            "iPad 上没有出现双栏设置页的左侧说明"
        )
    }

    func testDownloadOpensShareSheet() throws {
        let app = XCUIApplication()
        let webView = connectAndLogin(app)

        let download = webView.buttons["下载"].firstMatch
        XCTAssertTrue(download.waitForExistence(timeout: 10))
        download.tap()

        let shareSheet = app.otherElements["ActivityListView"]
        XCTAssertTrue(
            shareSheet.waitForExistence(timeout: 20),
            "下载文件后没有弹出系统分享面板"
        )
    }
}
