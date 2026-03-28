use std::{
    collections::HashMap,
    ffi::OsString,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use ferrisetw::{
    parser::Parser,
    provider::{kernel_providers, Provider},
    schema_locator::SchemaLocator,
    trace::KernelTrace,
    EventRecord,
};
use interprocess::TryClone;
use interprocess::local_socket::{prelude::*, GenericNamespaced, ListenerOptions, Stream};
use sysinfo::{ProcessesToUpdate, System};
use tauri::{AppHandle, Manager};

use crate::{
    clear_process_state, update_monitoring_state, update_process_state,
    models::{HelperCommand, HelperMessage, MonitoringState, PermissionState, ProcessSnapshot},
    SharedState,
};

pub struct HelperSession {
    pub writer: Arc<Mutex<Stream>>,
}

pub fn run_if_requested(args: impl IntoIterator<Item = OsString>) -> Option<i32> {
    match helper_args(args) {
        Ok(Some(config)) => Some(match run_helper(config) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("{error:#}");
                1
            }
        }),
        Ok(None) => None,
        Err(error) => {
            eprintln!("{error:#}");
            Some(1)
        }
    }
}

pub fn request_process_monitoring_inner(
    app: &AppHandle,
    state: SharedState,
) -> Result<MonitoringState> {
    {
        let monitoring = state.monitoring.read().unwrap().clone();
        if monitoring.process_details_enabled || monitoring.permission_state == PermissionState::Pending {
            return Ok(monitoring);
        }
    }

    let pipe_name = format!(
        "netmonitor-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default()
    );
    let token = format!("{}-{}", std::process::id(), now_epoch_ms());

    let namespaced = pipe_name
        .clone()
        .to_ns_name::<GenericNamespaced>()
        .context("failed to build helper pipe name")?;
    let listener = ListenerOptions::new()
        .name(namespaced)
        .create_sync()
        .context("failed to create helper pipe listener")?;

    update_monitoring_state(app, &state, |monitoring| {
        monitoring.process_details_enabled = false;
        monitoring.permission_state = PermissionState::Pending;
        monitoring.last_error = None;
    });

    let app_handle = app.clone();
    let state_for_accept = state.clone();
    let token_for_accept = token.clone();

    thread::spawn(move || {
        let result = accept_helper(&app_handle, &state_for_accept, listener, &token_for_accept);
        if let Err(error) = result {
            clear_process_state(&app_handle, &state_for_accept);
            update_monitoring_state(&app_handle, &state_for_accept, |monitoring| {
                monitoring.process_details_enabled = false;
                monitoring.permission_state = PermissionState::Error;
                monitoring.last_error = Some(error.to_string());
            });
        }
    });

    spawn_elevated_helper(app, &pipe_name, &token)
        .with_context(|| "failed to launch elevated process helper")?;

    Ok(state.monitoring.read().unwrap().clone())
}

pub fn stop_process_monitoring_inner(
    app: &AppHandle,
    state: SharedState,
) -> Result<MonitoringState> {
    if let Some(session) = state.helper.lock().unwrap().take() {
        let mut writer = session.writer.lock().unwrap();
        let _ = serde_json::to_writer(&mut *writer, &HelperCommand::Shutdown);
        let _ = writer.write_all(b"\n");
        let _ = writer.flush();
    }

    clear_process_state(app, &state);
    let snapshot = update_monitoring_state(app, &state, |monitoring| {
        monitoring.process_details_enabled = false;
        monitoring.permission_state = PermissionState::Idle;
        monitoring.last_error = None;
    });

    Ok(snapshot)
}

