import AppKit
import UniformTypeIdentifiers

struct ScreenshotEditorState: Equatable {
    private(set) var generation = 0
    private(set) var artifactID: String?
    private(set) var snapshot: NativeEditorSnapshot?
    private(set) var busy = false

    mutating func beginOpen(artifactID: String) -> Int {
        generation += 1; self.artifactID = artifactID; snapshot = nil; busy = true
        return generation
    }

    mutating func beginCommand() -> Int? {
        guard artifactID != nil, snapshot != nil, !busy else { return nil }
        busy = true; return generation
    }

    mutating func complete(_ value: NativeEditorSnapshot, generation: Int) -> Bool {
        guard self.generation == generation, artifactID == value.artifactID else { return false }
        snapshot = value; busy = false; return true
    }

    mutating func completeOutput(generation: Int, artifactID: String) -> Bool {
        guard self.generation == generation, self.artifactID == artifactID,
              snapshot != nil else { return false }
        busy = false; return true
    }

    mutating func fail(generation: Int) -> Bool {
        guard self.generation == generation else { return false }
        busy = false; return true
    }

    mutating func close() { generation += 1; artifactID = nil; snapshot = nil; busy = false }
}

/// Shipping `.screenshot-layer-list li` in the native 32pt row: kind icon,
/// shipping layer name, muted kind label and eye/lock quick actions.
private final class EditorLayerCell: NSTableCellView {
    let titleLabel: NSTextField
    let detailLabel: NSTextField
    let iconName: String
    let visibilityButton: CaptureButton
    let lockButton: CaptureButton
    private let tokens: Tokens

    init(title: NSTextField, detail: NSTextField, iconName: String, tokens: Tokens,
         visibility: CaptureButton, lock: CaptureButton) {
        titleLabel = title; detailLabel = detail; self.iconName = iconName; self.tokens = tokens
        visibilityButton = visibility; lockButton = lock
        super.init(frame: .zero)
        addSubview(title); addSubview(detail); addSubview(visibility); addSubview(lock)
        textField = title
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override var isFlipped: Bool { true }

    override func layout() {
        super.layout()
        let inset: CGFloat = 8
        let gap: CGFloat = 8
        let trailingEdge = min(bounds.maxX, visibleRect.maxX) - 4
        lockButton.frame = NSRect(x: trailingEdge - 22, y: (bounds.height - 26) / 2, width: 22, height: 26)
        visibilityButton.frame = NSRect(x: lockButton.frame.minX - 22, y: lockButton.frame.minY,
                                        width: 22, height: 26)
        let actionsLeft = visibilityButton.frame.minX - 4
        let titleLeft = inset + 16 + gap
        let titleWidth = titleLabel.intrinsicContentSize.width
        let detailWidth = detailLabel.intrinsicContentSize.width
        // The name has priority; the kind shows when both fit.
        let showDetail = titleLeft + titleWidth + gap + detailWidth <= actionsLeft
        detailLabel.isHidden = !showDetail
        detailLabel.frame = NSRect(x: actionsLeft - detailWidth, y: (bounds.height - 16) / 2,
                                   width: showDetail ? detailWidth : 0, height: 16)
        let titleRight = showDetail ? detailLabel.frame.minX - gap : actionsLeft - gap
        titleLabel.frame = NSRect(x: titleLeft, y: (bounds.height - 18) / 2,
                                  width: max(0, titleRight - titleLeft), height: 18)
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        tokens.color("text-muted").withAlphaComponent(titleLabel.alphaValue).setStroke()
        ShippingIcons.stroke(iconName, in: NSRect(x: 8, y: (bounds.height - 16) / 2, width: 16, height: 16))
    }
}

class EditorViewportGestureView: NSView {
    var onViewportZoom: ((Double, NSPoint) -> Void)?
    var onViewportPan: ((NSPoint) -> Void)?
    var onViewportPanBegan: (() -> Void)?
    private var viewportPanPoint: NSPoint?
    var isViewportPanning: Bool { viewportPanPoint != nil }
    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? {
        let target = super.hitTest(point)
        return target is NSImageView ? self : target
    }

    func claimsViewportPan(_ event: NSEvent) -> Bool {
        (event.type == .otherMouseDown && event.buttonNumber == 2)
            || (event.type == .leftMouseDown && !event.modifierFlags.intersection([.command, .control]).isEmpty)
    }
    func beginViewportPan(_ event: NSEvent) -> Bool {
        guard claimsViewportPan(event) else { return false }
        window?.makeFirstResponder(self)
        onViewportPanBegan?()
        viewportPanPoint = convert(event.locationInWindow, from: nil); return true
    }
    func continueViewportPan(_ event: NSEvent) -> Bool {
        guard let previous = viewportPanPoint else { return false }
        let next = convert(event.locationInWindow, from: nil)
        viewportPanPoint = next
        onViewportPan?(NSPoint(x: next.x - previous.x, y: next.y - previous.y)); return true
    }
    func endViewportPan() { viewportPanPoint = nil }
    override func mouseDown(with event: NSEvent) { if !beginViewportPan(event) { super.mouseDown(with: event) } }
    override func mouseDragged(with event: NSEvent) { if !continueViewportPan(event) { super.mouseDragged(with: event) } }
    override func mouseUp(with event: NSEvent) { _ = continueViewportPan(event); endViewportPan() }
    override func otherMouseDown(with event: NSEvent) { _ = beginViewportPan(event) }
    override func otherMouseDragged(with event: NSEvent) { _ = continueViewportPan(event) }
    override func otherMouseUp(with event: NSEvent) { _ = continueViewportPan(event); endViewportPan() }
    override func scrollWheel(with event: NSEvent) {
        guard !event.modifierFlags.intersection([.command, .control]).isEmpty
        else { super.scrollWheel(with: event); return }
        let pixels = event.hasPreciseScrollingDeltas ? -event.scrollingDeltaY : -event.scrollingDeltaY * 10
        guard let factor = NativeEditorViewport.wheelFactor(deltaPixels: pixels) else { return }
        onViewportZoom?(factor, convert(event.locationInWindow, from: nil))
    }
    override func magnify(with event: NSEvent) {
        let factor = 1 + Double(event.magnification)
        guard factor.isFinite, factor > 0 else { return }
        onViewportZoom?(factor, convert(event.locationInWindow, from: nil))
    }
    func cancelViewportPan() { viewportPanPoint = nil }
    override func setFrameSize(_ newSize: NSSize) {
        if newSize != frame.size { cancelViewportPan() }
        super.setFrameSize(newSize)
    }
    override var acceptsFirstResponder: Bool { true }
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { cancelViewportPan() }
        else { super.keyDown(with: event) }
    }
    override func resignFirstResponder() -> Bool {
        cancelViewportPan(); return super.resignFirstResponder()
    }
}

final class EditorCropOverlay: EditorViewportGestureView {
    var tokens: Tokens? { didSet { needsDisplay = true } }
    var canvasSize = NSSize.zero { didSet { if canvasSize != oldValue { cancelGesture() } } }
    var imageRect: (() -> NSRect)?
    var selection: NSRect? { didSet { needsDisplay = true } }
    var aspect = 0.0 { didSet { updateModifier(shift: shiftHeld) } }
    var croppingEnabled = false {
        didSet {
            isHidden = !croppingEnabled
            if !croppingEnabled { cancelGesture() }
            window?.invalidateCursorRects(for: self)
        }
    }
    var onChange: ((NSRect) -> Void)?
    var onCancel: (() -> Void)?
    private var cropDrag: NativeEditorCropDrag?
    private var startPoint: NSPoint?
    private var currentPoint: NSPoint?
    private var shiftHeld = false
    private var moved = false

    private func canvasPoint(_ point: NSPoint) -> NSPoint {
        let image = imageRect?() ?? .zero
        return NSPoint(x: (point.x - image.minX) * canvasSize.width / image.width,
                       y: (point.y - image.minY) * canvasSize.height / image.height)
    }

    func begin(at point: NSPoint, shift: Bool = false) {
        guard croppingEnabled, bounds.contains(point),
              let image = imageRect?(), image.width > 0, image.height > 0 else { return }
        cancelGesture()
        cropDrag = NativeEditorCropDrag(origin: canvasPoint(point), canvas: canvasSize,
                                         aspect: aspect, shift: shift)
        startPoint = point; currentPoint = point; shiftHeld = shift
    }

    func drag(to point: NSPoint, shift: Bool = false) {
        guard let startPoint else { return }
        currentPoint = point; shiftHeld = shift
        moved = moved || hypot(point.x - startPoint.x, point.y - startPoint.y) >= 3
        guard moved, let rect = cropDrag?.update(current: canvasPoint(point), aspect: aspect, shift: shift)
        else { return }
        selection = rect; onChange?(rect)
    }

    func updateModifier(shift: Bool) {
        shiftHeld = shift
        if let currentPoint { drag(to: currentPoint, shift: shift) }
    }

    func end(at point: NSPoint, shift: Bool = false) {
        drag(to: point, shift: shift)
        cancelGesture() // Keep the candidate; only Apply crop changes the document.
    }

    func cancelGesture() {
        cropDrag = nil; startPoint = nil; currentPoint = nil; moved = false
    }

    override func mouseDown(with event: NSEvent) {
        if beginViewportPan(event) { cancelGesture(); return }
        guard event.buttonNumber == 0 else { return }
        window?.makeFirstResponder(self)
        begin(at: convert(event.locationInWindow, from: nil), shift: event.modifierFlags.contains(.shift))
    }
    override func mouseDragged(with event: NSEvent) {
        if continueViewportPan(event) { return }
        drag(to: convert(event.locationInWindow, from: nil), shift: event.modifierFlags.contains(.shift))
    }
    override func mouseUp(with event: NSEvent) {
        if isViewportPanning { _ = continueViewportPan(event); endViewportPan(); return }
        end(at: convert(event.locationInWindow, from: nil), shift: event.modifierFlags.contains(.shift))
    }
    override func flagsChanged(with event: NSEvent) { updateModifier(shift: event.modifierFlags.contains(.shift)) }
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { cancelGesture(); cancelViewportPan(); onCancel?() }
        else { super.keyDown(with: event) }
    }
    override func resignFirstResponder() -> Bool { cancelGesture(); return super.resignFirstResponder() }
    override func setFrameSize(_ newSize: NSSize) {
        if newSize != frame.size { cancelGesture() }
        super.setFrameSize(newSize)
    }
    override func resetCursorRects() {
        if croppingEnabled { addCursorRect(bounds, cursor: .crosshair) }
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard let tokens, let selection, selection.width > 0, selection.height > 0,
              canvasSize.width > 0, canvasSize.height > 0, let image = imageRect?(),
              image.width > 0, image.height > 0 else { return }
        let scale = image.width / canvasSize.width
        let rect = NSRect(x: image.minX + selection.minX * scale,
                          y: image.minY + selection.minY * scale,
                          width: selection.width * scale, height: selection.height * scale).intersection(image)
        let visible = image.intersection(bounds)
        guard !rect.isNull, !visible.isNull else { return }
        NSGraphicsContext.saveGraphicsState()
        defer { NSGraphicsContext.restoreGraphicsState() }
        NSBezierPath(rect: bounds.intersection(image)).addClip()
        tokens.color("glass-veil-heavy").setFill()
        for area in [NSRect(x: image.minX, y: image.minY, width: image.width, height: rect.minY - image.minY),
                     NSRect(x: image.minX, y: rect.maxY, width: image.width, height: image.maxY - rect.maxY),
                     NSRect(x: image.minX, y: rect.minY, width: rect.minX - image.minX, height: rect.height),
                     NSRect(x: rect.maxX, y: rect.minY, width: image.maxX - rect.maxX, height: rect.height)] {
            NSBezierPath(rect: area).fill()
        }
        let border = NSBezierPath(rect: rect)
        border.lineWidth = tokens.number("s-1")
        border.setLineDash([tokens.number("s-3"), tokens.number("s-2")], count: 2, phase: 0)
        tokens.color("glass-text").setStroke(); border.stroke()
        let label = NSAttributedString(string: String(format: "%.0f × %.0f", Double(selection.width), Double(selection.height)),
            attributes: [.font: NSFont.systemFont(ofSize: tokens.number("text-sm")),
                         .foregroundColor: tokens.color("glass-text")])
        let padding = tokens.number("s-2")
        let size = label.size()
        let origin = NSPoint(x: min(max(rect.midX - size.width / 2, visible.minX + padding),
                                    max(visible.minX + padding, visible.maxX - size.width - padding)),
                             y: min(max(rect.minY + padding, visible.minY + padding),
                                    max(visible.minY + padding, visible.maxY - size.height - padding)))
        tokens.color("glass-strong").setFill()
        NSBezierPath(roundedRect: NSRect(origin: origin, size: size).insetBy(dx: -padding, dy: -padding),
                     xRadius: tokens.number("r-sm"), yRadius: tokens.number("r-sm")).fill()
        label.draw(at: origin)
    }
}

final class EditorDrawOverlay: EditorViewportGestureView {
    enum Shape: String, CaseIterable {
        case rectangle, ellipse, line, arrow, pen, wand, erase, restore, text
        case triangle, diamond, star

        var isBackgroundBrush: Bool { self == .erase || self == .restore }

        var polygonGeometryKind: UInt32? {
            switch self {
            case .triangle: return 2
            case .diamond: return 3
            case .star: return 4
            default: return nil
            }
        }
    }

    var shape: Shape = .rectangle { didSet { if shape != oldValue { cancelGesture() } } }
    var canvasSize = NSSize.zero {
        didSet { if canvasSize != oldValue { cancelGesture() }; needsDisplay = true }
    }
    var drawingEnabled = false {
        didSet {
            isHidden = !drawingEnabled
            if !drawingEnabled { cancelGesture() }
            window?.invalidateCursorRects(for: self)
        }
    }
    var onComplete: ((Shape, NSPoint, NSPoint, [NSPoint]) -> Void)?
    var onPreview: ((Shape, NSPoint, NSPoint, [NSPoint]) -> Void)?
    var onPreviewCancel: (() -> Void)?
    var pixelPreviewVisible = false { didSet { needsDisplay = true } }
    var onWand: ((NSPoint) -> Void)?
    var onBackgroundBrush: ((Shape, [NSPoint]) -> Void)?
    var brushDiameter: CGFloat = 28 { didSet { needsDisplay = true } }
    var brushOutlineColor = NSColor.labelColor
    var fillColor = NSColor.controlAccentColor.withAlphaComponent(0.22)
    var strokeColor = NSColor.controlAccentColor
    var annotationOpacity: CGFloat = 1 { didSet { needsDisplay = true } }
    var annotationStrokeWidth: CGFloat = 8 { didSet { needsDisplay = true } }
    var annotationStrokeEnabled = false { didSet { needsDisplay = true } }
    var annotationFillEnabled = true { didSet { needsDisplay = true } }
    var imageRect: (() -> NSRect)?
    private(set) var startPoint: NSPoint?
    private(set) var currentPoint: NSPoint?
    private(set) var penPoints: [NSPoint] = []
    private(set) var brushPoints: [NSPoint] = []
    private var previousMouseCoalescing: Bool?

    deinit {
        if let previousMouseCoalescing { NSEvent.isMouseCoalescingEnabled = previousMouseCoalescing }
    }

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override func setFrameSize(_ newSize: NSSize) {
        if newSize != frame.size { cancelGesture() }
        super.setFrameSize(newSize)
    }

    var presentedImageRect: NSRect {
        if let imageRect { return imageRect() }
        guard canvasSize.width > 0, canvasSize.height > 0,
              bounds.width > 0, bounds.height > 0 else { return .zero }
        let scale = min(bounds.width / canvasSize.width, bounds.height / canvasSize.height)
        let size = NSSize(width: canvasSize.width * scale, height: canvasSize.height * scale)
        return NSRect(x: (bounds.width - size.width) / 2,
                      y: (bounds.height - size.height) / 2,
                      width: size.width, height: size.height)
    }

    func canvasPoint(for point: NSPoint) -> NSPoint {
        let image = presentedImageRect
        guard image.width > 0, image.height > 0 else { return .zero }
        return NSPoint(x: (point.x - image.minX) * canvasSize.width / image.width,
                       y: (point.y - image.minY) * canvasSize.height / image.height)
    }

    func begin(at point: NSPoint) {
        guard drawingEnabled, presentedImageRect.width > 0 else { return }
        if (shape == .wand || shape == .text || shape.isBackgroundBrush)
            && (!bounds.contains(point) || !presentedImageRect.contains(point)) { return }
        cancelGesture()
        startPoint = point; currentPoint = point; needsDisplay = true
        if shape == .pen {
            penPoints = [canvasPoint(for: point)]
            previousMouseCoalescing = NSEvent.isMouseCoalescingEnabled
            NSEvent.isMouseCoalescingEnabled = false
        } else if shape.isBackgroundBrush {
            brushPoints = [canvasPoint(for: point)]
        }
        notifyPreview()
    }

    func drag(to point: NSPoint) {
        guard startPoint != nil else { return }
        currentPoint = point; needsDisplay = true
        if shape == .pen, let last = penPoints.last {
            let next = canvasPoint(for: point)
            let minimum = 1.5 * canvasSize.width / presentedImageRect.width
            if hypot(next.x - last.x, next.y - last.y) >= minimum { penPoints.append(next) }
        } else if shape.isBackgroundBrush {
            brushPoints.append(canvasPoint(for: point))
        }
        notifyPreview()
    }

    private func notifyPreview() {
        guard shape != .wand, shape != .text,
              let startPoint, let currentPoint else { return }
        onPreview?(shape, canvasPoint(for: startPoint), canvasPoint(for: currentPoint),
                   shape.isBackgroundBrush ? brushPoints : penPoints)
    }

    func end(at point: NSPoint) {
        guard let startPoint else { return }
        currentPoint = point
        let start = canvasPoint(for: startPoint)
        let end = canvasPoint(for: point)
        let points = penPoints
        var backgroundPoints = brushPoints
        let arrowLength = hypot(point.x - startPoint.x, point.y - startPoint.y)
        cancelGesture()
        if shape == .wand {
            guard arrowLength < 3, bounds.contains(point), presentedImageRect.contains(point) else { return }
            onWand?(start)
            return
        }
        if shape == .text {
            guard arrowLength < 3, bounds.contains(point), presentedImageRect.contains(point) else { return }
            onComplete?(shape, start, start, [])
            return
        }
        if shape.isBackgroundBrush {
            // Shipping stamps the release even at the last natural pixel: soft
            // edges accumulate, so deduplicating samples would change alpha.
            backgroundPoints.append(end)
            onBackgroundBrush?(shape, backgroundPoints)
            return
        }
        switch shape {
        case .rectangle, .ellipse, .triangle, .diamond, .star:
            guard start.x != end.x, start.y != end.y else { return }
        case .arrow:
            guard arrowLength >= 3,
                  let geometry = NativeEditorDrawGeometry(arrow: true, samples: [start, end]),
                  !geometry.points.isEmpty else { return }
        case .line, .pen: break
        case .text: preconditionFailure("handled above")
        case .wand: preconditionFailure("handled above")
        case .erase, .restore: preconditionFailure("handled above")
        }
        // Like shipping, Pen accepts movement samples, not the release location.
        onComplete?(shape, start, end, points)
    }

    func cancelGesture() {
        let active = startPoint != nil || pixelPreviewVisible
        startPoint = nil; currentPoint = nil; needsDisplay = true
        penPoints.removeAll(keepingCapacity: true)
        brushPoints.removeAll(keepingCapacity: true)
        if let previousMouseCoalescing {
            NSEvent.isMouseCoalescingEnabled = previousMouseCoalescing
            self.previousMouseCoalescing = nil
        }
        if active { onPreviewCancel?() }
    }

    override func mouseDown(with event: NSEvent) {
        if beginViewportPan(event) { cancelGesture(); return }
        guard event.buttonNumber == 0, !event.modifierFlags.contains(.command) else { return }
        window?.makeFirstResponder(self)
        begin(at: convert(event.locationInWindow, from: nil))
    }

    override func mouseDragged(with event: NSEvent) {
        if continueViewportPan(event) { return }
        drag(to: convert(event.locationInWindow, from: nil))
    }

    override func mouseUp(with event: NSEvent) {
        if isViewportPanning { _ = continueViewportPan(event); endViewportPan(); return }
        end(at: convert(event.locationInWindow, from: nil))
    }

    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { cancelGesture(); cancelViewportPan() }
        else { super.keyDown(with: event) }
    }

    override func resignFirstResponder() -> Bool {
        cancelGesture()
        return super.resignFirstResponder()
    }

    override func resetCursorRects() {
        if drawingEnabled { addCursorRect(bounds, cursor: .crosshair) }
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard let startPoint, let currentPoint else { return }
        NSGraphicsContext.saveGraphicsState()
        defer { NSGraphicsContext.restoreGraphicsState() }
        NSBezierPath(rect: bounds).addClip()
        if shape == .wand || shape == .text { return }
        if shape.isBackgroundBrush {
            let image = presentedImageRect
            let scale = image.width / canvasSize.width
            NSBezierPath(rect: bounds.intersection(image)).addClip()
            let displayPoints = brushPoints.map {
                NSPoint(x: image.minX + $0.x * scale, y: image.minY + $0.y * scale)
            }
            guard let first = displayPoints.first else { return }
            let path = NSBezierPath(); path.move(to: first)
            displayPoints.dropFirst().forEach { path.line(to: $0) }
            brushOutlineColor.setStroke()
            path.lineWidth = 1.5; path.lineCapStyle = .round
            path.lineJoinStyle = .round
            if !pixelPreviewVisible { path.stroke() }
            let radius = max(2, brushDiameter * scale / 2)
            let ring = NSBezierPath(ovalIn: NSRect(x: currentPoint.x - radius, y: currentPoint.y - radius,
                                                  width: radius * 2, height: radius * 2))
            ring.lineWidth = 1.5; ring.stroke()
            return
        }
        if pixelPreviewVisible { return }
        // Composite the annotation once, including overlapping fill and stroke.
        // Erase/Restore guides above do not use annotation opacity.
        let context = NSGraphicsContext.current?.cgContext
        context?.setAlpha(annotationOpacity)
        context?.beginTransparencyLayer(auxiliaryInfo: nil)
        defer { context?.endTransparencyLayer() }
        if shape == .line || shape == .arrow || shape == .pen || shape.polygonGeometryKind != nil {
            let samples = shape == .pen ? penPoints : [canvasPoint(for: startPoint), canvasPoint(for: currentPoint)]
            let kind = shape.polygonGeometryKind ?? (shape == .arrow ? 0 : 1)
            guard let geometry = NativeEditorDrawGeometry(kind: kind, samples: samples,
                                                         strokeWidth: Double(annotationStrokeWidth)),
                  let first = geometry.points.first else { return }
            let image = presentedImageRect
            let scale = image.width / canvasSize.width
            let position: (NSPoint) -> NSPoint = {
                NSPoint(x: image.minX + $0.x * scale, y: image.minY + $0.y * scale)
            }
            let path = NSBezierPath(); path.move(to: position(first))
            geometry.points.dropFirst().forEach { path.line(to: position($0)) }
            strokeColor.setFill(); strokeColor.setStroke()
            if shape.polygonGeometryKind != nil {
                path.close()
                if annotationFillEnabled { fillColor.setFill(); path.fill() }
                if annotationStrokeEnabled { path.lineWidth = annotationStrokeWidth * scale; path.stroke() }
            } else if shape == .arrow { path.close(); path.fill() }
            else if geometry.points.allSatisfy({ $0 == first }) {
                let radius = annotationStrokeWidth * scale / 2
                let center = position(first)
                NSBezierPath(ovalIn: NSRect(x: center.x - radius, y: center.y - radius,
                                           width: radius * 2, height: radius * 2)).fill()
            } else {
                path.lineWidth = annotationStrokeWidth * scale
                path.lineCapStyle = .round; path.lineJoinStyle = .round; path.stroke()
            }
            return
        }
        let rect = NSRect(x: min(startPoint.x, currentPoint.x),
                          y: min(startPoint.y, currentPoint.y),
                          width: abs(currentPoint.x - startPoint.x),
                          height: abs(currentPoint.y - startPoint.y))
        let path = shape == .rectangle ? NSBezierPath(rect: rect)
            : NSBezierPath(ovalIn: rect)
        if annotationFillEnabled { fillColor.setFill(); path.fill() }
        if annotationStrokeEnabled {
            strokeColor.setStroke()
            path.lineWidth = annotationStrokeWidth * presentedImageRect.width / canvasSize.width
            path.stroke()
        }
    }
}

final class EditorSelectionOverlay: EditorViewportGestureView {
    var canvasSize = NSSize.zero { didSet { cancelGesture(); needsDisplay = true } }
    var selectionEnabled = false { didSet { if !selectionEnabled { cancelGesture() }; isHidden = !selectionEnabled } }
    var selectedOutline: [CGPoint]? { didSet { needsDisplay = true } }
    var selectedLayerID: String? { didSet { if selectedLayerID != oldValue { cancelGesture() }; needsDisplay = true } }
    var documentJSON: String? { didSet { if documentJSON != oldValue { cancelGesture() } } }
    var selectedRotation = 0.0 { didSet { needsDisplay = true } }
    var rotationSnapDegrees = 15.0 {
        didSet { if rotationSnapDegrees != oldValue { cancelGesture() } }
    }
    var rotationEnabled = false { didSet { if !rotationEnabled { cancelGesture() }; needsDisplay = true } }
    var resizeEnabled = false { didSet { if !resizeEnabled { cancelGesture() }; needsDisplay = true } }
    var strokeColor = NSColor.controlAccentColor { didSet { needsDisplay = true } }
    var imageRect: (() -> NSRect)?
    var hitTestLayer: ((CGPoint, Double) throws -> String?)?
    var outlineForLayer: ((String) -> [CGPoint]?)?
    var onSelect: ((String?) -> Void)?
    var onMove: ((String, CGFloat, CGFloat, Double) -> Void)?
    var onRotate: ((String, Double) -> Void)?
    var onResize: ((String, String, CGPoint, Double, Bool) -> Void)?
    var onDoubleClick: ((CGPoint, Double) -> Bool)?
    var onError: ((Error) -> Void)?
    private(set) var startPoint: CGPoint?
    private(set) var currentPoint: CGPoint?
    private var hitLayerID: String?
    private var transientOutline: [CGPoint]?
    private var rotationStartOutline: [CGPoint]?
    private var rotationStartRadians = 0.0
    private(set) var rotationPreview: NativeEditorRotationPreview?
    private var rotatingLayerID: String?
    private var snapRotation = false
    private var resizeDrag: NativeEditorResizeDrag?
    private var resizeHandle: Int?
    private(set) var resizePreview: NativeEditorResizePreview?
    private var lockResizeAspect = false
    private var moveDrag: NativeEditorMoveDrag?
    private(set) var movePreview: NativeEditorResizePreview?

    static let resizeHandleNames = ["nw", "n", "ne", "e", "se", "s", "sw", "w"]

    var resizeHandlePoints: [CGPoint] {
        guard resizeEnabled, let outline = resizePreview?.outline ?? selectedOutline, outline.count == 4 else { return [] }
        return [outline[0], midpoint(outline[0], outline[1]), outline[1],
                midpoint(outline[1], outline[2]), outline[2], midpoint(outline[2], outline[3]),
                outline[3], midpoint(outline[3], outline[0])]
    }

