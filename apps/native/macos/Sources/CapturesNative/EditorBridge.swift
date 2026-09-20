import Foundation
import CoreGraphics
import ImageIO
import CCapturesSettings

/// Copies shared preview geometry out of its short-lived C owner. No session
/// worker or JSON is involved in drawing pointer feedback.
struct NativeEditorDrawGeometry {
    let points: [CGPoint]
    let strokeWidth: Double

    init?(arrow: Bool, samples: [CGPoint]) {
        let input = samples.map { CapturesSelectionPoint(x: $0.x, y: $0.y) }
        var output = CapturesEditorDrawPoints()
        let handle = input.withUnsafeBufferPointer {
            captures_editor_draw_geometry_v1(arrow ? 0 : 1, $0.baseAddress, $0.count, &output)
        }
        guard let handle else { return nil }
        defer { captures_editor_draw_geometry_free_v1(handle) }
        points = UnsafeBufferPointer(start: output.data, count: output.length)
            .map { CGPoint(x: $0.x, y: $0.y) }
        strokeWidth = output.stroke_width
    }
}

/// Display values resolved by Rust, never a replacement for authored document data.
struct NativeAnnotationStyle: Equatable {
    let closed: Bool
    var color: String
    var fill: String?
    var strokeWidth: Double
    var strokeEnabled: Bool
    var dropShadow: Bool
    var shadowColor: String
    var shadowOpacity: Double
    var shadowBlur: Double
    var shadowX: Double
    var shadowY: Double

    init?(_ value: [String: Any]) {
        guard let closed = value["closed"] as? Bool,
              let color = value["color"] as? String,
              let width = value["strokeWidth"] as? Double,
              let stroke = value["strokeEnabled"] as? Bool,
              let enabled = value["dropShadow"] as? Bool,
              let shadow = value["dropShadowStyle"] as? [String: Any],
              let shadowColor = shadow["color"] as? String,
              let opacity = shadow["opacity"] as? Double,
              let blur = shadow["blur"] as? Double,
              let x = shadow["offsetX"] as? Double,
              let y = shadow["offsetY"] as? Double else { return nil }
        self.closed = closed; self.color = color; fill = value["fill"] as? String
        strokeWidth = width; strokeEnabled = stroke; dropShadow = enabled
        self.shadowColor = shadowColor; shadowOpacity = opacity; shadowBlur = blur
        shadowX = x; shadowY = y
    }

    func patch(from original: Self) -> [String: Any] {
        var patch: [String: Any] = [:]
        if color != original.color { patch["color"] = color }
        if strokeWidth != original.strokeWidth { patch["strokeWidth"] = strokeWidth }
        if closed {
            if fill != original.fill { patch["fill"] = fill.map { $0 as Any } ?? NSNull() }
            if strokeEnabled != original.strokeEnabled { patch["strokeEnabled"] = strokeEnabled }
        }
        if dropShadow != original.dropShadow { patch["dropShadow"] = dropShadow }
        if dropShadow {
            var shadow: [String: Any] = [:]
            if shadowColor != original.shadowColor { shadow["color"] = shadowColor }
            if shadowOpacity != original.shadowOpacity { shadow["opacity"] = shadowOpacity }
            if shadowBlur != original.shadowBlur { shadow["blur"] = shadowBlur }
            if shadowX != original.shadowX { shadow["offsetX"] = shadowX }
            if shadowY != original.shadowY { shadow["offsetY"] = shadowY }
            if !shadow.isEmpty { patch["dropShadowStyle"] = shadow }
        }
        return patch
    }
}

struct NativeEditorLayer: Equatable {
    enum Kind: String {
        case image, text, shape, path
    }

    let id: String
    let name: String
    let kind: Kind
    let visible: Bool
    let locked: Bool
    let opacity: Double
    let x: Double
    let y: Double
    let annotation: NativeAnnotationStyle?

    init?(_ value: [String: Any], annotation: [String: Any]? = nil) {
        guard let id = value["id"] as? String, !id.isEmpty,
              let rawKind = value["kind"] as? String,
              let kind = Kind(rawValue: rawKind),
              let visible = value["visible"] as? Bool,
              let locked = value["locked"] as? Bool,
              let opacity = value["opacity"] as? NSNumber,
              let x = value["x"] as? NSNumber,
              let y = value["y"] as? NSNumber else { return nil }
        self.id = id; self.kind = kind
        self.visible = visible; self.locked = locked
        self.opacity = opacity.doubleValue; self.x = x.doubleValue; self.y = y.doubleValue
        self.annotation = annotation.flatMap(NativeAnnotationStyle.init)
        if annotation != nil && self.annotation == nil { return nil }
        switch kind {
        case .image: name = (value["name"] as? String) ?? "Image"
        case .text: name = "Text"
        case .shape: name = "Shape"
        case .path: name = "Drawing"
        }
    }
}

