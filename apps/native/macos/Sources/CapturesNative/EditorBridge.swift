import Foundation
import CoreGraphics
import ImageIO
import CCapturesSettings

/// UI-thread-only owner of shared crop aspect-latching and clamping geometry.
/// No worker session or JSON is borrowed during pointer feedback.
final class NativeEditorCropDrag {
    private let handle: OpaquePointer

    init?(origin: CGPoint, canvas: CGSize, aspect: Double, shift: Bool) {
        guard let handle = captures_editor_crop_begin_v1(
            CapturesSelectionPoint(x: origin.x, y: origin.y),
            CapturesSelectionBounds(width: canvas.width, height: canvas.height), aspect, shift)
        else { return nil }
        self.handle = handle
    }

    deinit { captures_editor_crop_free_v1(handle) }

    func update(current: CGPoint, aspect: Double, shift: Bool) -> CGRect? {
        var output = CapturesSelectionRect()
        guard captures_editor_crop_update_v1(handle,
            CapturesSelectionPoint(x: current.x, y: current.y), aspect, shift, &output)
        else { return nil }
        return CGRect(x: output.x, y: output.y, width: output.width, height: output.height)
    }
}

/// UI-thread-only, ephemeral viewport geometry. This never crosses the editor
/// session/JSON boundary and therefore cannot dirty a document or draft.
struct NativeEditorViewport: Equatable {
    var zoomPercent = 0.0
    var panX = 0.0
    var panY = 0.0

    private var native: CapturesEditorViewport {
        CapturesEditorViewport(zoom_percent: zoomPercent, pan_x: panX, pan_y: panY)
    }

    func rect(fit: CGRect, canvas: CGSize) -> CGRect? {
        var output = CapturesSelectionRect()
        guard captures_editor_viewport_rect_v1(native,
            CapturesSelectionRect(x: fit.minX, y: fit.minY, width: fit.width, height: fit.height),
            CapturesSelectionBounds(width: canvas.width, height: canvas.height), &output) else { return nil }
        return CGRect(x: output.x, y: output.y, width: output.width, height: output.height)
    }

    func zoomed(to percent: Double, anchor: CGPoint, fit: CGRect, canvas: CGSize) -> Self? {
        var output = CapturesEditorViewport()
        guard captures_editor_viewport_zoom_v1(native,
            CapturesSelectionRect(x: fit.minX, y: fit.minY, width: fit.width, height: fit.height),
            CapturesSelectionBounds(width: canvas.width, height: canvas.height), percent,
            CapturesSelectionPoint(x: anchor.x, y: anchor.y), &output) else { return nil }
        return Self(zoomPercent: output.zoom_percent, panX: output.pan_x, panY: output.pan_y)
    }

    static func wheelFactor(deltaPixels: Double) -> Double? {
        let value = captures_editor_viewport_wheel_factor_v1(deltaPixels)
        return value.isFinite && value > 0 ? value : nil
    }
}

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

struct NativeEditorRotationHandle {
    let anchor: CGPoint
    let handle: CGPoint
    let hitRadius: Double

    init?(outline: [CGPoint], radians: Double, displayScale: Double, canvas: CGSize) {
        guard outline.count == 4 else { return nil }
        let input = outline.map { CapturesSelectionPoint(x: $0.x, y: $0.y) }
        var output = CapturesEditorRotationHandle()
        let succeeded = input.withUnsafeBufferPointer {
            captures_editor_rotation_handle_v1($0.baseAddress, radians, displayScale,
                CapturesSelectionBounds(width: canvas.width, height: canvas.height), &output)
        }
        guard succeeded else { return nil }
        anchor = CGPoint(x: output.anchor.x, y: output.anchor.y)
        handle = CGPoint(x: output.handle.x, y: output.handle.y)
        hitRadius = output.hit_radius
    }
}

struct NativeEditorRotationPreview {
    let radians: Double
    let outline: [CGPoint]

    init?(outline: [CGPoint], radians: Double, start: CGPoint, current: CGPoint, snap: Bool) {
        guard outline.count == 4 else { return nil }
        let input = outline.map { CapturesSelectionPoint(x: $0.x, y: $0.y) }
        var output = CapturesEditorRotationPreview()
        let succeeded = input.withUnsafeBufferPointer {
            captures_editor_rotation_preview_v1($0.baseAddress, radians,
                CapturesSelectionPoint(x: start.x, y: start.y),
                CapturesSelectionPoint(x: current.x, y: current.y), snap, &output)
        }
        guard succeeded else { return nil }
        self.radians = output.radians
        self.outline = withUnsafePointer(to: &output.outline) {
            $0.withMemoryRebound(to: CapturesSelectionPoint.self, capacity: 4) {
                Array(UnsafeBufferPointer(start: $0, count: 4)).map { CGPoint(x: $0.x, y: $0.y) }
            }
        }
    }
}

