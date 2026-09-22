import AppKit
import CCapturesSettings

struct NativeRecordingEditorSnapshot {
    let artifactID: String
    let source: [String: Any]
    let edit: [String: Any]
    let export: [String: Any]
    let positionMilliseconds: UInt64
    let revision: UInt64
    let hasSystemAudio: Bool
    let hasMicrophoneAudio: Bool

    var durationMilliseconds: UInt64 {
        (source["duration_ms"] as? NSNumber)?.uint64Value ?? 0
    }
    var width: Int { (source["width"] as? NSNumber)?.intValue ?? 0 }
    var height: Int { (source["height"] as? NSNumber)?.intValue ?? 0 }

    init?(_ value: [String: Any]) {
        guard let artifactID = value["artifact_id"] as? String,
              let source = value["source"] as? [String: Any],
              let edit = value["edit"] as? [String: Any],
              let export = value["preview_export"] as? [String: Any],
              let position = value["position_ms"] as? NSNumber,
              let revision = value["revision"] as? NSNumber,
              let hasSystemAudio = value["has_system_audio"] as? Bool,
              let hasMicrophoneAudio = value["has_microphone_audio"] as? Bool else { return nil }
        self.artifactID = artifactID; self.source = source; self.edit = edit
        self.export = export; positionMilliseconds = position.uint64Value
        self.revision = revision.uint64Value
        self.hasSystemAudio = hasSystemAudio
        self.hasMicrophoneAudio = hasMicrophoneAudio
    }
}

struct RecordingEditorPresentation {
    let snapshot: NativeRecordingEditorSnapshot
    let image: CGImage
}

struct RecordingEditorProgress: Equatable {
    let completedPerMille: Int
    let message: String

    init?(_ value: [String: Any]) {
        guard let completed = value["completed_per_mille"] as? NSNumber,
              let stage = value["stage"] as? String else { return nil }
        completedPerMille = completed.intValue
        message = value["message"] as? String ?? stage.capitalized
    }
}

struct RecordingEditorEstimate: Equatable {
    let sizeBytes: UInt64
    let exact: Bool
}

enum RecordingEditorSaveResult: Equatable {
    case saved(path: String)
    case savedWithoutHistory(path: String, warning: String)
}

enum NativeRecordingTimelineEdge: UInt8 {
    case start = 0
    case end = 1
}

struct NativeRecordingCropRect: Equatable {
    var x: UInt32
    var y: UInt32
    var width: UInt32
    var height: UInt32

    init?(value: Any?, sourceWidth: UInt32, sourceHeight: UInt32) {
        guard let value = value as? [String: Any],
              let x = Self.uint32(value["x"]), let y = Self.uint32(value["y"]),
              let width = Self.uint32(value["width"]),
              let height = Self.uint32(value["height"]),
              width >= 2, height >= 2,
              UInt64(x) + UInt64(width) <= UInt64(sourceWidth),
              UInt64(y) + UInt64(height) <= UInt64(sourceHeight) else { return nil }
        self.init(x: x, y: y, width: width, height: height)
    }

    init(x: UInt32, y: UInt32, width: UInt32, height: UInt32) {
        self.x = x; self.y = y; self.width = width; self.height = height
    }

    var dictionary: [String: Any] {
        ["x": x, "y": y, "width": width, "height": height]
    }

    private static func uint32(_ value: Any?) -> UInt32? {
        guard let number = value as? NSNumber else { return nil }
        let raw = number.uint64Value
        return raw <= UInt64(UInt32.max) ? UInt32(raw) : nil
    }
}

struct NativeRecordingDimensions: Equatable {
    let width: UInt32
    let height: UInt32
}

enum NativeRecordingCropAxis: UInt8 {
    case width = 0
    case height = 1
}

enum NativeRecordingResolutionPreset: UInt8, CaseIterable {
    case original = 0
    case p1080 = 1
    case p720 = 2

    var title: String {
        switch self {
        case .original: "Original"
        case .p1080: "1080p maximum"
        case .p720: "720p maximum"
        }
    }
}

