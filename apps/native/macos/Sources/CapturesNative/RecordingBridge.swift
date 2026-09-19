import Foundation
import CCapturesSettings

struct NativeRecordingCapabilities: Equatable {
    let systemAudio: Bool
    let microphone: Bool
    let cursorControl: Bool
    let clickHighlights: Bool
    let controlsExcluded: Bool

    init?(_ value: [String: Any]) {
        guard let systemAudio = value["system_audio"] as? Bool,
              let microphone = value["microphone"] as? Bool,
              let cursorControl = value["cursor_control"] as? Bool,
              let clickHighlights = value["click_highlights"] as? Bool,
              let controlsExcluded = value["controls_excluded"] as? Bool
        else { return nil }
        self.systemAudio = systemAudio; self.microphone = microphone
        self.cursorControl = cursorControl; self.clickHighlights = clickHighlights
        self.controlsExcluded = controlsExcluded
    }
}

struct NativeMicrophoneDevice: Equatable {
    let id: String
    let name: String
    let isDefault: Bool

    init?(_ value: [String: Any]) {
        guard let id = value["id"] as? String,
              let name = value["name"] as? String,
              let isDefault = value["is_default"] as? Bool else { return nil }
        self.id = id; self.name = name; self.isDefault = isDefault
    }
}

struct NativeRecordingSnapshot: Equatable {
    let id: String
    let state: String
    let elapsedMilliseconds: UInt64
    let warning: String?

    init?(_ value: [String: Any]) {
        guard let id = value["id"] as? String,
              let state = value["state"] as? String,
              let elapsed = value["elapsed_ms"] as? NSNumber else { return nil }
        self.id = id; self.state = state; elapsedMilliseconds = elapsed.uint64Value
        warning = value["warning"] as? String
    }
}

struct NativeFinalizedRecording {
    let id: String
    let path: String
    let warning: String?

    init?(_ value: [String: Any]) {
        guard let entry = value["entry"] as? [String: Any],
              let id = entry["id"] as? String,
              let path = value["path"] as? String else { return nil }
        self.id = id; self.path = path; warning = value["warning"] as? String
    }
}

final class RecordingGenerationGate {
    private let lock = NSLock()
    private var generation: UInt64?

    func set(_ generation: UInt64?) { lock.withLock { self.generation = generation } }
    func accepts(_ value: UInt64) -> Bool { lock.withLock { generation == value } }
}

private func recordingIsCurrent(_ context: UnsafeMutableRawPointer?,
                                _ generation: UInt64) -> Bool {
    guard let context else { return false }
    return Unmanaged<RecordingGenerationGate>.fromOpaque(context)
        .takeUnretainedValue().accepts(generation)
}

struct NativeRecordingInfo {
    static func request(_ object: [String: Any]) throws -> [String: Any] {
        let data = try JSONSerialization.data(withJSONObject: object)
        let response = String(decoding: data, as: UTF8.self).withCString {
            captures_recording_info_v1($0)
        }
        guard let response else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(response) }
        return try AppBridge.decode(Data(bytes: response, count: strlen(response)))
    }

    static func capabilities(includeControls: Bool) throws -> NativeRecordingCapabilities {
        let result = try request([
            "operation": "capabilities",
            "include_recording_controls_in_captures": includeControls,
        ])
        guard let value = result["capabilities"] as? [String: Any],
              let capabilities = NativeRecordingCapabilities(value)
        else { throw AppBridgeError.invalidResponse }
        return capabilities
    }

    static func microphoneDevices() throws -> [NativeMicrophoneDevice] {
        let result = try request(["operation": "microphone_devices"])
        guard let values = result["devices"] as? [[String: Any]] else {
            throw AppBridgeError.invalidResponse
        }
        let devices = values.compactMap(NativeMicrophoneDevice.init)
        guard devices.count == values.count else { throw AppBridgeError.invalidResponse }
        return devices
    }
}

final class NativeRecordingSession {
    private let handle: OpaquePointer

    private init(handle: OpaquePointer) { self.handle = handle }
    deinit { captures_recording_free_v1(handle) }

    static func prepare(recoveryRoot: String, options: [String: Any],
                        display: [String: Any]) throws -> (NativeRecordingSession, NativeRecordingSnapshot) {
        let data = try JSONSerialization.data(withJSONObject: [
            "recovery_root": recoveryRoot, "options": options, "display": display,
        ])
        var response: UnsafeMutablePointer<CChar>?
        let handle = String(decoding: data, as: UTF8.self).withCString {
            captures_recording_prepare_v1($0, &response)
        }
        defer { captures_settings_free_v1(response) }
        do {
            guard let response else { throw AppBridgeError.invalidResponse }
            let result = try AppBridge.decode(Data(bytes: response, count: strlen(response)))
            guard let handle, let value = result["snapshot"] as? [String: Any],
                  let snapshot = NativeRecordingSnapshot(value)
            else { throw AppBridgeError.invalidResponse }
            return (NativeRecordingSession(handle: handle), snapshot)
        } catch {
            captures_recording_free_v1(handle)
            throw error
        }
    }