struct NativeEditorAlignmentGuide: Equatable {
    enum Orientation: UInt32 { case vertical = 0, horizontal = 1 }
    let orientation: Orientation
    let position: Double
}

struct NativeEditorResizePreview {
    let outline: [CGPoint]
    let guides: [NativeEditorAlignmentGuide]

    init?(_ output: CapturesEditorResizePreview) {
        var output = output
        guard output.guide_count <= 4 else { return nil }
        outline = withUnsafePointer(to: &output.outline) {
            $0.withMemoryRebound(to: CapturesSelectionPoint.self, capacity: 4) {
                Array(UnsafeBufferPointer(start: $0, count: 4)).map { CGPoint(x: $0.x, y: $0.y) }
            }
        }
        let guideCount = output.guide_count
        guides = withUnsafePointer(to: &output.guides) {
            $0.withMemoryRebound(to: CapturesEditorAlignmentGuide.self, capacity: 4) {
                Array(UnsafeBufferPointer(start: $0, count: guideCount)).compactMap { guide -> NativeEditorAlignmentGuide? in
                    guard let orientation = NativeEditorAlignmentGuide.Orientation(rawValue: guide.orientation) else { return nil }
                    return NativeEditorAlignmentGuide(orientation: orientation, position: guide.position)
                }
            }
        }
        guard guides.count == guideCount else { return nil }
    }
}

/// Owns the immutable Rust drag for the complete gesture, including modifier changes.
/// Creating this owner before decoding the response guarantees malformed responses
/// cannot leak a successfully-created native drag.
final class NativeEditorResizeDrag {
    private let handle: OpaquePointer

    private init(handle: OpaquePointer) { self.handle = handle }
    deinit { captures_editor_resize_free_v1(handle) }

    static func begin(documentJSON: String, layerID: String, point: CGPoint,
                      displayScale: Double) throws -> (NativeEditorResizeDrag?, Int?) {
        var nativeDrag: OpaquePointer?
        let response = documentJSON.withCString { document in
            layerID.withCString { layer in
                captures_editor_resize_begin_v1(document, layer,
                    CapturesSelectionPoint(x: point.x, y: point.y), displayScale, &nativeDrag)
            }
        }
        let owner = nativeDrag.map(NativeEditorResizeDrag.init)
        guard let response else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(response) }
        let result = try AppBridge.decode(Data(bytes: response, count: strlen(response)))
        if result["handle"] is NSNull {
            guard owner == nil else { throw AppBridgeError.invalidResponse }
            return (nil, nil)
        }
        guard let number = result["handle"] as? NSNumber,
              (0...7).contains(number.intValue), let owner else {
            throw AppBridgeError.invalidResponse
        }
        return (owner, number.intValue)
    }

    func preview(current: CGPoint, lockAspect: Bool) -> NativeEditorResizePreview? {
        var output = CapturesEditorResizePreview()
        guard captures_editor_resize_preview_v1(handle,
            CapturesSelectionPoint(x: current.x, y: current.y), lockAspect, &output) else { return nil }
        return NativeEditorResizePreview(output)
    }
}

/// Owns the immutable original move geometry and snap lines for one pointer gesture.
final class NativeEditorMoveDrag {
    private let handle: OpaquePointer

    private init(handle: OpaquePointer) { self.handle = handle }
    deinit { captures_editor_move_free_v1(handle) }

    static func begin(documentJSON: String, layerID: String,
                      displayScale: Double) throws -> NativeEditorMoveDrag {
        var nativeDrag: OpaquePointer?
        let response = documentJSON.withCString { document in
            layerID.withCString { layer in
                captures_editor_move_begin_v1(document, layer, displayScale, &nativeDrag)
            }
        }
        let owner = nativeDrag.map(NativeEditorMoveDrag.init)
        guard let response else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(response) }
        _ = try AppBridge.decode(Data(bytes: response, count: strlen(response)))
        guard let owner else { throw AppBridgeError.invalidResponse }
        return owner
    }

    func preview(delta: CGPoint) -> NativeEditorResizePreview? {
        var output = CapturesEditorResizePreview()
        guard captures_editor_move_preview_v1(handle,
            CapturesSelectionPoint(x: delta.x, y: delta.y), &output) else { return nil }
        return NativeEditorResizePreview(output)
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
    let rotation: Double
    let selectionOutline: [CGPoint]?
    let annotation: NativeAnnotationStyle?
    let textStyle: NativeTextStyle?

    init?(_ value: [String: Any], annotation: [String: Any]? = nil,
          textShadow: [String: Any]? = nil,
          selectionOutline: [[String: Any]]? = nil) {
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
        rotation = (value["rotation"] as? NSNumber)?.doubleValue ?? 0
        self.annotation = annotation.flatMap(NativeAnnotationStyle.init)
        if annotation != nil && self.annotation == nil { return nil }
        textStyle = kind == .text ? NativeTextStyle(value, shadow: textShadow) : nil
        if kind == .text && textStyle == nil { return nil }
        if let selectionOutline {
            let points = selectionOutline.compactMap { point -> CGPoint? in
                guard let x = point["x"] as? NSNumber, let y = point["y"] as? NSNumber else { return nil }
                return CGPoint(x: x.doubleValue, y: y.doubleValue)
            }
            guard points.count == 4 else { return nil }
            self.selectionOutline = points
        } else { self.selectionOutline = nil }
        switch kind {
        case .image: name = (value["name"] as? String) ?? "Image"
        case .text: name = "Text"
        case .shape: name = "Shape"
        case .path: name = "Drawing"
        }
    }
}

