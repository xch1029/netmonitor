<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

type PermissionState = "idle" | "pending" | "granted" | "denied" | "error";

type SummarySnapshot = {
  downBps: number;
  upBps: number;
  sampledAt: number;
  adapters: string[];
};

type ProcessSnapshot = {
  pid: number;
  imageName: string;
  displayName: string;
  downBps: number;
  upBps: number;
};

type MonitoringState = {
  processDetailsEnabled: boolean;
  permissionState: PermissionState;
  lastError: string | null;
};

type BootstrapState = {
  summary: SummarySnapshot;
  processes: ProcessSnapshot[];
  monitoringState: MonitoringState;
};

const summary = ref<SummarySnapshot>({
  downBps: 0,
  upBps: 0,
  sampledAt: 0,
  adapters: [],
});
const processes = ref<ProcessSnapshot[]>([]);
const monitoringState = ref<MonitoringState>({
  processDetailsEnabled: false,
  permissionState: "idle",
  lastError: null,
});
const isBusy = ref(false);
const unlisteners = ref<UnlistenFn[]>([]);

const totalTraffic = computed(() => summary.value.downBps + summary.value.upBps);
const adapterLabel = computed(() =>
  summary.value.adapters.length > 0 ? summary.value.adapters.join(", ") : "等待可用网卡",
);
const permissionHeadline = computed(() => {
  switch (monitoringState.value.permissionState) {
    case "pending":
      return "等待 Windows 管理员授权";
    case "granted":
      return "每进程监控已开启";
    case "denied":
      return "管理员授权被拒绝";
    case "error":
      return "进程监控出现错误";
    default:
      return "详细监控当前未开启";
  }
});

function formatSpeed(bps: number) {
  if (bps >= 1_000_000_000) return `${(bps / 1_000_000_000).toFixed(1)} Gbps`;
  if (bps >= 1_000_000) return `${(bps / 1_000_000).toFixed(1)} Mbps`;
  if (bps >= 1_000) return `${(bps / 1_000).toFixed(1)} Kbps`;
  return `${bps} bps`;
}

function formatTimestamp(value: number) {
  if (!value) return "尚未采样";
  return new Date(value).toLocaleTimeString();
}

async function loadBootstrap() {
  const bootstrap = await invoke<BootstrapState>("get_bootstrap_state");
  summary.value = bootstrap.summary;
  processes.value = bootstrap.processes;
  monitoringState.value = bootstrap.monitoringState;
}

async function requestProcessMonitoring() {
  isBusy.value = true;
  try {
    monitoringState.value = await invoke<MonitoringState>("request_process_monitoring");
  } finally {
    isBusy.value = false;
  }
}

async function stopProcessMonitoring() {
  isBusy.value = true;
  try {
    monitoringState.value = await invoke<MonitoringState>("stop_process_monitoring");
  } finally {
    isBusy.value = false;
  }
}

onMounted(async () => {
  try {
    await loadBootstrap();
  } catch (error) {
    monitoringState.value = {
      processDetailsEnabled: false,
      permissionState: "error",
      lastError: String(error),
    };
  }

  unlisteners.value = [
    await listen<SummarySnapshot>("monitor://summary", (event) => {
      summary.value = event.payload;
    }),
    await listen<ProcessSnapshot[]>("monitor://processes", (event) => {
      processes.value = [...event.payload];
    }),
    await listen<MonitoringState>("monitor://monitoring-state", (event) => {
      monitoringState.value = event.payload;
    }),
  ];
});

onBeforeUnmount(() => {
  for (const unlisten of unlisteners.value) unlisten();
});
</script>

