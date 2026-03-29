# NetMonitor

[中文](./README.md)

NetMonitor is a Windows desktop network monitor built with Tauri 2, Vue 3, TypeScript, and Rust. The current scope is a lightweight real-time bandwidth overview plus per-process traffic details.

## Features

- Shows live upload and download status in the system tray.
- Displays total download and upload speed in the main window.
- Supports per-process traffic ranking with download, upload, and PID data.
- Requests administrator permission through an elevated helper when process-level monitoring is enabled for the first time.
- Hides to the system tray instead of exiting when the main window is closed.

## Tech Stack

- Tauri 2
- Vue 3
- Vite
- TypeScript
- Rust
- Windows networking and tray-related APIs

## Requirements

- Windows 10/11
- Node.js
- pnpm
- Rust toolchain
- Tauri CLI prerequisites

This is effectively a Windows-first project. Both aggregate speed sampling and process-level monitoring rely on Windows-specific capabilities.

## Quick Start

Install frontend dependencies:

```bash
pnpm install
```

Start the frontend only:

```bash
pnpm dev
```

Start the Tauri desktop app:

```bash
pnpm tauri dev
```

Build the frontend:

```bash
pnpm build
```

Build the desktop app:

```bash
pnpm tauri build
```

Check Rust code only:

```bash
cargo check --manifest-path src-tauri/Cargo.toml
```

## Project Structure

- `src/`: Vue frontend source.
- `src/App.vue`: main UI for summary speed, process table, and permission state.
- `src/main.ts`: frontend bootstrap entry.
- `src-tauri/src/lib.rs`: Tauri setup, tray integration, window behavior, and command registration.
- `src-tauri/src/helper.rs`: elevated helper and process-monitoring flow.
- `src-tauri/src/summary_monitor.rs`: aggregate bandwidth sampling.
- `src-tauri/src/models.rs`: shared models and event payloads.
- `src-tauri/tauri.conf.json`: app metadata, window config, and build hooks.

## Development Notes

- `beforeDevCommand` and `beforeBuildCommand` are currently configured for `pnpm`; if that changes, update `src-tauri/tauri.conf.json` as well.
- The current application identifier is `com.zzzg.netmonitor`.
- When adding a new Tauri command, update both Rust registration and the frontend call site in the same change.
- Process-level monitoring depends on elevation, so development and testing should cover granted, denied, and helper-failure paths.

## Current Scope

The repository is no longer just the default Tauri starter. It already includes:

- tray-resident behavior
- aggregate bandwidth polling
- show/hide main window control
- an elevated helper pipeline for per-process traffic collection

Natural next steps would be traffic history, app filtering, adapter switching, unit or theme settings, and stronger recovery flows.