/// Shared Rust resolves these display values; AppKit never authors defaults.
struct NativeTextShadowStyle: Equatable {
    let color: String
    let opacity: Double
    let blur: Double
    let offsetX: Double
    let offsetY: Double

    init?(_ value: [String: Any]) {
        guard let color = value["color"] as? String,
              let opacity = value["opacity"] as? Double,
              let blur = value["blur"] as? Double,
              let x = value["offsetX"] as? Double,
              let y = value["offsetY"] as? Double else { return nil }
        self.color = color; self.opacity = opacity; self.blur = blur
        offsetX = x; offsetY = y
    }
}

/// Authored text values plus optional resolved shadow controls from Rust.
/// `fontFamily` is not normalized: old drafts retain their own pinned font map.
struct NativeTextStyle: Equatable {
    let text: String
    let fontSize: Double
    let fontFamily: String
    let bold: Bool
    let italic: Bool
    let align: String
    let color: String
    let background: String?
    let roundedBackground: Bool
    let dropShadow: Bool
    let outlined: Bool
    let shadowStyle: NativeTextShadowStyle?

    init?(_ value: [String: Any], shadow: [String: Any]? = nil) {
        guard let text = value["text"] as? String,
              let size = value["fontSize"] as? NSNumber,
              let family = value["fontFamily"] as? String, !family.isEmpty,
              let bold = value["bold"] as? Bool,
              let italic = value["italic"] as? Bool,
              let outlined = value["outlined"] as? Bool,
              let align = value["align"] as? String,
              ["left", "center", "right"].contains(align),
              let color = value["color"] as? String,
              let rounded = value["roundedBackground"] as? Bool else { return nil }
        self.text = text; fontSize = size.doubleValue; fontFamily = family
        self.bold = bold; self.italic = italic; self.align = align; self.color = color
        background = value["background"] as? String; roundedBackground = rounded
        dropShadow = value["dropShadow"] as? Bool ?? false
        self.outlined = outlined
        shadowStyle = shadow.flatMap(NativeTextShadowStyle.init)
        if shadow != nil && shadowStyle == nil { return nil }
    }
}

struct NativeEditorSnapshot: Equatable {
    let artifactID: String
    let width: Double
    let height: Double
    let background: String?
    let canUndo: Bool
    let canRedo: Bool
    let unsavedChanges: Bool
    let hasDraft: Bool
    let fontFamilies: [String: String]
    /// Shared documents store back-to-front. Native layer panels display front-to-back.
    let layers: [NativeEditorLayer]
    /// Stable, sorted JSON used by pointer-down hit testing without touching the session.
    let documentJSON: String

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
        let textShadows = value["text_shadow_styles"] as? [String: [String: Any]] ?? [:]
        let outlines = value["selection_outlines"] as? [String: [[String: Any]]] ?? [:]
        let layers = elements.compactMap { element in
            let id = element["id"] as? String
            return NativeEditorLayer(element, annotation: id.flatMap { annotations[$0] },
                                     textShadow: id.flatMap { textShadows[$0] },
                                     selectionOutline: id.flatMap { outlines[$0] })
        }
        guard layers.count == elements.count else { return nil }
        guard let documentData = try? JSONSerialization.data(withJSONObject: document, options: [.sortedKeys]) else {
            return nil
        }
        self.artifactID = artifactID
        self.width = width.doubleValue; self.height = height.doubleValue
        self.background = document["background"] as? String
        self.canUndo = canUndo; self.canRedo = canRedo
        self.unsavedChanges = unsavedChanges; self.hasDraft = hasDraft
        self.fontFamilies = value["font_families"] as? [String: String] ?? [:]
        self.layers = Array(layers.reversed())
        self.documentJSON = String(decoding: documentData, as: UTF8.self)
    }
}

enum NativeEditorHitTesting {
    static func hit(documentJSON: String, point: CGPoint, tolerance: Double) throws -> String? {
        let response = documentJSON.withCString {
            captures_editor_hit_test_document_v1($0, point.x, point.y, tolerance)
        }
        guard let response else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(response) }
        let result = try AppBridge.decode(Data(bytes: response, count: strlen(response)))
        if result["hit"] is NSNull { return nil }
        guard let hit = result["hit"] as? String else { throw AppBridgeError.invalidResponse }
        return hit
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