fn accept_helper(
    app: &AppHandle,
    state: &SharedState,
    listener: interprocess::local_socket::Listener,
    token: &str,
) -> Result<()> {
    let connection = listener.accept().context("helper failed to connect to pipe")?;
    let mut reader = BufReader::new(connection.try_clone().context("failed to clone helper stream")?);
    let mut hello = String::new();
    reader.read_line(&mut hello).context("failed to read helper handshake")?;

    let message: HelperMessage = serde_json::from_str(hello.trim()).context("invalid helper handshake")?;
    match message {
        HelperMessage::Hello { token: candidate } if candidate == token => {}
        _ => bail!("helper handshake token mismatch"),
    }

    let writer = Arc::new(Mutex::new(connection));
    {
        let mut helper = state.helper.lock().unwrap();
        *helper = Some(HelperSession {
            writer: writer.clone(),
        });
    }

    update_monitoring_state(app, state, |monitoring| {
        monitoring.process_details_enabled = true;
        monitoring.permission_state = PermissionState::Granted;
        monitoring.last_error = None;
    });

    let app_handle = app.clone();
    let state_for_reader = state.clone();
    thread::spawn(move || {
        let mut reader = reader;
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    break;
                }
                Ok(_) => match serde_json::from_str::<HelperMessage>(line.trim()) {
                    Ok(HelperMessage::Processes { processes, .. }) => {
                        update_process_state(&app_handle, &state_for_reader, processes);
                    }
                    Ok(HelperMessage::Error { message }) => {
                        update_monitoring_state(
                            &app_handle,
                            &state_for_reader,
                            |monitoring: &mut MonitoringState| {
                            monitoring.process_details_enabled = false;
                            monitoring.permission_state = PermissionState::Error;
                            monitoring.last_error = Some(message.clone());
                            },
                        );
                    }
                    Ok(HelperMessage::Hello { .. }) => {}
                    Err(error) => {
                        update_monitoring_state(
                            &app_handle,
                            &state_for_reader,
                            |monitoring: &mut MonitoringState| {
                                monitoring.process_details_enabled = false;
                                monitoring.permission_state = PermissionState::Error;
                                monitoring.last_error = Some(error.to_string());
                            },
                        );
                    }
                },
                Err(error) => {
                    update_monitoring_state(
                        &app_handle,
                        &state_for_reader,
                        |monitoring: &mut MonitoringState| {
                            monitoring.process_details_enabled = false;
                            monitoring.permission_state = PermissionState::Error;
                            monitoring.last_error = Some(error.to_string());
                        },
                    );
                    break;
                }
            }
        }

        {
            let mut helper = state_for_reader.helper.lock().unwrap();
            *helper = None;
        }
        clear_process_state(&app_handle, &state_for_reader);
        let current = state_for_reader.monitoring.read().unwrap().clone();
        if current.permission_state == PermissionState::Granted {
            update_monitoring_state(&app_handle, &state_for_reader, |monitoring| {
                monitoring.process_details_enabled = false;
                monitoring.permission_state = PermissionState::Idle;
                monitoring.last_error = None;
            });
        }
    });

    Ok(())
}

fn spawn_elevated_helper(app: &AppHandle, pipe_name: &str, token: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        use windows::{
            core::PCWSTR,
            Win32::Foundation::HINSTANCE,
            Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_HIDE},
        };

        let exe = current_executable_path()?;
        let parameters = format!(
            "--process-helper --pipe \"{pipe}\" --token \"{token}\"",
            pipe = pipe_name,
            token = token
        );

        let exe_wide = encode_wide(exe.as_os_str());
        let params_wide = encode_wide(OsString::from(parameters).as_os_str());
        let operation = encode_wide("runas");

        let result: HINSTANCE = unsafe {
            ShellExecuteW(
                None,
                PCWSTR(operation.as_ptr()),
                PCWSTR(exe_wide.as_ptr()),
                PCWSTR(params_wide.as_ptr()),
                PCWSTR::null(),
                SW_HIDE,
            )
        };

        if result.0 as usize <= 32 {
            let permission_state = if result.0 as usize == 5 {
                PermissionState::Denied
            } else {
                PermissionState::Error
            };

            let managed_state = app.state::<SharedState>();
            update_monitoring_state(
                app,
                managed_state.inner(),
                |monitoring: &mut MonitoringState| {
                    monitoring.process_details_enabled = false;
                    monitoring.permission_state = permission_state;
                    monitoring.last_error = Some("The elevated helper was not started.".to_string());
                },
            );

            bail!("ShellExecuteW failed with code {}", result.0 as usize);
        }

        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        let _ = pipe_name;
        let _ = token;
        bail!("elevated process monitoring is only supported on Windows")
    }
}

