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
    trace::{stop_trace_by_name, KernelTrace},
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

const PROCESS_TRACE_NAME: &str = "netmonitor-process-monitor";

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
    state
        .process_monitoring_prompted
        .store(true, Ordering::Relaxed);

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
        monitoring.process_details_enabled = false;
        monitoring.permission_state = PermissionState::Pending;
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
                    Ok(HelperMessage::Ready) => {
                        update_monitoring_state(
                            &app_handle,
                            &state_for_reader,
                            |monitoring: &mut MonitoringState| {
                                monitoring.process_details_enabled = true;
                                monitoring.permission_state = PermissionState::Granted;
                                monitoring.last_error = None;
                            },
                        );
                    }
                    Ok(HelperMessage::Processes { processes, .. }) => {
                        {
                            let current = state_for_reader.monitoring.read().unwrap().clone();
                            if current.permission_state == PermissionState::Pending {
                                update_monitoring_state(
                                    &app_handle,
                                    &state_for_reader,
                                    |monitoring: &mut MonitoringState| {
                                        monitoring.process_details_enabled = true;
                                        monitoring.permission_state = PermissionState::Granted;
                                        monitoring.last_error = None;
                                    },
                                );
                            }
                        }
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
                monitoring.permission_state = PermissionState::Error;
                monitoring.last_error = Some("进程监控 helper 已退出".to_string());
            });
        } else if current.permission_state == PermissionState::Pending {
            update_monitoring_state(&app_handle, &state_for_reader, |monitoring| {
                monitoring.process_details_enabled = false;
                monitoring.permission_state = PermissionState::Error;
                monitoring.last_error = Some("进程监控未成功启动".to_string());
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

    #[cfg(target_os = "windows")]
    let _winrt_apartment = WinRtApartment::initialize();

    let process_names = Arc::new(Mutex::new(load_process_names()));
    let display_names = Arc::new(Mutex::new(HashMap::<u32, String>::new()));
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

    let kernel_trace =
        start_process_trace(process_names.clone(), counters.clone(), display_names.clone());

    let kernel_trace = match kernel_trace {
        Ok(trace) => trace,
        Err(error) => {
            let _ = write_helper_message(
                &mut writer,
                &HelperMessage::Error {
                    message: format!("ETW 启动失败: {error}"),
                },
            );
            return Err(error);
        }
    };

    write_helper_message(&mut writer, &HelperMessage::Ready)?;

    while running.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_secs(3));
        let snapshots = drain_process_buckets(&counters, &process_names, &display_names);
        if let Err(error) = write_helper_message(
            &mut writer,
            &HelperMessage::Processes {
                sampled_at: now_epoch_ms(),
                processes: snapshots,
            },
        ) {
            let _ = write_helper_message(
                &mut writer,
                &HelperMessage::Error {
                    message: format!("发送进程监控数据失败: {error}"),
                },
            );
            return Err(error);
        }
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

fn start_process_trace(
    process_names: Arc<Mutex<HashMap<u32, String>>>,
    counters: Arc<Mutex<HashMap<u32, ProcessBucket>>>,
    display_names: Arc<Mutex<HashMap<u32, String>>>,
) -> Result<KernelTrace> {
    match build_process_trace(process_names.clone(), counters.clone(), display_names.clone()) {
        Ok(trace) => Ok(trace),
        Err(initial_error) => {
            let _ = stop_trace_by_name(PROCESS_TRACE_NAME);
            build_process_trace(process_names, counters, display_names).map_err(|retry_error| {
                anyhow::anyhow!(
                    "failed to start ETW kernel trace after resetting stale session: {retry_error:?}; initial error: {initial_error:?}"
                )
            })
        }
    }
}

fn build_process_trace(
    process_names: Arc<Mutex<HashMap<u32, String>>>,
    counters: Arc<Mutex<HashMap<u32, ProcessBucket>>>,
    display_names: Arc<Mutex<HashMap<u32, String>>>,
) -> std::result::Result<KernelTrace, ferrisetw::trace::TraceError> {
    let tcp_provider = build_tcp_provider(process_names.clone(), counters);
    let process_provider = build_process_provider(process_names, display_names);

    KernelTrace::new()
        .named(PROCESS_TRACE_NAME.to_string())
        .enable(tcp_provider)
        .enable(process_provider)
        .start_and_process()
}

fn build_process_provider(
    process_names: Arc<Mutex<HashMap<u32, String>>>,
    display_names: Arc<Mutex<HashMap<u32, String>>>,
) -> Provider {
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
                    let mut names = process_names.lock().unwrap();
                    match names.get_mut(&pid) {
                        Some(current) if looks_like_path(current) && !looks_like_path(&image_name) => {}
                        Some(current) => *current = image_name,
                        None => {
                            names.insert(pid, image_name);
                        }
                    }
                }
            }
            2 | 4 => {
                process_names.lock().unwrap().remove(&pid);
                display_names.lock().unwrap().remove(&pid);
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
    display_names: &Arc<Mutex<HashMap<u32, String>>>,
) -> Vec<ProcessSnapshot> {
    let mut buckets = counters.lock().unwrap();
    let known_names = process_names.lock().unwrap().clone();
    let mut display_name_cache = display_names.lock().unwrap();

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
                display_name: display_name_cache
                    .entry(pid)
                    .or_insert_with(|| display_name_for_process(pid, &image_name))
                    .clone(),
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

fn display_name_for_process(pid: u32, image_name: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        if let Some(friendly_name) = resolve_packaged_app_name(pid) {
            return friendly_name;
        }

        if let Some(process_path) = process_path_from_pid(pid) {
            if let Some(friendly_name) = file_description_from_path(&process_path) {
                return friendly_name;
            }
        }

        if let Some(friendly_name) = file_description_from_path(image_name) {
            return friendly_name;
        }
    }

    friendly_name_fallback(&display_stem_from_image(image_name))
}

fn display_stem_from_image(image_name: &str) -> String {
    PathBuf::from(image_name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| image_name.to_string())
}

fn looks_like_path(value: &str) -> bool {
    value.contains('\\') || value.contains('/') || value.contains(':')
}

fn friendly_name_fallback(name: &str) -> String {
    let normalized = name.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "msedge" => "Microsoft Edge".to_string(),
        "chrome" => "Google Chrome".to_string(),
        "code" => "Visual Studio Code".to_string(),
        "wechat" => "WeChat".to_string(),
        "qq" => "QQ".to_string(),
        "explorer" => "Windows Explorer".to_string(),
        "powershell" => "Windows PowerShell".to_string(),
        "pwsh" => "PowerShell".to_string(),
        "cmd" => "Command Prompt".to_string(),
        "spotify" => "Spotify".to_string(),
        _ => humanize_process_name(name),
    }
}

