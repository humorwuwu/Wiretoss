# AGENTS.md — Wiretoss

> 工作区总规约在上一级 [`AGENTS.md`](../AGENTS.md)（本机 `ongoing/`；仓库单独克隆时没有它，以本文件为准）。新建同类目录前先读总规约。总规约与本文件冲突时，以本文件更具体的那条为准，但不得放松总规约的红线。


局域网传文件的 Rust 应用，外加 iOS / Android 原生伴侣客户端。远端 `github.com/humorwuwu/Wiretoss`。

## 目录（不要发明新名字）

| 路径 | 角色 | 阶段结束后 |
|---|---|---|
| `src/`、`tests/`、`assets/`、`packaging/`、`scripts/` | 源码 | 保留 |
| `ios/` | iOS 原生客户端。`project.yml` 是 XcodeGen 源，`Wiretoss.xcodeproj` 由它生成，可一起提交 | 保留 |
| `android/` | Android 原生客户端（Gradle + Kotlin + Compose） | 保留 |
| `docs/` | 上架说明、隐私政策 | 保留 |
| `release/macos/` | 可运行的 `Wiretoss.app` 和 `wiretoss-v<version>-macos-universal.zip` | 保留 |
| `release/windows/` | `wiretoss-v<version>-windows-x64.zip` | 保留 |
| `release/ios/` | `wiretoss-v<version>-ios-arm64.ipa`（App Store 分发签名，由 `scripts/ios-export.sh` 生成） | 保留 |
| `release/android/` | `wiretoss-v<version>-android-universal.apk` 和 `.aab` | 保留 |
| `target/` | Cargo 中间产物，含 `target/<triple>/release/`。这个 `release` 是 Cargo 的优化构建，不是发行目录 | **删除**（`cargo clean` 或 `scripts/clean.sh`） |
| `ios/build/` | Xcode 归档、模拟器 DerivedData、测试结果 | **删除**（`scripts/clean.sh`） |
| `android/.gradle/`、`android/build/`、`android/app/build/` | Gradle 中间产物 | **删除**（`scripts/clean.sh`） |
| `android/local.properties` | 本机 SDK 路径（gitignored） | 默认保留；`clean.sh --deep` 删除 |
| `dist/` | 旧路径 | 不要再使用 |

平台目录只允许 `macos`、`windows`、`linux`、`ios`、`android`。文件名保持现有词干：

- `wiretoss-v<version>-<os>-<arch>.<ext>`
- iOS：`wiretoss-v0.2.0-ios-arm64.ipa`
- Android：`wiretoss-v0.2.0-android-universal.apk` / `.aab`

### 移动端签名（红线）

- `packaging/android/wiretoss-upload.jks`
- `packaging/android/keystore.properties`

这两个文件是 Android 上传签名和密码，已加入 `.gitignore`。**不要提交、不要推到公开仓库、不要清理**。清理前如果看起来像要动 `packaging/android/`，先停下来问用户。请提醒用户备份；丢失后同包名更新会很麻烦。

iOS 的分发签名不存放在仓库里，由 Xcode 的账号和钥匙串管理。`scripts/ios-export.sh` 只接受 `WIRETOSS_TEAM_ID` 环境变量，不保存 Apple ID 密码或 2FA。

### HTTPS 证书（红线）

服务端启用 HTTPS 后会在配置目录下生成 `certs/`：

- macOS：`~/Library/Application Support/Wiretoss/certs/`
- Windows：`%APPDATA%\Wiretoss\certs\`

其中 `ca-key.pem`、`server-key.pem` 是私钥，`ca.pem` 是分发给客户端的 CA。不要提交、
同步到公开位置，也不要在阶段清理时删除 `certs/`。`scripts/clean.sh` 不碰仓库外的配置目录。

`.github/workflows/release.yml` 里的 `dist/` 是 Actions 把各平台 artifact 下载到 runner 的目录，然后 `files: dist/*` 上传到 GitHub Release。那不是本仓库的发行目录。不要改 workflow 的 `dist/` 来「和本地统一」，除非连下载步骤一起改。

## 阶段清理

打包并确认 `release/` 里的产物能用之后，立刻：

```sh
bash scripts/clean.sh
```

它删 `target/`、`dist/`、`ios/build/`、`android/.gradle/`、`android/build/`、`android/app/build/`，保留 `release/`、`packaging/android/` 签名文件和源码。

## 命令

### 桌面 / 服务

```sh
cargo test --all-targets
cargo build --release          # 本机调试二进制在 target/release/，用完即清
./scripts/package.sh           # -> release/macos 与 release/windows
```

### iOS

```sh
# 需要 xcodegen（brew install xcodegen）
cd ios && xcodegen generate

# 模拟器构建
xcodebuild -project Wiretoss.xcodeproj -scheme Wiretoss \
  -configuration Debug -sdk iphonesimulator \
  -destination 'platform=iOS Simulator,name=iPhone 17' \
  -derivedDataPath build/DerivedData CODE_SIGNING_ALLOWED=NO build

# 端到端测试（先在本机 8080 启动 test/test 的 headless 服务）
xcodebuild test -project Wiretoss.xcodeproj -scheme Wiretoss \
  -destination 'platform=iOS Simulator,name=iPhone 17' \
  -derivedDataPath build/DerivedData CODE_SIGNING_ALLOWED=NO

# 生成 IPA（需要 Xcode 已登录付费开发者账号）
WIRETOSS_TEAM_ID=<TeamID> ./scripts/ios-export.sh
```

### Android

```sh
# 需要 JDK 17+；脚本默认用 Homebrew openjdk@21
# android/local.properties 里要有 sdk.dir
cd android && ./gradlew :app:assembleDebug
./scripts/android-release.sh   # 已签名 APK + AAB -> release/android
```

Android 平板（≥720dp）和 iPad（regular size class）使用双栏设置页；窄屏使用单栏。Web UI 自身的响应式布局负责文件页面的平板适配。

上架步骤、签名备份、审核备注和隐私政策托管见 [`docs/mobile-store.md`](docs/mobile-store.md) 与 [`docs/privacy-policy.md`](docs/privacy-policy.md)。
