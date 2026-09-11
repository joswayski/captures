fn main() {
    #[cfg(feature = "tauri-probe")]
    tauri_build::build();
}
