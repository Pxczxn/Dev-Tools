# Dev-Tools

Dev-Tools 是一个面向 Windows 本地开发环境的桌面控制台，用来集中处理开发过程中常见的端口冲突、进程占用、基础服务、运行时环境与项目启动管理。

当前版本：`1.0.0`（测试版）

## 主要能力

- **项目管理**：扫描并识别常见项目结构，维护项目目录与启动配置。
- **快捷启动 / 停止**：统一管理项目启动会话，并使用 Windows Job Object 约束进程树生命周期。
- **端口管理**：查询 TCP/UDP 端口占用，绑定进程身份快照后再执行安全终止。
- **目录进程管理**：扫描项目目录相关进程，并在终止前重新校验进程身份。
- **基础服务管理**：发现并控制 MySQL、Redis 等 Windows 服务。
- **环境检测**：检测 Java、Node.js、Python、PHP、Rust 等本机运行时，并支持手动覆盖与禁用。
- **配置管理**：支持配置导入、导出、备份恢复、主题与安全策略设置。
- **审计与错误记录**：记录关键操作及近期错误，便于排查问题。

## 安全与可靠性

Dev-Tools 会直接与本机进程、端口和 Windows 服务交互，因此默认采用较严格的保护策略：

- 危险启动命令和 shell 链接/重定向会被拒绝。
- 受保护系统进程不会被直接终止。
- 端口和目录进程终止均绑定进程启动时间与身份快照，降低 PID 复用和 TOCTOU 风险。
- 高风险操作使用一次性确认令牌，并限制待确认令牌数量。
- 配置写入采用原子替换与备份恢复策略。
- Windows 项目会话使用 Job Object 管理整棵进程树，避免只杀父进程留下孤儿进程。

## 配置位置

默认配置位于程序旁边的 `data/config.json`。便携版的 WebView 数据与缓存位于 `data/webview/`，退出程序后可整体移动或备份程序目录。

可通过设置中的数据目录迁移功能或环境变量 `XCHEN_DATA_DIR` 指定配置目录。已有配置的数据结构和存储方式不受本次改名影响。

日志采用滚动缓存：后台每服务最多保留设置的行数或约 8MiB（先达到者生效）；页面每服务最多保留 1000 行或约 2MiB 文本，全部服务合计约 8MiB，超出时淘汰旧缓存。切换服务可从后台重新读取近期记录，导出以后台仍保留的记录为准。超过 64KiB 的单行会显示截断提示；清空日志会释放缓存容量。

路径占用在全局搜索和“路径占用”页使用 Windows Restart Manager 查询文件资源的关联进程，支持选择进程后终止。目录查询递归收集其中的文件，不跟随链接或 junction；最多收集 20,000 项待查文件/目录、累计 2,000,000 个路径字符，并限制文件遍历为 5 秒，触限及跳过项会在结果中提示。它不覆盖仅持有目录句柄或以该目录为工作目录的进程，因此未查到结果不代表目录一定可以删除。释放操作会重新查询占用，并通过同一进程句柄核对启动时间再终止；拒绝终止当前应用、系统关键进程和 Windows 服务。

## 技术栈

- Tauri 2
- React 19
- TypeScript
- Rust
- Vite
- Vitest
- Windows API / Job Object

## 本地开发

要求：

- Node.js 22
- Rust 1.98.0（仓库已通过 `rust-toolchain.toml` 固定）
- Tauri 在 Windows 上所需的系统构建依赖

安装依赖（会自动从 `public/app-icon.svg` 生成 `src-tauri/icons/`）：

```bash
npm ci
```

若仅执行 Rust 构建且缺少图标，可手动运行：

```bash
npm run icons
```

启动开发环境：

```bash
npm run tauri dev
```

运行前端测试：

```bash
npm test
```

运行 Rust 测试：

```bash
cargo test --locked --manifest-path src-tauri/Cargo.toml -- --test-threads=1
```

严格 Clippy：

```bash
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

## Windows 安装包

项目使用 **NSIS** 打包 Windows x64 安装程序，采用当前用户安装模式，不要求管理员权限，并提供简体中文 / English 安装界面。

本地构建：

```bash
npm run tauri build -- --bundles nsis
```

生成文件位于：

```text
src-tauri\target\release\bundle\nsis\
```

应用图标以 `public/app-icon.svg` 为唯一源文件，开发和构建流程会自动生成 Tauri 所需的 Windows 图标集。

仓库中的 `Package Windows` GitHub Actions 工作流会在正式打包前执行版本一致性校验、依赖审计、前后端测试与静态检查，并对生成的安装程序进行静默安装 / 卸载回归，确认卸载不会删除用户配置，同时输出 SHA256 校验值。

推送符合 `v*` 规则的版本标签（例如 `v0.1.0`）后，`Release Windows` 工作流会再次执行完整发布检查，构建 NSIS 安装程序并创建或更新对应的 GitHub Release。

## CI

主分支和 Pull Request 会执行：

- npm 依赖审计
- 前端测试与构建
- Rust 格式检查
- Clippy `-D warnings`
- Rust 全量测试
- Tauri release application build（不生成安装包）

正式安装包由独立 Windows packaging workflow 构建，避免普通 CI 每次都承担 NSIS 打包成本。

## 状态

项目处于 `v0.1.0` 发布前封板阶段。核心功能、Windows 安装包构建、图标生成、版本一致性检查和安装 / 卸载持久化回归均已接入自动化流程；发布前不优先扩展新功能。

---

**Dev-Tools · By XchenDev**