fn humanize_process_name(name: &str) -> String {
    let mut result = String::new();
    let mut previous_is_lower_or_digit = false;

    for character in name.chars() {
        let is_separator = matches!(character, '_' | '-' | '.');
        if is_separator {
            if !result.ends_with(' ') && !result.is_empty() {
                result.push(' ');
            }
            previous_is_lower_or_digit = false;
            continue;
        }

        let is_upper = character.is_ascii_uppercase();
        if is_upper && previous_is_lower_or_digit && !result.ends_with(' ') {
            result.push(' ');
        }

        result.push(character);
        previous_is_lower_or_digit = character.is_ascii_lowercase() || character.is_ascii_digit();
    }

    let result = result.split_whitespace().collect::<Vec<_>>().join(" ");
    if result.is_empty() {
        name.to_string()
    } else {
        result
    }
}

#[cfg(target_os = "windows")]
fn resolve_packaged_app_name(pid: u32) -> Option<String> {
    use windows::{
        core::{HSTRING, PWSTR},
        Management::Deployment::PackageManager,
        Win32::{
            Foundation::{APPMODEL_ERROR_NO_PACKAGE, ERROR_INSUFFICIENT_BUFFER},
            Storage::Packaging::Appx::GetPackageFamilyName,
            System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        },
    };

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()? };
    let _process = OwnedHandle(process);

    let mut length = 0u32;
    let status = unsafe { GetPackageFamilyName(process, &mut length, None) };
    if status == APPMODEL_ERROR_NO_PACKAGE || status != ERROR_INSUFFICIENT_BUFFER || length == 0 {
        return None;
    }

    let mut buffer = vec![0u16; length as usize];
    let status = unsafe {
        GetPackageFamilyName(process, &mut length, Some(PWSTR(buffer.as_mut_ptr())))
    };
    if status.0 != 0 {
        return None;
    }

    let end = buffer.iter().position(|value| *value == 0).unwrap_or(buffer.len());
    let package_family_name = String::from_utf16_lossy(&buffer[..end]).trim().to_string();
    if package_family_name.is_empty() {
        return None;
    }

    let package_manager = PackageManager::new().ok()?;
    let packages = package_manager
        .FindPackagesByPackageFamilyName(&HSTRING::from(package_family_name))
        .ok()?;

    for package in packages {
        let display_name = package.DisplayName().ok()?.to_string();
        let display_name = display_name.trim();
        if !display_name.is_empty() {
            return Some(display_name.to_string());
        }
    }

    None
}

