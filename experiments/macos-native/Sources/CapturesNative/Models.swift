import Foundation

struct NativeFailure: LocalizedError {
    let message: String
    var errorDescription: String? { message }
    init(_ message: String) { self.message = message }
}

struct Artifact: Identifiable, Codable, Equatable {
    var id = UUID()
    var path: String
    var kind: String
    var width: Int
    var height: Int
    var createdAt = Date()
    var url: URL { URL(fileURLWithPath: path) }

    init(path: String, kind: String, width: Int = 0, height: Int = 0) {
        self.path = path; self.kind = kind; self.width = width; self.height = height
    }

    init(response: [String: Any]) throws {
        guard let path = response["path"] as? String, path.hasPrefix("/"),
              let kind = response["kind"] as? String,
              ["image", "video", "gif"].contains(kind) else {
            throw NativeFailure("The capture engine returned an invalid artifact.")
        }
        self.init(path: path, kind: kind, width: response["width"] as? Int ?? 0,
                  height: response["height"] as? Int ?? 0)
    }
}

struct NativeSettings: Codable, Equatable {
    var appearance = "system"
    var theme = "mustard"
    var accentHex = "#ffca28"
    var signalHex = "#ef4650"
    var outputDirectory = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Pictures/Captures Native Experiment").path
    var screenshotFormat = "png"
    var previewPlacement = "bottom-left"
    var autoCopy = true
    var showPreviews = true
    var freezeScreen = true
    var showCursor = true
    var autoStart = false
    var screenshotCountdown = 0
    var recordingCountdown = 3
    var videoFPS = 60
    var gifFPS = 15
    var gifWidth = 800
    var gifColors = 256
    var maxResolution = "original"
    var systemAudio = false
    var microphoneID = ""
    var microphoneMuted = false
    var excludeControls = true
    var openEditorAfterRecording = true
    // Carbon key codes and modifier masks. The experiment never modifies the
    // user's macOS Screenshot shortcuts; conflicts are reported instead.
    var shortcuts: [Shortcut] = Shortcut.defaults
}

struct Shortcut: Codable, Equatable, Identifiable {
    var id: String
    var title: String
    var keyCode: UInt32
    var modifiers: UInt32
    static let defaults = [
        Shortcut(id: "capture", title: "New capture", keyCode: 49, modifiers: 768),
        Shortcut(id: "region", title: "Capture region", keyCode: 21, modifiers: 768),
        Shortcut(id: "window", title: "Capture window", keyCode: 13, modifiers: 768),
        Shortcut(id: "display", title: "Capture display", keyCode: 20, modifiers: 768),
        Shortcut(id: "record", title: "Record region", keyCode: 23, modifiers: 768),
        Shortcut(id: "record-window", title: "Record window", keyCode: 13, modifiers: 2816),
        Shortcut(id: "record-display", title: "Record display", keyCode: 20, modifiers: 2816),
    ]
}

enum NativeStorage {
    static var directory: URL {
        if let override = ProcessInfo.processInfo.environment["CAPTURES_NATIVE_DATA"], !override.isEmpty {
            return URL(fileURLWithPath: override, isDirectory: true)
        }
        return FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/Captures Native Experiment", isDirectory: true)
    }

    static func write<T: Encodable>(_ value: T, to url: URL) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                                               withIntermediateDirectories: true,
                                               attributes: [.posixPermissions: 0o700])
        let data = try JSONEncoder().encode(value)
        try data.write(to: url, options: .atomic)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }

    static func read<T: Decodable>(_ type: T.Type, from url: URL) throws -> T? {
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        return try JSONDecoder().decode(type, from: Data(contentsOf: url))
    }

    static func recent(_ artifacts: [Artifact], now: Date = Date()) -> [Artifact] {
        let cutoff = now.addingTimeInterval(-30 * 24 * 60 * 60)
        return artifacts.filter { $0.createdAt >= cutoff }.sorted { $0.createdAt > $1.createdAt }
    }
}