enum NativeRecordingGeometry {
    static func resizeLocked(_ crop: NativeRecordingCropRect,
                             source: NativeRecordingDimensions,
                             axis: NativeRecordingCropAxis,
                             value: UInt32) -> NativeRecordingCropRect? {
        var input = CapturesRecordingCropRect()
        input.x = crop.x; input.y = crop.y
        input.width = crop.width; input.height = crop.height
        var dimensions = CapturesRecordingDimensions()
        dimensions.width = source.width; dimensions.height = source.height
        var output = CapturesRecordingCropRect()
        guard captures_recording_crop_resize_locked_v1(input, dimensions, axis.rawValue,
                                                        value, &output) else { return nil }
        return NativeRecordingCropRect(x: output.x, y: output.y,
                                       width: output.width, height: output.height)
    }

    static func constrain(_ input: NativeRecordingDimensions,
                          preset: NativeRecordingResolutionPreset) -> NativeRecordingDimensions? {
        var dimensions = CapturesRecordingDimensions()
        dimensions.width = input.width; dimensions.height = input.height
        var output = CapturesRecordingDimensions()
        guard captures_recording_max_resolution_constrain_v1(preset.rawValue, dimensions,
                                                              &output) else { return nil }
        return NativeRecordingDimensions(width: output.width, height: output.height)
    }
}

struct NativeRecordingTimelineDrag {
    private var value: CapturesRecordingTimelineTrimDrag

    static func begin(edge: NativeRecordingTimelineEdge, pointerX: Double,
                      startMilliseconds: Double, endMilliseconds: Double,
                      durationMilliseconds: Double) -> NativeRecordingTimelineDrag? {
        var value = CapturesRecordingTimelineTrimDrag()
        guard captures_recording_timeline_trim_begin_v1(edge.rawValue, pointerX,
            startMilliseconds, endMilliseconds, durationMilliseconds, &value) else { return nil }
        return NativeRecordingTimelineDrag(value: value)
    }

    mutating func update(pointerX: Double, trackLeft: Double,
                         trackWidth: Double) -> Double? {
        var output = CapturesRecordingTimelineTrimUpdate()
        guard captures_recording_timeline_trim_update_v1(value, pointerX, trackLeft,
                                                          trackWidth, &output) else { return nil }
        value = output.drag
        return output.time_ms
    }
}

enum NativeRecordingTimeline {
    static func ratio(milliseconds: Double, durationMilliseconds: Double) -> Double? {
        var output = 0.0
        guard captures_recording_timeline_ratio_v1(milliseconds, durationMilliseconds,
                                                    &output) else { return nil }
        return output
    }

    static func time(atX x: Double, trackLeft: Double, trackWidth: Double,
                     durationMilliseconds: Double) -> Double? {
        var output = 0.0
        guard captures_recording_timeline_time_at_x_v1(x, trackLeft, trackWidth,
                                                       durationMilliseconds, &output) else { return nil }
        return output
    }
}

final class NativeRecordingEditorFrame {
    private let handle: OpaquePointer
    init(handle: OpaquePointer) { self.handle = handle }
    deinit { captures_recording_editor_frame_free_v1(handle) }

