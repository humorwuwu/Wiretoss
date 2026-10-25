# 移动端上架说明

Wiretoss 的 iOS / Android 客户端是**电脑端 Wiretoss 的伴侣应用**：手机或平板通过局域网连接电脑上运行的服务，复用已有的 Web 文件界面，并用原生能力补上扫码、自动发现、服务器收藏和系统下载/分享。

| 项目 | iOS | Android |
|---|---|---|
| 源码 | `ios/` | `android/` |
| Bundle / Package ID | `dev.wiretoss.mobile` | `dev.wiretoss.mobile` |
| 最低系统 | iOS 17 | Android 8.0（API 26），targetSdk 35 |
| 版本 | 0.2.0（build 2） | 0.2.0（versionCode 2） |
| 产物目录 | `release/ios/` | `release/android/` |

两端都需要电脑上先启动 Wiretoss 服务；应用本身不收集、不上传任何账号或文件数据。

## Android

### 已生成的产物

- `release/android/wiretoss-v0.2.0-android-universal.apk`：可直接安装/侧载
- `release/android/wiretoss-v0.2.0-android-universal.aab`：Google Play 上传用
- `release/android/SHA256SUMS`

签名使用本地生成的 upload key：

- keystore：`packaging/android/wiretoss-upload.jks`
- 密码和别名：`packaging/android/keystore.properties`

这两个文件已被 `.gitignore` 排除，**不要提交、不要删除**。请立即把它们备份到安全位置（密码管理器、离线加密盘）。丢失 keystore 后，用同一个包名发布更新会非常麻烦。

### 重新打包

```bash
# 需要 JDK 17+；脚本默认用 Homebrew openjdk@21
./scripts/android-release.sh
```

脚本会执行 `assembleRelease` + `bundleRelease`，把 APK/AAB 复制到 `release/android/` 并重新生成 SHA256SUMS。不会安装到任何设备。

### Google Play 上架

1. 在 Play Console 创建应用，包名填 `dev.wiretoss.mobile`。
2. 上传 `.aab`，首次上传时启用 **Play App Signing**；上传密钥（上面的 keystore）由你自己保管。
3. 商店资料：
   - 应用名称：Wiretoss
   - 类别：工具
   - 隐私政策 URL：必须是一个公开可访问的页面，可用 `docs/privacy-policy.md` 的内容（见文末说明）
   - 截图：至少 2 张手机截图；因为适配了平板，建议再补 7 英寸和 10 英寸平板截图
   - Feature graphic：可用 `packaging/android/play-icon-512.png`（512×512，商店图标）和另外制作的 1024×500 横幅
4. Data safety：本应用不收集、不共享数据；不包含广告或分析 SDK。服务器地址仅保存在本机 SharedPreferences 中。文件只在用户自己的局域网设备之间传输。
5. 权限说明：
   - `INTERNET` / `ACCESS_NETWORK_STATE`：连接电脑上的服务
   - `CAMERA`：扫描电脑屏幕上的二维码
   - `CHANGE_WIFI_MULTICAST_STATE`：mDNS 自动发现
   - Android 10 以下：`WRITE_EXTERNAL_STORAGE`，仅用于把下载文件写入公共 Downloads
6. 应用默认使用明文 HTTP 连接局域网；服务端可选启用 HTTPS，但手机原生壳继续走 HTTP，不需要安装证书。在 Play 的审核备注里说明局域网 HTTP 是产品的核心场景。
7. targetSdk 35。若 Play 要求更高版本，把 `android/app/build.gradle.kts` 里的 `compileSdk` / `targetSdk` 升到对应版本，并在本机安装对应 SDK platform 后重新打包。

## iOS

### 当前状态

- Xcode 工程：`ios/Wiretoss.xcodeproj`（由 `ios/project.yml` 通过 XcodeGen 生成）
- 模拟器端到端测试已通过：连接本机服务 → Web 登录 → 注入“切换服务器”按钮 → 下载弹出系统分享面板
- iPad 使用双栏布局（左品牌说明、右连接表单）
- **App Store 分发包已生成**：`release/ios/wiretoss-v0.2.0-ios-arm64.ipa`
  - 使用 Apple Distribution 证书和 iOS Team Store Provisioning Profile 签名
  - 描述文件 `get-task-allow=false`，`beta-reports-active=true`，属于 App Store 分发型
  - 剩余步骤是在 App Store Connect 创建应用记录、上传 IPA 并提交审核（需要账号本人操作）

