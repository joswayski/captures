import AppKit
import CCapturesSettings

struct NativeRecordingEditorSnapshot {
    let artifactID: String
    let source: [String: Any]
    let edit: [String: Any]
    let export: [String: Any]
    let saveExport: [String: Any]
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
        self.export = export
        saveExport = value["save_export"] as? [String: Any] ?? export
        positionMilliseconds = position.uint64Value
        self.revision = revision.uint64Value
        self.hasSystemAudio = hasSystemAudio
        self.hasMicrophoneAudio = hasMicrophoneAudio
    }
}

struct RecordingEditorPresentation {
    let snapshot: NativeRecordingEditorSnapshot
    let image: CGImage
    let originalSavePath: String?

    init(snapshot: NativeRecordingEditorSnapshot, image: CGImage,
         originalSavePath: String? = nil) {
        self.snapshot = snapshot
        self.image = image
        self.originalSavePath = originalSavePath
    }
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

struct RecordingEditorComparison {
    let revision: UInt64
    let positionMilliseconds: UInt64
    let export: [String: Any]
    let before: CGImage
    let after: CGImage
}

struct RecordingPlaybackMetadata: Equatable {
    let startPositionMilliseconds: UInt64
    let width: UInt32
    let height: UInt32
    let framesPerSecond: UInt16
    let audioEnabled: Bool
}

struct RecordingPlaybackImage {
    let positionMilliseconds: UInt64
    let image: CGImage
}

struct RecordingSourceImage {
    let positionMilliseconds: UInt64
    let image: CGImage
}

enum RecordingPlaybackCompletion: Equatable {
    case eof
    case empty
    case cancelled
}

final class RecordingPlaybackLoopControl: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Bool

    init(enabled: Bool) { value = enabled }

    var isEnabled: Bool {
        get { lock.withLock { value } }
        set { lock.withLock { value = newValue } }
    }
}

enum RecordingEditorSaveResult: Equatable {
    case saved(path: String)
    case savedWithoutHistory(path: String, warning: String)
}

struct RecordingReplaceResult {
    let path: String
    let presentation: RecordingEditorPresentation
}

struct RecordingReplaceError: LocalizedError {
    let message: String
    let requiresReopen: Bool
    var errorDescription: String? { message }
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

enum NativeRecordingCropDragHandle: UInt8, CaseIterable {
    case move = 0
    case north = 1
    case northEast = 2
    case east = 3
    case southEast = 4
    case south = 5
    case southWest = 6
    case west = 7
    case northWest = 8
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
    static func afterDrag(_ crop: NativeRecordingCropRect,
                          source: NativeRecordingDimensions,
                          handle: NativeRecordingCropDragHandle,
                          deltaX: Double,
                          deltaY: Double,
                          lockAspect: Bool) -> NativeRecordingCropRect? {
        var input = CapturesRecordingCropRect()
        input.x = crop.x; input.y = crop.y
        input.width = crop.width; input.height = crop.height
        var dimensions = CapturesRecordingDimensions()
        dimensions.width = source.width; dimensions.height = source.height
        var output = CapturesRecordingCropRect()
        guard captures_recording_crop_after_drag_v1(input, dimensions, handle.rawValue,
            deltaX, deltaY, lockAspect, &output) else { return nil }
        return NativeRecordingCropRect(x: output.x, y: output.y,
                                       width: output.width, height: output.height)
    }

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

final class NativeRecordingEditorPlayback {
    private let handle: OpaquePointer
    let metadata: RecordingPlaybackMetadata

    init?(handle: OpaquePointer, metadata: [String: Any], audioEnabled: Bool) {
        guard let start = (metadata["start_position_ms"] as? NSNumber)?.uint64Value,
              let width = (metadata["width"] as? NSNumber)?.uint32Value,
              let height = (metadata["height"] as? NSNumber)?.uint32Value,
              let framesPerSecond = (metadata["frames_per_second"] as? NSNumber)?.uint16Value,
              width > 0, height > 0, framesPerSecond > 0, framesPerSecond <= 30 else { return nil }
        self.handle = handle
        self.metadata = RecordingPlaybackMetadata(startPositionMilliseconds: start,
                                                  width: width, height: height,
                                                  framesPerSecond: framesPerSecond,
                                                  audioEnabled: audioEnabled)
    }

