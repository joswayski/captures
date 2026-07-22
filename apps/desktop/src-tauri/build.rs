fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let output = std::process::Command::new("xcrun")
            .args(["--sdk", "macosx", "--show-sdk-path"])
            .output()
            .expect("xcrun must locate the macOS SDK for the recorder");
        assert!(
            output.status.success(),
            "xcrun could not locate the macOS SDK"
        );
        let sdk = String::from_utf8(output.stdout)
            .expect("macOS SDK path must be UTF-8")
            .trim()
            .to_owned();
        println!("cargo:rustc-link-search=native={}/usr/lib/swift", sdk);
        println!("cargo:rustc-link-lib=dylib=swift_Concurrency");
    }
    tauri_build::build();
}
