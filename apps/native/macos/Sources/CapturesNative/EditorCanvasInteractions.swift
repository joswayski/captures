import AppKit
import CCapturesSettings
import UniformTypeIdentifiers

/// Shipping canvas interactions (`ScreenshotEditor.tsx`, `styles/editor-image.css`)
/// for AppKit: image file drop guides, the Expand canvas overflow action and
/// line/arrow curve editing. Geometry and copy come from
/// `captures_app::editor_canvas` through snapshots and
/// `captures_editor_canvas_query_v1`, shared with the wgpu host.

private func canvasPoint(_ value: Any?) -> CGPoint? {
    guard let value = value as? [String: Any], let x = value["x"] as? NSNumber,
          let y = value["y"] as? NSNumber else { return nil }
    return CGPoint(x: x.doubleValue, y: y.doubleValue)
}

private func canvasRect(_ value: Any?) -> CGRect? {
    guard let value = value as? [String: Any], let x = value["x"] as? NSNumber,
          let y = value["y"] as? NSNumber, let width = value["width"] as? NSNumber,
          let height = value["height"] as? NSNumber else { return nil }
    return CGRect(x: x.doubleValue, y: y.doubleValue, width: width.doubleValue, height: height.doubleValue)
}

private func canvasPoints(_ value: Any?) -> [CGPoint]? {
    guard let values = value as? [Any] else { return nil }
    let points = values.compactMap(canvasPoint)
    return points.count == values.count ? points : nil
}

/// Curve dots and inspector state for one line/arrow (snapshot `curve_handles`).
struct NativeCurveHandles: Equatable {
    let start: CGPoint
    let end: CGPoint
    let controls: [CGPoint]
    let starters: [CGPoint]
    let bendPercent: Double
    let slider: Bool
    let straightenLabel: String
    let path: [CGPoint]

    init?(_ value: [String: Any]) {
        guard let start = canvasPoint(value["start"]), let end = canvasPoint(value["end"]),
              let controls = canvasPoints(value["controls"]), let starters = canvasPoints(value["starters"]),
              let bend = value["bend_percent"] as? NSNumber, let slider = value["slider"] as? Bool,
              let label = value["straighten_label"] as? String,
              let path = canvasPoints(value["path"]) else { return nil }
        self.start = start; self.end = end; self.controls = controls; self.starters = starters
        bendPercent = bend.doubleValue; self.slider = slider; straightenLabel = label; self.path = path
    }
}

/// Idle overflow preview for a layer past the canvas edge (snapshot `canvas_expand`).
struct NativeCanvasExpand: Equatable {
    enum Edge: String { case left, top, right, bottom }
    let edges: [Edge]
    let rect: CGRect
    let gaps: [CGRect]
    let anchor: CGPoint
    let anchorEdge: Edge

    init?(_ value: [String: Any]) {
        guard let rawEdges = value["edges"] as? [String], let rect = canvasRect(value["rect"]),
              let rawGaps = value["gaps"] as? [Any], let anchor = canvasPoint(value["anchor"]),
              let rawAnchorEdge = value["anchor_edge"] as? String,
              let anchorEdge = Edge(rawValue: rawAnchorEdge) else { return nil }
        let edges = rawEdges.compactMap(Edge.init)
        let gaps = rawGaps.compactMap(canvasRect)
        guard edges.count == rawEdges.count, gaps.count == rawGaps.count else { return nil }
        self.edges = edges; self.rect = rect; self.gaps = gaps
        self.anchor = anchor; self.anchorEdge = anchorEdge
    }

    /// The action center pushed `inset` document units outward from its edge.
    func anchor(inset: CGFloat) -> CGPoint {
        switch anchorEdge {
        case .left: return CGPoint(x: anchor.x - inset, y: anchor.y)
        case .right: return CGPoint(x: anchor.x + inset, y: anchor.y)
        case .top: return CGPoint(x: anchor.x, y: anchor.y - inset)
        case .bottom: return CGPoint(x: anchor.x, y: anchor.y + inset)
        }
    }
}

/// Live image-drop placement guide (`image_drop_guide`).
struct NativeEditorDropGuide: Equatable {
    let placement: String
    let label: String
    let target: CGRect
    let point: CGPoint
    let focus: CGRect

    init?(_ value: [String: Any]) {
        guard let placement = value["placement"] as? String, let label = value["label"] as? String,
              let target = canvasRect(value["target"]), let point = canvasPoint(value["point"]),
              let focus = canvasRect(value["focus"]) else { return nil }
        self.placement = placement; self.label = label; self.target = target
        self.point = point; self.focus = focus
    }
}