    func image() throws -> CGImage {
        var pixels = CapturesRegionPixels()
        guard captures_recording_editor_frame_pixels_v1(handle, &pixels),
              let data = pixels.data, pixels.width > 0, pixels.height > 0 else {
            throw AppBridgeError.invalidResponse
        }
        let retained = Unmanaged.passRetained(self)
        guard let provider = CGDataProvider(dataInfo: retained.toOpaque(), data: data,
            size: pixels.length, releaseData: { info, _, _ in
                if let info { Unmanaged<NativeRecordingEditorFrame>.fromOpaque(info).release() }
            }) else {
            retained.release(); throw AppBridgeError.invalidResponse
        }
        guard let image = CGImage(width: Int(pixels.width), height: Int(pixels.height),
            bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: pixels.bytes_per_row,
            space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
            provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
        else { throw AppBridgeError.invalidResponse }
        return image
    }
}

final class NativeRecordingEditorCancel {
    fileprivate let handle: OpaquePointer
    init?() {
        guard let handle = captures_recording_editor_cancel_create_v1() else { return nil }
        self.handle = handle
    }
    func cancel() { captures_recording_editor_cancel_v1(handle) }
    deinit { captures_recording_editor_cancel_free_v1(handle) }
}

private final class RecordingEditorProgressSink {
    let progress: (RecordingEditorProgress) -> Void
    init(_ progress: @escaping (RecordingEditorProgress) -> Void) { self.progress = progress }
}

private func recordingEditorProgressCallback(_ context: UnsafeMutableRawPointer?,
                                             _ json: UnsafePointer<CChar>?) {
    guard let context, let json,
          let value = try? JSONSerialization.jsonObject(with: Data(bytes: json, count: strlen(json)))
            as? [String: Any],
          let progress = RecordingEditorProgress(value) else { return }
    let sink = Unmanaged<RecordingEditorProgressSink>.fromOpaque(context).takeUnretainedValue()
    DispatchQueue.main.async { sink.progress(progress) }
}

final class NativeRecordingEditorSession {
    private let handle: OpaquePointer
    private init(handle: OpaquePointer) { self.handle = handle }
    deinit { captures_recording_editor_free_v1(handle) }

    static func open(historyRoot: String, artifactID: String,
                     tools: NativeMediaTools) throws -> (NativeRecordingEditorSession, RecordingEditorPresentation) {
        let request: [String: Any] = ["history_root": historyRoot, "artifact_id": artifactID,
                                      "ffmpeg": tools.ffmpeg, "ffprobe": tools.ffprobe]
        let data = try JSONSerialization.data(withJSONObject: request, options: [.sortedKeys])
        var response: UnsafeMutablePointer<CChar>?
        let handle = String(decoding: data, as: UTF8.self).withCString {
            captures_recording_editor_open_v1($0, &response)
        }
        defer { captures_settings_free_v1(response) }
        guard let response else {
            captures_recording_editor_free_v1(handle)
            throw AppBridgeError.invalidResponse
        }
        let value: [String: Any]
        do { value = try AppBridge.decode(Data(bytes: response, count: strlen(response))) }
        catch { captures_recording_editor_free_v1(handle); throw error }
        guard let handle, let snapshot = NativeRecordingEditorSnapshot(value) else {
            captures_recording_editor_free_v1(handle)
            throw AppBridgeError.invalidResponse
        }
        let session = NativeRecordingEditorSession(handle: handle)
        return (session, try session.presentation(snapshot))
    }

    func request(_ object: [String: Any]) throws -> RecordingEditorPresentation {
        let data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
        let response = String(decoding: data, as: UTF8.self).withCString {
            captures_recording_editor_request_v1(handle, $0)
        }
        guard let response else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(response) }
        let value = try AppBridge.decode(Data(bytes: response, count: strlen(response)))
        guard let snapshot = NativeRecordingEditorSnapshot(value) else {
            throw AppBridgeError.invalidResponse
        }
        return try presentation(snapshot)
    }

    func estimate(cancel: NativeRecordingEditorCancel) throws -> RecordingEditorEstimate {
        guard let response = captures_recording_editor_estimate_v1(handle, cancel.handle) else {
            throw AppBridgeError.invalidResponse
        }
        defer { captures_settings_free_v1(response) }
        let value = try AppBridge.decode(Data(bytes: response, count: strlen(response)))
        guard let bytes = value["size_bytes"] as? NSNumber,
              let exact = value["exact"] as? Bool else { throw AppBridgeError.invalidResponse }
        return RecordingEditorEstimate(sizeBytes: bytes.uint64Value, exact: exact)
    }