#[cfg(target_os = "windows")]
fn process_path_from_pid(pid: u32) -> Option<String> {
    use windows::{
        core::PWSTR,
        Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    };

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()? };
    let _process = OwnedHandle(process);

    let mut buffer = vec![0u16; 32768];
    let mut length = buffer.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut length)
            .ok()?;
    }

    let text = String::from_utf16_lossy(&buffer[..length as usize]).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

#[cfg(target_os = "windows")]
fn file_description_from_path(image_name: &str) -> Option<String> {
    use std::{ffi::c_void, ptr::null_mut, slice};

    use windows::{
        core::PCWSTR,
        Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW},
    };

    let path = PathBuf::from(image_name);
    if !path.is_file() {
        return None;
    }

    let wide_path = encode_wide(path.as_os_str());
    let size = unsafe { GetFileVersionInfoSizeW(PCWSTR(wide_path.as_ptr()), None) };
    if size == 0 {
        return None;
    }

    let mut version_data = vec![0u8; size as usize];
    if unsafe {
        GetFileVersionInfoW(
            PCWSTR(wide_path.as_ptr()),
            None,
            size,
            version_data.as_mut_ptr() as *mut c_void,
        )
    }
    .is_err()
    {
        return None;
    }

    let mut translation_ptr: *mut c_void = null_mut();
    let mut translation_len = 0u32;
    let translation_key = encode_wide(r"\VarFileInfo\Translation");
    let translation = if unsafe {
        VerQueryValueW(
            version_data.as_ptr() as *const c_void,
            PCWSTR(translation_key.as_ptr()),
            &mut translation_ptr,
            &mut translation_len,
        )
    }
    .as_bool()
        && translation_len >= 4
    {
        let values = unsafe { slice::from_raw_parts(translation_ptr as *const u16, 2) };
        Some((values[0], values[1]))
    } else {
        None
    };

    let candidates = match translation {
        Some((language, code_page)) => vec![
            format!(r"\StringFileInfo\{language:04x}{code_page:04x}\FileDescription"),
            r"\StringFileInfo\040904b0\FileDescription".to_string(),
        ],
        None => vec![
            r"\StringFileInfo\040904b0\FileDescription".to_string(),
            r"\StringFileInfo\080404b0\FileDescription".to_string(),
        ],
    };

    for candidate in candidates {
        let mut value_ptr: *mut c_void = null_mut();
        let mut value_len = 0u32;
        let candidate_wide = encode_wide(candidate);

        let success = unsafe {
            VerQueryValueW(
                version_data.as_ptr() as *const c_void,
                PCWSTR(candidate_wide.as_ptr()),
                &mut value_ptr,
                &mut value_len,
            )
        }
        .as_bool();

        if !success || value_ptr.is_null() || value_len == 0 {
            continue;
        }

        let raw = unsafe { slice::from_raw_parts(value_ptr as *const u16, value_len as usize) };
        let end = raw.iter().position(|value| *value == 0).unwrap_or(raw.len());
        let text = String::from_utf16_lossy(&raw[..end]).trim().to_string();
        if !text.is_empty() {
            return Some(text);
        }
    }

    None
}

#[cfg(target_os = "windows")]
struct OwnedHandle(windows::Win32::Foundation::HANDLE);

#[cfg(target_os = "windows")]
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.0) };
    }
}

#[cfg(target_os = "windows")]
struct WinRtApartment {
    initialized: bool,
}

#[cfg(target_os = "windows")]
impl WinRtApartment {
    fn initialize() -> Self {
        use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

        let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_ok();
        Self { initialized }
    }
}

#[cfg(target_os = "windows")]
impl Drop for WinRtApartment {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { windows::Win32::System::WinRT::RoUninitialize() };
        }
    }
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
