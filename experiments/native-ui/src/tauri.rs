use base64::Engine;
use captures_ui_probe::CaptureStore;
use serde::Serialize;

#[derive(Serialize)]
struct Preview {
    url: String,
    width: u32,
    height: u32,
    capture_encode_ms: f64,
}

#[tauri::command]
async fn capture(store: tauri::State<'_, std::sync::Arc<CaptureStore>>) -> Result<Preview, String> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let frame = store.capture()?;
        Ok(Preview {
            url: format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(frame.png)
            ),
            width: frame.width,
            height: frame.height,
            capture_encode_ms: frame.capture_encode_ms,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn save(store: tauri::State<'_, std::sync::Arc<CaptureStore>>) -> Result<String, String> {
    store.save()
}

#[tauri::command]
fn ready() -> Result<(), String> {
    captures_ui_probe::ready()
}

fn main() {
    tauri::Builder::default()
        .manage(std::sync::Arc::new(CaptureStore::default()))
        .invoke_handler(tauri::generate_handler![capture, save, ready])
        .run(tauri::generate_context!())
        .expect("Tauri probe failed");
}