/// Result of a curve handle/path query at a pointer.
struct NativeCurveHit: Equatable {
    /// `{kind, index?}` JSON for a curve `move` edit, or nil when no handle is hit.
    let handle: [String: AnyHashable]?
    let hint: String?
    let onPath: Bool
    let closest: CGPoint?

    var handleKind: String? { handle?["kind"] as? String }
    var handleIndex: Int? { handle?["index"] as? Int }
}

enum NativeEditorCanvas {
    /// Shipping copy; the geometry behind each lives in Rust.
    static let expandCanvas = "Expand canvas"
    static let expandInset: CGFloat = 22
    static let curveLabel = "Curve"
    static let curveMarks = ["Left", "Straight", "Right"]
    static let curveHelp = "Drag the curve dots to reshape. Double-click the path to add more points; double-click a point to remove it."
    static let dropImage = "Drop image"
    static let dropUnsupported = "Drop PNG, JPEG, WebP, TIFF, GIF, BMP, or SVG image files."

    static func isSupportedImage(_ url: URL) -> Bool {
        guard url.isFileURL, let type = UTType(filenameExtension: url.pathExtension.lowercased()) else { return false }
        return type.conforms(to: .image)
    }

    private static func query(documentJSON: String, _ request: [String: Any]) throws -> [String: Any] {
        let data = try JSONSerialization.data(withJSONObject: request, options: [.sortedKeys])
        let response = documentJSON.withCString { document in
            String(decoding: data, as: UTF8.self).withCString { request in
                captures_editor_canvas_query_v1(document, request)
            }
        }
        guard let response else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(response) }
        return try AppBridge.decode(Data(bytes: response, count: strlen(response)))
    }

    static func dropGuide(documentJSON: String, selectedID: String?, point: CGPoint?) throws -> NativeEditorDropGuide {
        let result = try query(documentJSON: documentJSON, [
            "operation": "drop_guide",
            "selected_id": selectedID.map { $0 as Any } ?? NSNull(),
            "point": point.map { ["x": Double($0.x), "y": Double($0.y)] as Any } ?? NSNull(),
        ])
        guard let guide = NativeEditorDropGuide(result) else { throw AppBridgeError.invalidResponse }
        return guide
    }

    static func shapeBodyHit(documentJSON: String, layerID: String, shape: String,
                             point: CGPoint, radius: Double) throws -> Bool {
        let result = try query(documentJSON: documentJSON, [
            "operation": "shape_body", "id": layerID, "shape": shape,
            "point": ["x": Double(point.x), "y": Double(point.y)], "radius": radius,
        ])
        guard let hit = result["hit"] as? Bool else { throw AppBridgeError.invalidResponse }
        return hit
    }

    static func curveHit(documentJSON: String, layerID: String, point: CGPoint, radius: Double) throws -> NativeCurveHit {
        let result = try query(documentJSON: documentJSON, [
            "operation": "curve", "id": layerID,
            "point": ["x": Double(point.x), "y": Double(point.y)], "radius": radius,
        ])
        var handle: [String: AnyHashable]?
        if let raw = result["handle"] as? [String: Any], let kind = raw["kind"] as? String {
            var value: [String: AnyHashable] = ["kind": kind]
            if let index = raw["index"] as? NSNumber { value["index"] = index.intValue }
            handle = value
        }
        return NativeCurveHit(handle: handle, hint: result["hint"] as? String,
                              onPath: result["on_path"] as? Bool ?? false,
                              closest: canvasPoint(result["closest"]))
    }

    static func curvePreview(documentJSON: String, layerID: String, handle: [String: AnyHashable],
                             point: CGPoint) throws -> NativeCurveHandles {
        let result = try query(documentJSON: documentJSON, [
            "operation": "curve_preview", "id": layerID, "handle": handle as [String: Any],
            "point": ["x": Double(point.x), "y": Double(point.y)],
        ])
        guard let handles = NativeCurveHandles(result) else { throw AppBridgeError.invalidResponse }
        return handles
    }
}

