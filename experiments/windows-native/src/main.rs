#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod windows;

#[cfg(windows)]
fn main() {
    if let Err(error) = windows::run() {
        windows::fatal(&error);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("captures-windows-native runs only on Windows; use `cargo test` for portable tests");
}