#[cfg(target_os = "windows")]
fn encode_wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    value.as_ref().encode_wide().chain(Some(0)).collect()
}

fn current_executable_path() -> Result<PathBuf> {
    std::env::current_exe().context("failed to resolve current executable path")
}

#[derive(Debug, Clone)]
struct HelperConfig {
    pipe_name: String,
    token: String,
}

fn helper_args(args: impl IntoIterator<Item = OsString>) -> Result<Option<HelperConfig>> {
    let values = args
        .into_iter()
        .map(|value| value.to_string_lossy().to_string())
        .collect::<Vec<_>>();

    if !values.iter().any(|value| value == "--process-helper") {
        return Ok(None);
    }

    let mut pipe_name = None;
    let mut token = None;
    let mut index = 0usize;

    while index < values.len() {
        match values[index].as_str() {
            "--pipe" => {
                pipe_name = values.get(index + 1).cloned();
                index += 2;
            }
            "--token" => {
                token = values.get(index + 1).cloned();
                index += 2;
            }
            _ => index += 1,
        }
    }

    Ok(Some(HelperConfig {
        pipe_name: pipe_name.context("missing --pipe argument for process helper")?,
        token: token.context("missing --token argument for process helper")?,
    }))
}

fn run_helper(config: HelperConfig) -> Result<()> {
    let socket_name = config
        .pipe_name
        .to_ns_name::<GenericNamespaced>()
        .context("failed to build helper connection name")?;
    let connection = Stream::connect(socket_name).context("failed to connect to netmonitor pipe")?;
    let reader_stream = connection.try_clone().context("failed to clone helper connection")?;

    let mut writer = connection;
    write_helper_message(
        &mut writer,
        &HelperMessage::Hello {
            token: config.token,
        },
    )?;

    let process_names = Arc::new(Mutex::new(load_process_names()));
    let counters = Arc::new(Mutex::new(HashMap::<u32, ProcessBucket>::new()));
    let running = Arc::new(AtomicBool::new(true));

    let running_for_reader = running.clone();
    let reader_handle = thread::spawn(move || -> Result<()> {
        let mut reader = BufReader::new(reader_stream);
        let mut line = String::new();

        while running_for_reader.load(Ordering::Relaxed) {
            line.clear();
            let read = reader.read_line(&mut line).context("failed to read helper command")?;
            if read == 0 {
                running_for_reader.store(false, Ordering::Relaxed);
                break;
            }

            let command: HelperCommand =
                serde_json::from_str(line.trim()).context("invalid helper command")?;
            if matches!(command, HelperCommand::Shutdown) {
                running_for_reader.store(false, Ordering::Relaxed);
            }
        }

        Ok(())
    });

    let tcp_provider = build_tcp_provider(process_names.clone(), counters.clone());
    let process_provider = build_process_provider(process_names.clone());
    let kernel_trace = KernelTrace::new()
        .named("netmonitor-helper".to_string())
        .enable(tcp_provider)
        .enable(process_provider)
        .start_and_process()
        .map_err(|error| anyhow::anyhow!("failed to start ETW kernel trace: {error:?}"))?;

    while running.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_secs(1));
        let snapshots = drain_process_buckets(&counters, &process_names);
        write_helper_message(
            &mut writer,
            &HelperMessage::Processes {
                sampled_at: now_epoch_ms(),
                processes: snapshots,
            },
        )?;
    }

    let _ = kernel_trace.stop();
    let _ = reader_handle.join();

    Ok(())
}

