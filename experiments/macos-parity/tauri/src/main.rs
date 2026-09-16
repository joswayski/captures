use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tauri::{State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

struct Input {
    config: Value,
    output: PathBuf,
}

#[tauri::command]
fn configuration(input: State<'_, Input>) -> Value {
    input.config.clone()
}

#[tauri::command]
async fn wait_for_start(input: State<'_, Input>) -> Result<(), String> {
    let Some(path) = input.config["startGatePath"].as_str().map(PathBuf::from) else {
        return Ok(());
    };
    if !path.is_absolute() {
        return Err("startGatePath must be absolute".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_secs(90);
        while !path.exists() {
            if Instant::now() >= deadline {
                return Err("start gate timed out".to_string());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(target_os = "macos")]
fn host_time_ns() -> Result<u64, String> {
    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }
    let mut info = Timebase { numer: 0, denom: 0 };
    // System clock conversion, shared with the WindowServer observer.
    unsafe {
        if mach_timebase_info(&mut info) != 0 || info.denom == 0 {
            return Err("Mach timebase unavailable".into());
        }
        Ok(
            (u128::from(mach_absolute_time()) * u128::from(info.numer) / u128::from(info.denom))
                as u64,
        )
    }
}

#[cfg(not(target_os = "macos"))]
fn host_time_ns() -> Result<u64, String> {
    Err("profiling requires macOS".into())
}

fn write_marker(output: &Path, suffix: &str, value: &Value) -> Result<(), String> {
    let target = PathBuf::from(format!("{}{suffix}", output.display()));
    let temporary = target.with_extension("tmp");
    fs::write(&temporary, serde_json::to_vec(value).unwrap()).map_err(|e| e.to_string())?;
    fs::rename(temporary, target).map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
fn write_profile_ready(
    window: &WebviewWindow,
    output: PathBuf,
    mut value: Value,
) -> Result<(), String> {
    // Benchmark-only WebKit SPI, never used by the shipping app. Run all ObjC
    // access on the WebView thread and fail closed if this OS lacks the SPI.
    window
        .with_webview(move |platform| {
            use objc2::{
                msg_send,
                runtime::{AnyObject, Bool},
                sel,
            };
            let result = (|| -> Result<Value, String> {
                let view = platform.inner().cast::<AnyObject>();
                unsafe {
                    let web: Bool =
                        msg_send![view, respondsToSelector: sel!(_webProcessIdentifier)];
                    let gpu: Bool =
                        msg_send![view, respondsToSelector: sel!(_gpuProcessIdentifier)];
                    if !web.as_bool() || !gpu.as_bool() {
                        return Err("WebKit process PID SPI unavailable".into());
                    }
                    let configuration: *mut AnyObject = msg_send![view, configuration];
                    let store: *mut AnyObject = msg_send![configuration, websiteDataStore];
                    let network: Bool =
                        msg_send![store, respondsToSelector: sel!(_networkProcessIdentifier)];
                    if !network.as_bool() {
                        return Err("WebKit network PID SPI unavailable".into());
                    }
                    let web_pid: i32 = msg_send![view, _webProcessIdentifier];
                    let gpu_pid: i32 = msg_send![view, _gpuProcessIdentifier];
                    let network_pid: i32 = msg_send![store, _networkProcessIdentifier];
                    if web_pid <= 0 || gpu_pid <= 0 || network_pid <= 0 {
                        return Err(
                            "WebKit returned an unstarted helper; refusing partial accounting"
                                .into(),
                        );
                    }
                    Ok(serde_json::json!([
                        {"role":"webContent", "pid":web_pid},
                        {"role":"gpu", "pid":gpu_pid},
                        {"role":"network", "pid":network_pid}
                    ]))
                }
            })();
            let written = match result {
                Ok(pids) => {
                    value["webkitProcesses"] = pids;
                    write_marker(&output, ".ready.json", &value)
                }
                Err(error) => {
                    write_marker(&output, ".error.json", &serde_json::json!({"error":error}))
                }
            };
            if let Err(error) = written {
                eprintln!("profile readiness: {error}");
                std::process::exit(1);
            }
        })
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
fn window_id(window: &WebviewWindow) -> Result<isize, String> {
    let pointer = window.ns_window().map_err(|e| e.to_string())?;
    // Tauri owns this live NSWindow; its windowNumber getter is read-only.
    let id = unsafe { objc2::msg_send![pointer.cast::<objc2::runtime::AnyObject>(), windowNumber] };
    Ok(id)
}

#[cfg(not(target_os = "macos"))]
fn window_id(_window: &WebviewWindow) -> Result<isize, String> {
    Ok(0) // Linux compilation is useful, but the runner only accepts macOS.
}

#[tauri::command]
fn record(
    window: WebviewWindow,
    input: State<'_, Input>,
    kind: String,
    mut value: Value,
) -> Result<(), String> {
    let suffix = match kind.as_str() {
        "ready" => ".ready.json",
        "start" => ".started.json",
        "result" => "",
        "error" => ".error.json",
        _ => return Err("unknown marker".into()),
    };
    value["pid"] = std::process::id().into();
    value["window_id"] = window_id(&window)?.into();
    value["measurementProtocol"] = 2.into();
    if kind == "start" {
        value["startHostTimeNs"] = host_time_ns()?.into();
    }
    #[cfg(target_os = "macos")]
    if kind == "ready" && input.config["profileResources"] == true {
        return write_profile_ready(&window, input.output.clone(), value);
    }
    write_marker(&input.output, suffix, &value)
}

fn main() {
    let arguments: Vec<_> = std::env::args_os().collect();
    assert_eq!(
        arguments.len(),
        3,
        "usage: captures-parity-tauri CONFIG_JSON OUTPUT_JSON"
    );
    let config: Value = serde_json::from_slice(&fs::read(&arguments[1]).expect("config file"))
        .expect("config JSON");
    assert_eq!(config["schema"], 1);
    let input = Input {
        config,
        output: arguments[2].clone().into(),
    };
    tauri::Builder::default()
        .manage(input)
        .invoke_handler(tauri::generate_handler![
            configuration,
            record,
            wait_for_start
        ])
        .setup(|app| {
            let window =
                WebviewWindowBuilder::new(app, "fixture", WebviewUrl::App("index.html".into()))
                    .title("Captures Effect Comparison")
                    .inner_size(640.0, 720.0)
                    .decorations(false)
                    .shadow(false)
                    .resizable(false)
                    .center()
                    .build()?;
            window.set_focus()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Tauri fixture failed");
}