    private func midpoint(_ a: CGPoint, _ b: CGPoint) -> CGPoint {
        CGPoint(x: (a.x + b.x) / 2, y: (a.y + b.y) / 2)
    }

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override func setFrameSize(_ newSize: NSSize) {
        if newSize != frame.size { cancelGesture() }
        super.setFrameSize(newSize)
    }
    var presentedImageRect: NSRect {
        if let imageRect { return imageRect() }
        guard canvasSize.width > 0, canvasSize.height > 0, bounds.width > 0, bounds.height > 0 else { return .zero }
        let scale = min(bounds.width / canvasSize.width, bounds.height / canvasSize.height)
        let size = NSSize(width: canvasSize.width * scale, height: canvasSize.height * scale)
        return NSRect(x: (bounds.width - size.width) / 2, y: (bounds.height - size.height) / 2,
                      width: size.width, height: size.height)
    }
    func canvasPoint(for point: CGPoint) -> CGPoint {
        let image = presentedImageRect
        return CGPoint(x: (point.x - image.minX) * canvasSize.width / image.width,
                       y: (point.y - image.minY) * canvasSize.height / image.height)
    }
    private func rotationHandle() -> NativeEditorRotationHandle? {
        guard rotationEnabled, let outline = selectedOutline else { return nil }
        return NativeEditorRotationHandle(outline: outline, radians: selectedRotation,
            displayScale: presentedImageRect.width / canvasSize.width, canvas: canvasSize)
    }
    func begin(at point: CGPoint, snap: Bool = false) {
        cancelGesture()
        guard selectionEnabled, presentedImageRect.contains(point) else { return }
        let documentPoint = canvasPoint(for: point)
        if let geometry = rotationHandle(), let id = selectedLayerID, let outline = selectedOutline,
           hypot(documentPoint.x - geometry.handle.x, documentPoint.y - geometry.handle.y) <= geometry.hitRadius {
            rotationStartOutline = outline
            rotationStartRadians = NativeEditorRotationPreview(outline: outline, radians: selectedRotation,
                start: documentPoint, current: documentPoint, snap: false)?.radians ?? selectedRotation
            rotatingLayerID = id; startPoint = point; currentPoint = point; snapRotation = snap
            rotationPreview = NativeEditorRotationPreview(outline: outline, radians: rotationStartRadians,
                start: documentPoint, current: documentPoint, snap: snap, snapDegrees: rotationSnapDegrees)
            needsDisplay = true; return
        }
        do {
            let scale = presentedImageRect.width / canvasSize.width
            if resizeEnabled, let id = selectedLayerID, let documentJSON {
                let result = try NativeEditorResizeDrag.begin(documentJSON: documentJSON, layerID: id,
                    point: documentPoint, displayScale: scale)
                if let drag = result.0, let handle = result.1 {
                    resizeDrag = drag; resizeHandle = handle
                    startPoint = point; currentPoint = point
                    lockResizeAspect = snap && handle.isMultiple(of: 2)
                    resizePreview = drag.preview(current: documentPoint, lockAspect: lockResizeAspect)
                    needsDisplay = true; return
                }
            }
            hitLayerID = try hitTestLayer?(documentPoint, 8 / scale)
            if let id = hitLayerID {
                guard let documentJSON else { throw AppBridgeError.invalidResponse }
                moveDrag = try NativeEditorMoveDrag.begin(documentJSON: documentJSON,
                    layerID: id, displayScale: scale)
            }
            transientOutline = hitLayerID.flatMap { outlineForLayer?($0) }
            startPoint = point; currentPoint = point; needsDisplay = true
        } catch { cancelGesture(); onError?(error) }
    }
    func drag(to point: CGPoint, snap: Bool? = nil) {
        guard let startPoint else { return }; currentPoint = point
        if rotatingLayerID != nil, let outline = rotationStartOutline {
            if let snap { snapRotation = snap }
            rotationPreview = NativeEditorRotationPreview(outline: outline, radians: rotationStartRadians,
                start: canvasPoint(for: startPoint), current: canvasPoint(for: point), snap: snapRotation,
                snapDegrees: rotationSnapDegrees)
        } else if let resizeDrag, let handle = resizeHandle {
            if let snap { lockResizeAspect = snap && handle.isMultiple(of: 2) }
            resizePreview = resizeDrag.preview(current: canvasPoint(for: point), lockAspect: lockResizeAspect)
        } else if let moveDrag {
            guard hypot(point.x - startPoint.x, point.y - startPoint.y) >= 3 else {
                movePreview = nil; needsDisplay = true; return
            }
            let scale = presentedImageRect.width / canvasSize.width
            movePreview = moveDrag.preview(delta: CGPoint(
                x: (point.x - startPoint.x) / scale, y: (point.y - startPoint.y) / scale))
            if movePreview == nil {
                cancelGesture()
                onError?(AppBridgeError.invalidResponse)
                return
            }
        }
        needsDisplay = true
    }
    func end(at point: CGPoint, snap: Bool? = nil) {
        guard let start = startPoint else { return }
        if let id = rotatingLayerID {
            drag(to: point, snap: snap)
            let angle = rotationPreview?.radians
            cancelGesture()
            if let angle, angle != rotationStartRadians { onRotate?(id, angle) }
            return
        }
        if let id = selectedLayerID, let handle = resizeHandle {
            drag(to: point, snap: snap)
            let distance = hypot(point.x - start.x, point.y - start.y)
            let current = canvasPoint(for: point)
            let scale = presentedImageRect.width / canvasSize.width
            let lockAspect = lockResizeAspect
            cancelGesture()
            if distance >= 3 {
                onResize?(id, Self.resizeHandleNames[handle], current, scale, lockAspect)
            }
            return
        }
        let hit = hitLayerID
        let distance = hypot(point.x - start.x, point.y - start.y)
        let scale = presentedImageRect.width / canvasSize.width
        cancelGesture()
        if distance >= 3, let hit { onMove?(hit, (point.x - start.x) / scale, (point.y - start.y) / scale, scale) }
        else { onSelect?(hit) }
    }
    func cancelGesture() {
        startPoint = nil; currentPoint = nil; hitLayerID = nil; transientOutline = nil
        rotationStartOutline = nil; rotatingLayerID = nil; rotationPreview = nil; needsDisplay = true
        resizeDrag = nil; resizeHandle = nil; resizePreview = nil; lockResizeAspect = false
        moveDrag = nil; movePreview = nil
    }
    override func mouseDown(with event: NSEvent) {
        if beginViewportPan(event) { cancelGesture(); return }
        let point = convert(event.locationInWindow, from: nil)
        if event.clickCount >= 2, presentedImageRect.contains(point) {
            let scale = presentedImageRect.width / canvasSize.width
            if onDoubleClick?(canvasPoint(for: point), 8 / scale) == true { return }
        }
        window?.makeFirstResponder(self)
        begin(at: point, snap: event.modifierFlags.contains(.shift))
    }
    override func mouseDragged(with event: NSEvent) { if continueViewportPan(event) { return }; drag(to: convert(event.locationInWindow, from: nil), snap: event.modifierFlags.contains(.shift)) }
    override func mouseUp(with event: NSEvent) { if isViewportPanning { _ = continueViewportPan(event); endViewportPan(); return }; end(at: convert(event.locationInWindow, from: nil), snap: event.modifierFlags.contains(.shift)) }
    override func keyDown(with event: NSEvent) { if event.keyCode == 53 { cancelGesture(); cancelViewportPan() } else { super.keyDown(with: event) } }
    override func flagsChanged(with event: NSEvent) {
        if rotatingLayerID != nil || resizeDrag != nil, let currentPoint {
            drag(to: currentPoint, snap: event.modifierFlags.contains(.shift))
        }
        else { super.flagsChanged(with: event) }
    }
    override func resignFirstResponder() -> Bool { cancelGesture(); return super.resignFirstResponder() }
    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard canvasSize.width > 0, canvasSize.height > 0,
              let outline = resizePreview?.outline ?? movePreview?.outline ?? rotationPreview?.outline
                ?? (startPoint == nil ? selectedOutline : transientOutline),
              outline.count == 4 else { return }
        NSGraphicsContext.saveGraphicsState()
        defer { NSGraphicsContext.restoreGraphicsState() }
        NSBezierPath(rect: bounds).addClip()
        let image = presentedImageRect, scale = image.width / canvasSize.width
        let path = NSBezierPath()
        for (index, point) in outline.enumerated() {
            let mapped = CGPoint(x: image.minX + point.x * scale,
                                 y: image.minY + point.y * scale)
            index == 0 ? path.move(to: mapped) : path.line(to: mapped)
        }
        path.close(); strokeColor.setStroke(); path.lineWidth = 2; path.stroke()
        if let preview = resizePreview ?? movePreview {
            let guides = NSBezierPath()
            for guide in preview.guides {
                if guide.orientation == .vertical {
                    let x = image.minX + guide.position * scale
                    guides.move(to: CGPoint(x: x, y: image.minY)); guides.line(to: CGPoint(x: x, y: image.maxY))
                } else {
                    let y = image.minY + guide.position * scale
                    guides.move(to: CGPoint(x: image.minX, y: y)); guides.line(to: CGPoint(x: image.maxX, y: y))
                }
            }
            guides.lineWidth = 1; guides.setLineDash([4, 3], count: 2, phase: 0); guides.stroke()
        }
        if resizeEnabled, startPoint == nil || resizeDrag != nil {
            strokeColor.setFill()
            for point in resizeHandlePoints {
                let center = CGPoint(x: image.minX + point.x * scale, y: image.minY + point.y * scale)
                NSBezierPath(rect: NSRect(x: center.x - 4, y: center.y - 4, width: 8, height: 8)).fill()
            }
        }
        if rotationEnabled, startPoint == nil || rotatingLayerID != nil,
           let geometry = NativeEditorRotationHandle(outline: outline,
               radians: rotationPreview?.radians ?? selectedRotation, displayScale: scale, canvas: canvasSize) {
            let map: (CGPoint) -> CGPoint = { CGPoint(x: image.minX + $0.x * scale, y: image.minY + $0.y * scale) }
            let connector = NSBezierPath(); connector.move(to: map(geometry.anchor)); connector.line(to: map(geometry.handle))
            connector.lineWidth = 2; connector.stroke()
            let center = map(geometry.handle), radius: CGFloat = 5
            strokeColor.setFill(); NSBezierPath(ovalIn: NSRect(x: center.x-radius, y: center.y-radius,
                                                               width: radius*2, height: radius*2)).fill()
        }
    }
}

/// Label drawn inside a button; clicks fall through to the button.
private final class EditorPassthroughLabel: NSTextField {
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

private final class EditorInlineTextView: NSTextView {
    var onEscape: (() -> Void)?
    var onBlur: (() -> Void)?

    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53, !hasMarkedText() {
            onEscape?()
        } else {
            // Return, marked text, selection, clipboard and undo stay with the
            // native field editor instead of becoming canvas shortcuts.
            super.keyDown(with: event)
        }
    }

    override func resignFirstResponder() -> Bool {
        let resigned = super.resignFirstResponder()
        if resigned { DispatchQueue.main.async { [weak self] in self?.onBlur?() } }
        return resigned
    }
}

private final class ScreenshotEditorWindow: NSWindow {
    var editorShortcut: ((NSEvent) -> Bool)?

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if attachedSheet == nil, editorShortcut?(event) == true { return true }
        return super.performKeyEquivalent(with: event)
    }

    override func sendEvent(_ event: NSEvent) {
        // Control shortcuts and focused field editors also reach this path.
        if attachedSheet == nil, editorShortcut?(event) == true { return }
        super.sendEvent(event)
    }
}

final class EditorLayerTable: NSTableView {
    var contextMenu: ((Int) -> NSMenu?)?

    override func rightMouseDown(with event: NSEvent) {
        guard let menu = menu(for: event) else { return }
        NSMenu.popUpContextMenu(menu, with: event, for: self)
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        contextMenu?(row(at: convert(event.locationInWindow, from: nil)))
    }
}

