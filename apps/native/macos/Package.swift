// swift-tools-version: 5.9
import PackageDescription
import Foundation

let packageRoot = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
let libraryPath = ProcessInfo.processInfo.environment["CAPTURES_NATIVE_LIB_DIR"]
    ?? packageRoot.appendingPathComponent("../../../target/release").standardizedFileURL.path

let package = Package(
    name: "CapturesNative",
    platforms: [.macOS(.v13)],
    products: [.executable(name: "CapturesNative", targets: ["CapturesNative"])],
    targets: [
        .systemLibrary(name: "CCapturesSettings", path: "Sources/CCapturesSettings"),
        .executableTarget(
            name: "CapturesNative",
            dependencies: ["CCapturesSettings"],
            resources: [.process("Resources")],
            linkerSettings: [
                .unsafeFlags(["-L", libraryPath]),
                .linkedLibrary("captures_settings_ffi"),
                .linkedFramework("AppKit"),
                .linkedFramework("CoreGraphics"),
                .linkedFramework("Security"),
                .linkedFramework("Foundation"),
                .linkedFramework("CoreFoundation"),
                .linkedFramework("IOSurface"),
                .linkedFramework("CoreVideo"),
            ]
        ),
        .testTarget(name: "CapturesNativeTests", dependencies: ["CapturesNative"], resources: [.process("Resources")]),
    ]
)
