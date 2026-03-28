# AGENTS.md

## Project Overview
- `netmonitor` is a desktop app built with Tauri 2, Vue 3, Vite, and TypeScript.
- The frontend lives in `src/` and the Rust/Tauri backend lives in `src-tauri/`.
- The current repository is still close to the default Tauri starter, so prefer small, incremental changes that keep frontend and Rust interfaces aligned.

## Repo Layout
- `src/main.ts`: Vue app bootstrap.
- `src/App.vue`: current top-level application shell.
- `src-tauri/src/main.rs`: native binary entry point.
- `src-tauri/src/lib.rs`: Tauri app setup and command registration.
- `src-tauri/tauri.conf.json`: app metadata, window config, and frontend build hooks.

## Dev Environment Tips
- `src-tauri/tauri.conf.json` is currently configured to use `pnpm` for `beforeDevCommand` and `beforeBuildCommand`.
- If you switch the project to `npm` or another package manager, update `src-tauri/tauri.conf.json` at the same time.
- Prefer keeping Vue components in Single File Component format with `<script setup lang="ts">`.
- When adding native features, expose small Rust commands first and call them from the frontend with `invoke` from `@tauri-apps/api/core`.

## Common Commands
- Install frontend dependencies: `pnpm install`
- Start frontend only: `pnpm dev`
- Start the Tauri desktop app: `pnpm tauri dev`
- Build the frontend: `pnpm build`
- Build the desktop app: `pnpm tauri build`
- Check Rust code only: `cargo check --manifest-path src-tauri/Cargo.toml`

## Implementation Guidance
- Keep frontend state and view code in Vue, and push OS-level capabilities such as process, file system, tray, and networking work into Rust.
- Keep Tauri commands narrow and typed so the frontend contract stays easy to evolve.
- If a feature needs polling or background monitoring, prefer designing a simple Rust-side service plus explicit events or commands instead of putting heavy loops in the Vue layer.

## Verification
- For frontend-only changes, run `pnpm build`.
- For Rust or Tauri command changes, run `cargo check --manifest-path src-tauri/Cargo.toml`.
- For changes that cross the frontend/native boundary, run both checks before handing off.

## Notes For Agents
- Do not silently change the configured package manager behavior.
- Preserve the application identifier `com.zzzg.netmonitor` unless there is an explicit product decision to change it.
- If you introduce new commands, update both Rust registration and the frontend call sites in the same task.