/// Transparent overlay for the drop guide, edge glow, stack light and toast.
/// Never takes pointer events; the gesture views below keep the drag session.
final class EditorDropGuideView: NSView {
    override var isFlipped: Bool { true }
    var guide: NativeEditorDropGuide? { didSet { guideChanged() } }
    /// A file drag is over the canvas even when no guide could be computed.
    var active = false { didSet { guideChanged() } }
    /// Resolves the bloom breathing and edge pulse; nil holds them still.
    var motionTokens: Tokens?
    var reducedMotion: () -> Bool = { NativeMotion.reduceMotion } {
        didSet { effectClock.reducedMotion = reducedMotion }
    }
    private lazy var effectClock = NativeEdgeEffectClock(view: self)
    /// True while the edge bloom, pulse and particles schedule redraws.
    var isAnimating: Bool { effectClock.isAnimating }
    /// The side the image joins while an edge snap (not a stack) shows.
    var glowingEdge: NativeCanvasExpand.Edge? {
        guard active, let guide else { return nil }
        return NativeCanvasExpand.Edge(rawValue: guide.placement)
    }
    var imageRect: () -> NSRect = { .zero }
    var canvasSize = NSSize.zero
    var accent = NSColor.controlAccentColor
    var glassFill = NSColor.windowBackgroundColor
    var glassText = NSColor.labelColor
    var toastLabel: String { guide?.label ?? NativeEditorCanvas.dropImage }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    private func guideChanged() {
        isHidden = guide == nil && !active
        effectClock.update(running: glowingEdge != nil)
        needsDisplay = true
    }

    func project(_ rect: CGRect) -> CGRect {
        let image = imageRect()
        guard canvasSize.width > 0, canvasSize.height > 0 else { return .zero }
        let scale = image.width / canvasSize.width
        return CGRect(x: image.minX + rect.minX * scale, y: image.minY + rect.minY * scale,
                      width: rect.width * scale, height: rect.height * scale)
    }

    /// Toast frame at the top center of the viewport (`.screenshot-drop-overlay`).
    var toastFrame: CGRect {
        let font = NSFont.systemFont(ofSize: 13, weight: .semibold)
        let width = ceil((toastLabel as NSString).size(withAttributes: [.font: font]).width) + 44 + 16
        return CGRect(x: bounds.midX - width / 2, y: 16, width: width, height: 40)
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard active else { return }
        if let guide {
            let target = project(guide.target)
            if guide.placement == "stack" {
                drawStackLight(project(guide.focus))
            } else {
                accent.withAlphaComponent(0.1).setFill()
                let plate = NSBezierPath(roundedRect: target, xRadius: 3, yRadius: 3)
                plate.fill()
                accent.withAlphaComponent(0.78).setStroke(); plate.lineWidth = 1; plate.stroke()
                if let edge = NativeCanvasExpand.Edge(rawValue: guide.placement),
                   let context = NSGraphicsContext.current?.cgContext {
                    // `min(96px, 42%)` deep, overhanging each end by 8 %.
                    let across = edge == .top || edge == .bottom ? target.height : target.width
                    let depth = min(NativeEditorPreviewPaint.snap("bloom", 96),
                                    NativeEditorPreviewPaint.snap("bloom_fraction", 0.42) * max(0, across))
                    NativeEdgeEffects.drawAccentEdge(context, target: target, edge: edge, depth: depth,
                        overhang: NativeEditorPreviewPaint.snap("bloom_overhang", 0.08), accent: accent,
                        tokens: motionTokens, elapsed: effectClock.elapsed, reduced: reducedMotion())
                }
            }
        }
        let toast = toastFrame
        let plate = NSBezierPath(roundedRect: toast, xRadius: 12, yRadius: 12)
        glassFill.setFill(); plate.fill()
        accent.withAlphaComponent(0.72).setStroke(); plate.lineWidth = 1; plate.stroke()
        let icon = NSBezierPath(roundedRect: CGRect(x: toast.minX + 15, y: toast.midY - 8, width: 18, height: 16),
                                xRadius: 3, yRadius: 3)
        accent.setStroke(); icon.lineWidth = 1.7; icon.stroke()
        let font = NSFont.systemFont(ofSize: 13, weight: .semibold)
        let text = toastLabel as NSString
        let size = text.size(withAttributes: [.font: font])
        text.draw(at: CGPoint(x: toast.minX + 44, y: toast.midY - size.height / 2),
                  withAttributes: [.font: font, .foregroundColor: glassText])
    }

    private func drawStackLight(_ focus: CGRect) {
        for (grow, alpha) in [(CGFloat(0.62), 0.05), (0.42, 0.09), (0.22, 0.14)] {
            NSColor(srgbRed: 1, green: 0.965, blue: 0.91, alpha: alpha).setFill()
            NSBezierPath(roundedRect: focus.insetBy(dx: -focus.width * grow, dy: -focus.height * grow),
                         xRadius: focus.height, yRadius: focus.height).fill()
        }
        for (offset, alpha) in [(CGFloat(18), 0.12), (8, 0.22), (2, 0.28)] {
            NSColor(white: 0, alpha: alpha).setFill()
            NSBezierPath(roundedRect: focus.offsetBy(dx: 0, dy: offset).insetBy(dx: -offset / 2, dy: -offset / 2),
                         xRadius: 8, yRadius: 8).fill()
        }
        let rim = NSBezierPath(roundedRect: focus, xRadius: 8, yRadius: 8)
        NSColor(white: 1, alpha: 0.36).setStroke(); rim.lineWidth = 1; rim.stroke()
    }
}