fn build_tcp_provider(
    process_names: Arc<Mutex<HashMap<u32, String>>>,
    counters: Arc<Mutex<HashMap<u32, ProcessBucket>>>,
) -> Provider {
    let callback = move |record: &EventRecord, locator: &SchemaLocator| {
        let opcode = record.opcode();
        if opcode != 10 && opcode != 11 {
            return;
        }

        let Ok(schema) = locator.event_schema(record) else {
            return;
        };
        let parser = Parser::create(record, &schema);
        let Ok(pid) = parser.try_parse::<u32>("PID") else {
            return;
        };
        let Ok(size) = parser.try_parse::<u32>("size") else {
            return;
        };

        if pid == 0 {
            return;
        }

        let mut map = counters.lock().unwrap();
        let bucket = map.entry(pid).or_default();
        if opcode == 10 {
            bucket.up_bytes += size as u64;
        } else {
            bucket.down_bytes += size as u64;
        }

        if bucket.image_name.is_empty() {
            if let Some(name) = process_names.lock().unwrap().get(&pid).cloned() {
                bucket.image_name = name;
            }
        }
    };

    Provider::kernel(&kernel_providers::TCP_IP_PROVIDER)
        .add_callback(callback)
        .build()
}

fn build_process_provider(process_names: Arc<Mutex<HashMap<u32, String>>>) -> Provider {
    let callback = move |record: &EventRecord, locator: &SchemaLocator| {
        let opcode = record.opcode();
        let Ok(schema) = locator.event_schema(record) else {
            return;
        };
        let parser = Parser::create(record, &schema);
        let Ok(pid) = parser.try_parse::<u32>("ProcessId") else {
            return;
        };

        match opcode {
            1 | 3 => {
                if let Ok(image_name) = parser.try_parse::<String>("ImageFileName") {
                    process_names.lock().unwrap().insert(pid, image_name);
                }
            }
            2 | 4 => {
                process_names.lock().unwrap().remove(&pid);
            }
            _ => {}
        }
    };

    Provider::kernel(&kernel_providers::PROCESS_PROVIDER)
        .add_callback(callback)
        .build()
}

#[derive(Debug, Default)]
struct ProcessBucket {
    down_bytes: u64,
    up_bytes: u64,
    image_name: String,
}

fn drain_process_buckets(
    counters: &Arc<Mutex<HashMap<u32, ProcessBucket>>>,
    process_names: &Arc<Mutex<HashMap<u32, String>>>,
) -> Vec<ProcessSnapshot> {
    let mut buckets = counters.lock().unwrap();
    let known_names = process_names.lock().unwrap().clone();

    let mut snapshots = buckets
        .drain()
        .filter_map(|(pid, bucket)| {
            if bucket.down_bytes == 0 && bucket.up_bytes == 0 {
                return None;
            }

            let image_name = if bucket.image_name.is_empty() {
                known_names
                    .get(&pid)
                    .cloned()
                    .unwrap_or_else(|| format!("pid-{pid}"))
            } else {
                bucket.image_name
            };

            Some(ProcessSnapshot {
                pid,
                display_name: display_name_from_image(&image_name),
                image_name,
                down_bps: bucket.down_bytes * 8,
                up_bps: bucket.up_bytes * 8,
            })
        })
        .collect::<Vec<_>>();

    snapshots.sort_by(|left, right| {
        right
            .down_bps
            .cmp(&left.down_bps)
            .then(right.up_bps.cmp(&left.up_bps))
            .then(left.display_name.cmp(&right.display_name))
    });

    snapshots
}

fn load_process_names() -> HashMap<u32, String> {
    let mut system = System::new_all();
    system.refresh_processes(ProcessesToUpdate::All, true);

    system
        .processes()
        .iter()
        .map(|(pid, process)| {
            let image_name = process
                .exe()
                .map(|path| path.to_string_lossy().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| process.name().to_string_lossy().to_string());
            (pid.as_u32(), image_name)
        })
        .collect()
}

fn display_name_from_image(image_name: &str) -> String {
    PathBuf::from(image_name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| image_name.to_string())
}

fn write_helper_message(writer: &mut Stream, message: &HelperMessage) -> Result<()> {
    serde_json::to_writer(&mut *writer, message).context("failed to serialize helper message")?;
    writer.write_all(b"\n").context("failed to write helper newline")?;
    writer.flush().context("failed to flush helper stream")?;
    Ok(())
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}