    deinit { captures_recording_editor_playback_free_v1(handle) }

    func nextFrame() throws -> RecordingPlaybackImage? {
        var response: UnsafeMutablePointer<CChar>?
        let handle = captures_recording_editor_playback_next_v1(self.handle, &response)
        defer { captures_settings_free_v1(response) }
        guard let response else {
            captures_recording_editor_frame_free_v1(handle)
            throw AppBridgeError.invalidResponse
        }
        let value: [String: Any]
        do { value = try AppBridge.decode(Data(bytes: response, count: strlen(response))) }
        catch {
            captures_recording_editor_frame_free_v1(handle)
            throw error
        }
        guard let eof = value["eof"] as? Bool else {
            captures_recording_editor_frame_free_v1(handle)
            throw AppBridgeError.invalidResponse
        }
        if eof {
            captures_recording_editor_frame_free_v1(handle)
            guard handle == nil else { throw AppBridgeError.invalidResponse }
            return nil
        }
        guard let handle,
              let position = (value["position_ms"] as? NSNumber)?.uint64Value else {
            captures_recording_editor_frame_free_v1(handle)
            throw AppBridgeError.invalidResponse
        }
        let frame = NativeRecordingEditorFrame(handle: handle)
        let image = try frame.image()
        guard image.width == Int(metadata.width), image.height == Int(metadata.height) else {
            throw AppBridgeError.invalidResponse
        }
        return RecordingPlaybackImage(positionMilliseconds: position, image: image)
    }
}

final class NativeRecordingEditorThumbnails {
    private let handle: OpaquePointer
    let frameCount: UInt32
    let frameWidth: UInt32
    let frameHeight: UInt32
    let spriteWidth: UInt32
    let spriteHeight: UInt32

    init?(handle: OpaquePointer, metadata: [String: Any]) {
        guard let frameCount = (metadata["frame_count"] as? NSNumber)?.uint32Value,
              let frameWidth = (metadata["frame_width"] as? NSNumber)?.uint32Value,
              let frameHeight = (metadata["frame_height"] as? NSNumber)?.uint32Value,
              let spriteWidth = (metadata["sprite_width"] as? NSNumber)?.uint32Value,
              let spriteHeight = (metadata["sprite_height"] as? NSNumber)?.uint32Value,
              frameCount > 0, frameWidth > 0, frameHeight > 0,
              spriteWidth == frameCount * frameWidth, spriteHeight == frameHeight else { return nil }
        self.handle = handle; self.frameCount = frameCount
        self.frameWidth = frameWidth; self.frameHeight = frameHeight
        self.spriteWidth = spriteWidth; self.spriteHeight = spriteHeight
    }

    deinit { captures_recording_editor_thumbnails_free_v1(handle) }

    func image() throws -> CGImage {
        var pixels = CapturesRegionPixels()
        guard captures_recording_editor_thumbnails_pixels_v1(handle, &pixels),
              let data = pixels.data,
              pixels.width == spriteWidth, pixels.height == spriteHeight,
              pixels.bytes_per_row == Int(spriteWidth) * 4,
              pixels.length == pixels.bytes_per_row * Int(spriteHeight) else {
            throw AppBridgeError.invalidResponse
        }
        let retained = Unmanaged.passRetained(self)
        guard let provider = CGDataProvider(dataInfo: retained.toOpaque(), data: data,
            size: pixels.length, releaseData: { info, _, _ in
                if let info { Unmanaged<NativeRecordingEditorThumbnails>.fromOpaque(info).release() }
            }) else {
            retained.release(); throw AppBridgeError.invalidResponse
        }
        guard let image = CGImage(width: Int(spriteWidth), height: Int(spriteHeight),
            bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: pixels.bytes_per_row,
            space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
            provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
        else { throw AppBridgeError.invalidResponse }
        return image
    }
}

final class NativeRecordingEditorCancel {
    let handle: OpaquePointer
    private let lock = NSLock()
    private var cancelled = false
    var isCancelled: Bool {
        lock.lock(); defer { lock.unlock() }
        return cancelled
    }
    init?() {
        guard let handle = captures_recording_editor_cancel_create_v1() else { return nil }
        self.handle = handle
    }
    func cancel() {
        lock.lock(); cancelled = true; lock.unlock()
        captures_recording_editor_cancel_v1(handle)
    }
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
        let frame = try session.presentation(snapshot)
        return (session, RecordingEditorPresentation(snapshot: frame.snapshot,
            image: frame.image, originalSavePath: try session.originalSavePath()))
    }

