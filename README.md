# Wiretoss

[![CI](https://github.com/humorwuwu/Wiretoss/actions/workflows/ci.yml/badge.svg)](https://github.com/humorwuwu/Wiretoss/actions/workflows/ci.yml)

Mac ↔ Windows 局域网文件快传。**任意一端运行一个可执行文件，另一端用浏览器打开就能互传。**

还可以切到**原生客户端模式**，把文件/文件夹拖进服务端窗口，像隔空投送一样直接发给指定设备，
客户端自动存入连接时配置的接收目录。

不需要云中转、不需要配置共享权限，跑满局域网带宽。

```
   Mac                                        Windows
┌──────────────┐      HTTP :8080        ┌──────────────┐
│ wiretoss     │◄──────────────────────►│ 浏览器        │
│ (axum 服务)   │                        │ localhost 或  │
│  ~/Wiretoss  │                        │ 192.168.x.x  │
└──────────────┘                        └──────────────┘
      ▲ 浏览器 localhost，同一套 Web UI
```

---

## 快速开始

### 1. 拿到可执行文件

从 [Releases](../../releases) 下载对应平台的包并解压：

| 平台 | 文件 |
|---|---|
| macOS（Apple Silicon + Intel 通用） | `wiretoss-<版本号>-macos-universal.zip` |
| Windows (x64) | `wiretoss-<版本号>-windows-x64.zip` |

macOS 那个包是 arm64 与 x86_64 用 `lipo` 合成的通用二进制，Apple Silicon 和 Intel
机器下同一个包都能跑，不用挑架构。每个包旁边都附有同名 `.sha256` 校验和文件。

或者自己编译（需要 Rust stable）：

```bash
cargo build --release
# 产物：target/release/wiretoss（Windows 上是 wiretoss.exe）
```

### 2. 打开程序

**macOS**：双击 `Wiretoss.app`。**Windows**：双击 `wiretoss.exe`。

打开后是一个控制面板窗口：

```
┌─ Wiretoss ─────────────────────────────────────────────────────┐
│ [logo] Wiretoss                    ● 运行中 · 端口 8080          │
│        局域网文件快传                                          │
│ [服务端] [客户端]                      对端用浏览器打开即可互传   │
├───────────────────────────────┬────────────────────────────────┤
│ 服务设置                       │ 扫码访问                        │
│ 共享目录 [____________] [选择…] │  ████  http://192.168... [复制] │
│ 用户名   [____________]        │  ████  http://localhost  [复制] │
│ 密码     [____________]        │                                │
│ ▸ 高级设置                     │ 传输活动                        │
│ [停止服务] [打开浏览器]         │  还没有传输记录。               │
├───────────────────────────────┤                                │
│ 在线设备 · 0                   │ 运行日志                        │
└───────────────────────────────┴────────────────────────────────┘
```

窗口默认按左右两栏排布，窄窗口会自动堆叠成一栏。

首次运行需要自己设一个密码（点「随机生成」或自己输入）。设好之后程序会把
密码存成 Argon2 哈希，**之后再打开就不需要重新输入了，会自动开服**。

> 密码只存哈希、不存明文，所以界面上没法把已保存的密码回显出来 ——
> 自己设的时候记牢，或者用个好记的。忘了就在窗口里重新设一个。

### 3. 另一端打开浏览器

把窗口里显示的**对端访问地址**（`http://192.168.1.5:8080` 那个）输入对端浏览器，
或者直接用手机扫窗口里的二维码。用设置的账号密码登录即可。

本机想用界面的话，点窗口里的「打开浏览器」按钮就行 —— 两端是同一套 UI。

### 4. 传文件

- **上传**：把文件/文件夹直接拖进浏览器窗口，或点「上传文件」/「上传文件夹」
- **下载**：点文件名，或多选后点「下载所选」（多选/文件夹会自动打包成 zip）

### 界面与手机 / 平板

浏览器端是一套深色玻璃风格的单页界面。宽屏下左侧是连接信息和快捷操作，右侧是文件列表；
手机浏览器打开时会收起侧栏，把文件表格变成卡片列表，并把「上传 / 上传文件夹 / 新建文件夹」
固定在底部操作栏，同时适配了刘海屏安全区和触控尺寸。

页面带 `manifest.webmanifest` 和主屏图标，可以像 App 一样添加到主屏幕：

- **iOS Safari**：分享 → 添加到主屏幕。之后从主屏打开会全屏运行，没有浏览器地址栏。
- **Android Chrome**：菜单 → 添加到主屏幕，生成一个快捷方式。

> 默认是明文 HTTP，Chrome 不会弹出「安装应用」的完整 PWA 提示（安装能力要求 HTTPS）。
> 如果启用了 HTTPS 并在设备上信任了 CA，PWA 安装条件会更好，但仍以浏览器实际提示为准。
> 主屏幕快捷方式仍然可以正常使用；需要完整安装体验的话，可以用下面的原生客户端。

#### 原生 iOS / Android 客户端

`ios/` 和 `android/` 是电脑端的**伴侣 App**，不把服务端搬到手机上：

- iOS：SwiftUI + WKWebView；Android：Kotlin + Compose + WebView
- 原生扫码、mDNS 自动发现、常用服务器收藏、系统下载/分享
- iPad 和宽度 ≥720dp 的 Android 平板使用左右双栏设置页；手机使用单栏
- 文件浏览/上传/下载仍复用同一套 Web UI，因此两端功能和服务端保持一致

产物：

- Android：`release/android/wiretoss-<版本>-android-universal.apk`（侧载）和 `.aab`（Play 上传）
- iOS：`release/ios/wiretoss-<版本>-ios-arm64.ipa`（App Store 分发签名，可用 Transporter / Xcode Organizer 上传）

签名、上架步骤、隐私政策和审核备注见 [`docs/mobile-store.md`](docs/mobile-store.md)。

### 关掉窗口会怎样

**只是把窗口藏起来，传输服务继续在后台跑** —— 这样对端随时都能连上来。
窗口会收进菜单栏（macOS）或系统托盘（Windows）：

- 点菜单栏图标 → 把窗口叫回来
- 右键 → 「启动 / 停止服务」「退出 Wiretoss」

想彻底退出就用托盘菜单里的「退出 Wiretoss」。

> 万一托盘图标创建失败（少见），程序会退化成「关闭窗口即退出」，
> 否则窗口藏起来之后你就再也找不回它了。这一点会写在窗口的日志里。

---

## 功能

- **深色玻璃界面** —— 桌面控制面板和浏览器端共用一套配色、卡片和排版，窄窗口自动堆叠
- **原生 iOS / Android 客户端** —— 扫码、mDNS 自动发现、收藏服务器、系统下载与分享
- **手机 / 平板适配** —— 手机响应式卡片和底部操作栏，iPad / Android 平板双栏设置页
- **浏览器移动端** —— 响应式卡片列表、底部操作栏、刘海屏安全区，可添加到主屏幕
- **无账号只读模式** —— 未登录访客可浏览、下载、保存到本机；上传、删除、投送仍需登录
- **保存到本机目录** —— 桌面 Chrome/Edge + HTTPS 下选择本地默认目录，选中服务器文件后
  直接落盘并保留目录结构；同名文件夹默认合并覆盖，删除同名项需二次确认
- **可选 HTTPS** —— 自签本地 CA，HTTP 继续保留给手机；证书和信任步骤见
  [`docs/https-local-save.md`](docs/https-local-save.md)
- **目录浏览** —— 面包屑导航，按名称/大小/时间排序，多选
- **分片断点续传上传** —— 8 MiB 分片、并发上传、刷新页面或断网后自动从断点继续
- **HTTP Range 下载** —— 大文件走浏览器原生下载器，天然支持断点续传，不占服务端内存
- **流式打包下载** —— 多选或整个文件夹边压边发，不落临时文件，内存占用与文件大小无关
- **实时进度** —— 每文件进度条、速度、剩余时间；对端传文件过来时你这边也会实时看到
- **共享目录实时同步** —— 在 Finder / 资源管理器里新增、删除、改名或替换文件，
  已经打开的网页会自动刷新，不需要手动按 F5
- **文件夹上传** —— 拖文件夹即可，目录结构原样保留
- **新建 / 删除** —— 删除是递归且不可撤销的，有二次确认
- **隔空投送式投递** —— 原生客户端连接后，服务端点选设备，把文件/文件夹拖进窗口即可自动送达；
  保留目录结构与空目录，断线重连后补收未完成批次

## 客户端模式（隔空投送式投递）

浏览器无法静默写入用户指定的任意目录，所以“自动存入配置目录”需要运行同一程序的**原生客户端模式**：

1. 服务端窗口启动服务，顶部切到「服务端」；
2. 另一台机器运行 `wiretoss --client`（或打开程序后手动切到「客户端」），填写服务器地址、
   账号密码、设备名称和接收目录，点「连接」；
3. 服务端「在线客户端」列表出现该设备。点击选中（只有一个客户端时自动选中），
   把文件或文件夹拖进服务端窗口任意位置，松开即发送；
4. 客户端自动接收，按原目录结构保存到连接时配置的接收目录，无需逐次确认；
   同名顶层项自动变成 `名称 (1)`，不会覆盖已有文件；
5. 网络中断会自动重连，重连后通过 pending 接口补收断线期间漏掉的批次。

多客户端在线且没有预先选中时，拖入文件后会弹出接收方选择窗口。
设备名默认取主机名，可在客户端表单里修改。

> 投送目标只包含原生客户端模式。浏览器客户端仍可使用原有的上传、下载和打包功能，
> 但不会出现在投送列表里。服务端默认提供 HTTP，可选启用 HTTPS（自签 CA）；
> HTTP 监听始终保留，手机和原生客户端不需要证书。

## 命令行模式（可选）

默认打开图形界面。加 `--headless` 则不开窗口，直接在终端里跑服务 ——
适合开机自启、脚本、CI 这类没有显示环境的场合：

```bash
./wiretoss --headless --user mac --password mysecret --root ~/共享
```

```
wiretoss [OPTIONS]

      --headless         不开图形界面，直接在终端里跑服务
      --client           以客户端模式启动图形界面：连接服务端并自动接收投送
  -p, --port <PORT>      监听端口（默认 8080）
  -b, --bind <ADDR>      监听地址（默认 0.0.0.0，即局域网内都可访问）
  -r, --root <DIR>       共享目录（默认 ~/Wiretoss）
      --user <NAME>      对端登录用的用户名
      --password <PASS>  对端登录用的密码
      --reset-password   重新生成一个随机密码并打印
      --no-mdns          不广播 mDNS 服务
      --no-qr            不打印二维码
  -h, --help             帮助
  -V, --version          版本
```

`--headless` 之外的那些参数只在 headless 模式下生效；窗口里的设置以界面为准。

> 密码输入框里的默认值是随机生成的，用的是剔除了 `0/O`、`1/l/I` 的 32 字符表、
> 长度 8 位。看着短，但配合登录限流（连错 5 次即指数退避）已经远超暴力破解的可行范围，
> 而它足够短，可以从一台机器的终端手敲到另一台 —— 这才是这个工具最高频的用法。

> Windows 上 release 版编译成无控制台的 GUI 程序，`--headless` 也不会打印到终端。
> 需要看日志请用图形界面，或改用 debug 构建。

---

## 配置

| 平台 | 位置 |
|---|---|
| macOS | `~/Library/Application Support/Wiretoss/config.json` |
| Windows | `%APPDATA%\Wiretoss\config.json` |
| Linux | `~/.config/Wiretoss/config.json` |

文件里存的是 **Argon2id 口令哈希**，不是明文密码，权限收紧到仅本人可读（0600）。
忘了密码就在窗口里重新设一个。

日志写在同一个目录下的 `wiretoss.log`（超过 1 MB 会在下次启动时清空）。
出问题时把这个文件发出来就能定位 —— 窗口里的日志面板只保留最近 500 条。

---

## Windows 使用注意

- **首次运行会弹出 Windows Defender 防火墙授权框，必须勾选「专用网络」并允许**，
  否则对端连不上。误点了「取消」的话，去「Windows 安全中心 → 防火墙和网络保护 →
  允许应用通过防火墙」里把 `wiretoss.exe` 勾上。
- Windows 会限制 260 字符的路径长度。极深的目录嵌套可能失败。
- 从 macOS 传来的文件名里的非法字符（`< > : " | ? *`）和保留设备名
  （`CON`、`NUL`、`COM1`…）会被自动处理成安全的名字，最终文件名以界面显示为准。

## 无账号只读 / HTTPS / 保存到本机目录

服务端可以在不设账号密码的情况下开放**只读浏览**：访客能看目录、下载、打包下载，
也可以把选中的文件保存到自己电脑的默认目录；上传、新建、删除、投送仍然需要登录。
开启入口在桌面端「服务设置」的「允许无账号只读访问」，同时会显示明显的局域网风险提示。

浏览器只有在**安全上下文**（HTTPS 或本机 localhost）里才允许网页写本机文件夹。
如果要在局域网另一台电脑上使用「保存到本机目录」，需要在服务端启用 HTTPS（自签 CA），
并在那台电脑信任一次 CA；详细步骤、证书位置和确认流程见
[`docs/https-local-save.md`](docs/https-local-save.md)。手机 App 的 WebView 不支持这个 API，
所以该入口在手机上自动隐藏，普通下载/打包下载不受影响。

## 安全说明

- **默认是局域网明文 HTTP**。传输内容不做加密，同网段的其他人理论上可以嗅探。
  可选启用 HTTPS 给桌面浏览器（保存到本机目录需要），但它使用自签本地 CA，
  不会自动获得公共信任；手机和原生客户端仍走 HTTP。
  这是在「可信局域网」前提下的取舍 —— 换来的是零证书告警、零配置、最高速度。
  **不要在公共 Wi-Fi 上使用。**
- 密码用 Argon2id 哈希存储，**不存明文**；会话是服务端内存里的一张表，
  登出即失效，服务重启后全部失效（需要重新登录）。
  代价是密码忘了找不回来，只能 `--reset-password` 重设。
- 登录失败会触发递增退避限流，防止局域网内暴力破解。
- 所有文件路径都经过校验：`..` 穿越、绝对路径、盘符前缀、
  以及指向共享目录外的符号链接一律拒绝。**共享目录之外的文件无法访问。**
- 投送的源路径只来自**本机 GUI 的拖放**，HTTP 请求只能凭批次 UUID 和数组下标取文件，
  不能提交任意路径；批次与发起投送的客户端 id 绑定。
- 所有写操作要求自定义请求头，配合 `SameSite=Lax` 阻断 CSRF。

## 常见问题

**对端连不上？**
1. 确认两台机器在同一个局域网（互相能 ping 通）
2. Windows 上确认防火墙允许了「专用网络」
3. 换 `http://主机名.local:8080` 试试（走 Bonjour，不依赖 IP）
4. 公司网络可能开了「AP 隔离」，会禁止同一 Wi-Fi 下的设备互访 —— 这需要网管解决

**传输速度慢？**
- 先确认不是走 Wi-Fi 的 2.4G 频段。千兆有线/5G Wi-Fi 下应该能跑到 80 MB/s 以上
- 单个大文件受限于单条 TCP 连接；很多小文件受限于往返次数

**端口被占用？** 用 `--port 9000` 换一个。

---

## 开发

```bash
cargo test              # 58 个单元测试 + 31 个集成测试
cargo run               # 带图形界面运行
cargo run -- --headless # 不带界面运行
cargo clippy --all-targets

# 在 macOS 上打出两个平台的可分发产物到 release/macos/ 与 release/windows/
./scripts/package.sh

# iOS：生成 Xcode 工程、模拟器构建（需要 xcodegen）
cd ios && xcodegen generate
xcodebuild -project Wiretoss.xcodeproj -scheme Wiretoss \
  -configuration Debug -sdk iphonesimulator \
  -destination 'platform=iOS Simulator,name=iPhone 17' \
  -derivedDataPath build/DerivedData CODE_SIGNING_ALLOWED=NO build

# iOS：登录付费开发者账号后生成 IPA
WIRETOSS_TEAM_ID=<TeamID> ./scripts/ios-export.sh

# Android：签名 APK + AAB（需要 JDK 17+）
./scripts/android-release.sh
```

移动端签名、上架、审核备注和隐私政策见 [`docs/mobile-store.md`](docs/mobile-store.md)。

交叉编译 Windows 需要先装 mingw：

```bash
brew install mingw-w64
rustup target add x86_64-pc-windows-gnu
```

集成测试会在真实 HTTP 栈上起服务，跑完整链路：
登录 → 分片（乱序）上传 → 下载回来比对 SHA-256 → 校验路径穿越被拒 → 解压校验。
投送链路也走真实 HTTP：登记客户端 → 建批次 → manifest → 逐文件下载 → 保存目录结构比对。
打包下载的产物会用**另一个 zip 实现**（`zip` crate）读回来验证格式合规 ——
自己写自己读会一起错。

### 代码结构

```
src/
  main.rs      入口：默认开图形界面，--headless 走终端模式
  gui/         桌面控制面板（egui）
    mod.rs       窗口布局、状态、与服务/客户端线程的对接、拖放投送
    theme.rs     配色、圆角、卡片和背景绘制
    server.rs    服务端生命周期：独立线程 + tokio 运行时
    client.rs    原生客户端运行时：SSE 读取、下载 worker、断线重连
    tray.rs      菜单栏 / 托盘图标
    logging.rs   把 tracing 日志接进窗口的日志面板
  config.rs    配置读写、首启随机密码、Argon2id、客户端设置
  state.rs     应用状态：会话表、上传注册表、下载券、在线客户端、投送批次、后台清理
  auth.rs      登录/登出、会话提取器、登录限流、CSRF
  fs_api.rs    目录列举 + 安全路径解析（整个程序唯一的安全边界）
  clients.rs   原生客户端登记、定向 SSE 通知、在线快照
  push.rs      投送批次：本地目录遍历、跨平台命名、manifest/文件/进度/完成 API
  client.rs    原生客户端协议：登录、SSE 解析、路径规划、流式下载与重试
  download.rs  单文件 Range 下载、流式打包 zip
  upload.rs    分片上传与断点续传
  events.rs    SSE 事件广播（含投送活动）
  net.rs       局域网地址枚举、mDNS 广播、终端二维码
  router.rs    路由表
  tls.rs       可选 HTTPS：本地 CA、服务器证书生成和证书下载路由
  watch.rs     共享目录外部变化 -> SSE 刷新事件
  assets.rs    把前端资源嵌进二进制
assets/        前端（原生 HTML/CSS/JS，无构建步骤）；含 PWA manifest 和主屏图标
ios/           iOS 客户端（SwiftUI + WKWebView）
  project.yml   XcodeGen 工程描述；Wiretoss.xcodeproj 由它生成
  Wiretoss/     源码、Info.plist、隐私清单、Assets
  WiretossUITests/ 模拟器端到端测试
android/       Android 客户端（Kotlin + Compose + WebView）
  app/src/main/java/dev/wiretoss/mobile/  设置页、WebView、发现、下载
  app/src/main/res/                       图标、主题、Manifest
packaging/     打包输入：应用图标 SVG、macOS Info.plist、Android 图标
  android/       Android upload keystore（gitignored，需备份）
docs/          移动端上架说明、隐私政策
scripts/       打包、导出、清理脚本（含 ios-export.sh / android-release.sh）
```

前端刻意不引入构建步骤：这是一个工具型单页应用，上 Vite/React 会让
「改一行文案」变成「先 npm install」，而单文件分发的价值更大。

## 许可

MIT，详见 [LICENSE](LICENSE)。变更记录见 [CHANGELOG.md](CHANGELOG.md)。