    func start(generation: UInt64, excludeCapturesApp: Bool,
               gate: RecordingGenerationGate) throws -> NativeRecordingSnapshot {
        try snapshot(request([
            "operation": "start", "generation": generation,
            "exclude_captures_app": excludeCapturesApp,
        ], callback: recordingIsCurrent,
           context: Unmanaged.passUnretained(gate).toOpaque()))
    }

    func pause() throws -> NativeRecordingSnapshot {
        try snapshot(request(["operation": "pause"]))
    }

    func restart() throws -> NativeRecordingSnapshot {
        try snapshot(request(["operation": "restart"]))
    }

    func snapshot() throws -> NativeRecordingSnapshot {
        try snapshot(request(["operation": "snapshot"]))
    }

    func stop() throws -> NativeRecordingSnapshot {
        try snapshot(request(["operation": "stop"]))
    }

    func discard() throws -> NativeRecordingSnapshot {
        try snapshot(request(["operation": "discard"]))
    }

    func finish(historyRoot: String, tools: NativeMediaTools) throws -> NativeFinalizedRecording {
        let result = try request([
            "operation": "finish", "history_root": historyRoot,
            "ffmpeg": tools.ffmpeg, "ffprobe": tools.ffprobe,
        ])
        guard let value = result["finalized"] as? [String: Any],
              let finalized = NativeFinalizedRecording(value)
        else { throw AppBridgeError.invalidResponse }
        return finalized
    }

    private func request(_ object: [String: Any],
                         callback: CapturesRecordingIsCurrent? = nil,
                         context: UnsafeMutableRawPointer? = nil) throws -> [String: Any] {
        let data = try JSONSerialization.data(withJSONObject: object)
        let response = String(decoding: data, as: UTF8.self).withCString {
            captures_recording_request_v1(handle, $0, callback, context)
        }
        guard let response else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(response) }
        return try AppBridge.decode(Data(bytes: response, count: strlen(response)))
    }

    private func snapshot(_ result: [String: Any]) throws -> NativeRecordingSnapshot {
        guard let value = result["snapshot"] as? [String: Any],
              let snapshot = NativeRecordingSnapshot(value)
        else { throw AppBridgeError.invalidResponse }
        return snapshot
    }
}

struct NativeMediaTools {
    let ffmpeg: String
    let ffprobe: String

    static func locate(environment: [String: String] = ProcessInfo.processInfo.environment,
                       executable: URL? = Bundle.main.executableURL) throws -> Self {
        func locate(_ name: String, override: String?) -> String? {
            if let override, FileManager.default.isExecutableFile(atPath: override) { return override }
            let suffix = ProcessInfo.processInfo.machineHardwareName == "arm64"
                ? "aarch64-apple-darwin" : "x86_64-apple-darwin"
            let binary = "\(name)-\(suffix)"
            let bundled = [
                executable?.deletingLastPathComponent().appendingPathComponent(binary),
                executable?.deletingLastPathComponent().appendingPathComponent("binaries/\(binary)"),
                URL(fileURLWithPath: #filePath).deletingLastPathComponent()
                    .appendingPathComponent("../../../../desktop/src-tauri/binaries/\(binary)")
                    .standardizedFileURL,
            ].compactMap { $0 }
            if let candidate = bundled.first(where: {
                FileManager.default.isExecutableFile(atPath: $0.path)
            }) { return candidate.path }
            return environment["PATH"]?.split(separator: ":").lazy
                .map { URL(fileURLWithPath: String($0)).appendingPathComponent(name).path }
                .first { FileManager.default.isExecutableFile(atPath: $0) }
        }
        guard let ffmpeg = locate("ffmpeg", override: environment["CAPTURES_FFMPEG"]),
              let ffprobe = locate("ffprobe", override: environment["CAPTURES_FFPROBE"])
        else {
            throw AppBridgeError.backend(
                "Recording requires the ffmpeg and ffprobe commands in PATH (or CAPTURES_FFMPEG and CAPTURES_FFPROBE).")
        }
        return Self(ffmpeg: ffmpeg, ffprobe: ffprobe)
    }

    func verify() throws {
        try verifyTool(ffmpeg, name: "FFmpeg")
        try verifyTool(ffprobe, name: "FFprobe")
    }

    private func verifyTool(_ path: String, name: String) throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: path)
        process.arguments = ["-version"]
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
            process.waitUntilExit()
        } catch {
            throw AppBridgeError.backend("\(name) could not start at \(path): \(error.localizedDescription)")
        }
        guard process.terminationReason == .exit, process.terminationStatus == 0 else {
            throw AppBridgeError.backend("\(name) at \(path) failed its startup check.")
        }
    }
}

private extension ProcessInfo {
    var machineHardwareName: String {
        var info = utsname(); uname(&info)
        return withUnsafePointer(to: &info.machine) {
            $0.withMemoryRebound(to: CChar.self, capacity: 1) { String(cString: $0) }
        }
    }
}