final class ScreenshotEditorController: NSObject, NSWindowDelegate, NSTableViewDataSource,
                                        NSTableViewDelegate, NSTextFieldDelegate, NSTextViewDelegate {
    let window: NSWindow
    let root: Surface
    private(set) var state = ScreenshotEditorState()
    private let worker: EditorWorking
    private let reportError: (String) -> Void
    private let editorNumberFormatter: NumberFormatter
    private let outputIntegerFormatter: NumberFormatter
    private var tokens: Tokens
    private let preview = NSImageView()
    private let viewportInput = EditorViewportGestureView()
    private(set) var viewport = NativeEditorViewport()
    private var viewportCanvasSize = NSSize.zero
    private var viewportBounds = NSRect.zero
    private let zoomPreset = NSPopUpButton()
    private let zoomSlider = NSSlider(value: 0, minValue: 0, maxValue: 1, target: nil, action: nil)
    private let cropX = NSTextField()
    private let cropY = NSTextField()
    private let cropWidth = NSTextField()
    private let cropHeight = NSTextField()
    private let canvasWidth = NSTextField()
    private let canvasHeight = NSTextField()
    private let backgroundColor = NSTextField()
    private var backgroundMode: NSPopUpButton!
    private var backgroundApply: CaptureButton!
    private var backgroundReset: CaptureButton!
    private var lastSolidBackground = "#f7f7f5"
    private let status = NSTextField(wrappingLabelWithString: "")
    private let geometryPanel = Surface()
    private let geometryContent = Surface()
    private let layersPanel = Surface()
    private let layerContent = Surface()
    private let layerCount = NSTextField(labelWithString: "0")
    private let layerHeadingRule = Surface()
    private var addLayerButton: CaptureButton!
    private var drawHeading: NSTextField?
    private var annotationControls: EditorAnnotationControls!
    private let rotationSnap = NSTextField()
    private var rotationSnapLabel: NSTextField!
    private let drawPanel = Surface()
    private let exportBar = Surface()
    private let exportSettingsPanel = Surface()
    let drawOverlay = EditorDrawOverlay()
    let selectionOverlay = EditorSelectionOverlay()
    let cropOverlay = EditorCropOverlay()
    private let inlineTextScroll = NSScrollView()
    private let inlineTextEditor = EditorInlineTextView()
    private var inlineTextDoneButton: CaptureButton!
    private var inlineTextCancelButton: CaptureButton!
    private let layerName = NSTextField()
    private let layerOpacity = NSTextField()
    private let layerX = NSTextField()
    private let layerY = NSTextField()
    private let outputQualityValue = NSTextField()
    private let outputPngPalette = NSTextField()
    private let outputByteBudget = NSTextField()
    private let outputCompressionPreset = NSPopUpButton()
    private let outputWidth = NSTextField()
    private let outputHeight = NSTextField()
    private let outputAspectLock = NSButton(checkboxWithTitle: "Lock aspect ratio", target: nil, action: nil)
    private let outputDimensions = NSTextField(labelWithString: "")
    private let outputFilename = NSTextField()
    private let outputLocation = NSTextField(labelWithString: "")
    private let exportDisclosureTitle = EditorPassthroughLabel(labelWithString: "Export settings")
    private let exportSummary = EditorPassthroughLabel(labelWithString: "")
    private let exportChevron = EditorPassthroughLabel(labelWithString: "▾")
    private let exportFilenameCaption = NSTextField(labelWithString: "Filename")
    private let exportSavingToCaption = NSTextField(labelWithString: "Saving to")
    private let exportStatus = NSTextField(labelWithString: "")
    private let exportEstimateValue = NSTextField(labelWithString: "—")
    private let exportEstimateDelta = NSTextField(labelWithString: "")
    private let saveAsNewSwitch = NSSwitch()
    private let saveAsNewLabel = NSTextField(labelWithString: "Save as new file")
    /// Settings groups behind the disclosure: caption, controls with their
    /// x offsets/widths inside the group, and the group width.
    private var exportGroups: [String: (caption: NSTextField, views: [(NSView, CGFloat, CGFloat)], width: CGFloat)] = [:]
    private var sectionControl: NSSegmentedControl!
    private var drawTool: NSPopUpButton!
    private var lastBackgroundTool: EditorDrawOverlay.Shape = .wand
    private var lastGroupedShape: EditorDrawOverlay.Shape = .rectangle
    private var toolRailButtons: [(key: String, button: CaptureButton)] = []
    // Shipping header chrome (`captures_app::editor_chrome`).
    private let headerBar = Surface()
    private let headerRule = Surface()
    private let canvasToolbar = Surface()
    private let canvasSplit = Surface()
    private var canvasToolbarLabels: [NSTextField] = []
    private var backgroundButton: CaptureButton!
    private let backgroundCard = Surface()
    private let zoomGroup = Surface()
    private var zoomDividers: [Surface] = []
    private var fitButton: CaptureButton!
    private var zoomOutButton: CaptureButton!
    private var zoomInButton: CaptureButton!
    private var addImagesButton: CaptureButton!
    private var draftMenuButton: CaptureButton!
    /// Native drafts are explicit (shipping autosaves), so Save draft and
    /// Discard edits share one compact header menu.
    let draftMenu = NSMenu(title: EditorChrome.text("header", "draft_menu"))
    private let saveDraftItem = NSMenuItem(title: EditorChrome.text("header", "save_draft"),
                                           action: nil, keyEquivalent: "")
    private let discardItem = NSMenuItem(title: EditorChrome.text("header", "discard_edits"),
                                         action: nil, keyEquivalent: "")
    private var recenterButton: CaptureButton!
    private let railPanel = Surface()
    private let railRule = Surface()
    private let railTip = EditorPassthroughLabel(labelWithString: "")
    private let draftBanner = Surface()
    private let draftBannerLabel = NSTextField(labelWithString: EditorChrome.text("header", "draft_restored"))
    private var draftDiscardButton: CaptureButton!
    private var draftDismissButton: CaptureButton!
    /// Shipping's "Restored unsaved edits" notice for a draft found at open.
    private(set) var draftRestored = false
    private let wandTolerance = NSTextField()
    private let wandContiguous = NSButton(checkboxWithTitle: "Contiguous only", target: nil, action: nil)
    private var wandToleranceLabel: NSTextField!
    private let brushSize = NSTextField()
    private let brushSoftness = NSTextField()
    private var brushSizeLabel: NSTextField!
    private var brushSoftnessLabel: NSTextField!
    private var drawHelper: NSTextField!
    private let createTextPreset = NSPopUpButton()
    private let createTextSize = NSTextField()
    private let createTextColor = NSTextField()
    private var createTextControls: [NSView] = []
    private var createTextDefaultsPublished = false
    private let drawingStroke = NSButton(checkboxWithTitle: "Stroke", target: nil, action: nil)
    private let drawingFill = NSButton(checkboxWithTitle: "Fill", target: nil, action: nil)
    private let drawingStrokeColor = NSTextField()
    private let drawingFillColor = NSTextField()
    private let drawingStrokeWidth = NSTextField()
    private let drawingOpacity = NSTextField()
    private var drawingDefaultControls: [NSView] = []
    private var drawingFillControls: [NSView] = []
    private var drawingDefaultsArtifactID: String?
    private let drawingDropShadow = NSButton(checkboxWithTitle: "Drop shadow", target: nil, action: nil)
    private var drawingShadowControls: [NSView] = []
    private var drawingShadowFields: [String: NSTextField] = [:]
    private var drawingShadowCustomized = false
    private let textEditor = NSTextView()
    private let textSize = NSTextField()
    private let textColor = NSTextField()
    private let textFamily = NSPopUpButton()
    private let textTraits = NSSegmentedControl(labels: ["Bold", "Italic"], trackingMode: .selectAny,
                                                target: nil, action: nil)
    private let textAlignment = NSSegmentedControl(labels: ["Left", "Center", "Right"], trackingMode: .selectOne,
                                                   target: nil, action: nil)
    private let textPlate = NSPopUpButton()
    private let textPlateColor = NSTextField()
    private let textShadow = NSButton(checkboxWithTitle: "Drop shadow", target: nil, action: nil)
    private let textOutline = NSButton(checkboxWithTitle: "Outline", target: nil, action: nil)
    private let textPreset = NSPopUpButton(frame: .zero, pullsDown: true)
    private var textPresetRounded: Bool?
    private let textShadowPanel = Surface()
    private var textShadowFields: [String: NSTextField] = [:]
    private let textShadowNumbers: [(String, KeyPath<NativeTextShadowStyle, Double>)] = [
        ("opacity", \.opacity), ("blur", \.blur), ("offsetX", \.offsetX), ("offsetY", \.offsetY)
    ]
    private var textApplyButton: CaptureButton!
    private var textCancelButton: CaptureButton!
    private var textControls: [NSView] = []
    private var textFieldsID: String?
    private var acceptedTextStyle: NativeTextStyle?
    private var textApplyPending = false
    private var hasStagedText: Bool { acceptedTextStyle.map { !textFieldsMatch($0) } ?? false }
    private struct InlineTextInput {
        var inputID: String
        var layerID: String?
        let isNew: Bool
        let anchor: NSPoint
        let fontSize: Double
        var beginTarget: [String: Any]?
        var acceptedText: String
        var bufferedText: String
        var requestInFlight = false
        var finishRequested: Bool?
        var finishInFlight = false
    }
    private var inlineTextInput: InlineTextInput?
    private var closeAfterTextInput = false
    private var outputFormat: NSPopUpButton!
    private var outputQuality: NSPopUpButton!
    private var outputSizeMode: NSPopUpButton!
    private var outputPreviewMode: NSSegmentedControl!
    private var layerTable: EditorLayerTable!
    private var visibilityButton: CaptureButton!
    private var lockButton: CaptureButton!
    private var renameButton: CaptureButton!
    private var opacityButton: CaptureButton!
    private var moveButton: CaptureButton!
    private var duplicateButton: CaptureButton!
    private var deleteButton: CaptureButton!
    private var moveUpButton: CaptureButton!
    private var moveDownButton: CaptureButton!
    private var combineLayers: NSPopUpButton!
    private var rotateLeftButton: CaptureButton!
    private var rotateRightButton: CaptureButton!
    private var flipHorizontalButton: CaptureButton!
    private var flipVerticalButton: CaptureButton!
    private var importImageButton: CaptureButton!
    private var undoButton: CaptureButton!
    private var redoButton: CaptureButton!
    private var applyCropButton: CaptureButton!
    private var drawCropButton: CaptureButton!
    private let cropAspect = NSPopUpButton()
    private var cropPrevious: [String]?
    private var trimButton: CaptureButton!
    private var exportDisclosure: CaptureButton!
    private var previewOutputButton: CaptureButton!
    private var copyImageButton: CaptureButton!
    private var changeOutputDirectoryButton: CaptureButton!
    private var showInFolderButton: CaptureButton!
    private var exportSaveButton: CaptureButton!
    /// Shared export-bar state: the opaque Rust target and its latest presentation.
    private var exportTarget: [String: Any]?
    private(set) var exportBarState: NativeExportBar?
    private(set) var exportSettingsOpen = false
    private var exportOptionsError: String?
    private var exportError: String?
    private var exportNotice: String?
    private var exportNoticeToken = 0
    private var copyConfirmed = false
    private var copyConfirmToken = 0
    private var saveInFlight = false
    private let exportBarRule = Surface()
    private(set) var lastSavedPath: String?
    private var estimate: EditorEstimate?
    private var estimatePending = false
    private var estimateGeneration = 0
    private var estimateWork: DispatchWorkItem?
    private var originalBytes: UInt64?
    private var fields: [NSTextField] = []
    private var closeAfterCommand = false
    private var selectedLayerID: String?
    private var selectedLayerIndex = 0
    private var preferredLayerID: String?
    private var reconcilingLayerSelection = false
    private var editedImage: NSImage?
    private var drawingPreviewEpoch = 0
    private var drawingPreviewInFlight = false
    private var drawingPreviewPending: [String: Any]?
    private var encodedOutput: EditorOutputPresentation?
    private var historyRoot = ""
    private var captureMode = "region"
    private var outputDirectory = ""
    private let directoryPicker: ((NSWindow, URL?, @escaping (URL?) -> Void) -> Void)?
    private let didSaveCopy: () -> Void
    private let didReplaceOriginal: (String) -> Void
    private let revealFiles: ([URL]) -> Void
    /// Shipping debounce before Est. size re-encodes, and confirmation duration.
    static let estimateDelay: TimeInterval = 0.22
    static let exportConfirmationDuration: TimeInterval = 4
    private let writeClipboard: (Data) -> Bool
    private let imagePicker: ((NSWindow, @escaping (URL?) -> Void) -> Void)?
    private let imageDecoder: (URL) throws -> EditorDecodedImage
    private static let imageDecodeQueue = DispatchQueue(label: "es.captures.native.editor-image-decode",
                                                        qos: .userInitiated)
    private var importToken = 0
    private var importLoading = false
    private var pendingImport: (image: EditorDecodedImage, generation: Int, artifactID: String)?

    private enum Section {
        static let geometry = 0
        static let layers = 1
        static let draw = 2
    }

    init(tokens: Tokens, worker: EditorWorking = EditorWorker(), numberLocale: Locale = .current,
         reportError: @escaping (String) -> Void = { _ in },
         directoryPicker: ((NSWindow, URL?, @escaping (URL?) -> Void) -> Void)? = nil,
         didSaveCopy: @escaping () -> Void = {},
         didReplaceOriginal: @escaping (String) -> Void = { _ in },
         revealFiles: @escaping ([URL]) -> Void = { NSWorkspace.shared.activateFileViewerSelecting($0) },
         imagePicker: ((NSWindow, @escaping (URL?) -> Void) -> Void)? = nil,
         imageDecoder: @escaping (URL) throws -> EditorDecodedImage = EditorImageDecoder.decode,
         writeClipboard: @escaping (Data) -> Bool = { png in
             let pasteboard = NSPasteboard.general
             pasteboard.clearContents()
             return pasteboard.setData(png, forType: .png)
         }) {
        self.tokens = tokens; self.worker = worker; self.reportError = reportError
        self.directoryPicker = directoryPicker; self.didSaveCopy = didSaveCopy
        self.didReplaceOriginal = didReplaceOriginal
        self.revealFiles = revealFiles
        self.imagePicker = imagePicker; self.imageDecoder = imageDecoder
        self.writeClipboard = writeClipboard
        editorNumberFormatter = NumberFormatter()
        outputIntegerFormatter = NumberFormatter()
        editorNumberFormatter.locale = numberLocale
        editorNumberFormatter.numberStyle = .decimal
        editorNumberFormatter.usesGroupingSeparator = false
        editorNumberFormatter.maximumFractionDigits = 3
        editorNumberFormatter.minimum = -1_000_000
        editorNumberFormatter.maximum = 1_000_000
        outputIntegerFormatter.locale = numberLocale
        outputIntegerFormatter.numberStyle = .decimal
        outputIntegerFormatter.usesGroupingSeparator = false
        outputIntegerFormatter.maximumFractionDigits = 0
        outputIntegerFormatter.minimum = 0
        // The full-width export bar adds 80pt below the historical 1000×700 layout.
        let bounds = NSRect(x: 0, y: 0, width: 1000, height: 780)
        root = Surface(frame: bounds)
        let editorWindow = ScreenshotEditorWindow(contentRect: bounds,
            styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window = editorWindow
        super.init()
        editorWindow.editorShortcut = { [weak self] in self?.handleEditorShortcut($0) ?? false }
        window.isReleasedWhenClosed = false; window.title = "Edit screenshot"
        window.contentMinSize = NSSize(width: 760, height: 540)
        window.delegate = self
        window.contentView = root
        build(); restyle(tokens); updateControls()
    }

    func present(artifact: CaptureArtifact, historyRoot: String, outputDirectory: String? = nil,
                 completion: ((Bool) -> Void)? = nil) {
        guard !state.busy else {
            showError("Wait for the current editor action to finish before opening another screenshot.")
            window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
            completion?(false)
            return
        }
        guard !artifact.isRecording else {
            showError("Recording editing is not available in this native editor.")
            completion?(false)
            return
        }
        if state.artifactID != artifact.id,
           state.snapshot?.unsavedChanges == true || hasStagedText || inlineTextInput != nil {
            showError("Apply or cancel pending text and save or discard screenshot edits before opening another capture.")
            window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
            completion?(false)
            return
        }
        if state.artifactID == artifact.id, state.snapshot != nil {
            // "Show in editor" also brings back a minimized editor.
            if window.isMiniaturized { window.deminiaturize(nil) }
            window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
            publishPresence()
            completion?(true)
            return
        }
        cancelPendingImport()
        inlineTextInput = nil; hideInlineTextEditor(); closeAfterTextInput = false
        let generation = state.beginOpen(artifactID: artifact.id)
        lastSolidBackground = "#f7f7f5"
        self.historyRoot = historyRoot
        captureMode = artifact.mode
        self.outputDirectory = outputDirectory ?? URL(fileURLWithPath: historyRoot)
            .deletingLastPathComponent().path
        outputSizeMode?.selectItem(at: 0)
        outputWidth.stringValue = ""; outputHeight.stringValue = ""
        resetExportState(originalBytes: artifact.sizeBytes)
        selectedLayerID = nil; selectedLayerIndex = 0; preferredLayerID = nil
        editedImage = nil; invalidateOutput(); preview.image = nil; window.title = "Edit screenshot"
        status.stringValue = "Opening screenshot…"; updateControls()
        fitWindowToScreen()
        window.center(); window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
        publishPresence()
        let draftsRoot = URL(fileURLWithPath: historyRoot).deletingLastPathComponent()
            .appendingPathComponent("editor-drafts", isDirectory: true).path
        worker.open(historyRoot: historyRoot, draftsRoot: draftsRoot, artifactID: artifact.id) {
            [weak self] result in
            guard let self else { completion?(false); return }
            let accepted: Bool
            switch result {
            case .success(let presentation):
                guard self.state.complete(presentation.snapshot, generation: generation) else {
                    completion?(false); return
                }
                accepted = true
                self.draftRestored = presentation.snapshot.hasDraft
                self.layoutEditor()
                self.createTextSize.stringValue = self.format(presentation.snapshot.initialTextSize)
                self.publishInitialDrawingDefaults(presentation.snapshot)
                self.publish(presentation, resetCrop: true)
                self.startExportTarget(presentation.snapshot)
                self.status.textColor = self.tokens.color("text-muted")
                self.status.stringValue = presentation.snapshot.hasDraft
                    ? "Draft restored." : "Ready. Changes affect only the native editor draft."
            case .failure(let error):
                guard self.state.fail(generation: generation) else {
                    completion?(false); return
                }
                accepted = false
                self.showError("Couldn’t open screenshot: \(error.localizedDescription)")
            }
            self.updateControls()
            self.submitPendingImportIfReady()
            completion?(accepted)
        }
    }

    var activeArtifactID: String? { window.isVisible ? state.artifactID : nil }

    /// Capture this editor window shows while open (visible or minimized), for
    /// the mini-preview "In editor" presence.
    var presentArtifactID: String? {
        window.isVisible || window.isMiniaturized ? state.artifactID : nil
    }
    /// Called on the main thread whenever `presentArtifactID` changes.
    var presenceChanged: (String?) -> Void = { _ in }
    private var reportedPresence: String?

    private func publishPresence() {
        let current = presentArtifactID
        guard current != reportedPresence else { return }
        reportedPresence = current
        presenceChanged(current)
    }

    func prepareForTermination() -> Bool {
        guard inlineTextInput != nil || !hasStagedText else {
            showError("Apply or cancel pending text before quitting.")
            window.makeKeyAndOrderFront(nil)
            return false
        }
        cancelDrawing()
        cancelPendingImport()
        let terminationInput = inlineTextInput.flatMap {
            $0.finishInFlight ? nil
                : EditorTerminationTextInput(inputID: $0.inputID, text: inlineTextEditor.string,
                                             commit: $0.finishRequested ?? true)
        }
        let result = worker.prepareForTermination(textInput: terminationInput)
        switch result {
        case .success:
            inlineTextInput = nil; hideInlineTextEditor()
            state.close(); editedImage = nil; invalidateOutput(); preview.image = nil
            estimateWork?.cancel(); estimateWork = nil; estimateGeneration += 1
            window.orderOut(nil); publishPresence(); return true
        case .failure(let error):
            if let failure = error as? EditorTerminationFailure,
               let presentation = failure.acceptedPresentation,
               state.complete(presentation.snapshot, generation: state.generation) {
                publish(presentation, resetCrop: false)
                if presentation.snapshot.activeTextInput == nil {
                    if terminationInput?.commit == true { invalidateOutput() }
                    inlineTextInput = nil; hideInlineTextEditor()
                }
            } else {
                _ = state.fail(generation: state.generation)
            }
            showError("Couldn’t save screenshot draft before quitting: \(error.localizedDescription)")
            if inlineTextInput != nil { showInlineTextEditor() }
            window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
            return false
        }
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        cancelCrop()
        cancelDrawing()
        if inlineTextInput != nil {
            closeAfterTextInput = true
            finishInlineTextInput(commit: true)
            return false
        }
        guard !hasStagedText else {
            showError("Apply or cancel pending text before closing.")
            return false
        }
        guard !state.busy else {
            status.stringValue = "Wait for the current editor action to finish."
            return false
        }
        guard state.snapshot?.unsavedChanges == true else { closeNow(); return false }
        let alert = NSAlert()
        alert.messageText = "Save screenshot edits?"
        alert.informativeText = "Save a native draft to continue later, close without saving this session, or cancel."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Save and Close")
        alert.addButton(withTitle: "Close Without Saving")
        alert.addButton(withTitle: "Cancel Close")
        alert.beginSheetModal(for: window) { [weak self] response in
            switch response {
            case .alertFirstButtonReturn: self?.saveDraft(closeAfter: true)
            // Freeing a session has no implicit write. This drops only changes
            // since the last save and retains any previously persisted draft.
            case .alertSecondButtonReturn: self?.closeNow()
            default: break
            }
        }
        return false
    }

    func windowDidResignKey(_ notification: Notification) {
        cancelCrop()
        cancelDrawing()
        cancelViewportPan()
        finishInlineTextInput(commit: true)
    }

    func windowDidResize(_ notification: Notification) { layoutEditor() }

    /// The default 1000×780 content is taller than some displays' visible
    /// frames. Shrink to the screen up front (never below the minimum size) so
    /// the frame-based layout keeps the export bar pinned inside the window
    /// instead of depending on AppKit constraining the frame after ordering in.
    private func fitWindowToScreen() {
        guard let visible = (window.screen ?? NSScreen.main)?.visibleFrame,
              visible.width > 0, visible.height > 0 else { return }
        let available = window.contentRect(forFrameRect: visible).size
        let current = root.bounds.size
        let fitted = NSSize(width: max(window.contentMinSize.width, min(current.width, available.width)),
                            height: max(window.contentMinSize.height, min(current.height, available.height)))
        guard fitted != current else { return }
        window.setContentSize(fitted)
    }

    /// Collapsed export bar height, plus the fixed settings area while open.
    var exportBarHeight: CGFloat { exportSettingsOpen ? 208 : 80 }

    /// Frame-based layout: the full-width export bar is pinned to the bottom;
    /// the viewport, zoom row and inspector sit above it.
    private func layoutEditor() {
        guard let previewPanel = viewportInput.superview else { return }
        let bar = exportBarHeight
        layoutHeader()
        let top = chromeTop
        let gap = tokens.number("s-5")
        let railWidth = EditorChrome.metric("rail_width")
        railPanel.frame = NSRect(x: 0, y: top, width: railWidth,
                                 height: max(0, root.bounds.height - bar - top))
        railRule.frame = NSRect(x: railWidth - 1, y: 0, width: 1, height: railPanel.frame.height)
        let side = EditorChrome.metric("rail_button")
        for (index, entry) in toolRailButtons.enumerated() {
            entry.button.frame = NSRect(x: (railWidth - side) / 2,
                y: top + tokens.number("s-4") + CGFloat(index) * (side + tokens.number("s-1")),
                width: side, height: side)
        }
        // Keep the inspector width stable; the canvas takes the remaining width.
        let inspectorX = root.bounds.width - 312
        let previewX = railWidth + gap
        previewPanel.frame = NSRect(x: previewX, y: top + gap,
                                    width: max(0, inspectorX - 24 - previewX),
                                    height: max(0, root.bounds.height - bar - 2 * gap - top))
        if let recenterButton {
            recenterButton.frame.origin = NSPoint(
                x: (previewPanel.bounds.width - recenterButton.frame.width) / 2, y: gap)
        }
        let statusY = root.bounds.height - bar - 16 - 96
        sectionControl?.frame.origin.x = inspectorX
        status.frame = NSRect(x: inspectorX, y: statusY, width: 272, height: 96)
        for panel in [geometryPanel, layersPanel, drawPanel] {
            panel.frame = NSRect(x: inspectorX, y: top + gap, width: 272,
                                 height: max(0, statusY - 14 - top - gap))
        }
        layoutExportBar()
        guard viewportBounds.size != viewportInput.bounds.size else { return }
        // A gesture cannot retain its old screen-to-document mapping while
        // the viewport changes. Resizing itself never submits a document edit.
        cancelCrop()
        cancelDrawing()
        cancelViewportPan()
        viewportBounds = viewportInput.bounds
        updateViewportGeometry()
    }

    /// The bottom of the header, below the restored-draft banner when it shows.
    private var chromeTop: CGFloat { headerBar.frame.maxY }

    /// The 52pt shipping header: the Canvas toolbar on the left; Undo/Redo (only
    /// above 1040pt), the zoom group, Add images and the draft menu on the right.
    /// Controls are placed first; the Canvas toolbar takes what remains.
    private func layoutHeader() {
        guard let addImagesButton, let draftMenuButton, let fitButton else { return }
        let width = root.bounds.width
        let pad = tokens.number("s-5")
        let small = NSFont.systemFont(ofSize: tokens.number("text-sm"), weight: .medium)
        func titleWidth(_ text: String) -> CGFloat {
            ceil((text as NSString).size(withAttributes: [.font: small]).width)
        }
        // Shipping `.screenshot-editor-draft-banner`: padding 6pt 12pt, h-sm actions.
        draftBanner.isHidden = !draftRestored
        let bannerHeight = draftRestored ? tokens.number("h-sm") + 2 * tokens.number("s-3") : 0
        draftBanner.frame = NSRect(x: 0, y: 0, width: width, height: bannerHeight)
        let bannerButtonY = tokens.number("s-3")
        let dismissWidth = titleWidth(draftDismissButton.title) + 2 * tokens.number("s-4")
        draftDismissButton.frame = NSRect(x: width - pad - dismissWidth, y: bannerButtonY,
                                          width: dismissWidth, height: tokens.number("h-sm"))
        let discardWidth = titleWidth(draftDiscardButton.title) + 2 * tokens.number("s-4")
        draftDiscardButton.frame = NSRect(x: draftDismissButton.frame.minX - tokens.number("s-3") - discardWidth,
                                          y: bannerButtonY, width: discardWidth, height: tokens.number("h-sm"))
        draftBannerLabel.frame = NSRect(x: pad, y: (bannerHeight - 17) / 2,
                                        width: max(0, draftDiscardButton.frame.minX - 2 * pad), height: 17)

        let height = EditorChrome.metric("header_height")
        headerBar.frame = NSRect(x: 0, y: bannerHeight, width: width, height: height)
        headerRule.frame = NSRect(x: 0, y: height - 1, width: width, height: 1)
        let layout = EditorChrome.headerLayout(width: width)
        let control = EditorChrome.metric("header_control")
        let y = (height - control) / 2
        let spacing = tokens.number("s-2")
        draftMenuButton.frame = NSRect(x: width - pad - control, y: y, width: control, height: control)
        let addWidth = 2 * pad + 16 + tokens.number("s-3") + titleWidth(addImagesButton.title)
        addImagesButton.frame = NSRect(x: draftMenuButton.frame.minX - spacing - addWidth, y: y,
                                       width: addWidth, height: control)
        let zoomButton = EditorChrome.metric("zoom_button")
        let zoomWidth = 3 * zoomButton + layout.sliderWidth + layout.presetWidth + 4 + 2
        zoomGroup.frame = NSRect(x: addImagesButton.frame.minX - 2 * spacing - zoomWidth, y: y,
                                 width: zoomWidth, height: control)
        var x: CGFloat = 1
        let inner = control - 2
        let sliderPad = tokens.number(layout.showHistory ? "s-4" : "s-3")
        let zoomViews: [(NSView, CGFloat)] = [
            (fitButton as NSView, zoomButton), (zoomOutButton! as NSView, zoomButton),
            (zoomSlider as NSView, layout.sliderWidth), (zoomInButton! as NSView, zoomButton),
            (zoomPreset as NSView, layout.presetWidth),
        ]
        for (index, entry) in zoomViews.enumerated() {
            let (view, viewWidth) = entry
            if view === zoomSlider {
                view.frame = NSRect(x: x + sliderPad, y: 1, width: viewWidth - 2 * sliderPad, height: inner)
            } else if view === zoomPreset {
                view.frame = NSRect(x: x, y: (control - 26) / 2, width: viewWidth, height: 26)
            } else {
                view.frame = NSRect(x: x, y: 1, width: viewWidth, height: inner)
            }
            x += viewWidth
            if index < zoomDividers.count {
                zoomDividers[index].frame = NSRect(x: x, y: 1, width: 1, height: inner)
                x += 1
            }
        }
        var right = zoomGroup.frame.minX - 2 * spacing
        undoButton.isHidden = !layout.showHistory
        redoButton.isHidden = !layout.showHistory
        if layout.showHistory {
            redoButton.frame = NSRect(x: right - control, y: y, width: control, height: control)
            undoButton.frame = NSRect(x: redoButton.frame.minX - spacing - control, y: y,
                                      width: control, height: control)
            right = undoButton.frame.minX
        }
        layoutCanvasToolbar(available: max(0, right - 2 * pad), y: y)
    }

    /// Shipping `.screenshot-canvas-toolbar`. With too little room it drops the
    /// "Canvas" label, then draws Trim and Background icon-only (their titles
    /// stay the accessible names, with tooltips), then clips like `overflow: hidden`.
    private func layoutCanvasToolbar(available: CGFloat, y: CGFloat) {
        guard canvasToolbarLabels.count == 4, let trimButton, let backgroundButton else { return }
        let labels = canvasToolbarLabels
        func textWidth(_ field: NSTextField) -> CGFloat { ceil(field.attributedStringValue.size().width) + 2 }
        let small = NSFont.systemFont(ofSize: tokens.number("text-sm"), weight: .medium)
        func titleWidth(_ text: String) -> CGFloat {
            ceil((text as NSString).size(withAttributes: [.font: small]).width)
        }
        let gap = tokens.number("s-2"), toolPad = tokens.number("s-4"), iconGap = tokens.number("s-3")
        let fieldWidth = EditorChrome.metric("canvas_field")
        let labelWidth = toolPad + textWidth(labels[0]) + tokens.number("s-3")
        let trimFull = 2 * toolPad + 13 + iconGap + titleWidth(EditorChrome.text("header", "trim"))
        let backgroundFull = 2 * toolPad + 14 + iconGap + titleWidth(Self.backgroundTitle)
        let compactTool: CGFloat = 28
        let dimensions = textWidth(labels[1]) + gap + fieldWidth + 2 + textWidth(labels[2]) + 2
            + textWidth(labels[3]) + gap + fieldWidth
        let split = 2 + 1 + 2 * gap + 2
        func total(_ showLabel: Bool, _ full: Bool) -> CGFloat {
            3 + (showLabel ? labelWidth + 2 : 0) + dimensions + split
                + (full ? trimFull + 2 + backgroundFull : 2 * compactTool + 2) + 3
        }
        let showLabel = total(true, true) <= available
        let full = showLabel || total(false, true) <= available
        let height = EditorChrome.metric("header_control")
        canvasToolbar.frame = NSRect(x: tokens.number("s-5"), y: y,
                                     width: max(0, min(total(showLabel, full), available)), height: height)
        let labelY = (height - 16) / 2
        var x: CGFloat = 3
        labels[0].isHidden = !showLabel
        if showLabel {
            labels[0].frame = NSRect(x: x + toolPad, y: labelY, width: textWidth(labels[0]), height: 16)
            x += labelWidth + 2
        }
        for (axis, field) in [canvasWidth, canvasHeight].enumerated() {
            let letter = labels[axis == 0 ? 1 : 3]
            letter.frame = NSRect(x: x, y: labelY, width: textWidth(letter), height: 16)
            x += letter.frame.width + gap
            field.frame = NSRect(x: x, y: (height - 22) / 2, width: fieldWidth, height: 22)
            x += fieldWidth
            if axis == 0 {
                x += 2
                labels[2].frame = NSRect(x: x, y: labelY, width: textWidth(labels[2]), height: 16)
                x += labels[2].frame.width + 2
            }
        }
        canvasSplit.frame = NSRect(x: x + 2 + gap, y: (height - 16) / 2, width: 1, height: 16)
        x += split
        trimButton.iconOnly = !full
        trimButton.frame = NSRect(x: x, y: (height - 28) / 2, width: full ? trimFull : compactTool, height: 28)
        x = trimButton.frame.maxX + 2
        backgroundButton.iconOnly = !full
        backgroundButton.frame = NSRect(x: x, y: (height - 28) / 2,
                                        width: full ? backgroundFull : compactTool, height: 28)
        backgroundCard.frame.origin = NSPoint(
            x: min(canvasToolbar.frame.minX + backgroundButton.frame.minX,
                   max(0, root.bounds.width - backgroundCard.frame.width - 8)),
            y: chromeTop + tokens.number("s-3"))
    }

    static let backgroundTitle = "Background color"

    /// Disclosure and filename widths: fixed actions first, then the disclosure
    /// grows to fit its summary, then the filename field takes what remains.
    static func exportWidths(_ width: CGFloat) -> (disclosure: CGFloat, filename: CGFloat) {
        let fixed: CGFloat = 72 + 96 + 148 + 96 + 5 * 8
        let flexible = max(0, width - 32 - fixed)
        let disclosure = min(210, max(150, flexible - 120))
        return (disclosure, min(320, max(120, flexible - disclosure)))
    }

    private func layoutExportBar() {
        guard exportDisclosure != nil else { return }
        let width = root.bounds.width
        exportBar.frame = NSRect(x: 0, y: root.bounds.height - exportBarHeight,
                                 width: width, height: exportBarHeight)
        exportBarRule.frame = NSRect(x: 0, y: 0, width: width, height: 1)
        exportSettingsPanel.isHidden = !exportSettingsOpen
        exportSettingsPanel.frame = NSRect(x: 16, y: 10, width: width - 32, height: 112)
        layoutExportSettings()
        let (disclosure, filename) = Self.exportWidths(width)
        let base: CGFloat = exportSettingsOpen ? 128 : 0
        let headingY = base + 10, rowY = base + 32
        exportDisclosure.frame = NSRect(x: 16, y: rowY, width: disclosure, height: 36)
        exportDisclosureTitle.frame = NSRect(x: 12, y: 3, width: disclosure - 40, height: 16)
        exportSummary.frame = NSRect(x: 12, y: 19, width: disclosure - 40, height: 14)
        exportChevron.frame = NSRect(x: disclosure - 26, y: 9, width: 16, height: 18)
        outputFilename.frame = NSRect(x: 16 + disclosure + 8, y: rowY + 3, width: filename, height: 30)
        outputFormat.frame = NSRect(x: outputFilename.frame.maxX + 8, y: rowY + 3, width: 72, height: 30)
        copyImageButton.frame = NSRect(x: outputFormat.frame.maxX + 8, y: rowY + 1, width: 96, height: 34)
        exportSaveButton.frame = NSRect(x: width - 16 - 96, y: rowY + 1, width: 96, height: 34)
        saveAsNewSwitch.frame = NSRect(x: exportSaveButton.frame.minX - 8 - 140, y: rowY + 8, width: 38, height: 20)
        saveAsNewLabel.frame = NSRect(x: saveAsNewSwitch.frame.maxX + 4, y: rowY + 9, width: 98, height: 18)
        let headingX = outputFilename.frame.minX
        let headingEnd = outputFormat.frame.maxX
        exportFilenameCaption.frame = NSRect(x: headingX, y: headingY, width: 58, height: 16)
        exportSavingToCaption.frame = NSRect(x: headingX + 60, y: headingY, width: 58, height: 16)
        changeOutputDirectoryButton.frame = NSRect(x: headingEnd - 72, y: headingY - 4, width: 72, height: 24)
        outputLocation.frame = NSRect(x: headingX + 120, y: headingY,
                                      width: max(24, changeOutputDirectoryButton.frame.minX - 4 - headingX - 120),
                                      height: 16)
        showInFolderButton.frame = NSRect(x: width - 16 - 112, y: headingY - 4, width: 112, height: 24)
        let statusRight = showInFolderButton.isHidden ? width - 16 : showInFolderButton.frame.minX - 8
        exportStatus.frame = NSRect(x: headingEnd + 12, y: headingY,
                                    width: max(0, statusRight - headingEnd - 12), height: 16)
    }

    /// Flow the visible settings groups into rows inside the fixed panel.
    private func layoutExportSettings() {
        let compress = outputQuality?.indexOfSelectedItem == 1
        let visible: [String] = ["size"]
            + (outputSizeMode?.indexOfSelectedItem == 3 ? ["custom"] : [])
            + ["quality"]
            + (compress ? ["preset"] : [])
            + (compress && outputFormat?.indexOfSelectedItem == 0 ? ["palette"] : [])
            + (outputQuality?.indexOfSelectedItem == 2 ? ["maximum"] : [])
            + ["estimate", "preview"]
        let available = exportSettingsPanel.bounds.width - 24
        var x: CGFloat = 12, row: CGFloat = 0
        for (key, group) in exportGroups {
            let shown = visible.contains(key)
            group.caption.isHidden = !shown
            group.views.forEach { $0.0.isHidden = !shown }
        }
        for key in visible {
            guard let group = exportGroups[key] else { continue }
            if x > 12 && x + group.width > 12 + available { x = 12; row += 1 }
            let captionY = 8 + row * 52
            group.caption.frame = NSRect(x: x, y: captionY, width: group.width, height: 16)
            for (view, offset, viewWidth) in group.views {
                let labelLike = (view as? NSTextField).map { !$0.isEditable } ?? false
                view.frame = NSRect(x: x + offset, y: captionY + (labelLike ? 24 : 18),
                                    width: viewWidth, height: labelLike ? 18 : 28)
            }
            x += group.width + 16
        }
    }

    private func build() {
        // The window title names the editor; like shipping, the chrome does not repeat it.
        buildHeader()
        buildToolRail()
        let previewPanel = Surface(frame: NSRect(x: 24 + tokens.number("s-12"), y: 90,
                                                width: 640 - tokens.number("s-12"), height: 550))
        previewPanel.wantsLayer = true
        previewPanel.layer?.masksToBounds = true
        previewPanel.layer?.cornerRadius = tokens.number("r-xl")
        previewPanel.layer?.borderWidth = 1
        previewPanel.autoresizingMask = [.width, .height]
        root.addSubview(previewPanel)
        viewportBounds = previewPanel.bounds.insetBy(dx: 18, dy: 18)
        viewportInput.frame = viewportBounds
        viewportInput.autoresizingMask = [.width, .height]
        viewportInput.wantsLayer = true
        viewportInput.layer?.masksToBounds = true
        viewportInput.setAccessibilityLabel("Screenshot viewport")
        previewPanel.addSubview(viewportInput)
        preview.frame = viewportInput.bounds
        preview.imageScaling = .scaleAxesIndependently
        preview.setAccessibilityLabel("Edited screenshot preview")
        viewportInput.addSubview(preview)
        drawOverlay.frame = viewportInput.bounds
        drawOverlay.autoresizingMask = [.width, .height]
        drawOverlay.setAccessibilityLabel("Screenshot drawing canvas")
        drawOverlay.onComplete = { [weak self] shape, start, end, points in
            self?.createDrawing(shape: shape, start: start, end: end, points: points)
        }
        drawOverlay.onPreview = { [weak self] shape, start, end, points in
            guard let self, !self.state.busy else { return }
            self.drawingPreviewPending = self.drawingRequest(shape: shape, start: start, end: end,
                                                            points: points, reportErrors: false)
            if self.drawingPreviewPending == nil { self.cancelDrawingPreview() }
            else { self.drainDrawingPreview() }
        }
        drawOverlay.onPreviewCancel = { [weak self] in self?.cancelDrawingPreview() }
        drawOverlay.onWand = { [weak self] point in self?.removeImageBackground(at: point) }
        drawOverlay.onBackgroundBrush = { [weak self] mode, points in
            self?.paintImageBackground(mode: mode, points: points)
        }
        viewportInput.addSubview(drawOverlay)
        selectionOverlay.frame = preview.frame
        selectionOverlay.autoresizingMask = [.width, .height]
        selectionOverlay.setAccessibilityLabel("Screenshot layer selection canvas")
        selectionOverlay.toolTip = "Drag a layer to move, its border to resize, or its round grip to rotate. Shift constrains corner resize and rotation. Escape cancels."
        selectionOverlay.hitTestLayer = { [weak self] point, tolerance in
            guard let json = self?.state.snapshot?.documentJSON else { return nil }
            return try NativeEditorHitTesting.hit(documentJSON: json, point: point, tolerance: tolerance)
        }
        selectionOverlay.outlineForLayer = { [weak self] id in
            self?.state.snapshot?.layers.first(where: { $0.id == id })?.selectionOutline
        }
        selectionOverlay.onSelect = { [weak self] id in self?.selectCanvasLayer(id) }
        selectionOverlay.onMove = { [weak self] id, dx, dy, scale in
            self?.moveCanvasLayer(id, dx: dx, dy: dy, displayScale: scale)
        }
        selectionOverlay.onRotate = { [weak self] id, radians in self?.rotateCanvasLayer(id, radians: radians) }
        selectionOverlay.onResize = { [weak self] id, handle, current, scale, lockAspect in
            self?.resizeCanvasLayer(id, handle: handle, current: current,
                                    displayScale: scale, lockAspect: lockAspect)
        }
        selectionOverlay.onDoubleClick = { [weak self] point, tolerance in
            self?.beginExistingTextInput(at: point, tolerance: tolerance) ?? false
        }
        selectionOverlay.onError = { [weak self] error in self?.showError("Layer interaction failed: \(error.localizedDescription)") }
        viewportInput.addSubview(selectionOverlay)
        cropOverlay.frame = viewportInput.bounds
        cropOverlay.autoresizingMask = [.width, .height]
        cropOverlay.setAccessibilityLabel("Screenshot crop canvas")
        cropOverlay.toolTip = "Drag to choose a crop. Hold Shift to lock the ratio. Escape cancels; Apply crop commits."
        cropOverlay.onChange = { [weak self] rect in self?.setCropFields(rect) }
        cropOverlay.onCancel = { [weak self] in self?.cancelCrop() }
        viewportInput.addSubview(cropOverlay)
        inlineTextScroll.isHidden = true
        inlineTextScroll.borderType = .lineBorder
        inlineTextScroll.hasVerticalScroller = true
        inlineTextScroll.drawsBackground = true
        inlineTextScroll.backgroundColor = tokens.color("surface-raised")
        inlineTextScroll.wantsLayer = true
        inlineTextScroll.layer?.cornerRadius = tokens.number("r-sm")
        inlineTextScroll.layer?.borderColor = tokens.color("theme-accent").cgColor
        inlineTextScroll.layer?.borderWidth = 2
        inlineTextEditor.isRichText = false
        inlineTextEditor.isHorizontallyResizable = false
        inlineTextEditor.isVerticallyResizable = true
        inlineTextEditor.allowsUndo = true
        inlineTextEditor.delegate = self
        inlineTextEditor.textContainer?.widthTracksTextView = true
        inlineTextEditor.textContainerInset = NSSize(width: tokens.number("s-2"),
                                                      height: tokens.number("s-2"))
        inlineTextEditor.setAccessibilityLabel("Inline screenshot text")
        inlineTextEditor.onEscape = { [weak self] in self?.finishInlineTextInput(commit: true) }
        inlineTextEditor.onBlur = { [weak self] in self?.finishInlineTextInput(commit: true) }
        inlineTextScroll.documentView = inlineTextEditor
        viewportInput.addSubview(inlineTextScroll)
        inlineTextDoneButton = button("Done", frame: .zero, parent: viewportInput) { [weak self] in
            self?.finishInlineTextInput(commit: true)
        }
        inlineTextDoneButton.primary = true
        inlineTextDoneButton.setAccessibilityLabel("Finish inline screenshot text")
        inlineTextCancelButton = button("Cancel", frame: .zero, parent: viewportInput) { [weak self] in
            self?.finishInlineTextInput(commit: false)
        }
        inlineTextCancelButton.glass = true
        inlineTextCancelButton.setAccessibilityLabel("Cancel inline screenshot text")
        inlineTextDoneButton.isHidden = true; inlineTextCancelButton.isHidden = true
        for view in [viewportInput, drawOverlay, selectionOverlay, cropOverlay] { configureViewportGestures(view) }
        drawOverlay.imageRect = { [weak self] in self?.presentedImageRect ?? .zero }
        selectionOverlay.imageRect = { [weak self] in self?.presentedImageRect ?? .zero }
        cropOverlay.imageRect = { [weak self] in self?.presentedImageRect ?? .zero }
        // Shipping `.screenshot-canvas-recenter`: fixed glass, shown only while
        // free pan leaves the canvas mostly off screen.
        recenterButton = CaptureButton(EditorChrome.text("header", "recenter"),
            frame: NSRect(x: 0, y: 0, width: 96, height: tokens.number("h-sm")), tokens: tokens, glass: true) {
            [weak self] in self?.recenterViewport()
        }
        recenterButton.textSize = tokens.number("text-sm")
        recenterButton.cornerRadius = tokens.number("h-sm") / 2
        recenterButton.isHidden = true
        previewPanel.addSubview(recenterButton)

        sectionControl = NSSegmentedControl(labels: ["Geometry", "Layers", "Draw"], trackingMode: .selectOne,
                                            target: self, action: #selector(changeSection))
        sectionControl.frame = NSRect(x: 688, y: 24, width: 272, height: 28)
        sectionControl.autoresizingMask = [.minXMargin]
        sectionControl.selectedSegment = 0
        sectionControl.setAccessibilityLabel("Editor section")
        // Shipping has no section tabs: the rail's tool chooses the inspector.
        // The hidden control keeps the section state and its action.
        sectionControl.isHidden = true
        root.addSubview(sectionControl)

        geometryPanel.frame = NSRect(x: 688, y: 66, width: 272, height: 346)
        layersPanel.frame = geometryPanel.frame; layersPanel.isHidden = true
        drawPanel.frame = geometryPanel.frame; drawPanel.isHidden = true
        geometryPanel.setAccessibilityLabel("Geometry controls")
        layersPanel.setAccessibilityLabel("Layer controls")
        drawPanel.setAccessibilityLabel("Drawing controls")
        // layoutEditor() places the inspector above the export bar.
        root.addSubview(geometryPanel); root.addSubview(layersPanel)
        root.addSubview(drawPanel)

        let geometryScroll = NSScrollView(frame: geometryPanel.bounds)
        geometryScroll.autoresizingMask = [.width, .height]
        geometryScroll.hasVerticalScroller = true; geometryScroll.drawsBackground = false
        geometryContent.frame = NSRect(x: 0, y: 0, width: 252, height: 240)
        geometryScroll.documentView = geometryContent
        geometryPanel.addSubview(geometryScroll)

        panelLabel("Crop", frame: NSRect(x: 0, y: 0, width: 118, height: 24),
                   size: 16, weight: .semibold, parent: geometryContent)
        drawCropButton = button("Draw crop", frame: NSRect(x: 128, y: 0, width: 124, height: 28),
                                parent: geometryContent) { [weak self] in self?.toggleCrop() }
        panelLabel("Aspect", frame: NSRect(x: 0, y: 34, width: 48, height: 22), muted: true,
                   parent: geometryContent)
        cropAspect.frame = NSRect(x: 54, y: 28, width: 198, height: 30)
        cropAspect.setAccessibilityLabel("Crop aspect")
        for (name, ratio) in [("Free", 0.0), ("1:1", 1.0), ("4:3", 4.0 / 3),
                              ("3:2", 3.0 / 2), ("16:9", 16.0 / 9)] {
            cropAspect.addItem(withTitle: name); cropAspect.lastItem?.representedObject = ratio
        }
        cropAspect.target = self; cropAspect.action = #selector(changeCropAspect)
        geometryContent.addSubview(cropAspect)
        panelFieldLabel("X", x: 0, y: 66, parent: geometryContent)
        panelFieldLabel("Y", x: 134, y: 66, parent: geometryContent)
        configure(cropX, frame: NSRect(x: 0, y: 90, width: 118, height: 30), label: "Crop X",
                  parent: geometryContent)
        configure(cropY, frame: NSRect(x: 134, y: 90, width: 118, height: 30), label: "Crop Y",
                  parent: geometryContent)
        panelFieldLabel("Width", x: 0, y: 128, parent: geometryContent)
        panelFieldLabel("Height", x: 134, y: 128, parent: geometryContent)
        configure(cropWidth, frame: NSRect(x: 0, y: 152, width: 118, height: 30), label: "Crop width",
                  parent: geometryContent)
        configure(cropHeight, frame: NSRect(x: 134, y: 152, width: 118, height: 30), label: "Crop height",
                  parent: geometryContent)
        [cropX, cropY, cropWidth, cropHeight].forEach { $0.delegate = self }
        applyCropButton = button("Apply crop", frame: NSRect(x: 0, y: 194, width: 252, height: 34),
                                 parent: geometryContent) {
            [weak self] in self?.applyCrop()
        }

        buildLayersPanel()
        buildDrawPanel()

        status.frame = NSRect(x: 688, y: 524, width: 272, height: 96)
        status.maximumNumberOfLines = 5; status.setAccessibilityLabel("Screenshot editor status")
        root.addSubview(status)
        buildExportBar()
        // Floating chrome stays above the canvas, inspector and export bar.
        root.addSubview(backgroundCard)
        root.addSubview(railTip)
        fields = [cropX, cropY, cropWidth, cropHeight, canvasWidth, canvasHeight]
        layoutEditor()
        installKeyViewLoop()
    }

    /// Tab follows the shipping DOM order: header, tool rail, canvas and
    /// inspector in build order, then the export footer like
    /// `.screenshot-export-bar` (open settings, disclosure, save location,
    /// filename, format, Show in Folder, Copy image, Save as new, Save). The
    /// active section's canvas starts focused, as `focusActiveCanvas` does.
    private func installKeyViewLoop() {
        let exportControls: [NSView?] = [
            exportSettingsPanel, exportDisclosure, changeOutputDirectoryButton, outputFilename,
            outputFormat, showInFolderButton, copyImageButton, saveAsNewSwitch, exportSaveButton,
        ]
        let order = root.subviews.filter { $0 !== exportBar } + exportControls.compactMap { $0 }
        let canvas: NSView
        switch sectionControl.selectedSegment {
        case Section.draw: canvas = drawOverlay
        case Section.layers: canvas = selectionOverlay
        default: canvas = cropOverlay
        }
        KeyViewLoop.install(order, window: window, initial: canvas)
    }

    /// The editor's key-view loop from its initial first responder, for tests.
    var keyViewOrder: [NSView] {
        guard let first = window.initialFirstResponder else { return [] }
        return KeyViewLoop.order(from: first)
    }

    /// Shipping header chrome: restored-draft banner, Canvas toolbar, Undo/Redo,
    /// zoom group, Add images and the native draft menu.
    private func buildHeader() {
        for view in [draftBanner, headerBar, headerRule, canvasToolbar, canvasSplit, zoomGroup,
                     railPanel, railRule, backgroundCard] {
            view.wantsLayer = true
        }
        draftBanner.setAccessibilityLabel(EditorChrome.text("header", "draft_restored"))
        draftBannerLabel.font = .systemFont(ofSize: tokens.number("text-sm"), weight: .medium)
        draftBannerLabel.lineBreakMode = .byTruncatingTail
        draftBanner.addSubview(draftBannerLabel)
        draftDiscardButton = button(EditorChrome.text("header", "draft_discard"), frame: .zero,
                                    parent: draftBanner) { [weak self] in self?.discardRestoredDraft() }
        draftDiscardButton.textSize = tokens.number("text-sm")
        draftDismissButton = button(EditorChrome.text("header", "draft_dismiss"), frame: .zero,
                                    parent: draftBanner) { [weak self] in
            self?.draftRestored = false; self?.layoutEditor()
        }
        draftDismissButton.quiet = true; draftDismissButton.textSize = tokens.number("text-sm")
        draftDismissButton.setAccessibilityLabel(EditorChrome.text("header", "draft_dismiss_label"))
        draftBanner.isHidden = true
        root.addSubview(draftBanner)

        headerBar.setAccessibilityLabel("Screenshot editor toolbar")
        root.addSubview(headerBar)
        headerBar.addSubview(headerRule)

        canvasToolbar.layer?.cornerRadius = tokens.number("r-lg")
        canvasToolbar.layer?.borderWidth = 1
        canvasToolbar.layer?.masksToBounds = true
        canvasToolbar.setAccessibilityLabel(EditorChrome.text("header", "canvas"))
        headerBar.addSubview(canvasToolbar)
        for text in [EditorChrome.text("header", "canvas"), "W", "×", "H"] {
            let label = NSTextField(labelWithString: text)
            label.font = .systemFont(ofSize: tokens.number("text-xs"), weight: .medium)
            label.setAccessibilityElement(false)
            canvasToolbarLabels.append(label); canvasToolbar.addSubview(label)
        }
        for (field, key) in [(canvasWidth, "canvas_width"), (canvasHeight, "canvas_height")] {
            configure(field, frame: .zero, label: EditorChrome.text("header", key), parent: canvasToolbar)
            field.alignment = .left; field.isBordered = false; field.drawsBackground = false
            field.focusRingType = .exterior
            field.font = .systemFont(ofSize: tokens.number("text-sm"))
            field.toolTip = EditorChrome.text("header", key)
            // Enter or leaving the field commits one canvas resize, as in shipping.
            field.target = self; field.action = #selector(canvasSizeCommitted)
            field.cell?.sendsActionOnEndEditing = true
        }
        canvasToolbar.addSubview(canvasSplit)
        trimButton = button(EditorChrome.text("header", "trim"), frame: .zero, parent: canvasToolbar) {
            [weak self] in
            self?.command(["operation": "trim_canvas"], message: "Trimming canvas…", resetCrop: true)
        }
        backgroundButton = button(Self.backgroundTitle, frame: .zero, parent: canvasToolbar) {
            [weak self] in self?.toggleBackgroundCard()
        }
        for (control, icon) in [(trimButton!, "trim"), (backgroundButton!, "")] {
            control.quiet = true; control.textSize = tokens.number("text-sm")
            control.cornerRadius = tokens.number("r-sm")
            if !icon.isEmpty { control.icon = .shipping(icon); control.iconSide = 13 }
        }
        trimButton.toolTip = EditorChrome.text("header", "trim_tooltip")
        backgroundButton.toolTip = "Canvas background color"
        backgroundButton.swatch = tokens.color("surface-raised")

        // Canvas background card (native keeps its explicit hex field and Apply).
        backgroundCard.frame = NSRect(x: 0, y: 0, width: 264, height: 128)
        backgroundCard.layer?.cornerRadius = tokens.number("r-xl")
        backgroundCard.layer?.borderWidth = 1
        backgroundCard.setAccessibilityLabel("Canvas background")
        backgroundCard.isHidden = true
        backgroundMode = NSPopUpButton()
        backgroundMode.addItems(withTitles: ["Solid", "Transparent"])
        backgroundMode.frame = NSRect(x: 12, y: 12, width: 240, height: 28)
        backgroundMode.setAccessibilityLabel("Canvas background mode")
        backgroundMode.target = self; backgroundMode.action = #selector(changeBackgroundMode)
        backgroundCard.addSubview(backgroundMode)
        configure(backgroundColor, frame: NSRect(x: 12, y: 48, width: 240, height: 26),
                  label: "Canvas background color", parent: backgroundCard)
        backgroundColor.placeholderString = "#RRGGBB or #RRGGBBAA"
        backgroundColor.formatter = nil; backgroundColor.alignment = .left
        backgroundApply = button("Apply background", frame: NSRect(x: 12, y: 86, width: 144, height: 30),
                                 parent: backgroundCard) { [weak self] in self?.applyBackground() }
        backgroundReset = button("Reset fields", frame: NSRect(x: 164, y: 86, width: 88, height: 30),
                                 parent: backgroundCard) { [weak self] in
            self?.publishBackgroundFields(); self?.updateControls()
        }

        undoButton = headerIcon("undo", label: EditorChrome.text("header", "undo")) {
            [weak self] in self?.undoDocument()
        }
        redoButton = headerIcon("redo", label: EditorChrome.text("header", "redo")) {
            [weak self] in self?.redoDocument()
        }

        zoomGroup.layer?.cornerRadius = tokens.number("r-lg")
        zoomGroup.layer?.borderWidth = 1
        zoomGroup.layer?.masksToBounds = true
        zoomGroup.setAccessibilityLabel(EditorChrome.text("header", "zoom_group"))
        headerBar.addSubview(zoomGroup)
        fitButton = headerIcon("fit", label: EditorChrome.text("header", "fit"), parent: zoomGroup) {
            [weak self] in self?.fitViewport()
        }
        fitButton.toolTip = EditorChrome.text("header", "fit_tooltip")
        zoomOutButton = headerIcon("minus", label: EditorChrome.text("header", "zoom_out"), parent: zoomGroup) {
            [weak self] in self?.scaleViewport(by: 1 / 1.25)
        }
        zoomOutButton.toolTip = EditorChrome.text("header", "zoom_out")
        zoomSlider.isContinuous = true
        zoomSlider.controlSize = .small
        zoomSlider.setAccessibilityLabel(EditorChrome.text("header", "zoom_slider"))
        zoomSlider.target = self; zoomSlider.action = #selector(changeZoomSlider)
        zoomGroup.addSubview(zoomSlider)
        zoomInButton = headerIcon("plus", label: EditorChrome.text("header", "zoom_in"), parent: zoomGroup) {
            [weak self] in self?.scaleViewport(by: 1.25)
        }
        zoomInButton.toolTip = EditorChrome.text("header", "zoom_in")
        for control in [fitButton!, zoomOutButton!, zoomInButton!] {
            control.cornerRadius = 0; control.iconSide = 14
        }
        zoomPreset.isBordered = false
        zoomPreset.font = .monospacedSystemFont(ofSize: tokens.number("text-xs"), weight: .regular)
        zoomPreset.setAccessibilityLabel(EditorChrome.text("header", "zoom_preset"))
        zoomPreset.toolTip = EditorChrome.text("header", "zoom_preset_tooltip")
        zoomPreset.target = self; zoomPreset.action = #selector(changeZoomPreset)
        zoomGroup.addSubview(zoomPreset)
        for _ in 0..<4 {
            let divider = Surface(); divider.wantsLayer = true
            zoomDividers.append(divider); zoomGroup.addSubview(divider)
        }
        publishZoomPreset()

        addImagesButton = button(EditorChrome.text("header", "add_images"), frame: .zero, parent: headerBar) {
            [weak self] in self?.chooseImage()
        }
        addImagesButton.icon = .shipping("image")
        addImagesButton.textSize = tokens.number("text-sm")

        draftMenuButton = headerIcon("more", label: EditorChrome.text("header", "draft_menu")) { [weak self] in
            guard let self, let control = self.draftMenuButton else { return }
            self.draftMenu.popUp(positioning: nil, at: NSPoint(x: 0, y: control.bounds.maxY + 4), in: control)
        }
        draftMenuButton.toolTip = EditorChrome.text("header", "draft_menu")
        draftMenu.autoenablesItems = false
        saveDraftItem.target = self; saveDraftItem.action = #selector(saveDraftFromMenu)
        discardItem.target = self; discardItem.action = #selector(discardFromMenu)
        draftMenu.addItem(saveDraftItem); draftMenu.addItem(discardItem)

        railPanel.setAccessibilityLabel("Screenshot tools")
        root.addSubview(railPanel)
        railPanel.addSubview(railRule)
        railTip.wantsLayer = true
        railTip.font = .systemFont(ofSize: tokens.number("text-xs"), weight: .medium)
        railTip.alignment = .center
        railTip.layer?.cornerRadius = tokens.number("r-sm")
        railTip.layer?.borderWidth = 1
        railTip.isHidden = true
    }

    /// A 34pt quiet header icon button with its accessible name.
    private func headerIcon(_ name: String, label: String, parent: NSView? = nil,
                            action: @escaping () -> Void) -> CaptureButton {
        let control = button("", frame: NSRect(x: 0, y: 0, width: 34, height: 34),
                             parent: parent ?? headerBar, action: action)
        control.quiet = true; control.icon = .shipping(name)
        control.setAccessibilityLabel(label)
        return control
    }

    @objc private func canvasSizeCommitted() {
        guard let snapshot = state.snapshot, let width = positive(canvasWidth),
              let height = positive(canvasHeight) else {
            if let snapshot = state.snapshot {
                canvasWidth.stringValue = format(snapshot.width)
                canvasHeight.stringValue = format(snapshot.height)
            }
            return
        }
        // Leaving an unchanged field is not an edit.
        guard width != snapshot.width || height != snapshot.height else { return }
        resizeCanvas()
    }

    private func toggleBackgroundCard() {
        backgroundCard.isHidden.toggle()
        backgroundButton.selected = !backgroundCard.isHidden
        if !backgroundCard.isHidden { publishBackgroundFields(); updateControls() }
        backgroundButton.needsDisplay = true
    }

    private func undoDocument() {
        guard state.snapshot?.canUndo == true, !state.busy, inlineTextInput == nil else { return }
        command(["operation": "undo"], message: "Undoing…")
    }

    private func redoDocument() {
        guard state.snapshot?.canRedo == true, !state.busy, inlineTextInput == nil else { return }
        command(["operation": "redo"], message: "Redoing…")
    }

    @objc private func saveDraftFromMenu() {
        guard saveDraftItem.isEnabled else { return }
        saveDraft()
    }

    @objc private func discardFromMenu() {
        guard discardItem.isEnabled else { return }
        confirmDiscard()
    }

    /// Shipping's banner Discard resets immediately, without confirmation.
    private func discardRestoredDraft() {
        guard state.snapshot != nil, !state.busy, inlineTextInput == nil else { return }
        draftRestored = false
        layoutEditor()
        discardEdits()
    }

    /// Shipping rail tip: fixed glass beside the button, shown at once on hover
    /// or focus with the tool label (the tooltip keeps the shortcut).
    private func showRailTip(_ control: CaptureButton, _ visible: Bool) {
        guard visible, let entry = toolRailButtons.first(where: { $0.button === control }),
              let tool = EditorChrome.rail.first(where: { $0.key == entry.key }) else {
            railTip.isHidden = true; return
        }
        railTip.stringValue = tool.label
        let size = railTip.attributedStringValue.size()
        let height = ceil(size.height) + 10
        railTip.frame = NSRect(x: control.frame.maxX + 10, y: control.frame.midY - height / 2,
                               width: ceil(size.width) + 2 * tokens.number("s-4"), height: height)
        railTip.isHidden = false
    }

    private func buildDrawPanel() {
        let scroll = NSScrollView(frame: drawPanel.bounds)
        scroll.autoresizingMask = [.width, .height]
        scroll.hasVerticalScroller = true; scroll.drawsBackground = false
        let content = Surface(frame: NSRect(x: 0, y: 0, width: 252, height: 390))
        scroll.documentView = content; drawPanel.addSubview(scroll)
        drawHeading = panelLabel("Draw", frame: NSRect(x: 0, y: 0, width: 272, height: 24),
                   size: 16, weight: .semibold, parent: content)
        panelLabel("Draw annotations, or click with Wand to remove pixels from an image.",
                   frame: NSRect(x: 0, y: 28, width: 252, height: 42), muted: true,
                   parent: content)
        panelFieldLabel("Tool", x: 0, y: 78, parent: content)
        drawTool = NSPopUpButton()
        drawTool.addItems(withTitles: ["Rectangle", "Ellipse", "Line", "Arrow", "Pen", "Wand",
                                           "Erase", "Restore", "Text", "Triangle", "Diamond", "Star"])
        drawTool.target = self; drawTool.action = #selector(changeDrawTool)
        drawTool.frame = NSRect(x: 0, y: 100, width: 252, height: 30)
        drawTool.selectItem(at: 0)
        drawTool.setAccessibilityLabel("Drawing tool")
        content.addSubview(drawTool)
        wandToleranceLabel = panelFieldLabel("Tolerance", x: 0, y: 146, parent: content)
        configure(wandTolerance, frame: NSRect(x: 0, y: 168, width: 252, height: 30),
                  label: "Wand color tolerance", parent: content)
        let formatter = NumberFormatter(); formatter.numberStyle = .decimal
        formatter.maximumFractionDigits = 0; formatter.minimum = 0; formatter.maximum = 255
        wandTolerance.formatter = formatter; wandTolerance.stringValue = "36"
        wandContiguous.frame = NSRect(x: 0, y: 208, width: 252, height: 24)
        wandContiguous.state = .on; wandContiguous.setAccessibilityLabel("Wand contiguous only")
        content.addSubview(wandContiguous)
        brushSizeLabel = panelFieldLabel("Brush diameter", x: 0, y: 146, parent: content)
        configure(brushSize, frame: NSRect(x: 0, y: 168, width: 252, height: 30),
                  label: "Brush diameter", parent: content)
        let sizeFormatter = NumberFormatter(); sizeFormatter.numberStyle = .decimal
        sizeFormatter.maximumFractionDigits = 0; sizeFormatter.minimum = 4; sizeFormatter.maximum = 120
        brushSize.formatter = sizeFormatter; brushSize.stringValue = "28"; brushSize.delegate = self
        brushSoftnessLabel = panelFieldLabel("Softness", x: 0, y: 208, parent: content)
        configure(brushSoftness, frame: NSRect(x: 0, y: 230, width: 252, height: 30),
                  label: "Brush softness", parent: content)
        let softnessFormatter = NumberFormatter(); softnessFormatter.numberStyle = .decimal
        softnessFormatter.maximumFractionDigits = 0; softnessFormatter.minimum = 0
        softnessFormatter.maximum = 100
        brushSoftness.formatter = softnessFormatter; brushSoftness.stringValue = "18"
        drawHelper = panelLabel("Other tools create one annotation layer on release.",
                                frame: NSRect(x: 0, y: 408, width: 252, height: 42), muted: true,
                                parent: content)
        buildDrawingDefaultControls(in: content)
        buildCreateTextControls(in: content)
        buildTextControls(in: content)
        publishDrawToolControls()
    }

    private func buildDrawingDefaultControls(in content: NSView) {
        let strokeColorLabel = panelFieldLabel("Stroke color", x: 0, y: 146, parent: content)
        let fillColorLabel = panelFieldLabel("Fill color", x: 132, y: 146, parent: content)
        configure(drawingStrokeColor, frame: NSRect(x: 0, y: 168, width: 120, height: 30),
                  label: "New drawing stroke color", parent: content)
        configure(drawingFillColor, frame: NSRect(x: 132, y: 168, width: 120, height: 30),
                  label: "New drawing fill color", parent: content)
        drawingStrokeColor.formatter = nil; drawingFillColor.formatter = nil
        let widthLabel = panelFieldLabel("Width (2–40)", x: 0, y: 208, parent: content)
        let opacityLabel = panelFieldLabel("Opacity (0–100)", x: 132, y: 208, parent: content)
        configure(drawingStrokeWidth, frame: NSRect(x: 0, y: 230, width: 120, height: 30),
                  label: "New drawing stroke width", parent: content)
        configure(drawingOpacity, frame: NSRect(x: 132, y: 230, width: 120, height: 30),
                  label: "New drawing opacity", parent: content)
        [drawingStrokeColor, drawingFillColor, drawingStrokeWidth, drawingOpacity].forEach { $0.delegate = self }
        drawingStroke.frame = NSRect(x: 0, y: 270, width: 120, height: 24)
        drawingFill.frame = NSRect(x: 132, y: 270, width: 120, height: 24)
        drawingStroke.setAccessibilityLabel("New drawing stroke")
        drawingFill.setAccessibilityLabel("New drawing fill")
        drawingStroke.target = self; drawingStroke.action = #selector(drawingDefaultsChanged(_:))
        drawingFill.target = self; drawingFill.action = #selector(drawingDefaultsChanged(_:))
        content.addSubview(drawingStroke); content.addSubview(drawingFill)
        drawingDefaultControls = [strokeColorLabel, fillColorLabel, drawingStrokeColor, drawingFillColor,
                                  widthLabel, opacityLabel, drawingStrokeWidth, drawingOpacity,
                                  drawingStroke, drawingFill]
        drawingFillControls = [fillColorLabel, drawingFillColor, drawingFill]
        drawingDropShadow.frame = NSRect(x: 0, y: 302, width: 252, height: 24)
        drawingDropShadow.setAccessibilityLabel("New drawing drop shadow")
        drawingDropShadow.target = self; drawingDropShadow.action = #selector(drawingDefaultsChanged(_:))
        content.addSubview(drawingDropShadow)
        drawingDefaultControls.append(drawingDropShadow)
        for (index, item) in [("color", "Shadow color"), ("opacity", "Shadow opacity"),
                              ("blur", "Blur (0–100)"), ("offsetX", "X offset"),
                              ("offsetY", "Y offset")].enumerated() {
            let x = CGFloat(index % 2) * 132
            let y = CGFloat(index / 2) * 62 + 336
            let label = panelFieldLabel(item.1, x: x, y: y, parent: content)
            let field = NSTextField()
            configure(field, frame: NSRect(x: x, y: y + 22, width: 120, height: 30),
                      label: "New drawing shadow \(item.0)", parent: content)
            field.formatter = nil; field.delegate = self
            drawingShadowFields[item.0] = field
            drawingShadowControls.append(contentsOf: [label, field])
        }
    }

    private func publishInitialDrawingDefaults(_ snapshot: NativeEditorSnapshot) {
        guard drawingDefaultsArtifactID != snapshot.artifactID, let style = snapshot.initialAnnotationStyle else { return }
        drawingDefaultsArtifactID = snapshot.artifactID
        drawingStrokeColor.stringValue = style.color
        drawingFillColor.stringValue = style.fill ?? style.color
        drawingStrokeWidth.stringValue = format(style.strokeWidth)
        drawingOpacity.stringValue = "100"
        drawingStroke.state = style.strokeEnabled ? .on : .off
        drawingFill.state = style.fill == nil ? .off : .on
        drawingDropShadow.state = style.dropShadow ? .on : .off
        drawingShadowCustomized = false
        drawingDefaultsChanged()
    }

    @objc private func drawingDefaultsChanged(_ sender: Any? = nil) {
        if sender as? NSButton === drawingFill, drawingFill.state == .on {
            drawingFillColor.stringValue = drawingStrokeColor.stringValue
        }
        updateDrawingPreviewStyle()
        publishDrawToolControls()
    }

    private func updateDrawingPreviewStyle() {
        if let color = NSColor(hex: drawingStrokeColor.stringValue) { drawOverlay.strokeColor = color }
        if let color = NSColor(hex: drawingFillColor.stringValue) { drawOverlay.fillColor = color }
        if let width = number(drawingStrokeWidth), (2...40).contains(width) {
            drawOverlay.annotationStrokeWidth = CGFloat(width)
            if !drawingShadowCustomized, let shadow = try? NativeDrawingStyle.defaultShadow(strokeWidth: width) {
                drawingShadowFields["color"]?.stringValue = shadow.color
                for (key, path) in textShadowNumbers {
                    drawingShadowFields[key]?.stringValue = format(shadow[keyPath: path])
                }
            }
        }
        if let opacity = number(drawingOpacity), (0...100).contains(opacity) {
            drawOverlay.annotationOpacity = CGFloat(opacity / 100)
        }
        drawOverlay.annotationStrokeEnabled = drawingStroke.state == .on
        drawOverlay.annotationFillEnabled = drawingFill.state == .on
        drawOverlay.needsDisplay = true
    }

    private func buildCreateTextControls(in content: NSView) {
        let styleLabel = panelFieldLabel("Style", x: 0, y: 146, parent: content)
        createTextPreset.frame = NSRect(x: 0, y: 168, width: 252, height: 30)
        createTextPreset.setAccessibilityLabel("New text style")
        content.addSubview(createTextPreset)
        let sizeLabel = panelFieldLabel("Size (8–512)", x: 0, y: 208, parent: content)
        sizeLabel.frame.size.width = 118
        let colorLabel = panelFieldLabel("Color", x: 134, y: 208, parent: content)
        colorLabel.frame.size.width = 118
        configure(createTextSize, frame: NSRect(x: 0, y: 230, width: 118, height: 30),
                  label: "New text size", parent: content)
        createTextSize.stringValue = format(24)
        configure(createTextColor, frame: NSRect(x: 134, y: 230, width: 118, height: 30),
                  label: "New text color", parent: content)
        createTextColor.formatter = nil; createTextColor.stringValue = "#ff3b5c"
        createTextControls = [styleLabel, createTextPreset, sizeLabel, colorLabel,
                              createTextSize, createTextColor]
    }

    private func publishCreateTextDefaults() {
        let selectedID = createTextPreset.selectedItem?.representedObject as? String
        createTextPreset.removeAllItems()
        createTextPreset.addItem(withTitle: "Plain")
        for preset in state.snapshot?.textStylePresets ?? [] {
            createTextPreset.addItem(withTitle: preset.label)
            createTextPreset.lastItem?.representedObject = preset.id
        }
        let desiredID = createTextDefaultsPublished ? selectedID
            : (state.snapshot?.textStylePresets.first(where: { $0.id == "rounded-box" })?.id
                ?? state.snapshot?.textStylePresets.first(where: { $0.id == "standard" })?.id)
        if let desiredID, let index = createTextPreset.itemArray.firstIndex(where: {
            $0.representedObject as? String == desiredID
        }) {
            createTextPreset.selectItem(at: index)
        } else {
            createTextPreset.selectItem(at: 0)
        }
        createTextDefaultsPublished = true
    }

    private func buildTextControls(in content: NSView) {
        content.frame.size.height = 720
        let heading = panelLabel("Text", frame: NSRect(x: 0, y: 326, width: 118, height: 24),
                                 size: 16, weight: .semibold, parent: content)
        textPreset.frame = NSRect(x: 126, y: 322, width: 126, height: 30)
        textPreset.setAccessibilityLabel("Text style preset")
        textPreset.target = self; textPreset.action = #selector(stageTextPreset)
        content.addSubview(textPreset)
        let familyLabel = panelFieldLabel("Font", x: 0, y: 356, parent: content)
        textFamily.frame = NSRect(x: 0, y: 378, width: 252, height: 30)
        textFamily.setAccessibilityLabel("Text font")
        content.addSubview(textFamily)
        let contentLabel = panelFieldLabel("Content", x: 0, y: 416, parent: content)
        let textScroll = NSScrollView(frame: NSRect(x: 0, y: 438, width: 252, height: 82))
        textScroll.hasVerticalScroller = true; textScroll.borderType = .lineBorder
        textEditor.frame = NSRect(x: 0, y: 0, width: 234, height: 82)
        textEditor.isRichText = false; textEditor.isVerticallyResizable = true
        textEditor.allowsUndo = true
        textEditor.isHorizontallyResizable = false; textEditor.textContainer?.widthTracksTextView = true
        textEditor.setAccessibilityLabel("Text content"); textScroll.documentView = textEditor
        content.addSubview(textScroll)
        let sizeLabel = panelFieldLabel("Size (8–512)", x: 0, y: 528, parent: content)
        configure(textSize, frame: NSRect(x: 0, y: 550, width: 78, height: 30), label: "Text size", parent: content)
        textSize.formatter = nil; textSize.stringValue = "32"
        textTraits.frame = NSRect(x: 86, y: 550, width: 166, height: 30)
        textTraits.setAccessibilityLabel("Text traits"); content.addSubview(textTraits)
        textAlignment.frame = NSRect(x: 0, y: 588, width: 252, height: 30)
        textAlignment.setAccessibilityLabel("Text alignment"); content.addSubview(textAlignment)
        let colorLabel = panelFieldLabel("Text color", x: 0, y: 626, parent: content)
        colorLabel.frame.size.width = 118
        textOutline.frame = NSRect(x: 126, y: 623, width: 126, height: 22)
        textOutline.setAccessibilityLabel("Text outline"); content.addSubview(textOutline)
        configure(textColor, frame: NSRect(x: 0, y: 648, width: 118, height: 30), label: "Text color", parent: content)
        textColor.formatter = nil; textColor.stringValue = "#111111"
        textPlate.frame = NSRect(x: 126, y: 648, width: 126, height: 30)
        textPlate.addItems(withTitles: ["No plate", "Square plate", "Rounded plate"])
        textPlate.setAccessibilityLabel("Text plate"); content.addSubview(textPlate)
        configure(textPlateColor, frame: NSRect(x: 0, y: 686, width: 118, height: 30),
                  label: "Text plate color", parent: content)
        textPlateColor.formatter = nil; textPlateColor.stringValue = "#ffffff"
        textShadow.frame = NSRect(x: 126, y: 686, width: 126, height: 30)
        textShadow.setAccessibilityLabel("Text drop shadow"); content.addSubview(textShadow)
        textShadow.target = self; textShadow.action = #selector(textShadowChanged)
        textShadowPanel.frame = NSRect(x: 0, y: 724, width: 252, height: 0)
        content.addSubview(textShadowPanel)
        for (index, row) in [("color", "Shadow color"), ("opacity", "Shadow opacity"),
                             ("blur", "Shadow blur"), ("offsetX", "Shadow X"),
                             ("offsetY", "Shadow Y")].enumerated() {
            let y = CGFloat(index * 38)
            let label = panelFieldLabel(row.1, x: 0, y: y + 4, parent: textShadowPanel)
            label.frame.size.width = 118
            let field = NSTextField()
            configure(field, frame: NSRect(x: 126, y: y, width: 126, height: 30),
                      label: "Text \(row.1.lowercased())", parent: textShadowPanel)
            field.formatter = nil
            textShadowFields[row.0] = field
        }
        textApplyButton = button("Apply", frame: NSRect(x: 0, y: 724, width: 118, height: 30),
                                 parent: content) { [weak self] in
            if self?.inlineTextInput != nil { self?.finishInlineTextInput(commit: true) }
            else { self?.applyTextEdits() }
        }
        textCancelButton = button("Cancel", frame: NSRect(x: 134, y: 724, width: 118, height: 30),
                                  parent: content) { [weak self] in
            if self?.inlineTextInput != nil { self?.finishInlineTextInput(commit: false) }
            else { self?.publishTextFields() }
        }
        textControls = [heading, textPreset, familyLabel, textFamily, contentLabel, textScroll, sizeLabel, textSize,
                        textTraits, textAlignment, colorLabel, textColor, textPlate, textPlateColor,
                        textShadow, textOutline, textShadowPanel, textApplyButton, textCancelButton]
    }

    /// Shipping bottom export bar: a settings disclosure with a live summary,
    /// filename and format suffix, save location, Copy image, the "Save as new
    /// file" switch and the primary Save.
    private func buildExportBar() {
        exportBar.wantsLayer = true
        exportBar.setAccessibilityLabel("Export bar")
        root.addSubview(exportBar)
        exportBarRule.wantsLayer = true
        exportBar.addSubview(exportBarRule)
        exportSettingsPanel.wantsLayer = true
        exportSettingsPanel.layer?.cornerRadius = tokens.number("r-xl")
        exportSettingsPanel.layer?.borderWidth = 1
        exportSettingsPanel.setAccessibilityLabel("Export settings")
        exportSettingsPanel.isHidden = true
        exportBar.addSubview(exportSettingsPanel)

        func caption(_ text: String) -> NSTextField {
            let label = NSTextField(labelWithString: text)
            label.font = .systemFont(ofSize: tokens.number("text-xs"), weight: .medium)
            label.identifier = NSUserInterfaceItemIdentifier("editor-muted")
            exportSettingsPanel.addSubview(label)
            return label
        }
        func add(_ view: NSView) { exportSettingsPanel.addSubview(view) }

        outputSizeMode = NSPopUpButton()
        outputSizeMode.addItems(withTitles: ["Original", "75%", "50%", "Custom"])
        outputSizeMode.setAccessibilityLabel("Output size")
        outputSizeMode.target = self; outputSizeMode.action = #selector(outputSizeModeChanged)
        add(outputSizeMode)
        outputDimensions.setAccessibilityLabel("Output dimensions")
        outputDimensions.font = .monospacedDigitSystemFont(ofSize: tokens.number("text-2xs"), weight: .regular)
        add(outputDimensions)
        configure(outputWidth, frame: .zero, label: "Custom output width", parent: exportSettingsPanel)
        configure(outputHeight, frame: .zero, label: "Custom output height", parent: exportSettingsPanel)
        [outputWidth, outputHeight].forEach {
            $0.formatter = outputIntegerFormatter; $0.delegate = self
        }
        outputAspectLock.title = "Lock"
        outputAspectLock.state = .on
        outputAspectLock.setAccessibilityLabel("Lock output aspect ratio")
        outputAspectLock.target = self; outputAspectLock.action = #selector(outputAspectLockChanged)
        add(outputAspectLock)

        outputQuality = NSPopUpButton()
        outputQuality.addItems(withTitles: ["Preserve quality", "Compress", "Maximum file size"])
        outputQuality.setAccessibilityLabel("Save quality")
        outputQuality.target = self; outputQuality.action = #selector(outputOptionsChanged)
        add(outputQuality)
        outputCompressionPreset.addItems(withTitles: Self.outputCompressionPresets.map { $0.name })
        outputCompressionPreset.setAccessibilityLabel("Output compression preset")
        outputCompressionPreset.target = self
        outputCompressionPreset.action = #selector(outputCompressionPresetChanged)
        add(outputCompressionPreset)
        configure(outputQualityValue, frame: .zero, label: "Output quality value", parent: exportSettingsPanel)
        configure(outputPngPalette, frame: .zero, label: "PNG maximum colors", parent: exportSettingsPanel)
        configure(outputByteBudget, frame: .zero, label: "Output byte budget", parent: exportSettingsPanel)
        outputQualityValue.stringValue = "98"
        outputPngPalette.placeholderString = "Optional"
        outputByteBudget.placeholderString = "Required"
        outputByteBudget.stringValue = "10000000"
        [outputQualityValue, outputPngPalette, outputByteBudget].forEach {
            $0.formatter = outputIntegerFormatter; $0.delegate = self
        }
        exportEstimateValue.setAccessibilityLabel("Estimated size")
        exportEstimateValue.font = .systemFont(ofSize: tokens.number("text-sm"), weight: .semibold)
        exportEstimateValue.toolTip = "Estimated export file size for the current format, quality, and output size"
        add(exportEstimateValue)
        exportEstimateDelta.setAccessibilityLabel("Estimated size change")
        exportEstimateDelta.font = .systemFont(ofSize: tokens.number("text-xs"), weight: .medium)
        exportEstimateDelta.toolTip = "Change versus the original image, before this export"
        add(exportEstimateDelta)
        outputPreviewMode = NSSegmentedControl(labels: ["Edited canvas", "Encoded output"],
                                               trackingMode: .selectOne, target: self,
                                               action: #selector(changeOutputPreview))
        outputPreviewMode.selectedSegment = 0
        outputPreviewMode.setAccessibilityLabel("Output preview image")
        add(outputPreviewMode)
        previewOutputButton = button("Preview output", frame: .zero, parent: exportSettingsPanel) {
            [weak self] in self?.previewOutput()
        }
        previewOutputButton.toolTip = "Encode the output into the canvas without saving a file or draft"
        func group(_ key: String, _ text: String, _ views: [(NSView, CGFloat, CGFloat)], _ width: CGFloat) {
            exportGroups[key] = (caption(text), views, width)
        }
        group("size", "Output size", [(outputSizeMode as NSView, 0, 110), (outputDimensions as NSView, 118, 110)], 228)
        group("custom", "Width × height", [(outputWidth as NSView, 0, 64), (outputHeight as NSView, 72, 64),
                                           (outputAspectLock as NSView, 144, 72)], 216)
        group("quality", "Save quality", [(outputQuality as NSView, 0, 150)], 150)
        group("preset", "Quality", [(outputCompressionPreset as NSView, 0, 100),
                                    (outputQualityValue as NSView, 108, 56)], 164)
        group("palette", "PNG colors", [(outputPngPalette as NSView, 0, 90)], 90)
        group("maximum", "Maximum file size (bytes)", [(outputByteBudget as NSView, 0, 150)], 160)
        group("estimate", "Est. size", [(exportEstimateValue as NSView, 0, 92),
                                        (exportEstimateDelta as NSView, 96, 56)], 152)
        group("preview", "Canvas", [(outputPreviewMode as NSView, 0, 200),
                                    (previewOutputButton as NSView, 208, 120)], 328)

        exportDisclosure = button("", frame: .zero, parent: exportBar) { [weak self] in
            self?.toggleExportSettings()
        }
        exportDisclosure.setAccessibilityLabel("Export settings")
        exportDisclosure.toolTip = "Show export settings"
        exportDisclosureTitle.font = .systemFont(ofSize: tokens.number("text-sm"), weight: .medium)
        exportSummary.font = .monospacedSystemFont(ofSize: tokens.number("text-2xs"), weight: .regular)
        exportSummary.lineBreakMode = .byTruncatingTail
        exportSummary.setAccessibilityLabel("Export summary")
        exportChevron.alignment = .center
        [exportDisclosureTitle, exportSummary, exportChevron].forEach { exportDisclosure.addSubview($0) }

        exportFilenameCaption.font = .systemFont(ofSize: tokens.number("text-xs"), weight: .medium)
        exportSavingToCaption.font = .systemFont(ofSize: tokens.number("text-xs"))
        exportSavingToCaption.identifier = NSUserInterfaceItemIdentifier("editor-muted")
        outputLocation.font = .monospacedSystemFont(ofSize: tokens.number("text-2xs"), weight: .regular)
        outputLocation.lineBreakMode = .byTruncatingMiddle
        outputLocation.setAccessibilityLabel("Save location")
        [exportFilenameCaption, exportSavingToCaption, outputLocation].forEach { exportBar.addSubview($0) }
        changeOutputDirectoryButton = button("Change…", frame: .zero, parent: exportBar) {
            [weak self] in self?.chooseOutputDirectory()
        }
        changeOutputDirectoryButton.setAccessibilityLabel("Change save location")
        outputFilename.setAccessibilityLabel("Saved filename")
        outputFilename.placeholderString = "Filename"
        outputFilename.delegate = self
        exportBar.addSubview(outputFilename)
        outputFormat = NSPopUpButton()
        outputFormat.addItems(withTitles: [".png", ".jpg", ".webp"])
        outputFormat.setAccessibilityLabel("Format")
        outputFormat.target = self; outputFormat.action = #selector(outputOptionsChanged)
        exportBar.addSubview(outputFormat)
        copyImageButton = button("Copy image", frame: .zero, parent: exportBar) {
            [weak self] in self?.copyEditedImage()
        }
        copyImageButton.toolTip = "Copy the edited image to the clipboard. Does not save a file."
        showInFolderButton = button("Show in Folder", frame: .zero, parent: exportBar) {
            [weak self] in self?.revealSavedFile()
        }
        showInFolderButton.isHidden = true
        exportStatus.font = .systemFont(ofSize: tokens.number("text-xs"))
        exportStatus.lineBreakMode = .byTruncatingTail
        exportStatus.alignment = .right
        exportStatus.setAccessibilityLabel("Export status")
        exportBar.addSubview(exportStatus)
        saveAsNewSwitch.setAccessibilityLabel("Save as new file")
        saveAsNewSwitch.toolTip = "Save as a new file and leave the original untouched"
        saveAsNewSwitch.target = self; saveAsNewSwitch.action = #selector(saveAsNewChanged)
        exportBar.addSubview(saveAsNewSwitch)
        saveAsNewLabel.font = .systemFont(ofSize: tokens.number("text-sm"))
        exportBar.addSubview(saveAsNewLabel)
        exportSaveButton = button("Save", frame: .zero, parent: exportBar) { [weak self] in self?.saveExport() }
        exportSaveButton.primary = true
        updateOutputOptionControls()
    }

    private func buildLayersPanel() {
        // Shipping `.screenshot-layers-heading`: title, count pill and Add image layer.
        let headingHeight: CGFloat = 30
        let title = NSTextField(labelWithString: EditorChrome.text("layers", "title"))
        title.font = .systemFont(ofSize: tokens.number("text-md"), weight: .semibold)
        title.frame = NSRect(x: 0, y: 6, width: 56, height: 18)
        title.textColor = tokens.color("text")
        title.sizeToFit(); title.frame.origin.y = (headingHeight - title.frame.height) / 2
        layersPanel.addSubview(title)
        layerCount.font = .monospacedSystemFont(ofSize: tokens.number("text-2xs"), weight: .regular)
        layerCount.alignment = .center
        layerCount.wantsLayer = true
        layerCount.layer?.cornerRadius = 9.5
        layerCount.frame = NSRect(x: title.frame.maxX + tokens.number("s-3"), y: (headingHeight - 19) / 2,
                                  width: 19, height: 19)
        layerCount.setAccessibilityLabel("Layer count")
        layersPanel.addSubview(layerCount)
        addLayerButton = button("", frame: NSRect(x: 272 - 30, y: 0, width: 30, height: 30),
                                parent: layersPanel) { [weak self] in self?.chooseImage() }
        addLayerButton.quiet = true; addLayerButton.icon = .shipping("plus")
        addLayerButton.autoresizingMask = [.minXMargin]
        addLayerButton.setAccessibilityLabel(EditorChrome.text("layers", "add"))
        addLayerButton.toolTip = EditorChrome.text("layers", "add")
        layerHeadingRule.wantsLayer = true
        layerHeadingRule.frame = NSRect(x: 0, y: headingHeight - 1, width: 272, height: 1)
        layerHeadingRule.autoresizingMask = [.width]
        layersPanel.addSubview(layerHeadingRule)
        let panelScroll = NSScrollView(frame: NSRect(x: 0, y: headingHeight, width: layersPanel.bounds.width,
                                                     height: max(0, layersPanel.bounds.height - headingHeight)))
        panelScroll.autoresizingMask = [.width, .height]
        panelScroll.hasVerticalScroller = true; panelScroll.scrollerStyle = .overlay
        panelScroll.drawsBackground = false
        layerContent.frame = NSRect(x: 0, y: 0, width: 272, height: 550)
        panelScroll.documentView = layerContent; layersPanel.addSubview(panelScroll)

        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 272, height: 106))
        scroll.hasVerticalScroller = true; scroll.drawsBackground = false
        layerTable = EditorLayerTable(frame: scroll.bounds)
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("editor-layer"))
        column.width = 252; layerTable.addTableColumn(column); layerTable.headerView = nil
        layerTable.rowHeight = 32; layerTable.dataSource = self; layerTable.delegate = self
        layerTable.allowsEmptySelection = true; layerTable.setAccessibilityLabel("Screenshot layers")
        layerTable.contextMenu = { [weak self] row in self?.layerContextMenu(row: row) }
        scroll.documentView = layerTable; layerContent.addSubview(scroll)

        panelFieldLabel("Name", x: 0, y: 112, parent: layerContent)
        layerName.frame = NSRect(x: 0, y: 130, width: 190, height: 30)
        layerName.setAccessibilityLabel("Layer name")
        layerName.alignment = .left; layerName.placeholderString = "Layer name"
        layerContent.addSubview(layerName)
        renameButton = button("Rename", frame: NSRect(x: 196, y: 130, width: 76, height: 30),
                              parent: layerContent) { [weak self] in self?.renameLayer() }
        visibilityButton = button("Hide", frame: NSRect(x: 0, y: 168, width: 128, height: 30),
                                  parent: layerContent) { [weak self] in self?.toggleVisibility() }
        lockButton = button("Lock", frame: NSRect(x: 144, y: 168, width: 128, height: 30),
                            parent: layerContent) { [weak self] in self?.toggleLock() }

        panelFieldLabel("Opacity (0–100)", x: 0, y: 204, parent: layerContent)
        configure(layerOpacity, frame: NSRect(x: 0, y: 222, width: 216, height: 30),
                  label: "Layer opacity", parent: layerContent)
        opacityButton = button("Set", frame: NSRect(x: 222, y: 222, width: 50, height: 30),
                               parent: layerContent) { [weak self] in self?.setOpacity() }

        panelFieldLabel("X", x: 0, y: 258, parent: layerContent)
        panelFieldLabel("Y", x: 92, y: 258, parent: layerContent)
        configure(layerX, frame: NSRect(x: 0, y: 276, width: 86, height: 30),
                  label: "Layer X", parent: layerContent)
        configure(layerY, frame: NSRect(x: 92, y: 276, width: 86, height: 30),
                  label: "Layer Y", parent: layerContent)
        moveButton = button("Move", frame: NSRect(x: 184, y: 276, width: 88, height: 30),
                            parent: layerContent) { [weak self] in self?.moveLayer() }

        panelFieldLabel("Transform", x: 0, y: 314, parent: layerContent)
        rotateLeftButton = button("Rotate left", frame: NSRect(x: 0, y: 334, width: 128, height: 30),
                                  parent: layerContent) {
            [weak self] in
            self?.transformLayer("rotate-counterclockwise", message: "Rotating layer left…")
        }
        rotateRightButton = button("Rotate right", frame: NSRect(x: 144, y: 334, width: 128, height: 30),
                                   parent: layerContent) {
            [weak self] in
            self?.transformLayer("rotate-clockwise", message: "Rotating layer right…")
        }
        flipHorizontalButton = button("Flip horizontal", frame: NSRect(x: 0, y: 372, width: 128, height: 30),
                                      parent: layerContent) {
            [weak self] in
            self?.transformLayer("flip-horizontal", message: "Flipping layer horizontally…")
        }
        flipVerticalButton = button("Flip vertical", frame: NSRect(x: 144, y: 372, width: 128, height: 30),
                                    parent: layerContent) {
            [weak self] in
            self?.transformLayer("flip-vertical", message: "Flipping layer vertically…")
        }

        duplicateButton = button("Duplicate", frame: NSRect(x: 0, y: 410, width: 128, height: 30),
                                 parent: layerContent) { [weak self] in self?.duplicateLayer() }
        deleteButton = button("Delete", frame: NSRect(x: 144, y: 410, width: 128, height: 30),
                              parent: layerContent) { [weak self] in self?.deleteLayer() }
        moveUpButton = button("Move up", frame: NSRect(x: 0, y: 448, width: 128, height: 30),
                              parent: layerContent) { [weak self] in self?.reorderLayer(up: true) }
        moveDownButton = button("Move down", frame: NSRect(x: 144, y: 448, width: 128, height: 30),
                                parent: layerContent) { [weak self] in self?.reorderLayer(up: false) }
        combineLayers = NSPopUpButton(frame: NSRect(x: 136, y: 494, width: 136, height: 34), pullsDown: true)
        combineLayers.autoenablesItems = false
        combineLayers.addItem(withTitle: "Combine layers")
        for title in ["Merge down", "Merge visible", "Flatten image"] { combineLayers.addItem(withTitle: title) }
        combineLayers.target = self; combineLayers.action = #selector(combineLayersSelected(_:))
        combineLayers.setAccessibilityLabel("Combine layers")
        layerContent.addSubview(combineLayers)
        importImageButton = button("Add image…", frame: NSRect(x: 0, y: 494, width: 128, height: 34),
                                   parent: layerContent) { [weak self] in self?.chooseImage() }
        importImageButton.primary = true
        rotationSnapLabel = panelFieldLabel("Shift rotation snap (1–180°)", x: 0, y: 550, parent: layerContent)
        configure(rotationSnap, frame: NSRect(x: 0, y: 574, width: 272, height: 30),
                  label: "Shift rotation snap", parent: layerContent)
        rotationSnap.stringValue = "15"
        rotationSnap.toolTip = "Hold Shift while dragging the rotate handle. Does not edit the document."
        rotationSnap.delegate = self
        rotationSnap.target = self; rotationSnap.action = #selector(rotationSnapChanged)
        annotationControls = EditorAnnotationControls(tokens: tokens, formatter: editorNumberFormatter)
        annotationControls.apply = { [weak self] patch in
            guard let self, !self.state.busy, let layer = self.selectedLayer else { return }
            self.layerCommand(layer, edit: ["action": "annotation_style", "patch": patch],
                              message: "Applying annotation style…")
        }
        annotationControls.reportError = { [weak self] message in self?.showError(message) }
        annotationControls.resized = { [weak self] height in
            self?.rotationSnapLabel.frame.origin.y = 558 + height
            self?.rotationSnap.frame.origin.y = 582 + height
            self?.layerContent.frame.size.height = 620 + height
        }
        layerContent.addSubview(annotationControls)
    }

    @objc private func changeSection() {
        cancelCrop()
        cancelDrawing()
        cancelViewportPan()
        geometryPanel.isHidden = sectionControl.selectedSegment != Section.geometry
        layersPanel.isHidden = sectionControl.selectedSegment != Section.layers
        drawPanel.isHidden = sectionControl.selectedSegment != Section.draw
        // The export bar and its encoded preview do not depend on the section.
        changeOutputPreview()
        updateDrawing()
    }

    @objc private func changeDrawTool() {
        cancelDrawing()
        drawOverlay.shape = EditorDrawOverlay.Shape.allCases[drawTool.indexOfSelectedItem]
        if drawOverlay.shape == .wand || drawOverlay.shape.isBackgroundBrush {
            lastBackgroundTool = drawOverlay.shape
        }
        if isGroupedShape(drawOverlay.shape) { lastGroupedShape = drawOverlay.shape }
        publishDrawToolControls()
        updateToolRail()
    }

    private func isGroupedShape(_ shape: EditorDrawOverlay.Shape) -> Bool {
        [.rectangle, .ellipse, .line, .triangle, .diamond, .star].contains(shape)
    }

    /// Shipping `.screenshot-tool-rail`, in `captures_app::editor_chrome` order.
    /// layoutEditor() places the buttons in the rail column.
    private func buildToolRail() {
        let icons: [String: CaptureButtonIcon] = [
            "v": .editorSelect, "c": .editorCrop, "t": .editorText, "shapes": .editorShapes,
            "a": .editorArrow, "p": .editorPen, "b": .editorBackground,
        ]
        let side = EditorChrome.metric("rail_button")
        for tool in EditorChrome.rail {
            let key = tool.key
            let control = CaptureButton("", frame: NSRect(x: 0, y: 0, width: side, height: side),
                                        tokens: tokens) { [weak self] in self?.chooseRailTool(key) }
            control.icon = icons[key]
            control.toolTip = tool.name
            control.setAccessibilityLabel(tool.name)
            control.highlightChanged = { [weak self] button, visible in self?.showRailTip(button, visible) }
            if key == "shapes" {
                let menu = NSMenu(title: tool.label)
                menu.autoenablesItems = false
                let shapes: [String: EditorDrawOverlay.Shape] = [
                    "rectangle": .rectangle, "ellipse": .ellipse, "line": .line,
                    "triangle": .triangle, "diamond": .diamond, "star": .star,
                ]
                for item in EditorChrome.shapes {
                    guard let shape = shapes[item.key] else { continue }
                    let option = NSMenuItem(title: item.name, action: #selector(chooseRailShape(_:)), keyEquivalent: "")
                    option.target = self
                    option.tag = EditorDrawOverlay.Shape.allCases.firstIndex(of: shape)!
                    let icon = item.icon
                    let image = NSImage(size: NSSize(width: 18, height: 18), flipped: true) { rect in
                        NSColor.black.setStroke()
                        ShippingIcons.stroke(icon, in: rect)
                        return true
                    }
                    image.isTemplate = true
                    option.image = image
                    menu.addItem(option)
                }
                control.menu = menu
            }
            toolRailButtons.append((key, control))
            root.addSubview(control)
        }
    }

    private func chooseRailTool(_ key: String) {
        guard state.snapshot != nil, !state.busy, !importLoading, window.attachedSheet == nil else { return }
        if key == "shapes" {
            activateTool(section: Section.draw, shape: lastGroupedShape)
            if let button = toolRailButtons.first(where: { $0.key == key })?.button {
                button.menu?.popUp(positioning: nil, at: NSPoint(x: button.bounds.maxX + tokens.number("s-4"), y: 0), in: button)
            }
        } else {
            window.makeFirstResponder(nil)
            _ = activateToolShortcut(key)
        }
        focusActiveCanvas()
    }

    @objc private func chooseRailShape(_ sender: NSMenuItem) {
        guard state.snapshot != nil, !state.busy, !importLoading, window.attachedSheet == nil else { return }
        activateTool(section: Section.draw, shape: EditorDrawOverlay.Shape.allCases[sender.tag])
        focusActiveCanvas()
    }

    private func focusActiveCanvas() {
        switch sectionControl.selectedSegment {
        case Section.draw: window.makeFirstResponder(drawOverlay)
        case Section.layers: window.makeFirstResponder(selectionOverlay)
        default: window.makeFirstResponder(cropOverlay)
        }
    }

    private func updateToolRail() {
        for (key, button) in toolRailButtons {
            let selected: Bool
            switch key {
            case "v": selected = sectionControl?.selectedSegment == Section.layers
            case "c": selected = sectionControl?.selectedSegment == Section.geometry && cropPrevious != nil
            case "shapes": selected = sectionControl?.selectedSegment == Section.draw && isGroupedShape(drawOverlay.shape)
            case "b": selected = sectionControl?.selectedSegment == Section.draw && (drawOverlay.shape == .wand || drawOverlay.shape.isBackgroundBrush)
            default:
                let shape: EditorDrawOverlay.Shape = key == "t" ? .text : key == "a" ? .arrow : .pen
                selected = sectionControl?.selectedSegment == Section.draw && drawOverlay.shape == shape
            }
            button.isEnabled = state.snapshot != nil && !state.busy && inlineTextInput == nil
                && !importLoading
            button.selected = selected; button.primary = selected
            button.setAccessibilityValue(selected ? 1 : 0)
            button.menu?.items.forEach { $0.state = $0.tag == drawTool?.indexOfSelectedItem ? .on : .off }
            if key == "shapes" {
                let names: [EditorDrawOverlay.Shape: String] = [
                    .rectangle: "rectangle", .ellipse: "ellipse", .line: "line",
                    .triangle: "triangle", .diamond: "diamond", .star: "star",
                ]
                let current = isGroupedShape(drawOverlay.shape) ? drawOverlay.shape : lastGroupedShape
                let tooltip = EditorChrome.shapesTooltip(names[current] ?? "rectangle")
                if button.toolTip != tooltip { button.toolTip = tooltip }
            }
            button.needsDisplay = true
        }
    }

    private func publishDrawToolControls() {
        guard let drawTool, drawTool.indexOfSelectedItem >= 0 else { return }
        let shape = EditorDrawOverlay.Shape.allCases[drawTool.indexOfSelectedItem]
        // Shipping's properties heading names the active tool.
        drawHeading?.stringValue = EditorChrome.toolLabel(
            shape == .text ? "t" : shape == .arrow ? "a" : shape.rawValue)
        let wand = shape == .wand
        let brush = shape.isBackgroundBrush
        let creatingText = shape == .text
        let textSelected = selectedLayer?.kind == .text
        let creatingDrawing = !wand && !brush && !creatingText && !textSelected
        wandToleranceLabel?.isHidden = !wand
        wandTolerance.isHidden = !wand; wandContiguous.isHidden = !wand
        brushSizeLabel?.isHidden = !brush; brushSize.isHidden = !brush
        brushSoftnessLabel?.isHidden = !brush; brushSoftness.isHidden = !brush
        createTextControls.forEach { $0.isHidden = !creatingText }
        drawingDefaultControls.forEach { $0.isHidden = !creatingDrawing }
        let closed = [.rectangle, .ellipse, .triangle, .diamond, .star].contains(shape)
        drawingStroke.isHidden = !creatingDrawing || !closed
        drawingFillControls.forEach { $0.isHidden = !creatingDrawing || !closed }
        let drawingShadowVisible = creatingDrawing && drawingDropShadow.state == .on
        drawingShadowControls.forEach { $0.isHidden = !drawingShadowVisible }
        drawHelper.stringValue = wand
            ? "Wand removes matching pixels from the frontmost visible image."
            : brush
                ? "Pixels preview while dragging. Release commits one undo step; Escape cancels."
                : shape == .text
                    ? "Click once to create empty auto-width text. Edit it below, then Apply."
                    : drawingShadowVisible ? "Drawing pixels update in the background while dragging."
                    : "This tool creates one annotation layer on release."
        textControls.forEach { $0.isHidden = !textSelected }
        // Other tools need no Wand/brush/text-default fields. Collapse their
        // reserved space without overlapping Text's creation controls.
        let compact = textSelected && !wand && !brush && !creatingText
        drawHelper.frame.origin.y = compact ? 148 : drawingShadowVisible ? 526 : creatingDrawing ? 338 : 278
        if let heading = textControls.first, let content = heading.superview {
            let offset = (compact ? 196.0 : creatingDrawing ? 358.0 : 326.0) - heading.frame.minY
            for control in textControls { control.frame.origin.y += offset }
            updateTextShadowControls()
            content.frame.size.height = textSelected
                ? textCancelButton.frame.maxY + 8 : drawHelper.frame.maxY + 8
        }
    }

    @objc private func outputOptionsChanged() {
        normalizeOutputQuality()
        synchronizeOutputCompressionPreset()
        invalidateOutput()
        updateOutputOptionControls()
        // A different encoding is no longer the file that was just saved.
        exportInputsChanged(clearsSaved: true)
        updateControls()
    }

    @objc private func outputSizeModeChanged() {
        if outputSizeMode.indexOfSelectedItem == 3, let snapshot = state.snapshot {
            outputWidth.stringValue = format(snapshot.width)
            outputHeight.stringValue = format(snapshot.height)
        }
        publishOutputDimensions()
        invalidateOutput()
        updateOutputOptionControls()
        exportInputsChanged(clearsSaved: false)
        updateControls()
    }

    @objc private func outputAspectLockChanged() {
        publishOutputDimensions()
        updateControls()
    }

    @objc private func rotationSnapChanged() {
        let value = number(rotationSnap) ?? selectionOverlay.rotationSnapDegrees
        let degrees = min(180, max(1, value.rounded()))
        rotationSnap.stringValue = format(degrees)
        selectionOverlay.rotationSnapDegrees = degrees
    }

    func controlTextDidEndEditing(_ notification: Notification) {
        if notification.object as? NSTextField === rotationSnap { rotationSnapChanged() }
    }

    func controlTextDidChange(_ notification: Notification) {
        guard let field = notification.object as? NSTextField else { return }
        if drawingShadowFields.values.contains(where: { $0 === field }) {
            drawingShadowCustomized = true
            return
        }
        if [drawingStrokeColor, drawingFillColor, drawingStrokeWidth, drawingOpacity]
            .contains(where: { $0 === field }) {
            updateDrawingPreviewStyle()
            return
        }
        if [cropX, cropY, cropWidth, cropHeight].contains(where: { $0 === field }) {
            publishCropSelection()
            return
        }
        if field === brushSize {
            if let value = Double(field.stringValue), value.isFinite {
                drawOverlay.brushDiameter = CGFloat(min(120, max(4, value)))
            }
            return
        }
        if field === outputWidth || field === outputHeight {
            if outputAspectLock.state == .on, let snapshot = state.snapshot,
               snapshot.width > 0, snapshot.height > 0, let value = outputInteger(field),
               (1...16_384).contains(value) {
                if field === outputWidth {
                    outputHeight.stringValue = String(max(1, UInt64((Double(value) * snapshot.height / snapshot.width).rounded())))
                } else {
                    outputWidth.stringValue = String(max(1, UInt64((Double(value) * snapshot.width / snapshot.height).rounded())))
                }
            }
            publishOutputDimensions()
            invalidateOutput()
            exportInputsChanged(clearsSaved: false)
            updateControls()
            return
        }
        if field === outputFilename {
            // Typing a name other than the source's turns on "Save as new file".
            refreshExportBar(action: ["kind": "set_stem", "stem": field.stringValue])
            clearSavedResult()
            updateControls()
            return
        }
        guard [outputQualityValue, outputPngPalette, outputByteBudget].contains(where: { $0 === field }) else { return }
        synchronizeOutputCompressionPreset()
        invalidateOutput()
        exportInputsChanged(clearsSaved: false)
        updateControls()
    }

    @objc private func outputCompressionPresetChanged() {
        guard outputQuality.indexOfSelectedItem == 1,
              let selected = outputCompressionPreset.titleOfSelectedItem,
              let preset = Self.outputCompressionPresets.first(where: { $0.name == selected }) else {
            synchronizeOutputCompressionPreset(); return
        }
        outputQualityValue.stringValue = String(preset.value)
        outputPngPalette.stringValue = ""
        outputOptionsChanged()
    }

    @objc private func changeOutputPreview() {
        // Like the shipping comparison, the encoded image shows only while the
        // export settings are open.
        if exportSettingsOpen, outputPreviewMode.selectedSegment == 1, let encodedOutput {
            preview.image = NSImage(cgImage: encodedOutput.image,
                                    size: NSSize(width: encodedOutput.image.width,
                                                 height: encodedOutput.image.height))
            viewportCanvasSize = NSSize(width: encodedOutput.image.width,
                                        height: encodedOutput.image.height)
        } else {
            if encodedOutput == nil { outputPreviewMode.selectedSegment = 0 }
            preview.image = editedImage
            if let editedImage { viewportCanvasSize = editedImage.size }
        }
        updateViewportGeometry()
    }

    private func previewOutput() {
        guard let artifactID = state.artifactID,
              let options = outputOptions(),
              let generation = state.beginCommand() else { return }
        invalidateOutput()
        status.stringValue = "Encoding output preview…"; updateControls()
        worker.encode(options) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let output):
                guard self.state.completeOutput(generation: generation, artifactID: artifactID) else { return }
                self.encodedOutput = output
                self.outputPreviewMode.selectedSegment = 1
                self.changeOutputPreview()
                self.status.textColor = self.tokens.color("text-muted")
                self.status.stringValue = "Output preview encoded. No file was saved."
            case .failure(let error):
                guard self.state.fail(generation: generation) else { return }
                self.invalidateOutput()
                self.showError("Output preview failed: \(error.localizedDescription)")
            }
            self.updateControls()
            self.submitPendingImportIfReady()
        }
    }

    private func copyEditedImage() {
        guard let artifactID = state.artifactID, let generation = state.beginCommand() else { return }
        exportError = nil
        status.stringValue = "Copying edited image…"; updateControls()
        // Copy the published edited frame, never the selected encoded preview or
        // export budget. Encoding stays on the existing serialized editor worker.
        worker.encode(["format": "png", "quality": "preserve", "quality_value": 100,
                       "max_size_bytes": NSNull(), "png": ["max_colors": NSNull()]]) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let output):
                guard self.state.completeOutput(generation: generation, artifactID: artifactID) else { return }
                if self.writeClipboard(output.data) {
                    self.status.textColor = self.tokens.color("text-muted")
                    self.status.stringValue = "Edited image copied. No file or draft was saved."
                    self.confirmCopy()
                } else {
                    self.showExportError("Couldn’t copy the edited image. The clipboard is unavailable; try again.")
                }
            case .failure(let error):
                guard self.state.fail(generation: generation) else { return }
                self.showExportError("Couldn’t copy the edited image: \(error.localizedDescription)")
            }
            self.updateControls()
            self.submitPendingImportIfReady()
        }
    }

    private func chooseOutputDirectory() {
        guard let artifactID = state.artifactID else { return }
        let generation = state.generation
        let directory = exportBarState?.directory ?? outputDirectory
        let current = directory.isEmpty ? nil : URL(fileURLWithPath: directory, isDirectory: true)
        let completion: (URL?) -> Void = { [weak self] selected in
            DispatchQueue.main.async {
                guard let self, let selected,
                      self.state.generation == generation,
                      self.state.artifactID == artifactID else { return }
                // A folder other than the source's turns on "Save as new file".
                self.refreshExportBar(action: ["kind": "set_directory", "directory": selected.path])
                self.clearSavedResult()
                self.updateControls()
            }
        }
        if let directoryPicker {
            directoryPicker(window, current, completion)
            return
        }
        let panel = Self.outputDirectoryPanel(current: current)
        panel.beginSheetModal(for: window) { response in
            completion(response == .OK ? panel.url : nil)
        }
    }

    static func outputDirectoryPanel(current: URL?) -> NSOpenPanel {
        let panel = NSOpenPanel()
        panel.title = "Choose save location"
        panel.message = "Choose save location"
        panel.prompt = "Choose"
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.canCreateDirectories = true
        panel.directoryURL = current
        return panel
    }

    // MARK: Export bar

    private func defaultOutputStem(_ date: Date = Date()) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.timeZone = .current
        formatter.dateFormat = "yyyy-MM-dd_HH-mm-ss"
        return "Captures_\(formatter.string(from: date))_edited"
    }

    /// Forget the previous screenshot's target, saved file and estimate.
    private func resetExportState(originalBytes: UInt64) {
        exportTarget = nil; exportBarState = nil
        exportError = nil; exportNotice = nil; exportNoticeToken += 1
        copyConfirmed = false; copyConfirmToken += 1
        lastSavedPath = nil
        estimate = nil; estimatePending = false; estimateGeneration += 1
        estimateWork?.cancel(); estimateWork = nil
        self.originalBytes = originalBytes > 0 ? originalBytes : nil
        outputFilename.stringValue = ""
        outputLocation.stringValue = outputDirectory; outputLocation.toolTip = outputDirectory
        publishExportBar()
    }

    /// A saved original starts in overwrite mode beside itself; otherwise the
    /// first Save writes a new file in the output folder.
    private func startExportTarget(_ snapshot: NativeEditorSnapshot) {
        let source: Any
        if let path = snapshot.originalExportPath {
            source = ["artifact_id": snapshot.artifactID, "path": path] as [String: Any]
        } else {
            source = NSNull()
        }
        refreshExportBar(initial: ["source": source, "default_directory": outputDirectory,
                                   "default_stem": defaultOutputStem()])
        scheduleEstimate()
    }

    /// Apply one target action through the shared model and republish the bar.
    private func refreshExportBar(action: [String: Any]? = nil, initial: [String: Any]? = nil) {
        guard let snapshot = state.snapshot else { publishExportBar(); return }
        var request: [String: Any] = [
            "document_size": [UInt32(max(1, snapshot.width.rounded())), UInt32(max(1, snapshot.height.rounded()))],
            "transparent_background": snapshot.background == nil,
        ]
        if let initial { request["init"] = initial } else if let exportTarget { request["target"] = exportTarget }
        else { publishExportBar(); return }
        if let action { request["action"] = action }
        var estimateValue: [String: Any] = ["pending": estimatePending]
        if let estimate {
            estimateValue["bytes"] = estimate.bytes
            if let baseline = estimate.baselineBytes { estimateValue["baseline_bytes"] = baseline }
        }
        request["estimate"] = estimateValue
        let options = outputOptionsResult()
        exportOptionsError = options.error
        // Invalid option fields still present the target; the bar explains the fix.
        request["options"] = options.options ?? fallbackOutputOptions()
        do {
            let bar = try NativeExportBar.present(request)
            exportTarget = bar.target
            exportBarState = bar
        } catch {
            exportError = "Couldn’t prepare the export: \(error.localizedDescription)"
        }
        publishExportBar()
    }

    /// Options that always encode, used only to present the target and copy.
    private func fallbackOutputOptions() -> [String: Any] {
        let formats = ["png", "jpeg", "webp"]
        let index = max(0, min(2, outputFormat?.indexOfSelectedItem ?? 0))
        return ["format": formats[index], "quality": "preserve", "quality_value": 100,
                "png": [String: Any](), "size": ["mode": "original"]]
    }

    private func publishExportBar() {
        guard exportDisclosure != nil else { return }
        let bar = exportBarState
        exportSummary.stringValue = bar?.summary ?? ""
        exportDisclosure.setAccessibilityValue(bar?.summary ?? "")
        // Typing sends set_stem with the typed text, so an edited field already matches.
        if let bar, outputFilename.stringValue != bar.stem { outputFilename.stringValue = bar.stem }
        let directory = bar?.directory ?? outputDirectory
        outputLocation.stringValue = directory; outputLocation.toolTip = directory
        if let bar {
            let jpeg = bar.sourcePath.map { URL(fileURLWithPath: $0).pathExtension.lowercased() == "jpeg" } == true
            outputFormat.item(at: 1)?.title = jpeg ? ".jpeg" : ".jpg"
        }
        saveAsNewSwitch.isHidden = bar?.formatRequiresCopy ?? true
        saveAsNewLabel.isHidden = saveAsNewSwitch.isHidden
        saveAsNewSwitch.state = bar?.savingCopy == false ? .off : .on
        exportEstimateValue.stringValue = bar?.estimateLabel ?? "—"
        exportEstimateValue.textColor = tokens.color(estimatePending ? "text-subtle" : "text")
        exportEstimateDelta.stringValue = bar?.deltaLabel ?? ""
        exportEstimateDelta.textColor = tokens.color((bar?.deltaPercent ?? 0) < 0 ? "positive-text" : "caution-text")
        let copyTitle = copyConfirmed ? "✓ Copied" : "Copy image"
        if copyImageButton.title != copyTitle { copyImageButton.title = copyTitle }
        copyImageButton.setAccessibilityLabel(copyConfirmed ? "Copied" : "Copy image")
        showInFolderButton.isHidden = lastSavedPath == nil
        exportSaveButton.toolTip = bar?.hint
        let message: (String, String)
        if let exportError {
            message = (exportError, "danger-text")
        } else if let error = exportOptionsError ?? bar?.error {
            message = (error, "danger-text")
        } else if let exportNotice {
            message = (exportNotice, "positive-text")
        } else if let bar {
            message = (bar.hint, bar.hintWarning ? "caution-text" : "text-subtle")
        } else {
            message = ("", "text-subtle")
        }
        exportStatus.stringValue = message.0
        exportStatus.toolTip = message.0
        exportStatus.textColor = tokens.color(message.1)
        layoutExportBar()
    }

    /// Output option or pixel changes: refresh the summary and re-estimate.
    private func exportInputsChanged(clearsSaved: Bool) {
        if clearsSaved { clearSavedResult() }
        exportError = nil
        refreshExportBar()
        scheduleEstimate()
    }

    /// A different file, folder or encoding is no longer the saved result.
    private func clearSavedResult() {
        lastSavedPath = nil
        exportNotice = nil; exportNoticeToken += 1
        // The footer echoes a failed save; a new name or folder retires it too.
        if let exportError, status.stringValue == exportError {
            status.stringValue = ""; status.textColor = tokens.color("text-muted")
        }
        exportError = nil
        publishExportBar()
    }

    private func toggleExportSettings() {
        exportSettingsOpen.toggle()
        exportChevron.stringValue = exportSettingsOpen ? "▴" : "▾"
        exportDisclosure.toolTip = exportSettingsOpen ? "Hide export settings" : "Show export settings"
        exportDisclosure.setAccessibilityExpanded(exportSettingsOpen)
        layoutEditor()
        changeOutputPreview()
    }

    @objc private func saveAsNewChanged() {
        refreshExportBar(action: ["kind": "set_save_as_new", "enabled": saveAsNewSwitch.state == .on])
        clearSavedResult()
        updateControls()
    }

    private func showExportError(_ message: String) {
        exportError = message
        showError(message)
        publishExportBar()
    }

    private func confirmCopy() {
        copyConfirmed = true
        copyConfirmToken += 1
        let token = copyConfirmToken
        publishExportBar()
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.exportConfirmationDuration) { [weak self] in
            guard let self, self.copyConfirmToken == token else { return }
            self.copyConfirmed = false
            self.publishExportBar()
        }
    }

    private func showExportNotice(_ notice: String) {
        exportNotice = notice
        exportNoticeToken += 1
        let token = exportNoticeToken
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.exportConfirmationDuration) { [weak self] in
            guard let self, self.exportNoticeToken == token else { return }
            self.exportNotice = nil
            self.publishExportBar()
        }
    }

    /// Save exactly as the shared plan says: overwrite the saved original (its
    /// History entry and path are revalidated from disk, so no extra step is
    /// needed) or publish a new file that never replaces anything.
    private func saveExport() {
        guard let artifactID = state.artifactID, let bar = exportBarState else { return }
        if let error = exportOptionsError ?? bar.error { showExportError(error); return }
        guard let plan = bar.plan, let options = outputOptions(),
              let generation = state.beginCommand() else { return }
        let overwrittenID = bar.planOverwrites ? bar.planArtifactID : nil
        let request: [String: Any] = [
            "history_root": historyRoot, "plan": plan, "options": options, "mode": captureMode,
        ]
        exportError = nil; exportNotice = nil; saveInFlight = true
        status.textColor = tokens.color("text-muted")
        status.stringValue = "Saving…"; updateControls(); publishExportBar()
        worker.save(request) { [weak self] result in
            guard let self else { return }
            self.saveInFlight = false
            switch result {
            case .success(let saved):
                guard self.state.completeOutput(generation: generation, artifactID: artifactID) else { return }
                self.lastSavedPath = saved.path
                self.showExportNotice(saved.notice)
                self.status.stringValue = saved.notice
                if let warning = saved.warning {
                    self.reportError("Saved \(saved.path), but couldn’t update History: \(warning)")
                }
                if let savedID = saved.artifactID {
                    // The saved file becomes the original, as in the shipping app.
                    if let size = saved.sizeBytes, size > 0 { self.originalBytes = size }
                    self.refreshExportBar(action: ["kind": "adopt",
                                                   "source": ["artifact_id": savedID, "path": saved.path]])
                    self.scheduleEstimate()
                } else {
                    self.publishExportBar()
                }
                if let overwrittenID {
                    self.didReplaceOriginal(overwrittenID)
                } else if saved.artifactID != nil {
                    self.didSaveCopy()
                }
            case .failure(let error):
                guard self.state.fail(generation: generation) else { return }
                self.showExportError(error.localizedDescription)
            }
            self.updateControls()
            self.submitPendingImportIfReady()
        }
    }

    private func revealSavedFile() {
        guard let lastSavedPath else { return }
        revealFiles([URL(fileURLWithPath: lastSavedPath)])
    }

    /// Re-encode exactly as Save would once edits or options settle (shipping
    /// debounce), off the session queue so it never delays accepted edits.
    private func scheduleEstimate() {
        estimateWork?.cancel()
        estimateGeneration += 1
        guard state.snapshot != nil else { estimatePending = false; return }
        estimatePending = true
        let generation = estimateGeneration
        let work = DispatchWorkItem { [weak self] in self?.runEstimate(generation: generation) }
        estimateWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.estimateDelay, execute: work)
        publishExportBar()
    }

    private func runEstimate(generation: Int) {
        guard generation == estimateGeneration else { return }
        let result = outputOptionsResult()
        guard let options = result.options, result.error == nil, exportBarState?.error == nil else {
            estimate = nil; estimatePending = false
            refreshExportBar()
            return
        }
        var request: [String: Any] = ["options": options]
        request["original_bytes"] = originalBytes.map { $0 as Any } ?? NSNull()
        worker.estimate(request) { [weak self] result in
            guard let self, generation == self.estimateGeneration else { return }
            self.estimatePending = false
            self.estimate = try? result.get()
            self.refreshExportBar()
        }
    }

    private func outputOptions() -> [String: Any]? {
        let result = outputOptionsResult()
        if let error = result.error { showExportError(error) }
        return result.options
    }

    /// Validated export options, or the message that explains the fix. No side effects.
    private func outputOptionsResult() -> (options: [String: Any]?, error: String?) {
        let formats = ["png", "jpeg", "webp"]
        let qualities = ["preserve", "compress", "maximum"]
        guard formats.indices.contains(outputFormat.indexOfSelectedItem),
              qualities.indices.contains(outputQuality.indexOfSelectedItem) else { return (nil, nil) }
        let format = formats[outputFormat.indexOfSelectedItem]
        let quality = qualities[outputQuality.indexOfSelectedItem]
        let qualityValue: UInt64
        if quality == "compress" {
            let minimum: UInt64 = format == "jpeg" ? 40 : 1
            guard let value = outputInteger(outputQualityValue), (minimum...100).contains(value) else {
                return (nil, "Output quality must be a whole number from \(minimum) through 100 for \(format.uppercased()).")
            }
            qualityValue = value
        } else {
            qualityValue = 100
        }
        var png: [String: Any] = [:]
        if format == "png", quality == "compress", !outputPngPalette.stringValue.isEmpty {
            guard let colors = outputInteger(outputPngPalette), (1...256).contains(colors) else {
                return (nil, "PNG palette size must be a whole number from 1 through 256.")
            }
            png["max_colors"] = colors
        }
        var options: [String: Any] = [
            "format": format, "quality": quality, "quality_value": qualityValue, "png": png,
        ]
        switch outputSizeMode.indexOfSelectedItem {
        case 0: options["size"] = ["mode": "original"]
        case 1: options["size"] = ["mode": "percent", "percent": 75]
        case 2: options["size"] = ["mode": "percent", "percent": 50]
        case 3:
            guard let width = outputInteger(outputWidth), let height = outputInteger(outputHeight),
                  (1...16_384).contains(width), (1...16_384).contains(height),
                  width <= 100_000_000 / height else {
                return (nil, "Custom output dimensions must be whole numbers from 1 through 16,384 and no more than 100 million pixels.")
            }
            options["size"] = ["mode": "custom", "width": width, "height": height]
        default: return (nil, nil)
        }
        if quality == "maximum" {
            guard let budget = outputInteger(outputByteBudget), budget >= 10_000 else {
                return (nil, "Enter a maximum file size of at least 10 KB.")
            }
            options["max_size_bytes"] = budget
        }
        return (options, nil)
    }

    /// Drop the encoded preview; Est. size re-encodes on its own schedule.
    private func invalidateOutput() {
        encodedOutput = nil
        outputPreviewMode?.selectedSegment = 0
        preview.image = editedImage
        if let editedImage { viewportCanvasSize = editedImage.size; updateViewportGeometry() }
    }

    private func updateOutputOptionControls() {
        guard outputFormat != nil, outputQuality != nil else { return }
        let ready = state.snapshot != nil && !state.busy
        let compress = outputQuality.indexOfSelectedItem == 1
        let maximum = outputQuality.indexOfSelectedItem == 2
        let pngPalette = compress && outputFormat.indexOfSelectedItem == 0
        outputQualityValue.isEnabled = ready && compress
        outputPngPalette.isEnabled = ready && pngPalette
        outputByteBudget.isEnabled = ready && maximum
        outputCompressionPreset.isEnabled = ready && compress
        outputSizeMode?.isEnabled = ready
        let custom = outputSizeMode?.indexOfSelectedItem == 3
        outputWidth.isEnabled = ready && custom; outputHeight.isEnabled = ready && custom
        outputAspectLock.isEnabled = ready && custom
        // Visibility follows the shared group flow inside the settings panel.
        layoutExportSettings()
    }

    private func publishOutputDimensions() {
        guard let snapshot = state.snapshot else { outputDimensions.stringValue = ""; return }
        let width: UInt64, height: UInt64
        switch outputSizeMode?.indexOfSelectedItem ?? 0 {
        case 1, 2:
            let percent = outputSizeMode.indexOfSelectedItem == 1 ? 75.0 : 50.0
            width = max(1, UInt64((snapshot.width * percent / 100).rounded()))
            height = max(1, UInt64((Double(width) * snapshot.height / snapshot.width).rounded()))
        case 3:
            guard let customWidth = outputInteger(outputWidth), let customHeight = outputInteger(outputHeight),
                  (1...16_384).contains(customWidth), (1...16_384).contains(customHeight),
                  customWidth <= 100_000_000 / customHeight else {
                outputDimensions.stringValue = "Invalid size"
                return
            }
            width = customWidth; height = customHeight
        default:
            width = UInt64(snapshot.width.rounded()); height = UInt64(snapshot.height.rounded())
        }
        outputDimensions.stringValue = "\(formatInteger(Int(width))) × \(formatInteger(Int(height)))"
    }

    private static let outputCompressionPresets: [(name: String, value: UInt64)] = [
        ("Tiny", 55), ("Smaller", 70), ("Balanced", 85), ("High", 92), ("Highest", 98),
    ]

    private func synchronizeOutputCompressionPreset() {
        guard outputCompressionPreset.superview != nil else { return }
        let value = outputInteger(outputQualityValue)
        let preset = (outputFormat.indexOfSelectedItem != 0 || outputPngPalette.stringValue.isEmpty)
            ? Self.outputCompressionPresets.first(where: { $0.value == value }) : nil
        if let preset {
            if outputCompressionPreset.item(withTitle: "Custom") != nil {
                outputCompressionPreset.removeItem(withTitle: "Custom")
            }
            outputCompressionPreset.selectItem(withTitle: preset.name)
        } else {
            if outputCompressionPreset.item(withTitle: "Custom") == nil {
                outputCompressionPreset.insertItem(withTitle: "Custom", at: 0)
            }
            outputCompressionPreset.selectItem(withTitle: "Custom")
        }
    }

    private func normalizeOutputQuality() {
        guard outputQuality.indexOfSelectedItem == 1,
              let current = outputInteger(outputQualityValue) else { return }
        let minimum: UInt64 = outputFormat.indexOfSelectedItem == 1 ? 40 : 1
        outputQualityValue.stringValue = String(min(100, max(minimum, current)))
    }

    private var selectedLayer: NativeEditorLayer? {
        guard let id = selectedLayerID else { return nil }
        return state.snapshot?.layers.first { $0.id == id }
    }

    private func layerCommand(_ layer: NativeEditorLayer, edit: [String: Any], message: String,
                              preferredSelection: String? = nil) {
        command(["operation": "layer", "id": layer.id, "edit": edit], message: message,
                preferredSelection: preferredSelection)
    }

    private func createDrawing(shape: EditorDrawOverlay.Shape, start: NSPoint, end: NSPoint, points: [NSPoint]) {
        guard shape != .wand, let layers = state.snapshot?.layers else { return }
        if shape == .text {
            beginTextInput(at: start)
            return
        }
        guard let request = drawingRequest(shape: shape, start: start, end: end, points: points,
                                           reportErrors: true) else { return }
        command(request, message: "Drawing \(shape.rawValue)…", createdLayerExistingIDs: Set(layers.map(\.id)))
    }

    private func drawingRequest(shape: EditorDrawOverlay.Shape, start: NSPoint, end: NSPoint,
                                points: [NSPoint], reportErrors: Bool) -> [String: Any]? {
        if shape.isBackgroundBrush {
            return backgroundBrushRequest(mode: shape, points: points, reportErrors: reportErrors)
        }
        guard let style = drawingRequestStyle(shape: shape, reportErrors: reportErrors) else { return nil }
        guard let opacity = number(drawingOpacity), (0...100).contains(opacity) else {
            if reportErrors { showError("Drawing opacity must be between 0 and 100.") }
            return nil
        }
        if shape == .pen {
            return ["operation": "create_freehand_path", "points": points.map { ["x": $0.x, "y": $0.y] },
                    "style": style, "opacity": opacity]
        }
        return ["operation": shape == .line || shape == .arrow ? "create_open_shape" : "create_closed_shape",
                "shape": shape.rawValue, "start": ["x": start.x, "y": start.y],
                "end": ["x": end.x, "y": end.y], "style": style, "opacity": opacity]
    }

    private func cancelDrawingPreview() {
        drawingPreviewEpoch += 1
        drawingPreviewPending = nil
        if drawOverlay.pixelPreviewVisible { preview.image = editedImage }
        drawOverlay.pixelPreviewVisible = false
    }

    private func drainDrawingPreview() {
        guard !drawingPreviewInFlight, !state.busy, state.snapshot != nil,
              let request = drawingPreviewPending else { return }
        drawingPreviewPending = nil; drawingPreviewInFlight = true
        let epoch = drawingPreviewEpoch, generation = state.generation
        worker.previewDrawing(request) { [weak self] result in
            guard let self else { return }
            self.drawingPreviewInFlight = false
            if self.drawingPreviewEpoch == epoch, self.state.generation == generation,
               !self.state.busy, self.drawOverlay.startPoint != nil {
                switch result {
                case .success(let image):
                    self.preview.image = NSImage(cgImage: image, size: NSSize(width: image.width, height: image.height))
                    self.drawOverlay.pixelPreviewVisible = true
                case .failure:
                    if self.drawOverlay.pixelPreviewVisible { self.preview.image = self.editedImage }
                    self.drawOverlay.pixelPreviewVisible = false
                }
            }
            self.drainDrawingPreview()
        }
    }

    private func drawingRequestStyle(shape: EditorDrawOverlay.Shape, reportErrors: Bool) -> [String: Any]? {
        guard let color = PreferencesController.normalizeHex(drawingStrokeColor.stringValue),
              let width = number(drawingStrokeWidth), (2...40).contains(width) else {
            if reportErrors { showError("Enter drawing colors as #RGB or #RRGGBB and stroke width from 2 to 40.") }
            return nil
        }
        let closed = [.rectangle, .ellipse, .triangle, .diamond, .star].contains(shape)
        var fill: Any = NSNull()
        if closed && drawingFill.state == .on {
            guard let color = PreferencesController.normalizeHex(drawingFillColor.stringValue) else {
                if reportErrors { showError("Enter drawing fill color as #RGB or #RRGGBB.") }
                return nil
            }
            fill = color
        }
        updateDrawingPreviewStyle()
        var style: [String: Any] = ["color": color, "fill": fill,
            "strokeWidth": width, "strokeEnabled": drawingStroke.state == .on,
            "dropShadow": drawingDropShadow.state == .on]
        if drawingShadowCustomized {
            var shadow: [String: Any] = [:]
            var valid = true
            if let color = drawingShadowFields["color"].flatMap({ PreferencesController.normalizeHex($0.stringValue) }) {
                shadow["color"] = color
            } else { valid = false }
            for (key, _) in textShadowNumbers {
                let range: ClosedRange<Double> = key.hasPrefix("offset") ? -500...500 : 0...100
                if let field = drawingShadowFields[key], let value = number(field), range.contains(value) {
                    shadow[key] = value
                } else { valid = false }
            }
            if valid { style["dropShadowStyle"] = shadow }
            else if drawingDropShadow.state == .on {
                if reportErrors { showError("Enter a shadow color, opacity/blur from 0 to 100, and offsets from −500 to 500.") }
                return nil
            }
        }
        return style
    }

    private func beginTextInput(at point: NSPoint) {
        if beginExistingTextInput(at: point, tolerance: 0) { return }
        guard let size = number(createTextSize), (8...512).contains(size) else {
            showError("Text size must be from 8 to 512."); return
        }
        guard !createTextColor.stringValue.isEmpty else {
            showError("Enter a text color."); return
        }
        let families = state.snapshot?.fontFamilies ?? [:]
        let family = families["sans"] != nil ? "sans" : families.keys.sorted().first ?? "sans"
        var create: [String: Any] = [
            "point": ["x": point.x, "y": point.y], "text": "",
            "fontSize": size, "fontFamily": family, "color": createTextColor.stringValue,
        ]
        if let preset = createTextPreset.selectedItem?.representedObject as? String {
            create["stylePreset"] = preset
        }
        beginTextInput(target: ["kind": "new", "create": create], initialText: "",
                       anchor: point, fontSize: size)
    }

    @discardableResult
    private func beginExistingTextInput(at point: NSPoint, tolerance: Double) -> Bool {
        guard inlineTextInput == nil, !state.busy,
              let snapshot = state.snapshot else { return false }
        do {
            guard let id = try NativeEditorHitTesting.hit(documentJSON: snapshot.documentJSON,
                                                           point: point, tolerance: tolerance),
                  let layer = snapshot.layers.first(where: { $0.id == id }),
                  layer.kind == .text, layer.visible, !layer.locked,
                  let text = layer.textStyle?.text else { return false }
            beginTextInput(target: ["kind": "existing", "id": id], initialText: text,
                           anchor: NSPoint(x: CGFloat(layer.x), y: CGFloat(layer.y)),
                           fontSize: layer.textStyle?.fontSize ?? 32)
            return true
        } catch {
            showError("Text interaction failed: \(error.localizedDescription)")
            return true
        }
    }

    private func beginTextInput(target: [String: Any], initialText: String,
                                anchor: NSPoint, fontSize: Double) {
        guard !hasStagedText else {
            showError("Apply or cancel staged inspector changes before editing text inline.")
            return
        }
        guard inlineTextInput == nil else { return }
        inlineTextInput = InlineTextInput(inputID: UUID().uuidString.lowercased(), layerID: nil,
            isNew: target["kind"] as? String == "new", anchor: anchor, fontSize: fontSize,
            beginTarget: target, acceptedText: initialText, bufferedText: initialText)
        inlineTextEditor.string = initialText
        showInlineTextEditor(selectAtEnd: true)
        sendBeginTextInput()
    }

    private func sendBeginTextInput() {
        guard var input = inlineTextInput, !input.requestInFlight,
              let target = input.beginTarget,
              let generation = state.beginCommand() else { return }
        input.inputID = UUID().uuidString.lowercased()
        input.requestInFlight = true
        inlineTextInput = input
        status.textColor = tokens.color("text-muted")
        status.stringValue = "Starting inline text input…"
        updateControls()
        worker.request(["operation": "begin_text_input", "input_id": input.inputID, "target": target]) {
            [weak self] result in
            guard let self, var current = self.inlineTextInput,
                  current.inputID == input.inputID else { return }
            current.requestInFlight = false
            switch result {
            case .success(let presentation):
                guard let active = presentation.snapshot.activeTextInput,
                      active.inputID == input.inputID,
                      self.state.complete(presentation.snapshot, generation: generation) else {
                    _ = self.state.fail(generation: generation)
                    current.finishRequested = nil
                    self.inlineTextInput = current
                    self.showError("Couldn’t start inline text input: invalid shared response. Retry or Cancel.")
                    self.updateControls()
                    return
                }
                let accepted = presentation.snapshot.layers
                    .first(where: { $0.id == active.layerID })?.textStyle?.text ?? current.acceptedText
                current.layerID = active.layerID
                current.beginTarget = nil
                current.acceptedText = accepted
                self.inlineTextInput = current
                self.selectedLayerID = active.layerID
                self.publishTextInputPresentation(presentation)
                self.showInlineTextEditor()
                self.status.stringValue = "Editing text inline. Return inserts a line; Escape or click away finishes."
                if current.bufferedText != current.acceptedText {
                    self.sendBufferedTextUpdateIfNeeded()
                } else if current.finishRequested != nil {
                    self.sendFinishTextInput()
                }
            case .failure(let error):
                guard self.state.fail(generation: generation) else { return }
                if current.finishRequested == false {
                    self.dismissPendingTextInput()
                    return
                }
                current.finishRequested = nil
                self.inlineTextInput = current
                self.showError("Couldn’t start inline text input: \(error.localizedDescription). Retry or Cancel.")
                self.showInlineTextEditor()
            }
            self.updateControls()
        }
    }

    func textDidChange(_ notification: Notification) {
        guard notification.object as? NSTextView === inlineTextEditor,
              var input = inlineTextInput, !input.finishInFlight else { return }
        input.bufferedText = inlineTextEditor.string
        inlineTextInput = input
        sendBufferedTextUpdateIfNeeded()
    }

    private func sendBufferedTextUpdateIfNeeded() {
        guard var input = inlineTextInput, !input.requestInFlight,
              input.layerID != nil, input.beginTarget == nil,
              input.bufferedText != input.acceptedText,
              let generation = state.beginCommand() else {
            if let input = inlineTextInput, !input.requestInFlight,
               input.bufferedText == input.acceptedText, input.finishRequested != nil {
                sendFinishTextInput()
            }
            return
        }
        let sentText = input.bufferedText
        input.requestInFlight = true
        inlineTextInput = input
        updateControls()
        worker.request(["operation": "update_text_input", "input_id": input.inputID,
                        "text": sentText]) { [weak self] result in
            guard let self, var current = self.inlineTextInput,
                  current.inputID == input.inputID else { return }
            current.requestInFlight = false
            switch result {
            case .success(let presentation):
                guard presentation.snapshot.activeTextInput?.inputID == input.inputID else {
                    guard self.state.fail(generation: generation) else { return }
                    current.finishRequested = nil
                    self.inlineTextInput = current
                    self.showError("Inline text preview returned a stale token. Retry or Cancel.")
                    self.showInlineTextEditor(); self.updateControls()
                    return
                }
                guard self.state.complete(presentation.snapshot, generation: generation) else { return }
                current.acceptedText = sentText
                if let active = presentation.snapshot.activeTextInput { current.layerID = active.layerID }
                self.inlineTextInput = current
                self.publishTextInputPresentation(presentation)
                if current.bufferedText != current.acceptedText {
                    self.sendBufferedTextUpdateIfNeeded()
                } else if current.finishRequested != nil {
                    self.sendFinishTextInput()
                }
            case .failure(let error):
                guard self.state.fail(generation: generation) else { return }
                if current.finishRequested == true { current.finishRequested = nil }
                self.inlineTextInput = current
                self.showError("Inline text preview failed: \(error.localizedDescription). Retry or Cancel.")
                if current.finishRequested == false { self.sendFinishTextInput() }
                else { self.window.makeFirstResponder(self.inlineTextEditor) }
            }
            self.updateControls()
        }
    }

    private func finishInlineTextInput(commit: Bool) {
        guard var input = inlineTextInput, !input.finishInFlight else { return }
        if input.finishRequested != nil {
            guard !commit else { return }
        }
        input.finishRequested = commit
        if !commit { input.bufferedText = input.acceptedText }
        inlineTextInput = input
        if input.requestInFlight { return }
        if input.layerID == nil {
            if commit { sendBeginTextInput() }
            else { dismissPendingTextInput() }
            return
        }
        if commit && input.bufferedText != input.acceptedText {
            sendBufferedTextUpdateIfNeeded()
        } else {
            sendFinishTextInput()
        }
    }

    private func sendFinishTextInput() {
        guard var input = inlineTextInput, !input.requestInFlight,
              input.layerID != nil,
              let commit = input.finishRequested,
              let generation = state.beginCommand() else { return }
        input.requestInFlight = true
        input.finishInFlight = true
        inlineTextInput = input
        status.stringValue = commit ? "Finishing text…" : "Cancelling text…"
        updateControls()
        worker.request(["operation": "finish_text_input", "input_id": input.inputID,
                        "commit": commit]) { [weak self] result in
            guard let self, var current = self.inlineTextInput,
                  current.inputID == input.inputID else { return }
            current.requestInFlight = false
            current.finishInFlight = false
            switch result {
            case .success(let presentation):
                guard presentation.snapshot.activeTextInput == nil else {
                    guard self.state.fail(generation: generation) else { return }
                    current.finishRequested = nil
                    self.inlineTextInput = current
                    self.showError("Inline text finish returned an active token. Retry or Cancel.")
                    self.showInlineTextEditor(); self.updateControls()
                    return
                }
                guard self.state.complete(presentation.snapshot, generation: generation) else { return }
                self.inlineTextInput = nil
                self.hideInlineTextEditor()
                self.publishTextInputPresentation(presentation)
                if commit { self.invalidateOutput() }
                self.status.textColor = self.tokens.color("text-muted")
                self.status.stringValue = commit
                    ? (presentation.snapshot.unsavedChanges ? "Unsaved changes." : "Text finished.")
                    : "Text input cancelled."
                self.updateControls()
                if self.closeAfterTextInput {
                    self.closeAfterTextInput = false
                    _ = self.windowShouldClose(self.window)
                }
            case .failure(let error):
                guard self.state.fail(generation: generation) else { return }
                current.finishRequested = nil
                self.inlineTextInput = current
                self.showError("Couldn’t finish inline text: \(error.localizedDescription). Retry or Cancel.")
                self.showInlineTextEditor()
                self.updateControls()
            }
        }
    }

    private func dismissPendingTextInput() {
        inlineTextInput = nil
        hideInlineTextEditor()
        status.textColor = tokens.color("text-muted")
        status.stringValue = "Text input cancelled."
        closeAfterTextInput = false
        updateControls()
    }

    private func textFieldsMatch(_ style: NativeTextStyle) -> Bool {
        let shadowMatches = textShadow.state != .on || (style.shadowStyle.map { shadow in
            textShadowFields["color"]?.stringValue == shadow.color && textShadowNumbers.allSatisfy {
                textShadowFields[$0.0]?.stringValue == format(shadow[keyPath: $0.1])
            }
        } ?? true)
        return shadowMatches && textEditor.string == style.text && textSize.stringValue == format(style.fontSize)
            && (textFamily.selectedItem?.representedObject as? String) == style.fontFamily
            && textColor.stringValue == style.color
            && textTraits.isSelected(forSegment: 0) == style.bold
            && textTraits.isSelected(forSegment: 1) == style.italic
            && textAlignment.selectedSegment == ["left", "center", "right"].firstIndex(of: style.align)
            && textPlate.indexOfSelectedItem == (style.background == nil ? 0 : style.roundedBackground ? 2 : 1)
            && (style.background == nil || textPlateColor.stringValue == style.background)
            && (textPlate.indexOfSelectedItem != 0 || (textPresetRounded ?? style.roundedBackground) == style.roundedBackground)
            && (textShadow.state == .on) == style.dropShadow
            && (textOutline.state == .on) == style.outlined
    }

    @objc private func textShadowChanged() { updateTextShadowControls() }

    @objc private func stageTextPreset() {
        let index = textPreset.indexOfSelectedItem - 1
        guard let presets = state.snapshot?.textStylePresets,
              presets.indices.contains(index), !state.busy else { return }
        let preset = presets[index]
        guard let family = textFamily.itemArray.firstIndex(where: {
            $0.representedObject as? String == preset.fontFamily
        }) else { return }
        textFamily.selectItem(at: family)
        if let color = preset.background {
            if textPlate.indexOfSelectedItem == 0 { textPlateColor.stringValue = color }
            textPlate.selectItem(at: preset.roundedBackground ? 2 : 1)
        } else { textPlate.selectItem(at: 0) }
        textOutline.state = preset.outlined ? .on : .off
        textPresetRounded = preset.roundedBackground
        if let choice = createTextPreset.itemArray.firstIndex(where: {
            $0.representedObject as? String == preset.id
        }) {
            createTextPreset.selectItem(at: choice)
        }
        textPreset.selectItem(at: 0)
    }

    private func updateTextShadowControls() {
        let expanded = selectedLayer?.kind == .text && textShadow.state == .on
            && acceptedTextStyle?.shadowStyle != nil
        textShadowPanel.isHidden = !expanded
        textShadowPanel.frame.size.height = expanded ? 190 : 0
        textApplyButton.frame.origin.y = textShadowPanel.frame.maxY
        textCancelButton.frame.origin.y = textShadowPanel.frame.maxY
        if selectedLayer?.kind == .text {
            textShadowPanel.superview?.frame.size.height = textCancelButton.frame.maxY + 8
        }
    }

    private func publishTextFields(preserveStaged: Bool = false) {
        guard let style = selectedLayer?.textStyle else {
            textControls.forEach { $0.isHidden = true }
            textFieldsID = nil; acceptedTextStyle = nil
            return
        }
        textControls.forEach { $0.isHidden = false }
        let preserve = preserveStaged && !textApplyPending && textFieldsID == selectedLayer?.id
            && acceptedTextStyle.map { !textFieldsMatch($0) } == true
        textFieldsID = selectedLayer?.id; acceptedTextStyle = style
        textPreset.removeAllItems()
        textPreset.addItem(withTitle: "Style…")
        for preset in state.snapshot?.textStylePresets ?? [] { textPreset.addItem(withTitle: preset.label) }
        if preserve { return }
        textPresetRounded = nil
        textEditor.string = style.text; textSize.stringValue = format(style.fontSize)
        textColor.stringValue = style.color; textPlateColor.stringValue = style.background ?? "#ffffff"
        textTraits.setSelected(style.bold, forSegment: 0)
        textTraits.setSelected(style.italic, forSegment: 1)
        textAlignment.selectedSegment = ["left", "center", "right"].firstIndex(of: style.align) ?? 0
        textPlate.selectItem(at: style.background == nil ? 0 : style.roundedBackground ? 2 : 1)
        textShadow.state = style.dropShadow ? .on : .off
        textOutline.state = style.outlined ? .on : .off
        textShadowFields["color"]?.stringValue = style.shadowStyle?.color ?? ""
        for (key, path) in textShadowNumbers {
            textShadowFields[key]?.stringValue = style.shadowStyle.map { format($0[keyPath: path]) } ?? ""
        }
        updateTextShadowControls()
        textFamily.removeAllItems()
        let families = state.snapshot?.fontFamilies ?? [:]
        for key in families.keys.sorted() {
            textFamily.addItem(withTitle: families[key]!)
            textFamily.lastItem?.representedObject = key
        }
        if families[style.fontFamily] == nil {
            textFamily.addItem(withTitle: "Saved font: \(style.fontFamily)")
            textFamily.lastItem?.representedObject = style.fontFamily
        }
        textFamily.selectItem(at: textFamily.itemArray.firstIndex {
            $0.representedObject as? String == style.fontFamily
        } ?? 0)
    }

    private func applyTextEdits() {
        guard let layer = selectedLayer, let style = layer.textStyle, !state.busy else { return }
        guard let size = Double(textSize.stringValue), size.isFinite,
              size == style.fontSize || (8...512).contains(size) else {
            showError("Text size must be from 8 to 512."); return
        }
        guard !textColor.stringValue.isEmpty else { showError("Enter a text color."); return }
        var patch: [String: Any] = [:]
        if textEditor.string != style.text { patch["text"] = textEditor.string }
        if size != style.fontSize { patch["fontSize"] = size }
        if let family = textFamily.selectedItem?.representedObject as? String, family != style.fontFamily {
            patch["fontFamily"] = family
        }
        if textTraits.isSelected(forSegment: 0) != style.bold { patch["bold"] = !style.bold }
        if textTraits.isSelected(forSegment: 1) != style.italic { patch["italic"] = !style.italic }
        let align = ["left", "center", "right"][max(0, textAlignment.selectedSegment)]
        if align != style.align { patch["align"] = align }
        if textColor.stringValue != style.color { patch["color"] = textColor.stringValue }
        let background = textPlate.indexOfSelectedItem == 0 ? nil : textPlateColor.stringValue
        if background != nil {
            guard !textPlateColor.stringValue.isEmpty else { showError("Enter a plate color."); return }
        }
        if background != style.background {
            if let background { patch["background"] = background }
            else { patch["background"] = NSNull() }
        }
        let rounded = background == nil ? (textPresetRounded ?? style.roundedBackground) : textPlate.indexOfSelectedItem == 2
        if rounded != style.roundedBackground { patch["roundedBackground"] = rounded }
        if (textShadow.state == .on) != style.dropShadow { patch["dropShadow"] = textShadow.state == .on }
        if (textOutline.state == .on) != style.outlined { patch["outlined"] = textOutline.state == .on }
        if textShadow.state == .on, let shadow = style.shadowStyle {
            var shadowPatch: [String: Any] = [:]
            if let color = textShadowFields["color"]?.stringValue, color != shadow.color {
                guard let value = PreferencesController.normalizeHex(color) else {
                    showError("Enter shadow color as #RGB or #RRGGBB."); return
                }
                shadowPatch["color"] = value
            }
            for (key, path) in textShadowNumbers {
                guard let field = textShadowFields[key] else { continue }
                // Formatting unchanged display values must not round authored precision.
                if field.stringValue != format(shadow[keyPath: path]) {
                    guard let value = number(field) else {
                        showError("Enter a finite shadow \(key) value."); return
                    }
                    shadowPatch[key] = value
                }
            }
            if !shadowPatch.isEmpty { patch["dropShadowStyle"] = shadowPatch }
        }
        guard !patch.isEmpty else { return }
        textApplyPending = true
        command(["operation": "edit_text", "id": layer.id, "patch": patch],
                message: "Applying text…", preferredSelection: layer.id,
                preserveStagedTextOnFailure: true)
    }

    private func removeImageBackground(at point: NSPoint) {
        guard !state.busy else { return }
        guard let value = Double(wandTolerance.stringValue),
              value >= 0, value <= 255, value == value.rounded() else {
            showError("Wand tolerance must be a whole number from 0 to 255."); return
        }
        let tolerance = value.rounded()
        wandTolerance.stringValue = String(Int(tolerance))
        command(["operation": "remove_image_background",
                 "point": ["x": point.x, "y": point.y],
                 "tolerance": tolerance, "contiguous": wandContiguous.state == .on],
                message: "Removing image background…")
    }

    private func paintImageBackground(mode: EditorDrawOverlay.Shape, points: [NSPoint]) {
        guard mode.isBackgroundBrush, !state.busy else { return }
        guard let request = backgroundBrushRequest(mode: mode, points: points, reportErrors: true) else { return }
        command(request, message: mode == .erase ? "Erasing image background…" : "Restoring image background…")
    }

    private func backgroundBrushRequest(mode: EditorDrawOverlay.Shape, points: [NSPoint],
                                        reportErrors: Bool) -> [String: Any]? {
        guard let size = Double(brushSize.stringValue), size >= 4, size <= 120,
              size == size.rounded() else {
            if reportErrors { showError("Brush diameter must be a whole number from 4 to 120.") }
            return nil
        }
        guard let softness = Double(brushSoftness.stringValue), softness >= 0, softness <= 100,
              softness == softness.rounded() else {
            if reportErrors { showError("Brush softness must be a whole number from 0 to 100.") }
            return nil
        }
        if reportErrors {
            brushSize.stringValue = String(Int(size)); brushSoftness.stringValue = String(Int(softness))
        }
        drawOverlay.brushDiameter = CGFloat(size)
        return ["operation": "paint_image_background", "points": points.map { ["x": $0.x, "y": $0.y] },
                "size": size, "softness": softness, "mode": mode.rawValue]
    }

    private func cancelDrawing() {
        cropOverlay.cancelGesture()
        drawOverlay.cancelGesture()
        selectionOverlay.cancelGesture()
    }

    private var fittedImageRect: NSRect {
        guard viewportCanvasSize.width > 0, viewportCanvasSize.height > 0 else { return .zero }
        // Match Tauri's 2–100% Fit range; manual zoom has its own 5–800% range.
        let scale = min(1, max(0.02, viewportBounds.width / viewportCanvasSize.width),
                        max(0.02, viewportBounds.height / viewportCanvasSize.height))
        let size = NSSize(width: viewportCanvasSize.width * scale,
                          height: viewportCanvasSize.height * scale)
        return NSRect(x: (viewportBounds.width - size.width) / 2,
                      y: (viewportBounds.height - size.height) / 2,
                      width: size.width, height: size.height)
    }

    var presentedImageRect: NSRect {
        viewport.rect(fit: fittedImageRect, canvas: viewportCanvasSize) ?? fittedImageRect
    }

    private func configureViewportGestures(_ view: EditorViewportGestureView) {
        view.onViewportZoom = { [weak self] factor, anchor in self?.scaleViewport(by: factor, anchor: anchor) }
        view.onViewportPan = { [weak self] delta in self?.panViewport(by: delta) }
        view.onViewportPanBegan = { [weak self] in self?.cancelDrawing() }
    }

    private func handleEditorShortcut(_ event: NSEvent) -> Bool {
        guard event.type == .keyDown else { return false }
        if event.keyCode == 53, cropPrevious != nil { cancelCrop(); return true }
        guard state.snapshot != nil else { return false }
        let command = event.modifierFlags.contains(.command) || event.modifierFlags.contains(.control)
        let key = event.charactersIgnoringModifiers ?? ""
        if command && (key == "+" || key == "=" || event.keyCode == 24 || event.keyCode == 69) {
            scaleViewport(by: 1.25)
        } else if command && (key == "-" || key == "_" || event.keyCode == 27 || event.keyCode == 78) {
            scaleViewport(by: 1 / 1.25)
        } else if command && (key == "0" || event.keyCode == 82) {
            setViewportZoom(100)
        } else {
            // Field editors retain typing and deletion. Document shortcuts use
            // the same accepted-command and enabled-state gates as the buttons.
            let responder = window.firstResponder
            guard !(responder is NSTextView), !(responder is NSTextField),
                  !(responder is NSPopUpButton), !(responder is NSComboBox),
                  !(responder is NSSlider) else { return false }
            if !command && !event.modifierFlags.contains(.option)
                && activateToolShortcut(key.lowercased()) { return true }
            let button: CaptureButton?
            if command && key.lowercased() == "z" {
                // Undo/Redo buttons hide at or below 1040pt, like shipping; the keys still work.
                cancelDrawing(); cancelViewportPan()
                if event.modifierFlags.contains(.shift) { redoDocument() } else { undoDocument() }
                return true
            } else if command && key.lowercased() == "d" {
                button = duplicateButton
            } else if command && key.lowercased() == "c" {
                copyLayer(id: selectedLayerID)
                return true
            } else if command && key.lowercased() == "v" {
                pasteLayer(after: selectedLayerID)
                return true
            } else if event.keyCode == 51 || event.keyCode == 117 {
                button = deleteButton
            } else if (123...126).contains(event.keyCode) {
                // Native tables and segmented selectors keep arrow navigation.
                if responder is NSControl && !(responder is CaptureButton) { return false }
                guard !state.busy, let layer = selectedLayer, !layer.locked else { return true }
                let distance = event.modifierFlags.contains(.shift) ? 10.0 : 1.0
                let delta: (Double, Double)
                switch event.keyCode {
                case 123: delta = (-distance, 0)
                case 124: delta = (distance, 0)
                case 125: delta = (0, distance)
                default: delta = (0, -distance)
                }
                cancelDrawing(); cancelViewportPan()
                layerCommand(layer, edit: ["action": "translate", "delta_x": delta.0,
                                           "delta_y": delta.1], message: "Moving layer…")
                return true
            } else { return false }
            if button?.isEnabled == true {
                cancelDrawing(); cancelViewportPan()
                button?.performClick(nil)
            }
        }
        return true
    }

    private func activateToolShortcut(_ key: String) -> Bool {
        let tool: (section: Int, shape: EditorDrawOverlay.Shape?)
        switch key {
        case "v": tool = (Section.layers, nil)
        case "c": tool = (Section.geometry, nil)
        case "t": tool = (Section.draw, .text)
        case "r": tool = (Section.draw, .rectangle)
        case "o": tool = (Section.draw, .ellipse)
        case "l": tool = (Section.draw, .line)
        case "d": tool = (Section.draw, .diamond)
        case "s": tool = (Section.draw, .star)
        case "a": tool = (Section.draw, .arrow)
        case "p": tool = (Section.draw, .pen)
        case "b": tool = (Section.draw, lastBackgroundTool)
        default: return false
        }
        // Native controls retain letter navigation; these keys belong to the canvas.
        guard !(window.firstResponder is NSControl) else { return false }
        guard !state.busy, !importLoading else { return true }
        activateTool(section: tool.section, shape: tool.shape)
        return true
    }

    private func activateTool(section: Int, shape: EditorDrawOverlay.Shape?) {
        if sectionControl.selectedSegment == section {
            if let shape, drawOverlay.shape == shape { return }
            if section == Section.layers || (section == Section.geometry && cropPrevious != nil) {
                return
            }
        }
        cancelDrawing(); cancelViewportPan()
        if sectionControl.selectedSegment != section {
            sectionControl.selectedSegment = section
            changeSection()
        }
        if let shape {
            drawTool.selectItem(at: EditorDrawOverlay.Shape.allCases.firstIndex(of: shape)!)
            changeDrawTool()
        } else if section == Section.geometry {
            toggleCrop()
        }
    }

    @objc private func changeZoomPreset() {
        guard let value = zoomPreset.selectedItem?.representedObject as? Double else { return }
        if value == 0 { fitViewport() } else { setViewportZoom(value) }
    }

    @objc private func changeZoomSlider() {
        guard let percent = NativeEditorViewport.sliderZoom(position: zoomSlider.doubleValue) else { return }
        setViewportZoom(percent)
    }

    private func publishZoomPreset() {
        let current = viewport.zoomPercent
        let displayed = current == 0
            ? fittedImageRect.width / max(1, viewportCanvasSize.width) * 100 : current
        zoomSlider.doubleValue = NativeEditorViewport.sliderPosition(percent: displayed) ?? 0
        let description = current == 0 ? "Fit (\(format(displayed))%)" : "\(format(current))%"
        zoomSlider.setAccessibilityValueDescription(description)
        zoomSlider.toolTip = "Canvas zoom: \(description). Drag to zoom from 5% to 800%."
        var values: [Double] = [0, 50, 100, 200]
        if !values.contains(current) { values.insert(current, at: 1) }
        if zoomPreset.itemArray.compactMap({ $0.representedObject as? Double }) != values {
            zoomPreset.removeAllItems()
            for value in values {
                zoomPreset.addItem(withTitle: value == 0 ? "Fit" : "\(format(value))%")
                zoomPreset.lastItem?.representedObject = value
            }
        }
        zoomPreset.selectItem(at: values.firstIndex(of: current) ?? 0)
        fitButton?.selected = current == 0; fitButton?.needsDisplay = true
        publishZoomLimits(ready: state.snapshot != nil && !state.busy && inlineTextInput == nil)
    }

    /// Shipping disables − and + at the 5% and 800% bounds.
    private func publishZoomLimits(ready: Bool) {
        let displayed = viewport.zoomPercent == 0
            ? fittedImageRect.width / max(1, viewportCanvasSize.width) * 100 : viewport.zoomPercent
        zoomOutButton?.isEnabled = ready && displayed > 5 + 0.05
        zoomInButton?.isEnabled = ready && displayed < 800 - 0.05
    }

    private func fitViewport() { cancelViewportPan(); changeViewport(to: NativeEditorViewport()) }
    private func recenterViewport() {
        cancelViewportPan()
        var next = viewport; next.panX = 0; next.panY = 0; changeViewport(to: next)
    }
    private func setViewportZoom(_ percent: Double) {
        zoomViewport(to: percent, anchor: NSPoint(x: viewportBounds.width / 2, y: viewportBounds.height / 2))
    }
    private func scaleViewport(by factor: Double, anchor: NSPoint? = nil) {
        guard factor.isFinite, factor > 0 else { return }
        let current = viewport.zoomPercent == 0
            ? fittedImageRect.width / max(1, viewportCanvasSize.width) * 100 : viewport.zoomPercent
        zoomViewport(to: current * factor,
                     anchor: anchor ?? NSPoint(x: viewportBounds.width / 2, y: viewportBounds.height / 2))
    }
    private func zoomViewport(to percent: Double, anchor: NSPoint) {
        guard let next = viewport.zoomed(to: percent, anchor: anchor,
            fit: fittedImageRect, canvas: viewportCanvasSize) else { return }
        cancelViewportPan()
        changeViewport(to: next)
    }
    private func panViewport(by delta: NSPoint) {
        guard delta.x.isFinite, delta.y.isFinite else { return }
        var next = viewport; next.panX += delta.x; next.panY += delta.y; changeViewport(to: next)
    }
    private func changeViewport(to next: NativeEditorViewport) {
        cancelDrawing(); viewport = next; updateViewportGeometry()
    }
    private func cancelViewportPan() {
        viewportInput.cancelViewportPan(); drawOverlay.cancelViewportPan(); selectionOverlay.cancelViewportPan()
        cropOverlay.cancelViewportPan()
    }
    private func updateViewportGeometry() {
        preview.frame = presentedImageRect
        recenterButton?.isHidden = editedImage == nil
            || !EditorChrome.canvasOffscreen(viewport: viewportInput.bounds, canvas: presentedImageRect)
        updateInlineTextFrame()
        publishZoomPreset()
        drawOverlay.needsDisplay = true; selectionOverlay.needsDisplay = true
        cropOverlay.needsDisplay = true
    }

    private func publishTextInputPresentation(_ presentation: EditorPresentation) {
        outputPreviewMode?.selectedSegment = 0
        publish(presentation, resetCrop: false)
    }

    private func showInlineTextEditor(selectAtEnd: Bool = false) {
        guard let input = inlineTextInput else { return }
        let layer = input.layerID.flatMap { id in state.snapshot?.layers.first(where: { $0.id == id }) }
        let style = layer?.textStyle
        let scale = presentedImageRect.width / max(1, CGFloat(state.snapshot?.width ?? 1))
        // Pinned font bytes live inside the shared Rust session and are not an
        // AppKit bundle resource. The native responder intentionally uses the
        // system editing face for caret/IME ownership; shared preview/final pixels
        // remain authoritative for family, traits, shaping and glyph coverage.
        inlineTextEditor.font = .systemFont(ofSize: min(96, max(13, CGFloat(style?.fontSize ?? input.fontSize) * scale)))
        inlineTextEditor.textColor = tokens.color("text")
        inlineTextEditor.alignment = style?.align == "center" ? .center
            : style?.align == "right" ? .right : .left
        inlineTextScroll.isHidden = false
        inlineTextDoneButton.isHidden = false; inlineTextCancelButton.isHidden = false
        updateInlineTextFrame()
        window.makeFirstResponder(inlineTextEditor)
        if selectAtEnd {
            inlineTextEditor.setSelectedRange(NSRange(location: inlineTextEditor.string.utf16.count, length: 0))
        }
    }

    private func hideInlineTextEditor() {
        inlineTextScroll.isHidden = true
        inlineTextDoneButton.isHidden = true
        inlineTextCancelButton.isHidden = true
    }

    private func updateInlineTextFrame() {
        guard !inlineTextScroll.isHidden, let input = inlineTextInput else { return }
        let layer = input.layerID.flatMap { id in state.snapshot?.layers.first(where: { $0.id == id }) }
        let image = presentedImageRect
        let visible = viewportInput.bounds.standardized
        guard image.width > 0, image.height > 0, visible.width > 0, visible.height > 0,
              image.minX.isFinite, image.minY.isFinite, image.width.isFinite, image.height.isFinite,
              visible.minX.isFinite, visible.minY.isFinite,
              visible.width.isFinite, visible.height.isFinite,
              let snapshot = state.snapshot else { return }
        let scale = image.width / CGFloat(snapshot.width)
        let bounds: NSRect
        if let outline = layer?.selectionOutline, !outline.isEmpty {
            let xs = outline.map(\.x), ys = outline.map(\.y)
            bounds = NSRect(x: xs.min()!, y: ys.min()!,
                            width: xs.max()! - xs.min()!, height: ys.max()! - ys.min()!)
        } else {
            let fontSize = CGFloat(layer?.textStyle?.fontSize ?? input.fontSize)
            let x = layer.map { CGFloat($0.x) } ?? input.anchor.x
            let y = layer.map { CGFloat($0.y) } ?? input.anchor.y
            bounds = NSRect(x: x, y: y,
                            width: max(160, fontSize * 8), height: max(44, fontSize * 1.6))
        }
        // The native responder is intentionally not a WYSIWYG text layer. Keep
        // its usable minimum and actions in the viewport even when a narrow
        // image, zoom, or pan moves the authoritative shared preview offscreen.
        // Intersecting with an offscreen image can produce CGRect.null, whose
        // infinite origin must never be assigned to an AppKit view frame.
        let desiredWidth = min(visible.width, max(220, bounds.width * scale + 12))
        let desiredHeight = min(visible.height, max(96, bounds.height * scale + 12))
        let desiredX = image.minX + bounds.minX * scale - 6
        let desiredY = image.minY + bounds.minY * scale - 6
        let editorFrame = NSRect(
            x: min(max(visible.minX, desiredX), visible.maxX - desiredWidth),
            y: min(max(visible.minY, desiredY), visible.maxY - desiredHeight),
            width: desiredWidth, height: desiredHeight)
        inlineTextScroll.frame = editorFrame
        let buttonWidth: CGFloat = 68, buttonHeight: CGFloat = 28, gap: CGFloat = 6
        let buttonsWidth = buttonWidth * 2 + gap
        let candidateY = editorFrame.maxY + gap + buttonHeight <= visible.maxY
            ? editorFrame.maxY + gap : editorFrame.minY - gap - buttonHeight
        let buttonsY = min(max(visible.minY, candidateY), visible.maxY - buttonHeight)
        let buttonsX = min(max(visible.minX, editorFrame.maxX - buttonsWidth),
                           visible.maxX - buttonsWidth)
        inlineTextCancelButton.frame = NSRect(x: buttonsX, y: buttonsY,
                                              width: buttonWidth, height: buttonHeight)
        inlineTextDoneButton.frame = NSRect(x: buttonsX + buttonWidth + gap, y: buttonsY,
                                            width: buttonWidth, height: buttonHeight)
    }

    private func updateDrawing() {
        updateToolRail()
        let inputResolved = inlineTextInput == nil
        let cropReady = sectionControl?.selectedSegment == Section.geometry && state.snapshot != nil
            && !state.busy && inputResolved
        cropOverlay.croppingEnabled = cropReady && cropPrevious != nil
        drawCropButton?.isEnabled = cropReady
        drawCropButton?.title = cropPrevious == nil ? "Draw crop" : "Cancel crop"
        cropAspect.isEnabled = cropReady && cropPrevious != nil
        let active = sectionControl?.selectedSegment == Section.draw
            && state.snapshot != nil && !state.busy && inputResolved
        drawTool?.isEnabled = state.snapshot != nil && !state.busy && inputResolved
        wandTolerance.isEnabled = active; wandContiguous.isEnabled = active
        brushSize.isEnabled = active; brushSoftness.isEnabled = active
        createTextPreset.isEnabled = active && createTextPreset.numberOfItems > 1
        createTextSize.isEnabled = active; createTextColor.isEnabled = active
        let textReady = active && selectedLayer?.kind == .text
        textEditor.isEditable = textReady
        textFamily.isEnabled = textReady && textFamily.numberOfItems > 1
        textPreset.isEnabled = textReady && textPreset.numberOfItems > 1
        let textFields: [NSControl] = [textSize, textColor, textTraits,
                                       textAlignment, textPlate, textPlateColor, textShadow, textOutline]
        textFields.forEach { $0.isEnabled = textReady }
        textShadowFields.values.forEach { $0.isEnabled = textReady }
        let inlineReady = inlineTextInput != nil && inlineTextInput?.finishInFlight != true
        // Freeze the native responder only during an accepted Finish and restore
        // its normal state once that input has resolved.
        inlineTextEditor.isEditable = inlineTextInput?.finishInFlight != true
        inlineTextDoneButton?.isEnabled = inlineReady
        inlineTextCancelButton?.isEnabled = inlineReady
        textApplyButton?.isEnabled = textReady || inlineReady
        textCancelButton?.isEnabled = textReady || inlineReady
        drawOverlay.drawingEnabled = active
        selectionOverlay.selectionEnabled = sectionControl?.selectedSegment == Section.layers
            && state.snapshot != nil && !state.busy && inputResolved && !importLoading
    }

    private func selectCanvasLayer(_ id: String?) {
        guard !state.busy else { return }
        selectedLayerID = id
        if let id, let index = state.snapshot?.layers.firstIndex(where: { $0.id == id }) { selectedLayerIndex = index }
        reconcileLayerSelection(state.snapshot?.layers ?? [], allowFallback: false)
    }

    private func moveCanvasLayer(_ id: String, dx: CGFloat, dy: CGFloat, displayScale: Double) {
        guard !state.busy, let layer = state.snapshot?.layers.first(where: { $0.id == id }),
              !layer.locked else { return }
        command(["operation": "layer", "id": id,
                 "edit": ["action": "drag_move", "delta_x": Double(dx), "delta_y": Double(dy),
                          "display_scale": displayScale]],
                message: "Moving layer…", preferredSelection: id)
    }

    private func resizeCanvasLayer(_ id: String, handle: String, current: CGPoint,
                                   displayScale: Double, lockAspect: Bool) {
        guard !state.busy, let layer = state.snapshot?.layers.first(where: { $0.id == id }),
              layer.visible, !layer.locked else { return }
        command(["operation": "layer", "id": id, "edit": [
            "action": "resize", "handle": handle,
            "current": ["x": current.x, "y": current.y],
            "display_scale": displayScale, "lock_aspect": lockAspect,
        ]], message: "Resizing layer…", preferredSelection: id)
    }

    private func rotateCanvasLayer(_ id: String, radians: Double) {
        guard !state.busy, let layer = state.snapshot?.layers.first(where: { $0.id == id }),
              layer.visible, !layer.locked else { return }
        command(["operation": "layer", "id": id,
                 "edit": ["action": "rotate", "radians": radians]],
                message: "Rotating layer…", preferredSelection: id)
    }

    private func toggleVisibility() {
        guard let layer = selectedLayer else { return }
        layerCommand(layer, edit: ["action": "visibility", "visible": !layer.visible],
                     message: layer.visible ? "Hiding layer…" : "Showing layer…")
    }

    private func chooseImage() {
        guard let artifactID = state.artifactID, state.snapshot != nil, !state.busy,
              !importLoading else { return }
        selectionOverlay.cancelGesture()
        importToken += 1
        let token = importToken
        let generation = state.generation
        let completion: (URL?) -> Void = { [weak self] url in
            DispatchQueue.main.async {
                guard let self, self.importToken == token,
                      self.state.generation == generation,
                      self.state.artifactID == artifactID,
                      let url else { return }
                self.importLoading = true
                self.status.textColor = self.tokens.color("text-muted")
                self.status.stringValue = "Reading image…"
                self.updateControls()
                let decoder = self.imageDecoder
                Self.imageDecodeQueue.async {
                    let result = Result { try decoder(url) }
                    DispatchQueue.main.async {
                        guard self.importToken == token,
                              self.state.generation == generation,
                              self.state.artifactID == artifactID else { return }
                        switch result {
                        case .success(let image):
                            self.pendingImport = (image, generation, artifactID)
                            self.submitPendingImportIfReady()
                        case .failure(let error):
                            self.importLoading = false
                            self.showError("Couldn’t import image: \(error.localizedDescription)")
                            self.updateControls()
                        }
                    }
                }
            }
        }
        if let imagePicker {
            imagePicker(window, completion)
            return
        }
        let panel = Self.imagePanel()
        panel.beginSheetModal(for: window) { response in
            completion(response == .OK ? panel.url : nil)
        }
    }

    static func imagePanel() -> NSOpenPanel {
        let panel = NSOpenPanel()
        panel.title = "Choose image"
        panel.message = "Choose an image to add as a new layer"
        panel.prompt = "Add Image"
        panel.allowedContentTypes = [.image]
        panel.canChooseDirectories = false
        panel.canChooseFiles = true
        panel.allowsMultipleSelection = false
        return panel
    }

    private func submitPendingImportIfReady() {
        guard let pendingImport else { return }
        guard pendingImport.generation == state.generation,
              pendingImport.artifactID == state.artifactID, state.snapshot != nil else {
            cancelPendingImport(); return
        }
        guard !state.busy, let generation = state.beginCommand() else { return }
        self.pendingImport = nil
        let token = importToken
        let selectedID = selectedLayerID
        status.textColor = tokens.color("text-muted")
        status.stringValue = "Adding image layer…"
        updateControls()
        worker.importImage(pendingImport.image, selectedID: selectedID) { [weak self] result in
            guard let self, self.importToken == token else { return }
            self.importLoading = false
            switch result {
            case .success(let imported):
                guard self.state.complete(imported.presentation.snapshot,
                                          generation: generation) else { return }
                self.invalidateOutput()
                self.preferredLayerID = imported.layerID
                self.publish(imported.presentation, resetCrop: false)
                self.status.textColor = self.tokens.color("text-muted")
                self.status.stringValue = "Image added. Unsaved changes."
            case .failure(let error):
                guard self.state.fail(generation: generation) else { return }
                self.showError("Couldn’t import image: \(error.localizedDescription)")
            }
            self.updateControls()
        }
    }

    private func cancelPendingImport() {
        importToken += 1
        importLoading = false
        pendingImport = nil
    }

    private func toggleLock() {
        guard let layer = selectedLayer else { return }
        layerCommand(layer, edit: ["action": "lock", "locked": !layer.locked],
                     message: layer.locked ? "Unlocking layer…" : "Locking layer…")
    }

    private func renameLayer() {
        guard let layer = selectedLayer, layer.kind == .image,
              !layerName.stringValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            showError("Image layer names cannot be empty."); return
        }
        layerCommand(layer, edit: ["action": "rename", "name": layerName.stringValue],
                     message: "Renaming layer…")
    }

    private func setOpacity() {
        guard let layer = selectedLayer, let opacity = number(layerOpacity),
              (0...100).contains(opacity) else {
            showError("Layer opacity must be between 0 and 100."); return
        }
        layerCommand(layer, edit: ["action": "opacity", "opacity": opacity],
                     message: "Updating layer opacity…")
    }

    private func moveLayer() {
        guard let layer = selectedLayer, !layer.locked,
              let x = number(layerX), let y = number(layerY) else {
            showError("Unlocked layer coordinates must be finite numbers."); return
        }
        layerCommand(layer, edit: ["action": "translate", "delta_x": x - layer.x,
                                   "delta_y": y - layer.y], message: "Moving layer…")
    }

    private func transformLayer(_ transform: String, message: String) {
        guard let layer = selectedLayer, layer.kind == .image else { return }
        layerCommand(layer, edit: ["action": "image_transform", "transform": transform],
                     message: message)
    }

    private var layerActionsReady: Bool {
        state.snapshot != nil && !state.busy && !importLoading
            && inlineTextInput == nil && window.attachedSheet == nil
    }

    private func duplicateLayer() {
        guard let layer = selectedLayer else { return }
        duplicateLayer(id: layer.id)
    }

    private func duplicateLayer(id: String) {
        guard layerActionsReady,
              let layer = state.snapshot?.layers.first(where: { $0.id == id }) else { return }
        let newID = UUID().uuidString.lowercased()
        cancelDrawing(); cancelViewportPan()
        layerCommand(layer, edit: ["action": "duplicate", "new_id": newID],
                     message: "Duplicating layer…", preferredSelection: newID)
    }

    private func deleteLayer() {
        guard let layer = selectedLayer, !layer.locked else { return }
        deleteLayer(id: layer.id)
    }

    private func deleteLayer(id: String) {
        guard layerActionsReady,
              let layer = state.snapshot?.layers.first(where: { $0.id == id }), !layer.locked else { return }
        cancelDrawing(); cancelViewportPan()
        layerCommand(layer, edit: ["action": "delete"], message: "Deleting layer…")
    }

    private func copyLayer(id: String?) {
        guard layerActionsReady, let id,
              state.snapshot?.layers.contains(where: { $0.id == id }) == true else { return }
        cancelDrawing(); cancelViewportPan()
        command(["operation": "copy_layer", "id": id], message: "Copying layer…",
                preserveOutputAndStatus: true)
    }

    private func pasteLayer(after id: String?) {
        guard layerActionsReady,
              let snapshot = state.snapshot, snapshot.canPasteLayer else { return }
        guard id == nil || snapshot.layers.contains(where: { $0.id == id }) else { return }
        let newID = UUID().uuidString.lowercased()
        var request: [String: Any] = ["operation": "paste_layer", "new_id": newID]
        if let id { request["after_id"] = id }
        cancelDrawing(); cancelViewportPan()
        command(request, message: "Pasting layer…", preferredSelection: newID,
                selectToolOnSuccess: true)
    }

    private func combine(_ operation: String, id: String? = nil) {
        guard layerActionsReady, let snapshot = state.snapshot else { return }
        if operation == "merge_down" {
            guard let id, snapshot.mergeDownIDs.contains(id) else { return }
        } else if operation == "merge_visible" {
            guard snapshot.canMergeVisible else { return }
        } else {
            guard operation == "flatten", snapshot.canFlatten else { return }
        }
        let newID = UUID().uuidString.lowercased()
        var request: [String: Any] = ["operation": operation, "new_id": newID]
        if let id { request["id"] = id }
        cancelDrawing(); cancelViewportPan()
        command(request, message: "Combining layers…", preferredSelection: newID,
                selectToolOnSuccess: true)
    }

    @objc private func combineLayersSelected(_ sender: NSPopUpButton) {
        defer { sender.selectItem(at: 0) }
        switch sender.indexOfSelectedItem {
        case 1: combine("merge_down", id: selectedLayer?.id)
        case 2: combine("merge_visible")
        case 3: combine("flatten")
        default: break
        }
    }

    func layerContextMenu(row: Int) -> NSMenu? {
        guard window.attachedSheet == nil,
              let snapshot = state.snapshot else { return nil }
        let targetID = snapshot.layers.indices.contains(row) ? snapshot.layers[row].id : nil
        let target = targetID.flatMap { id in snapshot.layers.first { $0.id == id } }
        let menu = NSMenu(title: "Layer actions")
        menu.autoenablesItems = false
        func add(_ title: String, _ action: Selector, enabled: Bool, id: String? = nil) {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
            item.target = self; item.representedObject = id; item.isEnabled = enabled
            menu.addItem(item)
        }
        let ready = layerActionsReady
        if let target {
            add("Copy layer", #selector(copyLayerFromMenu(_:)), enabled: ready, id: target.id)
        }
        add("Paste layer", #selector(pasteLayerFromMenu(_:)),
            enabled: ready && snapshot.canPasteLayer, id: targetID)
        if let target {
            menu.addItem(.separator())
            add("Duplicate", #selector(duplicateLayerFromMenu(_:)), enabled: ready, id: target.id)
            add("Delete", #selector(deleteLayerFromMenu(_:)), enabled: ready && !target.locked, id: target.id)
            add("Merge down", #selector(mergeDownFromMenu(_:)),
                enabled: ready && snapshot.mergeDownIDs.contains(target.id), id: target.id)
        }
        menu.addItem(.separator())
        add("Merge visible", #selector(mergeVisibleFromMenu(_:)), enabled: ready && snapshot.canMergeVisible)
        add("Flatten image", #selector(flattenFromMenu(_:)), enabled: ready && snapshot.canFlatten)
        return menu
    }

    @objc private func copyLayerFromMenu(_ sender: NSMenuItem) {
        copyLayer(id: sender.representedObject as? String)
    }

    @objc private func pasteLayerFromMenu(_ sender: NSMenuItem) {
        pasteLayer(after: sender.representedObject as? String)
    }

    @objc private func duplicateLayerFromMenu(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String else { return }
        duplicateLayer(id: id)
    }

    @objc private func deleteLayerFromMenu(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String else { return }
        deleteLayer(id: id)
    }

    @objc private func mergeDownFromMenu(_ sender: NSMenuItem) {
        combine("merge_down", id: sender.representedObject as? String)
    }

    @objc private func mergeVisibleFromMenu(_ sender: NSMenuItem) { combine("merge_visible") }
    @objc private func flattenFromMenu(_ sender: NSMenuItem) { combine("flatten") }

    private func reorderLayer(up: Bool) {
        guard let snapshot = state.snapshot, let layer = selectedLayer,
              !layer.locked, let index = snapshot.layers.firstIndex(where: { $0.id == layer.id }) else { return }
        let targetIndex = up ? index - 1 : index + 1
        guard snapshot.layers.indices.contains(targetIndex), !snapshot.layers[targetIndex].locked else { return }
        layerCommand(layer, edit: ["action": "reorder",
                                   "target_id": snapshot.layers[targetIndex].id,
                                   "placement": up ? "before" : "after"],
                     message: up ? "Moving layer up…" : "Moving layer down…")
    }

    private func toggleCrop() {
        guard state.snapshot != nil, !state.busy else { return }
        if cropPrevious != nil { cancelCrop(); return }
        cropPrevious = [cropX, cropY, cropWidth, cropHeight].map(\.stringValue)
        publishCropSelection()
        updateControls()
        window.makeFirstResponder(cropOverlay)
    }

    private func cancelCrop() {
        guard let previous = cropPrevious else { return }
        cropPrevious = nil
        cropOverlay.cancelGesture()
        for (field, value) in zip([cropX, cropY, cropWidth, cropHeight], previous) { field.stringValue = value }
        publishCropSelection()
        updateControls()
    }

    @objc private func changeCropAspect() {
        cropOverlay.aspect = cropAspect.selectedItem?.representedObject as? Double ?? 0
    }

    private func setCropFields(_ rect: NSRect) {
        cropX.stringValue = format(rect.minX); cropY.stringValue = format(rect.minY)
        cropWidth.stringValue = format(rect.width); cropHeight.stringValue = format(rect.height)
    }

    private func publishCropSelection() {
        guard let x = number(cropX), let y = number(cropY),
              let width = positive(cropWidth), let height = positive(cropHeight) else {
            cropOverlay.selection = nil; return
        }
        cropOverlay.selection = NSRect(x: x, y: y, width: width, height: height)
    }

    private func applyCrop() {
        guard let x = number(cropX), let y = number(cropY),
              let width = positive(cropWidth), let height = positive(cropHeight) else {
            showError("Crop values must be finite numbers with positive width and height."); return
        }
        cropPrevious = nil
        cropOverlay.cancelGesture()
        command(["operation": "crop", "rect": [
            "x": x, "y": y, "width": width, "height": height,
        ]], message: "Applying crop…", resetCrop: true)
    }

    private func resizeCanvas() {
        guard let width = positive(canvasWidth), let height = positive(canvasHeight) else {
            showError("Canvas width and height must be finite positive numbers."); return
        }
        command(["operation": "resize_canvas", "width": width, "height": height],
                message: "Resizing canvas…", resetCrop: true)
    }

    @objc private func changeBackgroundMode() { updateControls() }

    private func publishBackgroundFields() {
        guard let snapshot = state.snapshot else { return }
        if let color = snapshot.background { lastSolidBackground = color }
        backgroundMode.selectItem(at: snapshot.background == nil ? 1 : 0)
        backgroundColor.stringValue = lastSolidBackground
        backgroundButton?.swatch = snapshot.background.flatMap { NSColor(hex: $0) } ?? .clear
        backgroundButton?.setAccessibilityLabel("Background color: \(snapshot.background ?? "transparent")")
    }

    private func applyBackground() {
        let color: Any = backgroundMode.indexOfSelectedItem == 0
            ? backgroundColor.stringValue as Any : NSNull()
        command(["operation": "set_background", "color": color],
                message: "Changing canvas background…")
    }

    private func saveDraft(closeAfter: Bool = false) {
        closeAfterCommand = closeAfter
        command(["operation": "save_draft", "updated_at_ms": EditorWorker.timestamp()],
                message: "Saving draft…")
    }

    private func confirmDiscard() {
        let alert = NSAlert()
        alert.messageText = "Discard all screenshot edits?"
        alert.informativeText = "This removes the native editor draft and restores the original History image."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Discard Edits"); alert.addButton(withTitle: "Cancel")
        alert.beginSheetModal(for: window) { [weak self] response in
            if response == .alertFirstButtonReturn { self?.discardEdits() }
        }
    }

    private func discardEdits(closeAfter: Bool = false) {
        closeAfterCommand = closeAfter
        command(["operation": "discard_draft"], message: "Discarding edits…", resetCrop: true)
    }

    private func command(_ object: [String: Any], message: String, resetCrop: Bool = false,
                         preferredSelection: String? = nil,
                         createdLayerExistingIDs: Set<String>? = nil,
                         preserveStagedTextOnFailure: Bool = false,
                         preserveOutputAndStatus: Bool = false,
                         selectToolOnSuccess: Bool = false) {
        guard inlineTextInput == nil else {
            showError("Finish or cancel inline text before another editor action.")
            return
        }
        guard let generation = state.beginCommand() else { return }
        cancelDrawingPreview()
        if !preserveOutputAndStatus { invalidateOutput() }
        preferredLayerID = preferredSelection
        if !preserveOutputAndStatus { status.stringValue = message }
        updateControls()
        worker.request(object) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let presentation):
                guard self.state.complete(presentation.snapshot, generation: generation) else { return }
                if let createdLayerExistingIDs {
                    self.preferredLayerID = presentation.snapshot.layers
                        .first { !createdLayerExistingIDs.contains($0.id) }?.id
                }
                if preserveOutputAndStatus {
                    self.preferredLayerID = nil
                } else {
                    self.publish(presentation, resetCrop: resetCrop)
                    self.status.textColor = self.tokens.color("text-muted")
                    self.status.stringValue = presentation.snapshot.unsavedChanges
                        ? "Unsaved changes." : presentation.snapshot.hasDraft ? "Draft saved." : "Original restored."
                }
                if selectToolOnSuccess { self.activateTool(section: Section.layers, shape: nil) }
                if self.closeAfterCommand { self.closeAfterCommand = false; self.closeNow(); return }
            case .failure(let error):
                guard self.state.fail(generation: generation) else { return }
                self.preferredLayerID = nil
                self.closeAfterCommand = false
                self.showError("Editor action failed: \(error.localizedDescription)")
                if !preserveStagedTextOnFailure { self.publishSelectedLayerFields() }
                self.publishBackgroundFields()
            }
            self.textApplyPending = false
            self.updateControls()
            self.submitPendingImportIfReady()
        }
    }

    private func publish(_ presentation: EditorPresentation, resetCrop: Bool) {
        let snapshot = presentation.snapshot
        editedImage = NSImage(cgImage: presentation.image,
            size: NSSize(width: CGFloat(presentation.image.width),
                         height: CGFloat(presentation.image.height)))
        preview.image = editedImage
        cancelViewportPan()
        viewportCanvasSize = NSSize(width: presentation.image.width, height: presentation.image.height)
        drawOverlay.canvasSize = NSSize(width: snapshot.width, height: snapshot.height)
        selectionOverlay.canvasSize = drawOverlay.canvasSize
        cropOverlay.canvasSize = viewportCanvasSize // Match wgpu's rendered-pixel crop bounds.
        updateViewportGeometry()
        publishOutputDimensions()
        canvasWidth.stringValue = format(snapshot.width); canvasHeight.stringValue = format(snapshot.height)
        publishBackgroundFields()
        if !snapshot.hasDraft && draftRestored { draftRestored = false; layoutEditor() }
        if resetCrop || cropWidth.stringValue.isEmpty {
            cropPrevious = nil
            cropX.stringValue = "0"; cropY.stringValue = "0"
            cropWidth.stringValue = format(snapshot.width); cropHeight.stringValue = format(snapshot.height)
        }
        publishCropSelection()
        publishCreateTextDefaults()
        reconcileLayerSelection(snapshot.layers)
        publishLayerCount(snapshot.layers.count)
        window.title = snapshot.unsavedChanges ? "Edit screenshot — Unsaved" : "Edit screenshot"
        // New pixels or dimensions: refresh the summary and re-estimate the export.
        refreshExportBar()
        scheduleEstimate()
    }

    private func closeNow() {
        cancelCrop()
        cancelDrawing()
        cancelPendingImport()
        closeAfterCommand = false; closeAfterTextInput = false
        inlineTextInput = nil; hideInlineTextEditor()
        selectedLayerID = nil; preferredLayerID = nil
        state.close(); editedImage = nil; invalidateOutput(); preview.image = nil
        estimateWork?.cancel(); estimateWork = nil; estimateGeneration += 1
        cancelViewportPan()
        viewport = NativeEditorViewport(); viewportCanvasSize = .zero
        worker.close(); window.orderOut(nil); updateControls()
        publishPresence()
    }

    private func updateControls() {
        let ready = state.snapshot != nil && !state.busy && inlineTextInput == nil
        fields.forEach { $0.isEnabled = ready }
        applyCropButton?.isEnabled = ready
        trimButton?.isEnabled = ready; backgroundButton?.isEnabled = ready
        backgroundMode?.isEnabled = ready
        backgroundColor.isEnabled = ready && backgroundMode?.indexOfSelectedItem == 0
        backgroundApply?.isEnabled = ready; backgroundReset?.isEnabled = ready
        undoButton?.isEnabled = ready && state.snapshot?.canUndo == true
        redoButton?.isEnabled = ready && state.snapshot?.canRedo == true
        saveDraftItem.isEnabled = ready && state.snapshot?.unsavedChanges == true
        discardItem.isEnabled = ready && (state.snapshot?.hasDraft == true || state.snapshot?.unsavedChanges == true)
        draftMenuButton?.isEnabled = ready
        draftDiscardButton?.isEnabled = ready
        addImagesButton?.isEnabled = ready && !importLoading
        addLayerButton?.isEnabled = ready && !importLoading
        sectionControl?.isEnabled = ready
        outputFormat?.isEnabled = ready; outputQuality?.isEnabled = ready
        exportDisclosure?.isEnabled = state.snapshot != nil
        copyImageButton?.isEnabled = ready
        outputFilename.isEnabled = ready
        changeOutputDirectoryButton?.isEnabled = ready
        showInFolderButton?.isEnabled = ready
        saveAsNewSwitch.isEnabled = ready
        let exportReady = exportBarState.map { $0.plan != nil && $0.error == nil } == true
            && exportOptionsError == nil
        exportSaveButton?.isEnabled = ready && exportReady
        exportSaveButton?.title = saveInFlight ? "Saving…" : "Save"
        previewOutputButton?.isEnabled = ready
        outputPreviewMode?.isEnabled = ready && encodedOutput != nil
        fitButton?.isEnabled = ready
        publishZoomLimits(ready: ready)
        zoomPreset.isEnabled = ready
        zoomSlider.isEnabled = ready
        updateOutputOptionControls()
        updateDrawing()
        layerTable?.isEnabled = ready
        importImageButton?.isEnabled = ready && !importLoading
        let combineReady = layerActionsReady
        combineLayers?.isEnabled = combineReady && (state.snapshot?.canMergeVisible == true
            || state.snapshot?.canFlatten == true || !(state.snapshot?.mergeDownIDs.isEmpty ?? true))
        combineLayers?.item(at: 1)?.isEnabled = combineReady && selectedLayer.map {
            state.snapshot?.mergeDownIDs.contains($0.id) == true
        } == true
        combineLayers?.item(at: 2)?.isEnabled = combineReady && state.snapshot?.canMergeVisible == true
        combineLayers?.item(at: 3)?.isEnabled = combineReady && state.snapshot?.canFlatten == true
        annotationControls?.setReady(ready)
        let layer = ready ? selectedLayer : nil
        let image = layer?.kind == .image
        rotationSnap.isEnabled = layer != nil
        layerName.isEnabled = image; renameButton?.isEnabled = image
        rotateLeftButton?.isEnabled = image; rotateRightButton?.isEnabled = image
        flipHorizontalButton?.isEnabled = image; flipVerticalButton?.isEnabled = image
        layerOpacity.isEnabled = layer != nil; opacityButton?.isEnabled = layer != nil
        visibilityButton?.isEnabled = layer != nil; lockButton?.isEnabled = layer != nil
        duplicateButton?.isEnabled = layer != nil
        let movable = layer != nil && layer?.locked == false
        layerX.isEnabled = movable; layerY.isEnabled = movable; moveButton?.isEnabled = movable
        deleteButton?.isEnabled = movable
        if let snapshot = state.snapshot, let layer,
           let index = snapshot.layers.firstIndex(where: { $0.id == layer.id }) {
            moveUpButton?.isEnabled = movable && index > 0 && !snapshot.layers[index - 1].locked
            moveDownButton?.isEnabled = movable && index + 1 < snapshot.layers.count
                && !snapshot.layers[index + 1].locked
        } else {
            moveUpButton?.isEnabled = false; moveDownButton?.isEnabled = false
        }
        visibilityButton?.title = layer?.visible == false ? "Show" : "Hide"
        lockButton?.title = layer?.locked == true ? "Unlock" : "Lock"
    }

    private func reconcileLayerSelection(_ layers: [NativeEditorLayer], allowFallback: Bool = true) {
        let preferred = preferredLayerID
        preferredLayerID = nil
        if let preferred, layers.contains(where: { $0.id == preferred }) {
            selectedLayerID = preferred
        } else if let selectedLayerID, layers.contains(where: { $0.id == selectedLayerID }) {
            self.selectedLayerID = selectedLayerID
        } else if layers.isEmpty {
            selectedLayerID = nil; selectedLayerIndex = 0
        } else if allowFallback {
            selectedLayerIndex = min(selectedLayerIndex, layers.count - 1)
            selectedLayerID = layers[selectedLayerIndex].id
        }
        reconcilingLayerSelection = true
        defer { reconcilingLayerSelection = false }
        layerTable?.reloadData()
        if let selectedLayerID,
           let index = layers.firstIndex(where: { $0.id == selectedLayerID }) {
            selectedLayerIndex = index
            layerTable?.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
            layerTable?.scrollRowToVisible(index)
        } else {
            layerTable?.deselectAll(nil)
        }
        publishSelectedLayerFields()
    }

    private func publishSelectedLayerFields() {
        annotationControls?.setStyle(selectedLayer?.annotation)
        publishTextFields(preserveStaged: true)
        publishDrawToolControls()
        selectionOverlay.documentJSON = state.snapshot?.documentJSON
        selectionOverlay.selectedOutline = selectedLayer?.selectionOutline
        selectionOverlay.selectedLayerID = selectedLayer?.id
        selectionOverlay.selectedRotation = selectedLayer?.rotation ?? 0
        selectionOverlay.rotationEnabled = selectedLayer?.visible == true && selectedLayer?.locked == false
        selectionOverlay.resizeEnabled = selectedLayer?.visible == true && selectedLayer?.locked == false
        guard let layer = selectedLayer else {
            [layerName, layerOpacity, layerX, layerY].forEach { $0.stringValue = "" }
            updateControls(); return
        }
        layerName.stringValue = layer.name
        layerOpacity.stringValue = format(layer.opacity)
        layerX.stringValue = format(layer.x); layerY.stringValue = format(layer.y)
        updateControls()
    }

    func numberOfRows(in tableView: NSTableView) -> Int { state.snapshot?.layers.count ?? 0 }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard !reconcilingLayerSelection else { return }
        guard let layers = state.snapshot?.layers, layers.indices.contains(layerTable.selectedRow) else {
            selectedLayerID = nil; publishSelectedLayerFields(); return
        }
        selectedLayerIndex = layerTable.selectedRow
        selectedLayerID = layers[selectedLayerIndex].id
        publishSelectedLayerFields()
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let layers = state.snapshot?.layers, layers.indices.contains(row) else { return nil }
        let layer = layers[row]
        let title = NSTextField(labelWithString: layer.rowName)
        title.lineBreakMode = .byTruncatingTail; title.toolTip = layer.rowName
        title.font = .systemFont(ofSize: tokens.number("text-sm"), weight: .medium)
        title.textColor = tokens.color("text")
        let detail = NSTextField(labelWithString: layer.rowKind)
        detail.alignment = .right; detail.font = .systemFont(ofSize: tokens.number("text-xs"))
        detail.textColor = tokens.color("text-subtle"); detail.toolTip = layer.rowKind
        // Shipping fades hidden layers' name and preview.
        if !layer.visible { title.alphaValue = 0.42; detail.alphaValue = 0.42 }
        let id = layer.id
        let visibility = CaptureButton("", frame: .zero, tokens: tokens) { [weak self] in
            self?.toggleVisibility(id: id)
        }
        visibility.icon = .shipping(layer.visible ? "eye" : "eye-off")
        visibility.setAccessibilityLabel("\(layer.visible ? "Hide" : "Show") \(layer.rowName)")
        visibility.toolTip = EditorChrome.text("layers", layer.visible ? "hide" : "show")
        let lock = CaptureButton("", frame: .zero, tokens: tokens) { [weak self] in
            self?.toggleLock(id: id)
        }
        lock.icon = .shipping(layer.locked ? "lock" : "unlock")
        lock.setAccessibilityLabel("\(layer.locked ? "Unlock" : "Lock") \(layer.rowName)")
        lock.toolTip = EditorChrome.text("layers", layer.locked ? "unlock" : "lock")
        for (control, on) in [(visibility, !layer.visible), (lock, layer.locked)] {
            control.quiet = true; control.iconSide = 14
            control.cornerRadius = tokens.number("r-sm"); control.selected = on
            control.isEnabled = state.snapshot != nil && !state.busy && inlineTextInput == nil
        }
        return EditorLayerCell(title: title, detail: detail, iconName: layer.rowIcon, tokens: tokens,
                               visibility: visibility, lock: lock)
    }

    /// Row quick actions target their own layer; lock also selects it, like shipping.
    private func toggleVisibility(id: String) {
        guard let layer = state.snapshot?.layers.first(where: { $0.id == id }) else { return }
        layerCommand(layer, edit: ["action": "visibility", "visible": !layer.visible],
                     message: layer.visible ? "Hiding layer…" : "Showing layer…",
                     preferredSelection: selectedLayerID)
    }

    private func toggleLock(id: String) {
        guard let layer = state.snapshot?.layers.first(where: { $0.id == id }) else { return }
        layerCommand(layer, edit: ["action": "lock", "locked": !layer.locked],
                     message: layer.locked ? "Unlocking layer…" : "Locking layer…",
                     preferredSelection: id)
    }

    private func restyle(_ tokens: Tokens) {
        self.tokens = tokens; root.wantsLayer = true
        root.layer?.backgroundColor = tokens.color("surface-canvas").cgColor
        let dark = tokens.color("text").brightnessComponent > 0.5
        window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
        for case let label as NSTextField in root.subviews {
            label.textColor = tokens.color(label.identifier?.rawValue == "editor-muted"
                ? "text-muted" : "text")
        }
        status.textColor = tokens.color("text-muted")
        restyleChrome()
        exportBar.layer?.backgroundColor = tokens.color("surface-raised").cgColor
        exportBarRule.layer?.backgroundColor = tokens.color("border-subtle").cgColor
        exportSettingsPanel.layer?.backgroundColor = tokens.color("surface-sunken").cgColor
        exportSettingsPanel.layer?.borderColor = tokens.color("border").cgColor
        for label in [exportFilenameCaption, saveAsNewLabel, exportDisclosureTitle] {
            label.textColor = tokens.color("text")
        }
        for label in [exportSavingToCaption, outputLocation, exportSummary, exportChevron, outputDimensions] {
            label.textColor = tokens.color("text-subtle")
        }
        for group in exportGroups.values { group.caption.textColor = tokens.color("text-muted") }
        publishExportBar()
        textEditor.backgroundColor = tokens.color("surface-sunken")
        textEditor.textColor = tokens.color("text")
        textEditor.insertionPointColor = tokens.color("text")
        textEditor.font = .systemFont(ofSize: tokens.number("text-md"))
        inlineTextScroll.backgroundColor = tokens.color("surface-raised")
        inlineTextScroll.layer?.borderColor = tokens.color("theme-accent").cgColor
        inlineTextEditor.backgroundColor = tokens.color("surface-raised")
        inlineTextEditor.textColor = tokens.color("text")
        inlineTextEditor.insertionPointColor = tokens.color("text")
        cropOverlay.tokens = tokens
        drawOverlay.fillColor = tokens.color("theme-accent").withAlphaComponent(0.22)
        drawOverlay.strokeColor = tokens.color("theme-accent")
        drawOverlay.brushOutlineColor = tokens.color("text")
        selectionOverlay.strokeColor = tokens.color("theme-accent")
        drawOverlay.needsDisplay = true
        preview.superview?.layer?.backgroundColor = tokens.color("surface-sunken").cgColor
        preview.superview?.layer?.borderColor = tokens.color("border").cgColor
    }

    private func publishLayerCount(_ count: Int) {
        layerCount.stringValue = "\(count)"
        layerCount.frame.size.width = max(19, ceil(layerCount.attributedStringValue.size().width) + 8)
    }

    private func restyleChrome() {
        let raised = tokens.color("surface-raised").cgColor
        let border = tokens.color("border-subtle").cgColor
        headerBar.layer?.backgroundColor = raised
        railPanel.layer?.backgroundColor = raised
        headerRule.layer?.backgroundColor = border
        railRule.layer?.backgroundColor = border
        for group in [canvasToolbar, zoomGroup] {
            group.layer?.backgroundColor = tokens.color("surface-sunken").cgColor
            group.layer?.borderColor = border
        }
        zoomDividers.forEach { $0.layer?.backgroundColor = border }
        canvasSplit.layer?.backgroundColor = tokens.color("border").cgColor
        canvasToolbarLabels.forEach { $0.textColor = tokens.color("text-subtle") }
        for field in [canvasWidth, canvasHeight] { field.textColor = tokens.color("text") }
        backgroundCard.layer?.backgroundColor = tokens.color("surface-overlay").cgColor
        backgroundCard.layer?.borderColor = tokens.color("border").cgColor
        draftBanner.layer?.backgroundColor = tokens.color("caution-surface").cgColor
        draftBannerLabel.textColor = tokens.color("caution-text")
        railTip.backgroundColor = .clear
        railTip.layer?.backgroundColor = tokens.color("glass-strong").cgColor
        railTip.layer?.borderColor = tokens.color("glass-border").cgColor
        railTip.textColor = tokens.color("glass-text")
        layerCount.textColor = tokens.color("text-subtle")
        layerCount.layer?.backgroundColor = tokens.color("surface-sunken").cgColor
        layerHeadingRule.layer?.backgroundColor = border
        let controls: [CaptureButton?] = [trimButton, backgroundButton, undoButton, redoButton, fitButton,
                                          zoomOutButton, zoomInButton, addImagesButton, draftMenuButton,
                                          recenterButton, draftDiscardButton, draftDismissButton, addLayerButton]
        for control in controls.compactMap({ $0 }) { control.tokens = tokens; control.needsDisplay = true }
    }

    private func configure(_ field: NSTextField, frame: NSRect, label: String,
                           parent: NSView? = nil) {
        field.frame = frame; field.setAccessibilityLabel(label)
        field.alignment = .right; field.placeholderString = "0"
        field.formatter = editorNumberFormatter; (parent ?? root).addSubview(field)
    }

    private func number(_ field: NSTextField) -> Double? {
        guard let value = editorNumberFormatter.number(from: field.stringValue)?.doubleValue,
              value.isFinite else { return nil }
        return value
    }
    private func positive(_ field: NSTextField) -> Double? {
        guard let value = number(field), value > 0 else { return nil }; return value
    }
    private func format(_ value: Double) -> String {
        editorNumberFormatter.string(from: NSNumber(value: value)) ?? String(value)
    }
    private func outputInteger(_ field: NSTextField) -> UInt64? {
        guard let value = outputIntegerFormatter.number(from: field.stringValue)?.doubleValue,
              value.isFinite, value >= 0, value.rounded() == value,
              value < Double(UInt64.max) else { return nil }
        return UInt64(value)
    }
    private func formatInteger(_ value: Int) -> String {
        outputIntegerFormatter.string(from: NSNumber(value: value)) ?? String(value)
    }

    private func showError(_ message: String) {
        status.stringValue = message; status.textColor = tokens.color("danger-text")
        reportError(message)
    }

    @discardableResult private func label(_ text: String, frame: NSRect, size: CGFloat = 13,
                                           weight: NSFont.Weight = .regular,
                                           muted: Bool = false) -> NSTextField {
        let label = NSTextField(wrappingLabelWithString: text); label.frame = frame
        label.font = .systemFont(ofSize: size, weight: weight)
        label.textColor = tokens.color(muted ? "text-muted" : "text")
        if muted { label.identifier = NSUserInterfaceItemIdentifier("editor-muted") }
        root.addSubview(label); return label
    }

    @discardableResult private func panelFieldLabel(_ text: String, x: CGFloat, y: CGFloat,
                                                     parent: NSView) -> NSTextField {
        panelLabel(text, frame: NSRect(x: x, y: y, width: 128, height: 20),
                   muted: true, parent: parent)
    }

    @discardableResult private func panelLabel(_ text: String, frame: NSRect, size: CGFloat = 13,
                                                weight: NSFont.Weight = .regular,
                                                muted: Bool = false,
                                                parent: NSView) -> NSTextField {
        let label = NSTextField(wrappingLabelWithString: text); label.frame = frame
        label.font = .systemFont(ofSize: size, weight: weight)
        label.textColor = tokens.color(muted ? "text-muted" : "text")
        if muted { label.identifier = NSUserInterfaceItemIdentifier("editor-muted") }
        parent.addSubview(label); return label
    }

    @discardableResult private func button(_ title: String, frame: NSRect,
                                            parent: NSView? = nil,
                                            action: @escaping () -> Void) -> CaptureButton {
        let button = CaptureButton(title, frame: frame, tokens: tokens, action: action)
        (parent ?? root).addSubview(button); return button
    }
}