<template>
  <main class="shell">
    <section class="hero card">
      <div class="eyebrow">网速监控</div>
      <div class="hero-grid">
        <div>
          <h1>Windows 托盘网速监控</h1>
          <p class="hero-copy">
            托盘区域持续显示实时上传和下载速度。这个窗口用于查看每个应用的详细网络占用情况。
          </p>
        </div>
        <div class="live-chip">
          <span>当前总速率</span>
          <strong>{{ formatSpeed(totalTraffic) }}</strong>
          <small>最近采样 {{ formatTimestamp(summary.sampledAt) }}</small>
        </div>
      </div>
    </section>

    <section class="metrics">
      <article class="metric card down">
        <span>下载</span>
        <strong>{{ formatSpeed(summary.downBps) }}</strong>
        <small>{{ adapterLabel }}</small>
      </article>
      <article class="metric card up">
        <span>上传</span>
        <strong>{{ formatSpeed(summary.upBps) }}</strong>
        <small>活动网卡会自动刷新</small>
      </article>
    </section>

    <section class="status card">
      <div>
        <div class="eyebrow">进程详情</div>
        <h2>{{ permissionHeadline }}</h2>
        <p class="status-copy">
          每进程实时流量统计依赖提权 helper，这样主程序在普通托盘模式下可以保持轻量运行。
        </p>
        <p v-if="monitoringState.lastError" class="error-copy">{{ monitoringState.lastError }}</p>
      </div>
      <div class="actions">
        <button
          class="primary"
          type="button"
          :disabled="isBusy || monitoringState.permissionState === 'pending'"
          @click="requestProcessMonitoring"
        >
          {{ monitoringState.processDetailsEnabled ? "重启提权 helper" : "开启每应用详情" }}
        </button>
        <button
          class="secondary"
          type="button"
          :disabled="isBusy || !monitoringState.processDetailsEnabled"
          @click="stopProcessMonitoring"
        >
          停止进程监控
        </button>
      </div>
    </section>

    <section class="table-card card">
      <div class="table-header">
        <div>
          <div class="eyebrow">应用占用</div>
          <h2>网络占用排行</h2>
        </div>
        <span class="table-meta">{{ processes.length }} 条活动记录</span>
      </div>

      <div v-if="processes.length === 0" class="empty-state">
        <strong>暂时没有进程流量数据</strong>
        <p>打开浏览器、播放流媒体，或者开启提权 helper 后，这里会显示实时排行。</p>
      </div>

      <div v-else class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>应用</th>
              <th>进程</th>
              <th>下载</th>
              <th>上传</th>
              <th>PID</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="process in processes" :key="`${process.pid}-${process.imageName}`">
              <td>{{ process.displayName }}</td>
              <td>{{ process.imageName }}</td>
              <td>{{ formatSpeed(process.downBps) }}</td>
              <td>{{ formatSpeed(process.upBps) }}</td>
              <td>{{ process.pid }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>
  </main>
</template>

<style>
:root {
  color: #f6f4ef;
  background:
    radial-gradient(circle at top left, rgba(63, 108, 222, 0.35), transparent 32%),
    radial-gradient(circle at top right, rgba(214, 110, 42, 0.3), transparent 28%),
    linear-gradient(180deg, #0a1020 0%, #11192c 52%, #0b0f16 100%);
  font-family: "Segoe UI", "Microsoft YaHei UI", sans-serif;
  line-height: 1.5;
  font-weight: 400;
  font-synthesis: none;
  text-rendering: optimizeLegibility;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
}

* {
  box-sizing: border-box;
}

body {
  margin: 0;
  min-width: 320px;
}

button {
  font: inherit;
}

#app {
  min-height: 100vh;
}

.shell {
  min-height: 100vh;
  padding: 28px;
  display: grid;
  gap: 18px;
}

.card {
  border: 1px solid rgba(255, 255, 255, 0.08);
  background: rgba(7, 11, 20, 0.72);
  backdrop-filter: blur(18px);
  box-shadow: 0 22px 48px rgba(0, 0, 0, 0.28);
}

.hero {
  padding: 28px;
  border-radius: 28px;
}

.hero-grid {
  display: grid;
  grid-template-columns: minmax(0, 1.5fr) minmax(240px, 0.8fr);
  gap: 18px;
  align-items: end;
}

.eyebrow {
  font-size: 12px;
  letter-spacing: 0.22em;
  text-transform: uppercase;
  color: #8ea8d9;
  margin-bottom: 10px;
}

h1,
h2,
p {
  margin: 0;
}

h1 {
  font-size: clamp(32px, 5vw, 54px);
  line-height: 1;
  max-width: 10ch;
}

h2 {
  font-size: 24px;
  line-height: 1.1;
}

.hero-copy,
.status-copy,
.empty-state p {
  margin-top: 12px;
  color: rgba(233, 239, 248, 0.76);
  max-width: 62ch;
}

.live-chip {
  padding: 20px;
  border-radius: 24px;
  background: linear-gradient(145deg, rgba(36, 63, 119, 0.92), rgba(19, 31, 57, 0.92));
  display: grid;
  gap: 6px;
}

.live-chip span,
.metric span,
.table-meta,
.empty-state strong {
  color: rgba(226, 234, 244, 0.78);
  font-size: 13px;
  text-transform: uppercase;
  letter-spacing: 0.14em;
}

.live-chip strong,
.metric strong {
  font-size: clamp(28px, 3vw, 36px);
  line-height: 1;
}

.live-chip small,
.metric small {
  color: rgba(226, 234, 244, 0.72);
}

.metrics {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 18px;
}

.metric {
  padding: 22px;
  border-radius: 24px;
  display: grid;
  gap: 10px;
}

.metric.down {
  background: linear-gradient(140deg, rgba(30, 87, 164, 0.85), rgba(10, 19, 34, 0.96));
}

.metric.up {
  background: linear-gradient(140deg, rgba(150, 79, 34, 0.88), rgba(22, 14, 9, 0.96));
}

.status,
.table-card {
  border-radius: 28px;
  padding: 24px;
}

.status {
  display: grid;
  grid-template-columns: minmax(0, 1.4fr) auto;
  gap: 16px;
  align-items: center;
}

.actions {
  display: flex;
  flex-wrap: wrap;
  justify-content: flex-end;
  gap: 12px;
}

.primary,
.secondary {
  border: none;
  border-radius: 999px;
  padding: 12px 18px;
  cursor: pointer;
  transition: transform 0.18s ease, opacity 0.18s ease;
}

.primary {
  background: linear-gradient(135deg, #f2f6ff, #b8d5ff);
  color: #0f1d38;
  font-weight: 700;
}

.secondary {
  background: rgba(255, 255, 255, 0.08);
  color: #f5f1e8;
  border: 1px solid rgba(255, 255, 255, 0.12);
}

.primary:disabled,
.secondary:disabled {
  cursor: not-allowed;
  opacity: 0.55;
}

.primary:not(:disabled):hover,
.secondary:not(:disabled):hover {
  transform: translateY(-1px);
}

.error-copy {
  margin-top: 10px;
  color: #ffb48a;
}

.table-header {
  display: flex;
  justify-content: space-between;
  align-items: end;
  gap: 12px;
  margin-bottom: 20px;
}

.table-wrap {
  overflow: auto;
  border-radius: 18px;
  border: 1px solid rgba(255, 255, 255, 0.08);
}

table {
  width: 100%;
  border-collapse: collapse;
  min-width: 720px;
}

thead {
  background: rgba(255, 255, 255, 0.05);
}

th,
td {
  text-align: left;
  padding: 15px 16px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.06);
}

th {
  font-size: 12px;
  letter-spacing: 0.16em;
  text-transform: uppercase;
  color: rgba(220, 229, 240, 0.72);
}

td {
  color: #f7f7f3;
}

.empty-state {
  min-height: 220px;
  display: grid;
  place-content: center;
  text-align: center;
  gap: 6px;
  color: rgba(242, 246, 255, 0.74);
}

@media (max-width: 900px) {
  .shell {
    padding: 16px;
  }

  .hero-grid,
  .metrics,
  .status {
    grid-template-columns: 1fr;
  }

  .actions {
    justify-content: flex-start;
  }
}
</style>