/// Inspector Curve section: a slider for straight/single-control strokes that
/// commits once on release, Straighten for multi-point strokes, and help copy.
final class EditorCurveControls: NSView {
    override var isFlipped: Bool { true }
    var apply: ([String: Any]) -> Void = { _ in }
    var resized: (CGFloat) -> Void = { _ in }
    private let tokens: Tokens
    private let heading = NSTextField(labelWithString: NativeEditorCanvas.curveLabel)
    /// Shipping `RangeSlider` (#841 primitive) with Left/Straight/Right marks.
    let bendSlider = TokenSlider(value: 0, minValue: -100, maxValue: 100, target: nil, action: nil)
    let bendValue = NSTextField(labelWithString: "0%")
    private var marks: [NSTextField] = []
    private(set) var straightenButton: CaptureButton!
    private let help = NSTextField(wrappingLabelWithString: NativeEditorCanvas.curveHelp)
    private(set) var handles: NativeCurveHandles?
    private var stagedBend: Double?
    private var ready = false

    init(tokens: Tokens, width: CGFloat = ScreenshotEditorController.contentWidth) {
        self.tokens = tokens
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: 0))
        setAccessibilityLabel("Curve controls")
        heading.font = .systemFont(ofSize: 13, weight: .semibold)
        heading.textColor = tokens.color("text")
        heading.frame = NSRect(x: 0, y: 0, width: 200, height: 24)
        addSubview(heading)
        bendValue.alignment = .right
        bendValue.font = .monospacedDigitSystemFont(ofSize: 12, weight: .regular)
        bendValue.textColor = tokens.color("text-muted")
        bendValue.frame = NSRect(x: width - 60, y: 2, width: 60, height: 20)
        addSubview(bendValue)
        bendSlider.frame = NSRect(x: 0, y: 26, width: width, height: 24)
        bendSlider.tokens = tokens
        bendSlider.isContinuous = false
        bendSlider.numberOfTickMarks = 3
        bendSlider.target = self; bendSlider.action = #selector(bendReleased)
        bendSlider.setAccessibilityLabel(NativeEditorCanvas.curveLabel)
        addSubview(bendSlider)
        for (index, title) in NativeEditorCanvas.curveMarks.enumerated() {
            let mark = NSTextField(labelWithString: title)
            mark.font = .systemFont(ofSize: 11); mark.textColor = tokens.color("text-subtle")
            mark.alignment = index == 0 ? .left : index == 1 ? .center : .right
            mark.frame = NSRect(x: CGFloat(index) * width / 3, y: 52, width: width / 3 - 1, height: 16)
            addSubview(mark); marks.append(mark)
        }
        straightenButton = CaptureButton("Straighten line", frame: NSRect(x: 0, y: 26, width: width, height: 30),
                                         tokens: tokens) { [weak self] in
            guard let self, self.ready else { return }
            self.apply(["kind": "straighten"])
        }
        addSubview(straightenButton)
        help.font = .systemFont(ofSize: 11); help.textColor = tokens.color("text-muted")
        help.frame = NSRect(x: 0, y: 74, width: width, height: 44)
        addSubview(help)
        setHandles(nil)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func setHandles(_ value: NativeCurveHandles?) {
        handles = value
        stagedBend = nil
        isHidden = value == nil
        let slider = value?.slider ?? true
        bendSlider.isHidden = !slider; bendValue.isHidden = !slider
        marks.forEach { $0.isHidden = !slider }
        straightenButton.isHidden = slider
        if let value {
            bendSlider.doubleValue = value.bendPercent
            bendValue.stringValue = "\(Int(value.bendPercent))%"
            straightenButton.title = value.straightenLabel
            straightenButton.setAccessibilityLabel(value.straightenLabel)
        }
        help.frame.origin.y = slider ? 74 : 62
        let height: CGFloat = value == nil ? 0 : help.frame.maxY + 8
        frame.size.height = height
        refresh()
        resized(height)
    }

    func setReady(_ value: Bool) { ready = value; refresh() }

    private func refresh() {
        bendSlider.isEnabled = ready && handles != nil
        straightenButton.isEnabled = ready && handles != nil
    }

    @objc private func bendReleased() {
        guard ready, let handles else { return }
        let percent = bendSlider.doubleValue.rounded()
        bendValue.stringValue = "\(Int(percent))%"
        guard percent != (stagedBend ?? handles.bendPercent) else { return }
        stagedBend = percent
        apply(["kind": "bend", "bend": percent / 100])
    }
}