struct NativeEditorSnapshot: Equatable {
    let artifactID: String
    let width: Double
    let height: Double
    let canUndo: Bool
    let canRedo: Bool
    let unsavedChanges: Bool
    let hasDraft: Bool
    /// Shared documents store back-to-front. Native layer panels display front-to-back.
    let layers: [NativeEditorLayer]

    init?(_ value: [String: Any]) {
        guard let artifactID = value["artifact_id"] as? String,
              let document = value["document"] as? [String: Any],
              let width = document["width"] as? NSNumber,
              let height = document["height"] as? NSNumber,
              let canUndo = value["can_undo"] as? Bool,
              let canRedo = value["can_redo"] as? Bool,
              let unsavedChanges = value["unsaved_changes"] as? Bool,
              let hasDraft = value["has_draft"] as? Bool,
              width.doubleValue > 0, height.doubleValue > 0 else { return nil }
        let elements = document["elements"] as? [[String: Any]] ?? []
        let annotations = value["annotation_controls"] as? [String: [String: Any]] ?? [:]
        let layers = elements.compactMap { element in
            NativeEditorLayer(element, annotation: (element["id"] as? String).flatMap { annotations[$0] })
        }
        guard layers.count == elements.count else { return nil }
        self.artifactID = artifactID
        self.width = width.doubleValue; self.height = height.doubleValue
        self.canUndo = canUndo; self.canRedo = canRedo
        self.unsavedChanges = unsavedChanges; self.hasDraft = hasDraft
        self.layers = Array(layers.reversed())
    }
}

struct EditorPresentation {
    let snapshot: NativeEditorSnapshot
    let image: CGImage
}

struct EditorOutputPresentation {
    let data: Data
    let image: CGImage

    var length: Int { data.count }
}

struct EditorDecodedImage: Equatable {
    let data: Data
    let width: Int
    let height: Int
    let bytesPerRow: Int
    let name: String
}

struct EditorImportPresentation {
    let layerID: String
    let presentation: EditorPresentation
}

enum EditorSavePresentation: Equatable {
    case saved(path: String)
    case savedWithoutHistory(path: String, warning: String)
}

/// Independently retained immutable Rust pixels. The CGImage provider retains
/// this frame, so draws may safely finish after a later edit or session close.
final class NativeEditorFrame {
    private let handle: OpaquePointer

    init(handle: OpaquePointer) { self.handle = handle }
    deinit { captures_editor_frame_free_v1(handle) }

    func image() throws -> CGImage {
        var pixels = CapturesRegionPixels()
        guard captures_editor_frame_pixels_v1(handle, &pixels),
              let data = pixels.data, pixels.width > 0, pixels.height > 0 else {
            throw AppBridgeError.invalidResponse
        }
        let retained = Unmanaged.passRetained(self)
        guard let provider = CGDataProvider(dataInfo: retained.toOpaque(), data: data,
            size: pixels.length, releaseData: { info, _, _ in
                if let info { Unmanaged<NativeEditorFrame>.fromOpaque(info).release() }
            }) else {
            retained.release(); throw AppBridgeError.invalidResponse
        }
        guard let image = CGImage(width: Int(pixels.width), height: Int(pixels.height),
            bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: pixels.bytes_per_row,
            space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
            provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
        else { throw AppBridgeError.invalidResponse }
        return image
    }
}

private final class NativeEditorSession {
    private let handle: OpaquePointer

    private init(handle: OpaquePointer) { self.handle = handle }
    deinit { captures_editor_free_v1(handle) }

    static func open(historyRoot: String, draftsRoot: String,
                     artifactID: String) throws -> (NativeEditorSession, NativeEditorSnapshot) {
        let data = try JSONSerialization.data(withJSONObject: [
            "history_root": historyRoot, "drafts_root": draftsRoot,
            "artifact_id": artifactID,
        ], options: [.sortedKeys])
        var response: UnsafeMutablePointer<CChar>?
        let handle = String(decoding: data, as: UTF8.self).withCString {
            captures_editor_open_v1($0, &response)
        }
        defer { captures_settings_free_v1(response) }
        do {
            guard let response else { throw AppBridgeError.invalidResponse }
            let result = try AppBridge.decode(Data(bytes: response, count: strlen(response)))
            guard let handle, let snapshot = NativeEditorSnapshot(result) else {
                throw AppBridgeError.invalidResponse
            }
            return (NativeEditorSession(handle: handle), snapshot)
        } catch {
            captures_editor_free_v1(handle)
            throw error
        }
    }

