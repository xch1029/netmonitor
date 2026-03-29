# NetMonitor

[English](./README.en.md)

NetMonitor 是一个基于 Tauri 2、Vue 3、TypeScript 和 Rust 的 Windows 桌面网络监控应用，当前重点提供轻量、实时的总带宽概览，以及按进程划分的网络占用详情。

## 功能概览

- 托盘图标实时显示网络上下行状态。
- 主窗口展示当前总下载速率和上传速率。
- 支持按进程查看实时下载、上传和 PID 排行。
- 首次启用进程级监控时会拉起提权 helper，请求管理员权限。
- 关闭主窗口后不会直接退出，而是隐藏到系统托盘继续运行。

## 技术栈

- Tauri 2
- Vue 3
- Vite
- TypeScript
- Rust
- Windows 平台网络与托盘相关 API

## 运行环境

- Windows 10/11
- Node.js
- pnpm
- Rust toolchain
- Tauri CLI 相关依赖

这是一个明显偏 Windows 的项目。总网速采样和进程级监控都依赖 Windows 能力，非 Windows 环境下不适合作为主要运行目标。

## 快速开始

安装前端依赖：

```bash
pnpm install
```

启动前端开发服务：

```bash
pnpm dev
```

启动 Tauri 桌面应用：

```bash
pnpm tauri dev
```

构建前端：

```bash
pnpm build
```

构建桌面应用：

```bash
pnpm tauri build
```

只检查 Rust 代码：

```bash
cargo check --manifest-path src-tauri/Cargo.toml
```

## 项目结构

- `src/`：Vue 前端代码。
- `src/App.vue`：当前主界面，负责展示汇总速率、进程表格和权限状态。
- `src/main.ts`：前端启动入口。
- `src-tauri/src/lib.rs`：Tauri 应用初始化、托盘、窗口行为和命令注册。
- `src-tauri/src/helper.rs`：进程级监控 helper 与提权逻辑。
- `src-tauri/src/summary_monitor.rs`：总网速采样逻辑。
- `src-tauri/src/models.rs`：前后端共享的数据结构与事件模型。
- `src-tauri/tauri.conf.json`：应用元信息、窗口配置和构建命令。

## 开发说明

- 当前 `beforeDevCommand` 和 `beforeBuildCommand` 配置为 `pnpm`，如果改包管理器，需要同时更新 `src-tauri/tauri.conf.json`。
- 应用标识符当前为 `com.zzzg.netmonitor`。
- 如果新增 Tauri command，需要同时更新 Rust 注册和前端调用。
- 进程级监控涉及管理员授权，开发和测试时要覆盖授权成功、拒绝授权和 helper 异常退出这几类情况。

## 当前状态

仓库已经不再是默认 Tauri 起步模板。当前实现包含：

- 托盘常驻
- 总带宽轮询采样
- 主窗口隐藏/显示控制
- 进程级网络详情的提权采集链路

如果后续继续扩展，比较自然的方向包括历史流量记录、应用筛选、适配器切换、单位或主题设置，以及更完整的错误恢复能力。
