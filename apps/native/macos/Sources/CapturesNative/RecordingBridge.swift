import Foundation
import CCapturesSettings

struct NativeRecordingCapabilities: Equatable {
    let systemAudio: Bool
    let microphone: Bool
    let cursorControl: Bool
    let clickHighlights: Bool
    let controlsExcluded: Bool
    /// False on Linux; macOS can keep the controls out of captures.
    let canExcludeControls: Bool

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
        canExcludeControls = value["can_exclude_controls"] as? Bool ?? true
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
    let hasMicrophone: Bool
    let microphoneMuted: Bool
    let region: CGRect?
    let warning: String?

    init?(_ value: [String: Any]) {
        guard let id = value["id"] as? String,
              let state = value["state"] as? String,
              let elapsed = value["elapsed_ms"] as? NSNumber,
              let options = value["options"] as? [String: Any],
              let audio = options["audio"] as? [String: Any],
              let microphoneMuted = audio["microphone_muted"] as? Bool else { return nil }
        self.id = id; self.state = state; elapsedMilliseconds = elapsed.uint64Value
        hasMicrophone = audio["microphone_device_id"] as? String != nil
        self.microphoneMuted = microphoneMuted
        if let target = options["target"] as? [String: Any], target["type"] as? String == "region",
           let rect = target["rect"] as? [String: Int],
           let x = rect["x"], let y = rect["y"],
           let width = rect["width"], let height = rect["height"] {
            region = CGRect(x: CGFloat(x), y: CGFloat(y), width: CGFloat(width), height: CGFloat(height))
        } else {
            region = nil
        }
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

final class RecordingMicrophoneSampler {
    private let queue: DispatchQueue
    private let read: () throws -> Double
    private let useful: () -> Bool
    private let deliver: (Double) -> Void
    private let interval: TimeInterval
    private let gate = RecordingGenerationGate()
    private var generation: UInt64 = 0
    private var active = false
    private var pending = false
    private var timer: Timer?

    init(queue: DispatchQueue, read: @escaping () throws -> Double,
         useful: @escaping () -> Bool, interval: TimeInterval = 0.1,
         deliver: @escaping (Double) -> Void) {
        self.queue = queue; self.read = read; self.useful = useful
        self.interval = interval; self.deliver = deliver
    }

    func setActive(_ enabled: Bool) {
        guard active != enabled else { return }
        active = enabled
        generation &+= 1
        gate.set(enabled ? generation : nil)
        if enabled {
            let timer = Timer(timeInterval: interval, repeats: true) { [weak self] _ in self?.sample() }
            self.timer = timer
            RunLoop.main.add(timer, forMode: .common)
        } else {
            timer?.invalidate(); timer = nil
            deliver(0)
        }
    }

    func sample() {
        guard active, !pending, useful() else { return }
        pending = true
        let generation = generation
        queue.async { [self] in
            let level = gate.accepts(generation) ? (try? read()) ?? 0 : 0
            DispatchQueue.main.async { [self] in
                pending = false
                guard active, gate.accepts(generation), useful() else { return }
                deliver(level)
            }
        }
    }

    deinit { timer?.invalidate(); gate.set(nil) }
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

struct RecordingRecoveryDraft {
    let sessionID: String
    let status: String
    let kind: String?
    let createdAtMilliseconds: UInt64?
    let completedDurationMilliseconds: UInt64
    let identity: String?
    let reason: String?

    init?(_ value: [String: Any]) {
        guard let id = value["session_id"] as? String, !id.isEmpty,
              let status = value["status"] as? String,
              ["recoverable", "unavailable"].contains(status),
              let duration = value["completed_duration_ms"] as? NSNumber,
              let kindValue = value["kind"], kindValue is NSNull || ["video", "gif"].contains(kindValue as? String ?? ""),
              let created = value["created_at_ms"], created is NSNull || created is NSNumber,
              let identityValue = value["identity"], identityValue is NSNull || identityValue is String,
              let reasonValue = value["reason"], reasonValue is NSNull || reasonValue is String else { return nil }
        let identity = identityValue as? String
        guard status != "recoverable" || identity?.isEmpty == false else { return nil }
        sessionID = id; self.status = status; kind = kindValue as? String
        createdAtMilliseconds = (created as? NSNumber)?.uint64Value
        completedDurationMilliseconds = duration.uint64Value
        self.identity = identity; reason = reasonValue as? String
    }
}

struct RecordingRecoveryResult {
    let artifactID: String
    let warning: String?
}

private final class RecordingRecoveryProgressSink {
    let report: (String) -> Void
    init(_ report: @escaping (String) -> Void) { self.report = report }
}

private func recordingRecoveryProgress(_ context: UnsafeMutableRawPointer?,
                                       _ json: UnsafePointer<CChar>?) {
    guard let context, let json,
          let value = try? JSONSerialization.jsonObject(with: Data(bytes: json, count: strlen(json))) as? [String: Any],
          let stage = value["stage"] as? String,
          ["scanning", "assembling", "poster", "publishing"].contains(stage) else { return }
    let sink = Unmanaged<RecordingRecoveryProgressSink>.fromOpaque(context).takeUnretainedValue()
    DispatchQueue.main.async { sink.report(stage) }
}

protocol RecordingRecoveryWorking: AnyObject {
    func list(historyRoot: String, completion: @escaping (Result<[RecordingRecoveryDraft], Error>) -> Void)
    func recover(historyRoot: String, draft: RecordingRecoveryDraft, cancel: NativeRecordingEditorCancel,
                 progress: @escaping (String) -> Void,
                 completion: @escaping (Result<RecordingRecoveryResult, Error>) -> Void)
    func discard(historyRoot: String, draft: RecordingRecoveryDraft,
                 completion: @escaping (Result<Void, Error>) -> Void)
}

final class RecordingRecoveryWorker: RecordingRecoveryWorking {
    private static func request(_ object: [String: Any],
                                call: (UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>?) throws -> [String: Any] {
        let data = try JSONSerialization.data(withJSONObject: object)
        let response = String(decoding: data, as: UTF8.self).withCString(call)
        guard let response else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(response) }
        return try AppBridge.decode(Data(bytes: response, count: strlen(response)))
    }

    func list(historyRoot: String, completion: @escaping (Result<[RecordingRecoveryDraft], Error>) -> Void) {
        LiveCaptureController.queue.async {
            let result = Result { () throws -> [RecordingRecoveryDraft] in
                let value = try Self.request(["history_root": historyRoot], call: captures_recording_recovery_list_v1)
                guard let values = value["drafts"] as? [[String: Any]] else { throw AppBridgeError.invalidResponse }
                let drafts = values.compactMap(RecordingRecoveryDraft.init)
                guard drafts.count == values.count else { throw AppBridgeError.invalidResponse }
                return drafts
            }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func recover(historyRoot: String, draft: RecordingRecoveryDraft, cancel: NativeRecordingEditorCancel,
                 progress: @escaping (String) -> Void,
                 completion: @escaping (Result<RecordingRecoveryResult, Error>) -> Void) {
        LiveCaptureController.queue.async {
            let result = Result { () throws -> RecordingRecoveryResult in
                guard let identity = draft.identity else { throw AppBridgeError.invalidResponse }
                let tools = try NativeMediaTools.locate()
                let sink = RecordingRecoveryProgressSink(progress)
                let value = try Self.request(["history_root": historyRoot, "session_id": draft.sessionID,
                    "expected_identity": identity, "ffmpeg": tools.ffmpeg, "ffprobe": tools.ffprobe], call: {
                    captures_recording_recovery_recover_v1($0, cancel.handle,
                        recordingRecoveryProgress, Unmanaged.passUnretained(sink).toOpaque())
                })
                withExtendedLifetime(sink) {}
                guard let status = value["status"] as? String,
                      ["recovered", "already_recovered"].contains(status),
                      let entry = value["entry"] as? [String: Any],
                      let id = entry["id"] as? String, !id.isEmpty,
                      value["path"] as? String != nil else { throw AppBridgeError.invalidResponse }
                return RecordingRecoveryResult(artifactID: id, warning: value["warning"] as? String)
            }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func discard(historyRoot: String, draft: RecordingRecoveryDraft,
                 completion: @escaping (Result<Void, Error>) -> Void) {
        LiveCaptureController.queue.async {
            let result = Result { () throws -> Void in
                guard let identity = draft.identity else { throw AppBridgeError.invalidResponse }
                let value = try Self.request(["history_root": historyRoot, "session_id": draft.sessionID,
                    "expected_identity": identity], call: captures_recording_recovery_discard_v1)
                guard value["status"] as? String == "discarded" else { throw AppBridgeError.invalidResponse }
            }
            DispatchQueue.main.async { completion(result) }
        }
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

    func setMicrophoneMuted(_ muted: Bool, generation: UInt64,
                            excludeCapturesApp: Bool,
                            gate: RecordingGenerationGate) throws -> NativeRecordingSnapshot {
        try snapshot(request([
            "operation": "set_microphone_muted", "muted": muted,
            "generation": generation, "exclude_captures_app": excludeCapturesApp,
        ], callback: recordingIsCurrent,
           context: Unmanaged.passUnretained(gate).toOpaque()))
    }

    func restart() throws -> NativeRecordingSnapshot {
        try snapshot(request(["operation": "restart"]))
    }

    func snapshot() throws -> NativeRecordingSnapshot {
        try snapshot(request(["operation": "snapshot"]))
    }

    func microphoneLevel() throws -> Double {
        let value = try request(["operation": "microphone_level"])
        guard let peak = value["microphone_peak"] as? NSNumber,
              peak.doubleValue.isFinite, (0...1).contains(peak.doubleValue) else {
            throw AppBridgeError.invalidResponse
        }
        return peak.doubleValue
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
