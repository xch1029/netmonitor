use serde::{Deserialize, Serialize};

pub const EVENT_SUMMARY: &str = "monitor://summary";
pub const EVENT_PROCESSES: &str = "monitor://processes";
pub const EVENT_MONITORING_STATE: &str = "monitor://monitoring-state";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummarySnapshot {
    pub down_bps: u64,
    pub up_bps: u64,
    pub sampled_at: u64,
    pub adapters: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessSnapshot {
    pub pid: u32,
    pub image_name: String,
    pub display_name: String,
    pub down_bps: u64,
    pub up_bps: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Idle,
    Pending,
    Granted,
    Denied,
    Error,
}

impl Default for PermissionState {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringState {
    pub process_details_enabled: bool,
    pub permission_state: PermissionState,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapState {
    pub summary: SummarySnapshot,
    pub processes: Vec<ProcessSnapshot>,
    pub monitoring_state: MonitoringState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HelperMessage {
    Hello { token: String },
    Processes {
        sampled_at: u64,
        processes: Vec<ProcessSnapshot>,
    },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HelperCommand {
    Shutdown,
}
