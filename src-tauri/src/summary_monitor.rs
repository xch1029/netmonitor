use std::{
    collections::HashMap,
    mem::zeroed,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use tauri::AppHandle;

use crate::{models::SummarySnapshot, update_summary_state, SharedState};

#[derive(Debug, Clone)]
struct InterfaceCounter {
    alias: String,
    in_octets: u64,
    out_octets: u64,
}

pub fn start(app: AppHandle, state: SharedState) {
    thread::spawn(move || {
        let mut sampler = SummarySampler::default();

        loop {
            let snapshot = sampler.sample().unwrap_or_else(|_| SummarySnapshot {
                sampled_at: now_epoch_ms(),
                ..SummarySnapshot::default()
            });

            update_summary_state(&app, &state, snapshot);
            thread::sleep(Duration::from_millis(500));
        }
    });
}

#[derive(Default)]
struct SummarySampler {
    last_at: Option<Instant>,
    last_counters: HashMap<u32, InterfaceCounter>,
}

impl SummarySampler {
    fn sample(&mut self) -> Result<SummarySnapshot> {
        let now = Instant::now();
        let counters = collect_interface_counters()?;
        let sampled_at = now_epoch_ms();
        let adapters = counters.values().map(|counter| counter.alias.clone()).collect();

        let mut down_bps = 0u64;
        let mut up_bps = 0u64;

        if let Some(previous_time) = self.last_at {
            let elapsed = now.duration_since(previous_time).as_secs_f64().max(0.5);
            for (index, counter) in &counters {
                if let Some(previous) = self.last_counters.get(index) {
                    down_bps += (((counter.in_octets.saturating_sub(previous.in_octets)) as f64 / elapsed)
                        as u64)
                        * 8;
                    up_bps += (((counter.out_octets.saturating_sub(previous.out_octets)) as f64 / elapsed)
                        as u64)
                        * 8;
                }
            }
        }

        self.last_at = Some(now);
        self.last_counters = counters;

        Ok(SummarySnapshot {
            down_bps,
            up_bps,
            sampled_at,
            adapters,
        })
    }
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
fn collect_interface_counters() -> Result<HashMap<u32, InterfaceCounter>> {
    use windows::Win32::Foundation::WIN32_ERROR;
    use windows::Win32::NetworkManagement::IpHelper::{GetBestInterface, GetIfEntry2, MIB_IF_ROW2};

    let mut counters = HashMap::new();
    let mut best_index = 0u32;
    let result = unsafe { GetBestInterface(u32::from_be_bytes([1, 1, 1, 1]), &mut best_index) };
    if result != 0 {
        return Ok(counters);
    }

    let mut row: MIB_IF_ROW2 = unsafe { zeroed() };
    row.InterfaceIndex = best_index;

    if unsafe { GetIfEntry2(&mut row) } != WIN32_ERROR(0) {
        return Ok(counters);
    }

    let alias = wide_to_string(&row.Alias);
    counters.insert(
        best_index,
        InterfaceCounter {
            alias: if alias.is_empty() { format!("if#{best_index}") } else { alias },
            in_octets: row.InOctets,
            out_octets: row.OutOctets,
        },
    );

    Ok(counters)
}

#[cfg(not(target_os = "windows"))]
fn collect_interface_counters() -> Result<HashMap<u32, InterfaceCounter>> {
    Ok(HashMap::new())
}

#[cfg(target_os = "windows")]
fn wide_to_string(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|value| *value == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end]).trim().to_string()
}
