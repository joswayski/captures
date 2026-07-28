use std::{env, path::PathBuf, process::Command};

fn command_output(program: &str, arguments: &[&str]) -> String {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .unwrap_or_else(|error| panic!("failed to run {program}: {error}"));
    assert!(
        output.status.success(),
        "{program} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("tool output must be UTF-8")
        .trim()
        .to_owned()
}

fn main() {
    println!("cargo:rerun-if-changed=swift/MediaWriter.swift");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let output_directory = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    let source = PathBuf::from("swift/MediaWriter.swift");
    let object = output_directory.join("captures-media-writer.o");
    let library = output_directory.join("libcaptures_media_writer.a");
    let architecture = env::var("CARGO_CFG_TARGET_ARCH").expect("target architecture is set");
    let target = match architecture.as_str() {
        "aarch64" => "arm64-apple-macosx13.0",
        "x86_64" => "x86_64-apple-macosx13.0",
        unsupported => panic!("unsupported macOS recording architecture: {unsupported}"),
    };
    let swiftc = command_output("xcrun", &["--find", "swiftc"]);
    let default_sdk = command_output("xcrun", &["--sdk", "macosx", "--show-sdk-path"]);
    let versioned_sdk = PathBuf::from(&default_sdk)
        .parent()
        .map(|directory| directory.join("MacOSX15.sdk"));
    // Beta Command Line Tools can temporarily point MacOSX.sdk at a newer
    // SDK build than their bundled Swift compiler supports. Prefer the stable
    // macOS 15 SDK link when it is present; CI/Xcode installs without that
    // link continue using xcrun's active SDK.
    let sdk = versioned_sdk
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from(default_sdk));
    let module_cache = output_directory.join("swift-module-cache");
    std::fs::create_dir_all(&module_cache).expect("Swift module cache can be created");
    let status = Command::new(&swiftc)
        .args([
            "-parse-as-library",
            "-O",
            "-whole-module-optimization",
            "-target",
            target,
            "-sdk",
            sdk.to_str().expect("SDK path is Unicode"),
            "-emit-object",
        ])
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .env("CLANG_MODULE_CACHE_PATH", &module_cache)
        .env("SWIFTPM_MODULECACHE_OVERRIDE", &module_cache)
        .status()
        .expect("failed to run swiftc");
    assert!(status.success(), "failed to compile the macOS media writer");

    let status = Command::new("ar")
        .arg("rcs")
        .arg(&library)
        .arg(&object)
        .status()
        .expect("failed to archive the macOS media writer");
    assert!(status.success(), "failed to archive the macOS media writer");

    println!(
        "cargo:rustc-link-search=native={}",
        output_directory.display()
    );
    println!(
        "cargo:rustc-link-search=native={}",
        sdk.join("usr/lib/swift").display()
    );
    let swift_compatibility_runtime = PathBuf::from(&swiftc)
        .parent()
        .and_then(|directory| directory.parent())
        .expect("swiftc must be inside a toolchain")
        .join("lib/swift/macosx");
    if swift_compatibility_runtime.exists() {
        // The SDK stub above must remain first so libswift_Concurrency keeps
        // its system install name. This later path supplies the static Swift
        // compatibility libraries required for a macOS 13 target.
        println!(
            "cargo:rustc-link-search=native={}",
            swift_compatibility_runtime.display()
        );
    }
    // Link the system Swift concurrency library through the SDK stub. This
    // gives the executable the stable /usr/lib install name available on the
    // macOS 13 deployment target instead of an @rpath toolchain dependency.
    println!("cargo:rustc-link-lib=dylib=swift_Concurrency");
    println!("cargo:rustc-link-lib=static=captures_media_writer");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=AVFoundation");
    println!("cargo:rustc-link-lib=framework=CoreGraphics");
    println!("cargo:rustc-link-lib=framework=CoreMedia");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=VideoToolbox");
}
