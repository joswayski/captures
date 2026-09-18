// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "CapturesNative",
    platforms: [.macOS(.v13)],
    products: [.executable(name: "CapturesNative", targets: ["CapturesNative"])],
    targets: [
        .executableTarget(name: "CapturesNative", resources: [.process("Resources")]),
        .testTarget(name: "CapturesNativeTests", dependencies: ["CapturesNative"], resources: [.process("Resources")]),
    ]
)