    private func originalSavePath() throws -> String? {
        guard let response = captures_recording_editor_original_save_path_v1(handle) else {
            throw AppBridgeError.invalidResponse
        }
        defer { captures_settings_free_v1(response) }
        let result = try AppBridge.decode(Data(bytes: response, count: strlen(response)))
        guard let path = result["path"], path is NSNull || path is String else {
            throw AppBridgeError.invalidResponse
        }
        return path as? String
    }

    func request(_ object: [String: Any]) throws -> RecordingEditorPresentation {
        let data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
        let response = String(decoding: data, as: UTF8.self).withCString {
            captures_recording_editor_request_v2(handle, $0)
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
        guard let response = captures_recording_editor_estimate_v2(handle, cancel.handle) else {
            throw AppBridgeError.invalidResponse
        }
        defer { captures_settings_free_v1(response) }
        let value = try AppBridge.decode(Data(bytes: response, count: strlen(response)))
        guard let bytes = value["size_bytes"] as? NSNumber,
              let exact = value["exact"] as? Bool else { throw AppBridgeError.invalidResponse }
        return RecordingEditorEstimate(sizeBytes: bytes.uint64Value, exact: exact)
    }

    func comparison(cancel: NativeRecordingEditorCancel) throws -> RecordingEditorComparison {
        var response: UnsafeMutablePointer<CChar>?
        let owner = captures_recording_editor_comparison_v1(handle, cancel.handle, &response)
        defer {
            captures_settings_free_v1(response)
            captures_recording_editor_comparison_free_v1(owner)
        }
        guard let response else { throw AppBridgeError.invalidResponse }
        let value = try AppBridge.decode(Data(bytes: response, count: strlen(response)))
        guard let owner, value["basis"] as? String == "accepted_preview_first_attempt",
              let revision = (value["revision"] as? NSNumber)?.uint64Value,
              let position = (value["position_ms"] as? NSNumber)?.uint64Value,
              let export = value["export"] as? [String: Any],
              let width = (value["width"] as? NSNumber)?.intValue,
              let height = (value["height"] as? NSNumber)?.intValue else {
            throw AppBridgeError.invalidResponse
        }
        guard let beforeOwner = captures_recording_editor_comparison_before_frame_v1(owner) else {
            throw AppBridgeError.invalidResponse
        }
        let before = NativeRecordingEditorFrame(handle: beforeOwner)
        guard let afterOwner = captures_recording_editor_comparison_after_frame_v1(owner) else {
            throw AppBridgeError.invalidResponse
        }
        let after = NativeRecordingEditorFrame(handle: afterOwner)
        let beforeImage = try before.image(), afterImage = try after.image()
        guard beforeImage.width == width, beforeImage.height == height,
              afterImage.width == width, afterImage.height == height else {
            throw AppBridgeError.invalidResponse
        }
        return RecordingEditorComparison(revision: revision, positionMilliseconds: position,
                                         export: export, before: beforeImage, after: afterImage)
    }

    func playback(positionMilliseconds: UInt64, soundEnabled: Bool,
                  cancel: NativeRecordingEditorCancel) throws -> NativeRecordingEditorPlayback {
        var response: UnsafeMutablePointer<CChar>?
        let handle = soundEnabled
            ? captures_recording_editor_playback_open_v2(
                self.handle, positionMilliseconds, cancel.handle, &response)
            : captures_recording_editor_playback_open_v1(
                self.handle, positionMilliseconds, cancel.handle, &response)
        defer { captures_settings_free_v1(response) }
        guard let response else {
            captures_recording_editor_playback_free_v1(handle)
            throw AppBridgeError.invalidResponse
        }
        let metadata: [String: Any]
        do { metadata = try AppBridge.decode(Data(bytes: response, count: strlen(response))) }
        catch {
            captures_recording_editor_playback_free_v1(handle)
            throw error
        }
        let audioEnabled: Bool
        if soundEnabled {
            guard let value = metadata["audio_enabled"] as? Bool else {
                captures_recording_editor_playback_free_v1(handle)
                throw AppBridgeError.invalidResponse
            }
            audioEnabled = value
        } else {
            audioEnabled = false
        }
        guard let handle,
              let playback = NativeRecordingEditorPlayback(handle: handle, metadata: metadata,
                                                            audioEnabled: audioEnabled) else {
            captures_recording_editor_playback_free_v1(handle)
            throw AppBridgeError.invalidResponse
        }
        return playback
    }

    func sourceFrame(cancel: NativeRecordingEditorCancel) throws -> RecordingSourceImage {
        var response: UnsafeMutablePointer<CChar>?
        let handle = captures_recording_editor_source_frame_v1(self.handle, cancel.handle,
                                                                &response)
        defer { captures_settings_free_v1(response) }
        guard let response else {
            captures_recording_editor_frame_free_v1(handle)
            throw AppBridgeError.invalidResponse
        }
        let value: [String: Any]
        do { value = try AppBridge.decode(Data(bytes: response, count: strlen(response))) }
        catch {
            captures_recording_editor_frame_free_v1(handle)
            throw error
        }
        guard let handle,
              let position = (value["position_ms"] as? NSNumber)?.uint64Value,
              let width = (value["width"] as? NSNumber)?.intValue,
              let height = (value["height"] as? NSNumber)?.intValue,
              width > 0, height > 0 else {
            captures_recording_editor_frame_free_v1(handle)
            throw AppBridgeError.invalidResponse
        }
        let frame = NativeRecordingEditorFrame(handle: handle)
        let image = try frame.image()
        guard image.width == width, image.height == height else {
            throw AppBridgeError.invalidResponse
        }
        return RecordingSourceImage(positionMilliseconds: position, image: image)
    }

    func thumbnails(cancel: NativeRecordingEditorCancel) throws -> NativeRecordingEditorThumbnails {
        var response: UnsafeMutablePointer<CChar>?
        let handle = captures_recording_editor_thumbnails_v1(self.handle, cancel.handle, &response)
        defer { captures_settings_free_v1(response) }
        guard let response else {
            captures_recording_editor_thumbnails_free_v1(handle)
            throw AppBridgeError.invalidResponse
        }
        let metadata: [String: Any]
        do { metadata = try AppBridge.decode(Data(bytes: response, count: strlen(response))) }
        catch {
            captures_recording_editor_thumbnails_free_v1(handle)
            throw error
        }
        guard let handle,
              let thumbnails = NativeRecordingEditorThumbnails(handle: handle, metadata: metadata) else {
            captures_recording_editor_thumbnails_free_v1(handle)
            throw AppBridgeError.invalidResponse
        }
        return thumbnails
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

    func replaceOriginal(cancel: NativeRecordingEditorCancel,
                         progress: @escaping (RecordingEditorProgress) -> Void) throws
        -> RecordingReplaceResult {
        let sink = RecordingEditorProgressSink(progress)
        let response = captures_recording_editor_replace_original_v1(
            handle, cancel.handle, recordingEditorProgressCallback,
            Unmanaged.passUnretained(sink).toOpaque())
        withExtendedLifetime(sink) {}
        guard let response else {
            throw RecordingReplaceError(message: "No replacement result was returned.",
                                        requiresReopen: true)
        }
        defer { captures_settings_free_v1(response) }
        guard let envelope = try? JSONSerialization.jsonObject(
            with: Data(bytes: response, count: strlen(response))) as? [String: Any],
              let ok = envelope["ok"] as? Bool else {
            throw RecordingReplaceError(message: "Replacement result was invalid.",
                                        requiresReopen: true)
        }
        guard ok else {
            throw RecordingReplaceError(message: envelope["error"] as? String ?? "Replacement failed.",
                                        requiresReopen: envelope["requires_reopen"] as? Bool ?? true)
        }
        guard let result = envelope["result"] as? [String: Any],
              let replacement = result["replacement"] as? [String: Any],
              replacement["status"] as? String == "replaced",
              let path = replacement["path"] as? String, !path.isEmpty,
              let value = result["snapshot"] as? [String: Any],
              let snapshot = NativeRecordingEditorSnapshot(value),
              let frame = try? presentation(snapshot) else {
            throw RecordingReplaceError(message: "Replacement completed but its new frame could not be loaded.",
                                        requiresReopen: true)
        }
        return RecordingReplaceResult(path: path, presentation: frame)
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
    func comparison(cancel: NativeRecordingEditorCancel,
                    completion: @escaping (Result<RecordingEditorComparison, Error>) -> Void)
    func playback(positionMilliseconds: UInt64, loopStartMilliseconds: UInt64,
                  soundEnabled: Bool,
                  loop: RecordingPlaybackLoopControl, cancel: NativeRecordingEditorCancel,
                  started: @escaping (RecordingPlaybackMetadata) -> Void,
                  frame: @escaping (RecordingPlaybackImage) -> Void,
                  completion: @escaping (Result<RecordingPlaybackCompletion, Error>) -> Void)
    func sourceFrame(cancel: NativeRecordingEditorCancel,
                     completion: @escaping (Result<RecordingSourceImage, Error>) -> Void)
    func thumbnails(cancel: NativeRecordingEditorCancel,
                    completion: @escaping (Result<CGImage, Error>) -> Void)
    func save(destination: String, export: [String: Any], cancel: NativeRecordingEditorCancel,
              progress: @escaping (RecordingEditorProgress) -> Void,
              completion: @escaping (Result<RecordingEditorSaveResult, Error>) -> Void)
    func replaceOriginal(cancel: NativeRecordingEditorCancel,
                         progress: @escaping (RecordingEditorProgress) -> Void,
                         completion: @escaping (Result<RecordingReplaceResult, Error>) -> Void)
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

    func comparison(cancel: NativeRecordingEditorCancel,
                    completion: @escaping (Result<RecordingEditorComparison, Error>) -> Void) {
        let storage = storage
        Self.queue.async {
            let result = Result {
                guard let session = storage.session else {
                    throw AppBridgeError.backend("The recording editor is closed.")
                }
                return try session.comparison(cancel: cancel)
            }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func playback(positionMilliseconds: UInt64, loopStartMilliseconds: UInt64,
                  soundEnabled: Bool,
                  loop: RecordingPlaybackLoopControl, cancel: NativeRecordingEditorCancel,
                  started: @escaping (RecordingPlaybackMetadata) -> Void,
                  frame: @escaping (RecordingPlaybackImage) -> Void,
                  completion: @escaping (Result<RecordingPlaybackCompletion, Error>) -> Void) {
        let delivery = RecordingPlaybackDelivery(shouldDiscardFrames: { cancel.isCancelled },
            frame: frame) { result in
                completion(cancel.isCancelled ? .success(.cancelled) : result)
            }
        let storage = storage
        Self.queue.async {
            let result: Result<RecordingPlaybackCompletion, Error>
            do {
                guard let session = storage.session else {
                    throw AppBridgeError.backend("The recording editor is closed.")
                }
                var didStart = false
                func runLap(from position: UInt64) throws -> Int {
                    let playback = try session.playback(positionMilliseconds: position,
                                                        soundEnabled: soundEnabled,
                                                        cancel: cancel)
                    if !didStart {
                        didStart = true
                        let metadata = playback.metadata
                        DispatchQueue.main.async { started(metadata) }
                    }
                    var frameCount = 0
                    while let value = try playback.nextFrame() {
                        frameCount += 1; delivery.offer(value)
                    }
                    return frameCount
                }

                var position = positionMilliseconds
                while true {
                    let frameCount = try runLap(from: position)
                    if cancel.isCancelled {
                        result = .success(.cancelled); break
                    }
                    guard frameCount > 0 else {
                        result = .success(.empty); break
                    }
                    guard loop.isEnabled else {
                        result = .success(.eof); break
                    }
                    position = loopStartMilliseconds
                }
            } catch {
                result = .failure(error)
            }
            delivery.finish(result)
        }
    }

    func sourceFrame(cancel: NativeRecordingEditorCancel,
                     completion: @escaping (Result<RecordingSourceImage, Error>) -> Void) {
        let storage = storage
        Self.queue.async {
            let result = Result {
                guard let session = storage.session else {
                    throw AppBridgeError.backend("The recording editor is closed.")
                }
                return try session.sourceFrame(cancel: cancel)
            }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func thumbnails(cancel: NativeRecordingEditorCancel,
                    completion: @escaping (Result<CGImage, Error>) -> Void) {
        let storage = storage
        Self.queue.async {
            let result = Result {
                guard let session = storage.session else {
                    throw AppBridgeError.backend("The recording editor is closed.")
                }
                return try session.thumbnails(cancel: cancel).image()
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

    func replaceOriginal(cancel: NativeRecordingEditorCancel,
                         progress: @escaping (RecordingEditorProgress) -> Void,
                         completion: @escaping (Result<RecordingReplaceResult, Error>) -> Void) {
        let storage = storage
        Self.queue.async {
            let result = Result {
                guard let session = storage.session else {
                    throw RecordingReplaceError(message: "The recording editor is closed.",
                                                requiresReopen: true)
                }
                return try session.replaceOriginal(cancel: cancel, progress: progress)
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

final class RecordingPlaybackDelivery: @unchecked Sendable {
    private let lock = NSLock()
    private let shouldDiscardFrames: () -> Bool
    private let frame: (RecordingPlaybackImage) -> Void
    private let completion: (Result<RecordingPlaybackCompletion, Error>) -> Void
    private var latest: RecordingPlaybackImage?
    private var terminal: Result<RecordingPlaybackCompletion, Error>?
    private var scheduled = false

    init(shouldDiscardFrames: @escaping () -> Bool,
         frame: @escaping (RecordingPlaybackImage) -> Void,
         completion: @escaping (Result<RecordingPlaybackCompletion, Error>) -> Void) {
        self.shouldDiscardFrames = shouldDiscardFrames
        self.frame = frame; self.completion = completion
    }

    func offer(_ value: RecordingPlaybackImage) {
        lock.lock()
        guard terminal == nil else { lock.unlock(); return }
        latest = value
        scheduleLocked()
        lock.unlock()
    }

    func finish(_ result: Result<RecordingPlaybackCompletion, Error>) {
        lock.lock()
        guard terminal == nil else { lock.unlock(); return }
        if shouldDiscardFrames() { latest = nil }
        terminal = result
        scheduleLocked()
        lock.unlock()
    }

    private func scheduleLocked() {
        guard !scheduled else { return }
        scheduled = true
        DispatchQueue.main.async { [self] in drain() }
    }

    private func drain() {
        lock.lock()
        if shouldDiscardFrames() { latest = nil }
        let next = latest
        latest = nil
        let result = next == nil ? terminal : nil
        if result != nil { terminal = nil }
        let again = latest != nil || terminal != nil
        if !again { scheduled = false }
        lock.unlock()

        if let next { frame(next) }
        if let result { completion(result) }
        if again { DispatchQueue.main.async { [self] in drain() } }
    }
}
