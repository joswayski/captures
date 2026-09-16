use serde_json::Value;
use std::{fs, path::PathBuf};
use tauri::{State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

struct Input {
    config: Value,
    output: PathBuf,
}

#[tauri::command]
fn configuration(input: State<'_, Input>) -> Value {
    input.config.clone()
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
        "result" => "",
        "error" => ".error.json",
        _ => return Err("unknown marker".into()),
    };
    value["pid"] = std::process::id().into();
    value["window_id"] = window_id(&window)?.into();
    let target = PathBuf::from(format!("{}{suffix}", input.output.display()));
    let temporary = target.with_extension("tmp");
    fs::write(&temporary, serde_json::to_vec(&value).unwrap()).map_err(|e| e.to_string())?;
    fs::rename(temporary, target).map_err(|e| e.to_string())
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
        .invoke_handler(tauri::generate_handler![configuration, record])
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
