# AGENTS.md — Wiretoss

> 工作区总规约在上一级 [`AGENTS.md`](../AGENTS.md)（本机 `ongoing/`；仓库单独克隆时没有它，以本文件为准）。新建同类目录前先读总规约。总规约与本文件冲突时，以本文件更具体的那条为准，但不得放松总规约的红线。


局域网传文件的 Rust 应用。远端 `github.com/humorwuwu/Wiretoss`。

## 目录（不要发明新名字）

| 路径 | 角色 | 阶段结束后 |
|---|---|---|
| `src/`、`tests/`、`assets/`、`packaging/`、`scripts/` | 源码 | 保留 |
| `release/macos/` | 可运行的 `Wiretoss.app` 和 `wiretoss-v<version>-macos-universal.zip` | 保留 |
| `release/windows/` | `wiretoss-v<version>-windows-x64.zip` | 保留 |
| `target/` | Cargo 中间产物，含 `target/<triple>/release/`。这个 `release` 是 Cargo 的优化构建，不是发行目录 | **删除**（`cargo clean` 或 `scripts/clean.sh`） |
| `dist/` | 旧路径 | 不要再使用 |

平台目录只允许 `macos`、`windows`、`linux`、`ios`、`android`。文件名保持现有词干 `wiretoss-v<version>-<os>-<arch>.zip`，不要改成别的产品名。

`.github/workflows/release.yml` 里的 `dist/` 是 Actions 把各平台 artifact 下载到 runner 的目录，然后 `files: dist/*` 上传到 GitHub Release。那不是本仓库的发行目录。不要改 workflow 的 `dist/` 来「和本地统一」，除非连下载步骤一起改。

## 阶段清理

打包并确认 `release/` 里的 app / zip 能用之后，立刻：

```sh
bash scripts/clean.sh
```

它删 `target/` 和残留的 `dist/`，不删 `release/`。

## 命令

```sh
cargo test --all-targets
cargo build --release          # 本机调试二进制在 target/release/，用完即清
./scripts/package.sh           # -> release/macos 与 release/windows
```