### 重新生成 / 上传

1. 确认 Xcode → Settings → Accounts 已登录 Apple Developer 账号（本次已经登录并成功导出）。
2. 打开 `ios/Wiretoss.xcodeproj`，选中 Wiretoss target → Signing & Capabilities → Team 选择你的团队。Bundle Identifier 保持 `dev.wiretoss.mobile`。
3. 代码改动后在模拟器跑一遍测试，再执行下面的命令重新出包。
4. 上传：Xcode → Window → Organizer，选中 archive → Distribute App → App Store Connect → Upload；或下载 Transporter App 直接上传 `release/ios/wiretoss-v0.2.0-ios-arm64.ipa`。上传前先在 App Store Connect 创建 bundle id 为 `dev.wiretoss.mobile` 的应用记录。
5. 命令行导出：

```bash
# 开发签名 IPA（注册过的设备可安装）
WIRETOSS_TEAM_ID=你的TeamID ./scripts/ios-export.sh

# App Store 分发包（需要分发证书；Xcode 会尝试自动创建）
WIRETOSS_TEAM_ID=你的TeamID WIRETOSS_EXPORT_METHOD=app-store-connect ./scripts/ios-export.sh
```

脚本会把 IPA 放到 `release/ios/` 并生成 SHA256SUMS。不要把 Apple ID 密码或 2FA 交给脚本；分发和上传必须账号本人完成。

### App Store Connect 资料

- 名称：Wiretoss
- 类别：工具
- 隐私政策 URL：见 `docs/privacy-policy.md`
- 截图：iPhone 6.9" / 6.5"；因为支持 iPad，还需要 13" iPad 截图
- 审核备注（建议逐条写清）：
  1. 这是电脑端 Wiretoss 的伴侣应用，需要先在同一局域网电脑上运行 Wiretoss（开源，仓库地址）。
  2. 审核时没有现成服务器，附件提供一段 30–60 秒演示视频：电脑启动服务 → 手机扫码/自动发现 → 登录 → 上传/下载。
  3. 相机仅用于扫描二维码；本地网络权限用于发现和连接电脑；应用不收集数据。
  4. App Transport Security 使用 `NSAllowsLocalNetworking`，因为服务端是局域网 HTTP。
- App Store 4.2「最小功能」是最主要的风险：纯 WebView 壳容易被拒。本应用已加入原生二维码扫描、Bonjour 自动发现、服务器收藏、原生下载/分享、iPad 双栏布局。若仍被拒，建议补一个内置演示模式或更深的原生文件管理功能，而不是继续加更多 WebView 页面。
- 隐私清单：`ios/Wiretoss/Resources/PrivacyInfo.xcprivacy`，声明无追踪、无数据收集，UserDefaults 使用原因为 CA92.1。

### 重新生成 Xcode 工程

```bash
cd ios && xcodegen generate
```

修改 `project.yml` 或增删 Swift 文件后需要重新生成。`Wiretoss.xcodeproj` 可以一起提交，方便没有 XcodeGen 的人直接打开。

## 测试清单

iOS（模拟器，已有自动化测试）：

```bash
# 先在 8080 启动一个 test/test 账号的 headless 服务
cd ios
xcodebuild test -project Wiretoss.xcodeproj -scheme Wiretoss \
  -destination 'platform=iOS Simulator,name=iPhone 17' \
  -derivedDataPath build/DerivedData CODE_SIGNING_ALLOWED=NO
```

Android（当前按要求未安装到设备/模拟器，仅完成编译和签名）：

1. 安装 `release/android/wiretoss-v0.2.0-android-universal.apk`
2. 启动电脑端服务，确认自动发现或手动输入 `IP:8080`
3. 扫码连接、登录、上传/下载、系统分享
4. 平板横竖屏确认双栏布局；窄屏确认单栏
5. 断开电脑端服务，确认原生错误页的重试/返回按钮

## 版本同步

发布新版本时需要同时改：

- `Cargo.toml` 的 `version`
- `ios/project.yml` 的 `MARKETING_VERSION` 和 `CURRENT_PROJECT_VERSION`
- `android/app/build.gradle.kts` 的 `versionName` 和 `versionCode`
- `CHANGELOG.md`

## 隐私政策托管

App Store 和 Google Play 都要求一个公开的隐私政策 URL。`docs/privacy-policy.md` 是模板，请填写生效日期和联系方式，然后托管到你的网站或 GitHub Pages，并把 URL 填进两个商店后台。
