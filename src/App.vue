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

const processMonitoringActive = computed(
  () =>
    monitoringState.value.processDetailsEnabled &&
    monitoringState.value.permissionState === "granted",
);

const visibleProcesses = computed(() =>
  processes.value.filter((process) => Math.max(process.downBps, process.upBps) >= 5_000),
);

const primaryActionLabel = computed(() => {
  switch (monitoringState.value.permissionState) {
    case "pending":
      return "等待授权中";
    case "denied":
    case "error":
      return "重新申请授权";
    default:
      return "开启应用详情";
  }
});



const panelDescription = computed(() => {
  switch (monitoringState.value.permissionState) {
    case "pending":
      return "等待 Windows 管理员授权，授权完成后会自动开始采集每个应用的实时上下行速率。";
    case "denied":
      return "你已拒绝管理员授权。再次申请后，才会显示每个应用的实时网络占用。";
    case "error":
      return "进程级监控启动失败，可以在这里直接重新申请授权并重试。";
    default:
      return "授权后会展示每个应用的实时下载、上传和 PID 排行。";
  }
});

function formatSpeed(bps: number) {
  if (bps >= 1_000_000_000) return `${(bps / 1_000_000_000).toFixed(1)} Gbps`;
  if (bps >= 1_000_000) return `${(bps / 1_000_000).toFixed(1)} Mbps`;
  if (bps >= 1_000) return `${(bps / 1_000).toFixed(1)} Kbps`;
  return `${bps} bps`;
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
    <section class="metrics">
      <article class="metric card down">
        <span>下载</span>
        <strong>{{ formatSpeed(summary.downBps) }}</strong>
      </article>
      <article class="metric card up">
        <span>上传</span>
        <strong>{{ formatSpeed(summary.upBps) }}</strong>
      </article>
    </section>

    <section class="table-card card">
      <div class="table-header">
        <div>
          <div class="eyebrow">网络占用排行</div>
          <p class="table-copy">{{ panelDescription }}</p>
          <p v-if="monitoringState.lastError" class="error-copy">{{ monitoringState.lastError }}</p>
        </div>

        <div class="table-header-side">
          <button
            v-if="!processMonitoringActive"
            class="primary"
            type="button"
            :disabled="isBusy || monitoringState.permissionState === 'pending'"
            @click="requestProcessMonitoring"
          >
            {{ primaryActionLabel }}
          </button>
          <span v-else class="table-meta">{{ visibleProcesses.length }} 条活跃记录</span>
        </div>
      </div>

      <div v-if="!processMonitoringActive" class="empty-state">
        <strong>未开启应用级详情</strong>
        <p>接受管理员授权后，这里会显示每个应用的实时网络占用排行。</p>
      </div>

      <div v-else-if="visibleProcesses.length === 0" class="empty-state">
        <strong>当前没有活跃网络数据</strong>
        <p>当前低于 5 Kbps 的进程已被隐藏，产生更明显的网络流量后会自动显示。</p>
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
            <tr v-for="process in visibleProcesses" :key="`${process.pid}-${process.imageName}`">
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
  padding: 10px;
  display: grid;
  gap: 10px;
  grid-template-rows: auto 1fr;
}

.card {
  border: 1px solid rgba(255, 255, 255, 0.08);
  background: rgba(7, 11, 20, 0.72);
  backdrop-filter: blur(18px);
  box-shadow: 0 22px 48px rgba(0, 0, 0, 0.28);
}

.metrics {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 8px;
}

.metric {
  padding: 8px 12px;
  border-radius: 14px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.metric span,
.table-meta,
.empty-state strong {
  color: rgba(226, 234, 244, 0.78);
  font-size: 12px;
  text-transform: uppercase;
  letter-spacing: 0.14em;
}

.metric strong {
  font-size: clamp(20px, 2.6vw, 28px);
  line-height: 1.05;
  text-align: right;
}

.metric.down {
  background: linear-gradient(140deg, rgba(30, 87, 164, 0.85), rgba(10, 19, 34, 0.96));
}

.metric.up {
  background: linear-gradient(140deg, rgba(150, 79, 34, 0.88), rgba(22, 14, 9, 0.96));
}

.table-card {
  border-radius: 24px;
  padding: 20px;
}

.eyebrow {
  font-size: 12px;
  letter-spacing: 0.22em;
  text-transform: uppercase;
  color: #8ea8d9;
  margin-bottom: 8px;
}

h2,
p {
  margin: 0;
}

h2 {
  font-size: 22px;
  line-height: 1.1;
}

.table-copy {
  margin-top: 10px;
  color: rgba(233, 239, 248, 0.76);
  max-width: 60ch;
}

.error-copy {
  margin-top: 8px;
  color: #ffb48a;
}

.table-header {
  display: flex;
  justify-content: space-between;
  align-items: start;
  gap: 16px;
  margin-bottom: 18px;
}

.table-header-side {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  min-width: max-content;
}

.primary {
  border: none;
  border-radius: 999px;
  padding: 11px 18px;
  cursor: pointer;
  transition: transform 0.18s ease, opacity 0.18s ease;
  background: linear-gradient(135deg, #f2f6ff, #b8d5ff);
  color: #0f1d38;
  font-weight: 700;
}

.primary:disabled {
  cursor: not-allowed;
  opacity: 0.55;
}

.primary:not(:disabled):hover {
  transform: translateY(-1px);
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
  padding: 14px 16px;
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
    padding: 8px;
  }

  .metrics,
  .table-header {
    grid-template-columns: 1fr;
  }

  .table-header {
    display: grid;
  }

  .table-header-side {
    justify-content: flex-start;
    min-width: 0;
  }
}
</style>