    func request(_ object: [String: Any]) throws -> NativeEditorSnapshot {
        let data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
        let response = String(decoding: data, as: UTF8.self).withCString {
            captures_editor_request_v1(handle, $0)
        }
        guard let response else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(response) }
        let result = try AppBridge.decode(Data(bytes: response, count: strlen(response)))
        guard let snapshot = NativeEditorSnapshot(result) else { throw AppBridgeError.invalidResponse }
        return snapshot
    }

    func presentation(_ snapshot: NativeEditorSnapshot) throws -> EditorPresentation {
        guard let handle = captures_editor_frame_v1(handle) else {
            throw AppBridgeError.invalidResponse
        }
        let frame = NativeEditorFrame(handle: handle)
        return EditorPresentation(snapshot: snapshot, image: try frame.image())
    }

    func encode(_ options: [String: Any]) throws -> EditorOutputPresentation {
        let data = try JSONSerialization.data(withJSONObject: options, options: [.sortedKeys])
        var response: UnsafeMutablePointer<CChar>?
        let exported = String(decoding: data, as: UTF8.self).withCString {
            captures_editor_encode_v1(handle, $0, &response)
        }
        defer { captures_settings_free_v1(response) }
        guard let response else {
            captures_editor_export_free_v1(exported)
            throw AppBridgeError.invalidResponse
        }
        let result: [String: Any]
        do { result = try AppBridge.decode(Data(bytes: response, count: strlen(response))) }
        catch {
            captures_editor_export_free_v1(exported)
            throw error
        }
        guard let exported, let expectedLength = (result["length"] as? NSNumber)?.intValue else {
            captures_editor_export_free_v1(exported)
            throw AppBridgeError.invalidResponse
        }
        defer { captures_editor_export_free_v1(exported) }
        var bytes = CapturesEditorBytes()
        guard captures_editor_export_bytes_v1(exported, &bytes),
              let pointer = bytes.data, bytes.length == expectedLength else {
            throw AppBridgeError.invalidResponse
        }
        let encoded = Data(bytes: pointer, count: bytes.length)
        guard let source = CGImageSourceCreateWithData(encoded as CFData, nil),
              let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
            throw AppBridgeError.invalidResponse
        }
        return EditorOutputPresentation(data: encoded, image: image)
    }

    func saveNew(_ request: [String: Any]) throws -> EditorSavePresentation {
        let data = try JSONSerialization.data(withJSONObject: request, options: [.sortedKeys])
        let response = String(decoding: data, as: UTF8.self).withCString {
            captures_editor_save_new_v1(handle, $0)
        }
        guard let response else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(response) }
        let result = try AppBridge.decode(Data(bytes: response, count: strlen(response)))
        guard let status = result["status"] as? String,
              let path = result["path"] as? String, !path.isEmpty else {
            throw AppBridgeError.invalidResponse
        }
        switch status {
        case "saved":
            guard result["artifact"] is [String: Any] else { throw AppBridgeError.invalidResponse }
            return .saved(path: path)
        case "saved_without_history":
            guard let warning = result["warning"] as? String, !warning.isEmpty else {
                throw AppBridgeError.invalidResponse
            }
            return .savedWithoutHistory(path: path, warning: warning)
        default:
            throw AppBridgeError.invalidResponse
        }
    }

    func importImage(_ image: EditorDecodedImage, selectedID: String?) throws
        -> EditorImportPresentation {
        var request: [String: Any] = ["name": image.name]
        if let selectedID { request["selected_id"] = selectedID }
        let requestData = try JSONSerialization.data(withJSONObject: request, options: [.sortedKeys])
        guard let width = UInt32(exactly: image.width), let height = UInt32(exactly: image.height),
              width > 0, height > 0 else { throw AppBridgeError.invalidResponse }
        let result: [String: Any] = try image.data.withUnsafeBytes { bytes in
            guard let data = bytes.bindMemory(to: UInt8.self).baseAddress else {
                throw AppBridgeError.invalidResponse
            }
            var pixels = CapturesRegionPixels(
                data: data, length: image.data.count,
                width: width, height: height,
                bytes_per_row: image.bytesPerRow)
            let response = String(decoding: requestData, as: UTF8.self).withCString {
                captures_editor_import_image_v1(handle, &pixels, $0)
            }
            guard let response else { throw AppBridgeError.invalidResponse }
            defer { captures_settings_free_v1(response) }
            return try AppBridge.decode(Data(bytes: response, count: strlen(response)))
        }
        guard let layerID = result["layer_id"] as? String,
              let snapshotValue = result["snapshot"] as? [String: Any],
              let snapshot = NativeEditorSnapshot(snapshotValue) else {
            throw AppBridgeError.invalidResponse
        }
        return EditorImportPresentation(layerID: layerID,
            presentation: try presentation(snapshot))
    }
}