    func save(destination: String, export: [String: Any], cancel: NativeRecordingEditorCancel,
              progress: @escaping (RecordingEditorProgress) -> Void) throws -> RecordingEditorSaveResult {
        let data = try JSONSerialization.data(withJSONObject: ["destination": destination,
                                                                 "export": export], options: [.sortedKeys])
        let sink = RecordingEditorProgressSink(progress)
        let response = String(decoding: data, as: UTF8.self).withCString {
            captures_recording_editor_save_new_v1(handle, $0, cancel.handle,
                recordingEditorProgressCallback,
                Unmanaged.passUnretained(sink).toOpaque())
        }
        withExtendedLifetime(sink) {}
        guard let response else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(response) }
        let value = try AppBridge.decode(Data(bytes: response, count: strlen(response)))
        guard let status = value["status"] as? String,
              let path = value["path"] as? String, !path.isEmpty else {
            throw AppBridgeError.invalidResponse
        }
        if status == "saved" { return .saved(path: path) }
        guard status == "saved_without_history", let warning = value["warning"] as? String else {
            throw AppBridgeError.invalidResponse
        }
        return .savedWithoutHistory(path: path, warning: warning)
    }

    private func presentation(_ snapshot: NativeRecordingEditorSnapshot) throws
        -> RecordingEditorPresentation {
        guard let handle = captures_recording_editor_frame_v1(handle) else {
            throw AppBridgeError.invalidResponse
        }
        let frame = NativeRecordingEditorFrame(handle: handle)
        return RecordingEditorPresentation(snapshot: snapshot, image: try frame.image())
    }
}

protocol RecordingEditorWorking: AnyObject {
    func open(historyRoot: String, artifactID: String,
              completion: @escaping (Result<RecordingEditorPresentation, Error>) -> Void)
    func request(_ object: [String: Any],
                 completion: @escaping (Result<RecordingEditorPresentation, Error>) -> Void)
    func estimate(cancel: NativeRecordingEditorCancel,
                  completion: @escaping (Result<RecordingEditorEstimate, Error>) -> Void)
    func save(destination: String, export: [String: Any], cancel: NativeRecordingEditorCancel,
              progress: @escaping (RecordingEditorProgress) -> Void,
              completion: @escaping (Result<RecordingEditorSaveResult, Error>) -> Void)
    func close()
}

final class RecordingEditorWorker: RecordingEditorWorking {
    private static let queue = DispatchQueue(label: "es.captures.native.recording-editor",
                                             qos: .userInitiated)
    private final class Storage { var session: NativeRecordingEditorSession? }
    private let storage = Storage()

    func open(historyRoot: String, artifactID: String,
              completion: @escaping (Result<RecordingEditorPresentation, Error>) -> Void) {
        let storage = storage
        Self.queue.async {
            let result = Result { () throws -> RecordingEditorPresentation in
                storage.session = nil
                let opened = try NativeRecordingEditorSession.open(historyRoot: historyRoot,
                    artifactID: artifactID, tools: NativeMediaTools.locate())
                storage.session = opened.0; return opened.1
            }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func request(_ object: [String: Any],
                 completion: @escaping (Result<RecordingEditorPresentation, Error>) -> Void) {
        let storage = storage
        Self.queue.async {
            let result = Result {
                guard let session = storage.session else {
                    throw AppBridgeError.backend("The recording editor is closed.")
                }
                return try session.request(object)
            }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func estimate(cancel: NativeRecordingEditorCancel,
                  completion: @escaping (Result<RecordingEditorEstimate, Error>) -> Void) {
        let storage = storage
        Self.queue.async {
            let result = Result {
                guard let session = storage.session else {
                    throw AppBridgeError.backend("The recording editor is closed.")
                }
                return try session.estimate(cancel: cancel)
            }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func save(destination: String, export: [String: Any], cancel: NativeRecordingEditorCancel,
              progress: @escaping (RecordingEditorProgress) -> Void,
              completion: @escaping (Result<RecordingEditorSaveResult, Error>) -> Void) {
        let storage = storage
        Self.queue.async {
            let result = Result {
                guard let session = storage.session else {
                    throw AppBridgeError.backend("The recording editor is closed.")
                }
                return try session.save(destination: destination, export: export,
                                        cancel: cancel, progress: progress)
            }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func close() {
        let storage = storage
        Self.queue.async { storage.session = nil }
    }
    static func flush() { queue.sync {} }
}