protocol EditorWorking: AnyObject {
    func open(historyRoot: String, draftsRoot: String, artifactID: String,
              completion: @escaping (Result<EditorPresentation, Error>) -> Void)
    func request(_ object: [String: Any],
                 completion: @escaping (Result<EditorPresentation, Error>) -> Void)
    func encode(_ options: [String: Any],
                completion: @escaping (Result<EditorOutputPresentation, Error>) -> Void)
    func saveNew(_ request: [String: Any],
                 completion: @escaping (Result<EditorSavePresentation, Error>) -> Void)
    func importImage(_ image: EditorDecodedImage, selectedID: String?,
                     completion: @escaping (Result<EditorImportPresentation, Error>) -> Void)
    func close()
    func prepareForTermination() -> Result<Void, Error>
}

/// The opaque mutable session never leaves this queue. Frame ownership is split
/// before delivery to AppKit and remains valid independently of the session.
final class EditorWorker: EditorWorking {
    private static let queue = DispatchQueue(label: "es.captures.native.editor",
                                             qos: .userInitiated)
    private final class Storage {
        var session: NativeEditorSession?
        var snapshot: NativeEditorSnapshot?
    }
    private let storage = Storage()

    deinit {
        let storage = storage
        Self.queue.async { storage.snapshot = nil; storage.session = nil }
    }

    func open(historyRoot: String, draftsRoot: String, artifactID: String,
              completion: @escaping (Result<EditorPresentation, Error>) -> Void) {
        let storage = storage
        Self.queue.async {
            let result = Result { () throws -> EditorPresentation in
                storage.session = nil; storage.snapshot = nil
                let (session, snapshot) = try NativeEditorSession.open(
                    historyRoot: historyRoot, draftsRoot: draftsRoot, artifactID: artifactID)
                storage.session = session; storage.snapshot = snapshot
                return try session.presentation(snapshot)
            }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func request(_ object: [String: Any],
                 completion: @escaping (Result<EditorPresentation, Error>) -> Void) {
        let storage = storage
        Self.queue.async {
            let result = Result { () throws -> EditorPresentation in
                guard let session = storage.session else {
                    throw AppBridgeError.backend("The screenshot editor is closed.")
                }
                let snapshot = try session.request(object)
                storage.snapshot = snapshot
                return try session.presentation(snapshot)
            }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func encode(_ options: [String: Any],
                completion: @escaping (Result<EditorOutputPresentation, Error>) -> Void) {
        let storage = storage
        Self.queue.async {
            let result = Result { () throws -> EditorOutputPresentation in
                guard let session = storage.session else {
                    throw AppBridgeError.backend("The screenshot editor is closed.")
                }
                return try session.encode(options)
            }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func saveNew(_ request: [String: Any],
                 completion: @escaping (Result<EditorSavePresentation, Error>) -> Void) {
        let storage = storage
        Self.queue.async {
            let result = Result { () throws -> EditorSavePresentation in
                guard let session = storage.session else {
                    throw AppBridgeError.backend("The screenshot editor is closed.")
                }
                return try session.saveNew(request)
            }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func importImage(_ image: EditorDecodedImage, selectedID: String?,
                     completion: @escaping (Result<EditorImportPresentation, Error>) -> Void) {
        let storage = storage
        Self.queue.async {
            let result = Result { () throws -> EditorImportPresentation in
                guard let session = storage.session else {
                    throw AppBridgeError.backend("The screenshot editor is closed.")
                }
                let imported = try session.importImage(image, selectedID: selectedID)
                storage.snapshot = imported.presentation.snapshot
                return imported
            }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func close() {
        let storage = storage
        Self.queue.async {
            storage.snapshot = nil
            storage.session = nil
        }
    }

    /// Called on AppKit's termination path. It waits behind every accepted edit,
    /// saves the newest state, and frees only after that save succeeds.
    func prepareForTermination() -> Result<Void, Error> {
        let storage = storage
        return Self.queue.sync {
            Result {
                guard let session = storage.session else { return }
                if storage.snapshot?.unsavedChanges == true {
                    storage.snapshot = try session.request([
                        "operation": "save_draft",
                        "updated_at_ms": Self.timestamp(),
                    ])
                }
                storage.snapshot = nil; storage.session = nil
            }
        }
    }

    static func flush() { queue.sync {} }
    static func timestamp(_ date: Date = Date()) -> UInt64 {
        UInt64(max(0, date.timeIntervalSince1970 * 1_000))
    }
}
