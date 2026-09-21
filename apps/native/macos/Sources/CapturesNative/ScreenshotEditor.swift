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

private final class EditorLayerCell: NSTableCellView {
    let titleLabel: NSTextField
    let detailLabel: NSTextField

    init(title: NSTextField, detail: NSTextField) {
        titleLabel = title; detailLabel = detail
        super.init(frame: .zero)
        addSubview(title); addSubview(detail); textField = title
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func layout() {
        super.layout()
        let inset: CGFloat = 8
        let gap: CGFloat = 8
        let trailingEdge = min(bounds.maxX, visibleRect.maxX) - inset
        let detailWidth = min(detailLabel.intrinsicContentSize.width,
                              max(0, trailingEdge - inset))
        detailLabel.frame = NSRect(x: trailingEdge - detailWidth, y: 4,
                                   width: detailWidth, height: 22)
        titleLabel.frame = NSRect(x: inset, y: 4,
                                  width: max(0, detailLabel.frame.minX - inset - gap), height: 22)
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

final class EditorDrawOverlay: EditorViewportGestureView {
    enum Shape: String, CaseIterable {
        case rectangle, ellipse, line, arrow, pen, wand, erase, restore

        var isBackgroundBrush: Bool { self == .erase || self == .restore }
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
    var onWand: ((NSPoint) -> Void)?
    var onBackgroundBrush: ((Shape, [NSPoint]) -> Void)?
    var brushDiameter: CGFloat = 28 { didSet { needsDisplay = true } }
    var brushOutlineColor = NSColor.labelColor
    var fillColor = NSColor.controlAccentColor.withAlphaComponent(0.22)
    var strokeColor = NSColor.controlAccentColor
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
        if (shape == .wand || shape.isBackgroundBrush)
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
        if shape.isBackgroundBrush {
            // Shipping stamps the release even at the last natural pixel: soft
            // edges accumulate, so deduplicating samples would change alpha.
            backgroundPoints.append(end)
            onBackgroundBrush?(shape, backgroundPoints)
            return
        }
        switch shape {
        case .rectangle, .ellipse:
            guard start.x != end.x, start.y != end.y else { return }
        case .arrow:
            guard arrowLength >= 3,
                  let geometry = NativeEditorDrawGeometry(arrow: true, samples: [start, end]),
                  !geometry.points.isEmpty else { return }
        case .line, .pen: break
        case .wand: preconditionFailure("handled above")
        case .erase, .restore: preconditionFailure("handled above")
        }
        // Like shipping, Pen accepts movement samples, not the release location.
        onComplete?(shape, start, end, points)
    }

    func cancelGesture() {
        startPoint = nil; currentPoint = nil; needsDisplay = true
        penPoints.removeAll(keepingCapacity: true)
        brushPoints.removeAll(keepingCapacity: true)
        if let previousMouseCoalescing {
            NSEvent.isMouseCoalescingEnabled = previousMouseCoalescing
            self.previousMouseCoalescing = nil
        }
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
        if shape == .wand { return }
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
            path.lineJoinStyle = .round; path.stroke()
            let radius = max(2, brushDiameter * scale / 2)
            let ring = NSBezierPath(ovalIn: NSRect(x: currentPoint.x - radius, y: currentPoint.y - radius,
                                                  width: radius * 2, height: radius * 2))
            ring.lineWidth = 1.5; ring.stroke()
            return
        }
        if shape == .line || shape == .arrow || shape == .pen {
            let samples = shape == .pen ? penPoints : [canvasPoint(for: startPoint), canvasPoint(for: currentPoint)]
            guard let geometry = NativeEditorDrawGeometry(arrow: shape == .arrow, samples: samples),
                  let first = geometry.points.first else { return }
            let image = presentedImageRect
            let scale = image.width / canvasSize.width
            let position: (NSPoint) -> NSPoint = {
                NSPoint(x: image.minX + $0.x * scale, y: image.minY + $0.y * scale)
            }
            let path = NSBezierPath(); path.move(to: position(first))
            geometry.points.dropFirst().forEach { path.line(to: position($0)) }
            strokeColor.setFill(); strokeColor.setStroke()
            if shape == .arrow { path.close(); path.fill() }
            else if geometry.points.allSatisfy({ $0 == first }) {
                let radius = geometry.strokeWidth * scale / 2
                let center = position(first)
                NSBezierPath(ovalIn: NSRect(x: center.x - radius, y: center.y - radius,
                                           width: radius * 2, height: radius * 2)).fill()
            } else {
                path.lineWidth = geometry.strokeWidth * scale
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
        fillColor.setFill(); path.fill()
        strokeColor.setStroke(); path.lineWidth = 2; path.stroke()
    }
}

final class EditorSelectionOverlay: EditorViewportGestureView {
    var canvasSize = NSSize.zero { didSet { cancelGesture(); needsDisplay = true } }
    var selectionEnabled = false { didSet { if !selectionEnabled { cancelGesture() }; isHidden = !selectionEnabled } }
    var selectedOutline: [CGPoint]? { didSet { needsDisplay = true } }
    var selectedLayerID: String? { didSet { if selectedLayerID != oldValue { cancelGesture() }; needsDisplay = true } }
    var documentJSON: String? { didSet { if documentJSON != oldValue { cancelGesture() } } }
    var selectedRotation = 0.0 { didSet { needsDisplay = true } }
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
                start: documentPoint, current: documentPoint, snap: snap)
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
                start: canvasPoint(for: startPoint), current: canvasPoint(for: point), snap: snapRotation)
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
    override func mouseDown(with event: NSEvent) { if beginViewportPan(event) { cancelGesture(); return }; window?.makeFirstResponder(self); begin(at: convert(event.locationInWindow, from: nil), snap: event.modifierFlags.contains(.shift)) }
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

private final class ScreenshotEditorWindow: NSWindow {
    var zoomShortcut: ((NSEvent) -> Bool)?

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if attachedSheet == nil, zoomShortcut?(event) == true { return true }
        return super.performKeyEquivalent(with: event)
    }

    override func sendEvent(_ event: NSEvent) {
        // Control shortcuts and focused field editors also reach this path.
        if attachedSheet == nil, zoomShortcut?(event) == true { return }
        super.sendEvent(event)
    }
}

final class ScreenshotEditorController: NSObject, NSWindowDelegate, NSTableViewDataSource,
                                        NSTableViewDelegate, NSTextFieldDelegate {
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
    private var viewportButtons: [CaptureButton] = []
    private let zoomPreset = NSPopUpButton()
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
    private let dimensions = NSTextField(labelWithString: "")
    private let geometryPanel = Surface()
    private let geometryContent = Surface()
    private let layersPanel = Surface()
    private let layerContent = Surface()
    private var annotationControls: EditorAnnotationControls!
    private let drawPanel = Surface()
    private let outputPanel = Surface()
    private let outputContent = Surface()
    let drawOverlay = EditorDrawOverlay()
    let selectionOverlay = EditorSelectionOverlay()
    private let layerName = NSTextField()
    private let layerOpacity = NSTextField()
    private let layerX = NSTextField()
    private let layerY = NSTextField()
    private let outputQualityValue = NSTextField()
    private let outputPngPalette = NSTextField()
    private let outputByteBudget = NSTextField()
    private let outputFilename = NSTextField()
    private let outputLocation = NSTextField(labelWithString: "")
    private let outputSize = NSTextField(wrappingLabelWithString: "No encoded preview yet.")
    private var outputQualityValueLabel: NSTextField!
    private var outputPngPaletteLabel: NSTextField!
    private var outputByteBudgetLabel: NSTextField!
    private var sectionControl: NSSegmentedControl!
    private var drawTool: NSPopUpButton!
    private let wandTolerance = NSTextField()
    private let wandContiguous = NSButton(checkboxWithTitle: "Contiguous only", target: nil, action: nil)
    private var wandToleranceLabel: NSTextField!
    private let brushSize = NSTextField()
    private let brushSoftness = NSTextField()
    private var brushSizeLabel: NSTextField!
    private var brushSoftnessLabel: NSTextField!
    private var drawHelper: NSTextField!
    private var outputFormat: NSPopUpButton!
    private var outputQuality: NSPopUpButton!
    private var outputPreviewMode: NSSegmentedControl!
    private var layerTable: NSTableView!
    private var visibilityButton: CaptureButton!
    private var lockButton: CaptureButton!
    private var renameButton: CaptureButton!
    private var opacityButton: CaptureButton!
    private var moveButton: CaptureButton!
    private var duplicateButton: CaptureButton!
    private var deleteButton: CaptureButton!
    private var moveUpButton: CaptureButton!
    private var moveDownButton: CaptureButton!
    private var rotateLeftButton: CaptureButton!
    private var rotateRightButton: CaptureButton!
    private var flipHorizontalButton: CaptureButton!
    private var flipVerticalButton: CaptureButton!
    private var importImageButton: CaptureButton!
    private var undoButton: CaptureButton!
    private var redoButton: CaptureButton!
    private var saveButton: CaptureButton!
    private var discardButton: CaptureButton!
    private var applyCropButton: CaptureButton!
    private var resizeButton: CaptureButton!
    private var trimButton: CaptureButton!
    private var previewOutputButton: CaptureButton!
    private var copyImageButton: CaptureButton!
    private var changeOutputDirectoryButton: CaptureButton!
    private var saveNewCopyButton: CaptureButton!
    private var fields: [NSTextField] = []
    private var closeAfterCommand = false
    private var selectedLayerID: String?
    private var selectedLayerIndex = 0
    private var preferredLayerID: String?
    private var reconcilingLayerSelection = false
    private var editedImage: NSImage?
    private var encodedOutput: EditorOutputPresentation?
    private var historyRoot = ""
    private var captureMode = "region"
    private var outputDirectory = ""
    private let directoryPicker: ((NSWindow, URL?, @escaping (URL?) -> Void) -> Void)?
    private let didSaveCopy: () -> Void
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
        static let output = 3
    }

    init(tokens: Tokens, worker: EditorWorking = EditorWorker(), numberLocale: Locale = .current,
         reportError: @escaping (String) -> Void = { _ in },
         directoryPicker: ((NSWindow, URL?, @escaping (URL?) -> Void) -> Void)? = nil,
         didSaveCopy: @escaping () -> Void = {},
         imagePicker: ((NSWindow, @escaping (URL?) -> Void) -> Void)? = nil,
         imageDecoder: @escaping (URL) throws -> EditorDecodedImage = EditorImageDecoder.decode,
         writeClipboard: @escaping (Data) -> Bool = { png in
             let pasteboard = NSPasteboard.general
             pasteboard.clearContents()
             return pasteboard.setData(png, forType: .png)
         }) {
        self.tokens = tokens; self.worker = worker; self.reportError = reportError
        self.directoryPicker = directoryPicker; self.didSaveCopy = didSaveCopy
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
        let bounds = NSRect(x: 0, y: 0, width: 1000, height: 700)
        root = Surface(frame: bounds)
        let editorWindow = ScreenshotEditorWindow(contentRect: bounds,
            styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
        window = editorWindow
        super.init()
        editorWindow.zoomShortcut = { [weak self] in self?.handleZoomShortcut($0) ?? false }
        window.isReleasedWhenClosed = false; window.title = "Edit screenshot"
        window.delegate = self
        window.contentView = root
        build(); restyle(tokens); updateControls()
    }

    func present(artifact: CaptureArtifact, historyRoot: String, outputDirectory: String? = nil) {
        guard !state.busy else {
            showError("Wait for the current editor action to finish before opening another screenshot.")
            window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
            return
        }
        guard !artifact.isRecording else {
            showError("Recording editing is not available in this native editor.")
            return
        }
        if state.snapshot?.unsavedChanges == true, state.artifactID != artifact.id {
            showError("Save or discard the open screenshot edits before editing another capture.")
            window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
            return
        }
        if state.artifactID == artifact.id, state.snapshot != nil {
            window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
            return
        }
        cancelPendingImport()
        let generation = state.beginOpen(artifactID: artifact.id)
        lastSolidBackground = "#f7f7f5"
        self.historyRoot = historyRoot
        captureMode = artifact.mode
        self.outputDirectory = outputDirectory ?? URL(fileURLWithPath: historyRoot)
            .deletingLastPathComponent().path
        outputFilename.stringValue = defaultOutputFilename()
        publishOutputLocation()
        selectedLayerID = nil; selectedLayerIndex = 0; preferredLayerID = nil
        editedImage = nil; invalidateOutput(); preview.image = nil; window.title = "Edit screenshot"
        status.stringValue = "Opening screenshot…"; updateControls()
        window.center(); window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
        let draftsRoot = URL(fileURLWithPath: historyRoot).deletingLastPathComponent()
            .appendingPathComponent("editor-drafts", isDirectory: true).path
        worker.open(historyRoot: historyRoot, draftsRoot: draftsRoot, artifactID: artifact.id) {
            [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let presentation):
                guard self.state.complete(presentation.snapshot, generation: generation) else { return }
                self.publish(presentation, resetCrop: true)
                self.status.textColor = self.tokens.color("text-muted")
                self.status.stringValue = presentation.snapshot.hasDraft
                    ? "Draft restored." : "Ready. Changes affect only the native editor draft."
            case .failure(let error):
                guard self.state.fail(generation: generation) else { return }
                self.showError("Couldn’t open screenshot: \(error.localizedDescription)")
            }
            self.updateControls()
            self.submitPendingImportIfReady()
        }
    }

    func prepareForTermination() -> Bool {
        cancelDrawing()
        cancelPendingImport()
        let result = worker.prepareForTermination()
        switch result {
        case .success:
            state.close(); editedImage = nil; invalidateOutput(); preview.image = nil
            window.orderOut(nil); return true
        case .failure(let error):
            showError("Couldn’t save screenshot draft before quitting: \(error.localizedDescription)")
            window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
            return false
        }
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        cancelDrawing()
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
        cancelDrawing()
        cancelViewportPan()
    }

    private func build() {
        label("Screenshot editor", frame: NSRect(x: 24, y: 20, width: 400, height: 30),
              size: 21, weight: .semibold)
        label("Crop and resize a recoverable native draft. The original History image is unchanged.",
              frame: NSRect(x: 24, y: 54, width: 640, height: 22), muted: true)

        let previewPanel = Surface(frame: NSRect(x: 24, y: 90, width: 640, height: 550))
        previewPanel.wantsLayer = true
        previewPanel.layer?.masksToBounds = true
        previewPanel.layer?.cornerRadius = tokens.number("r-xl")
        previewPanel.layer?.borderWidth = 1
        root.addSubview(previewPanel)
        viewportBounds = previewPanel.bounds.insetBy(dx: 18, dy: 18)
        viewportInput.frame = viewportBounds
        viewportInput.autoresizingMask = []
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
        selectionOverlay.onError = { [weak self] error in self?.showError("Layer interaction failed: \(error.localizedDescription)") }
        viewportInput.addSubview(selectionOverlay)
        for view in [viewportInput, drawOverlay, selectionOverlay] { configureViewportGestures(view) }
        drawOverlay.imageRect = { [weak self] in self?.presentedImageRect ?? .zero }
        selectionOverlay.imageRect = { [weak self] in self?.presentedImageRect ?? .zero }
        let fit = button("Fit", frame: NSRect(x: 24, y: 650, width: 64, height: 30), parent: root) {
            [weak self] in self?.fitViewport()
        }
        fit.setAccessibilityLabel("Fit screenshot in viewport"); viewportButtons.append(fit)
        zoomPreset.frame = NSRect(x: 96, y: 650, width: 80, height: 30)
        zoomPreset.setAccessibilityLabel("Canvas zoom preset")
        zoomPreset.target = self; zoomPreset.action = #selector(changeZoomPreset)
        root.addSubview(zoomPreset)
        publishZoomPreset()
        let controls: [(String, String, () -> Void)] = [
            ("−", "Zoom out", { [weak self] in self?.scaleViewport(by: 1 / 1.25) }),
            ("+", "Zoom in", { [weak self] in self?.scaleViewport(by: 1.25) }),
            ("Recenter", "Recenter screenshot", { [weak self] in self?.recenterViewport() }),
        ]
        var x: CGFloat = 184
        for (title, accessibility, action) in controls {
            let width: CGFloat = title == "Recenter" ? 92 : 64
            let control = button(title, frame: NSRect(x: x, y: 650, width: width, height: 30),
                                 parent: root, action: action)
            control.setAccessibilityLabel(accessibility); viewportButtons.append(control); x += width + 8
        }
        dimensions.frame = NSRect(x: 440, y: 654, width: 224, height: 20)
        dimensions.setAccessibilityLabel("Edited canvas dimensions"); root.addSubview(dimensions)

        sectionControl = NSSegmentedControl(labels: ["Geometry", "Layers", "Draw", "Output"], trackingMode: .selectOne,
                                            target: self, action: #selector(changeSection))
        sectionControl.frame = NSRect(x: 688, y: 24, width: 272, height: 28)
        sectionControl.selectedSegment = 0
        sectionControl.setAccessibilityLabel("Editor section")
        root.addSubview(sectionControl)

        geometryPanel.frame = NSRect(x: 688, y: 66, width: 272, height: 390)
        layersPanel.frame = geometryPanel.frame; layersPanel.isHidden = true
        drawPanel.frame = geometryPanel.frame; drawPanel.isHidden = true
        outputPanel.frame = geometryPanel.frame; outputPanel.isHidden = true
        geometryPanel.setAccessibilityLabel("Geometry controls")
        layersPanel.setAccessibilityLabel("Layer controls")
        drawPanel.setAccessibilityLabel("Drawing controls")
        outputPanel.setAccessibilityLabel("Output controls")
        root.addSubview(geometryPanel); root.addSubview(layersPanel)
        root.addSubview(drawPanel); root.addSubview(outputPanel)

        let geometryScroll = NSScrollView(frame: geometryPanel.bounds)
        geometryScroll.autoresizingMask = [.width, .height]
        geometryScroll.hasVerticalScroller = true; geometryScroll.drawsBackground = false
        geometryContent.frame = NSRect(x: 0, y: 0, width: 252, height: 746)
        geometryScroll.documentView = geometryContent
        geometryPanel.addSubview(geometryScroll)

        panelLabel("Crop", frame: NSRect(x: 0, y: 0, width: 252, height: 24),
                   size: 16, weight: .semibold, parent: geometryContent)
        panelLabel("Shared Rust owns canvas geometry.", frame: NSRect(x: 0, y: 28, width: 252, height: 22),
                   muted: true, parent: geometryContent)
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
        applyCropButton = button("Apply crop", frame: NSRect(x: 0, y: 194, width: 252, height: 34),
                                 parent: geometryContent) {
            [weak self] in self?.applyCrop()
        }

        panelLabel("Canvas", frame: NSRect(x: 0, y: 242, width: 252, height: 24),
                   size: 16, weight: .semibold, parent: geometryContent)
        panelLabel("Resize canvas without scaling the image.", frame: NSRect(x: 0, y: 270, width: 252, height: 22),
                   muted: true, parent: geometryContent)
        panelFieldLabel("Width", x: 0, y: 300, parent: geometryContent)
        panelFieldLabel("Height", x: 134, y: 300, parent: geometryContent)
        configure(canvasWidth, frame: NSRect(x: 0, y: 324, width: 118, height: 30), label: "Canvas width",
                  parent: geometryContent)
        configure(canvasHeight, frame: NSRect(x: 134, y: 324, width: 118, height: 30), label: "Canvas height",
                  parent: geometryContent)
        resizeButton = button("Resize canvas", frame: NSRect(x: 0, y: 356, width: 252, height: 34),
                              parent: geometryContent) {
            [weak self] in self?.resizeCanvas()
        }

        panelFieldLabel("Canvas background", x: 0, y: 414, parent: geometryContent)
        backgroundMode = NSPopUpButton()
        backgroundMode.addItems(withTitles: ["Solid", "Transparent"])
        backgroundMode.frame = NSRect(x: 0, y: 440, width: 252, height: 30)
        backgroundMode.setAccessibilityLabel("Canvas background mode")
        backgroundMode.target = self; backgroundMode.action = #selector(changeBackgroundMode)
        geometryContent.addSubview(backgroundMode)
        configure(backgroundColor, frame: NSRect(x: 0, y: 482, width: 252, height: 30),
                  label: "Canvas background color", parent: geometryContent)
        backgroundColor.placeholderString = "#RRGGBB or #RRGGBBAA"
        backgroundColor.formatter = nil
        backgroundApply = button("Apply background", frame: NSRect(x: 0, y: 526, width: 146, height: 34),
                                 parent: geometryContent) { [weak self] in self?.applyBackground() }
        backgroundReset = button("Reset fields", frame: NSRect(x: 156, y: 526, width: 96, height: 34),
                                 parent: geometryContent) { [weak self] in
            self?.publishBackgroundFields(); self?.updateControls()
        }
        panelLabel("Changes canvas fill, not an image layer’s background.",
                   frame: NSRect(x: 0, y: 574, width: 252, height: 42), muted: true,
                   parent: geometryContent)
        trimButton = button("Trim edges", frame: NSRect(x: 0, y: 632, width: 252, height: 34),
                            parent: geometryContent) { [weak self] in
            self?.command(["operation": "trim_canvas"], message: "Trimming canvas…", resetCrop: true)
        }
        panelLabel("Fits visible layer bounds, including off-canvas content. Does not trim transparent pixels within images.",
                   frame: NSRect(x: 0, y: 680, width: 252, height: 66), muted: true,
                   parent: geometryContent)

        buildLayersPanel()
        buildDrawPanel()
        buildOutputPanel()

        undoButton = button("Undo", frame: NSRect(x: 688, y: 470, width: 128, height: 34)) {
            [weak self] in self?.command(["operation": "undo"], message: "Undoing…")
        }
        undoButton.keyEquivalent = "z"; undoButton.keyEquivalentModifierMask = .command
        redoButton = button("Redo", frame: NSRect(x: 832, y: 470, width: 128, height: 34)) {
            [weak self] in self?.command(["operation": "redo"], message: "Redoing…")
        }
        redoButton.keyEquivalent = "Z"; redoButton.keyEquivalentModifierMask = [.command, .shift]
        saveButton = button("Save draft", frame: NSRect(x: 688, y: 524, width: 128, height: 34)) {
            [weak self] in self?.saveDraft()
        }
        saveButton.primary = true
        discardButton = button("Discard edits…", frame: NSRect(x: 824, y: 524, width: 136, height: 34)) {
            [weak self] in self?.confirmDiscard()
        }
        status.frame = NSRect(x: 688, y: 568, width: 272, height: 96)
        status.maximumNumberOfLines = 5; status.setAccessibilityLabel("Screenshot editor status")
        root.addSubview(status)
        fields = [cropX, cropY, cropWidth, cropHeight, canvasWidth, canvasHeight]
    }

    private func buildDrawPanel() {
        let scroll = NSScrollView(frame: drawPanel.bounds)
        scroll.autoresizingMask = [.width, .height]
        scroll.hasVerticalScroller = true; scroll.drawsBackground = false
        let content = Surface(frame: NSRect(x: 0, y: 0, width: 252, height: 390))
        scroll.documentView = content; drawPanel.addSubview(scroll)
        panelLabel("Draw", frame: NSRect(x: 0, y: 0, width: 272, height: 24),
                   size: 16, weight: .semibold, parent: content)
        panelLabel("Draw annotations, or click with Wand to remove pixels from an image.",
                   frame: NSRect(x: 0, y: 28, width: 252, height: 42), muted: true,
                   parent: content)
        panelFieldLabel("Tool", x: 0, y: 78, parent: content)
        drawTool = NSPopUpButton()
        drawTool.addItems(withTitles: ["Rectangle", "Ellipse", "Line", "Arrow", "Pen", "Wand",
                                           "Erase", "Restore"])
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
                                frame: NSRect(x: 0, y: 278, width: 252, height: 72), muted: true,
                                parent: content)
        publishDrawToolControls()
    }

    private func buildOutputPanel() {
        let scroll = NSScrollView(frame: outputPanel.bounds)
        scroll.autoresizingMask = [.width, .height]
        scroll.hasVerticalScroller = true; scroll.drawsBackground = false
        outputContent.frame = NSRect(x: 0, y: 0, width: 252, height: 628)
        scroll.documentView = outputContent
        outputPanel.addSubview(scroll)

        panelLabel("Output preview", frame: NSRect(x: 0, y: 0, width: 252, height: 24),
                   size: 16, weight: .semibold, parent: outputContent)
        panelLabel("Encode without saving or changing the draft.",
                   frame: NSRect(x: 0, y: 28, width: 252, height: 22), muted: true,
                   parent: outputContent)

        panelFieldLabel("Format", x: 0, y: 58, parent: outputContent)
        outputFormat = NSPopUpButton(frame: NSRect(x: 0, y: 78, width: 112, height: 30))
        outputFormat.addItems(withTitles: ["PNG", "JPEG", "WebP"])
        outputFormat.setAccessibilityLabel("Output format")
        outputFormat.target = self; outputFormat.action = #selector(outputOptionsChanged)
        outputContent.addSubview(outputFormat)

        panelFieldLabel("Quality mode", x: 120, y: 58, parent: outputContent)
        outputQuality = NSPopUpButton(frame: NSRect(x: 120, y: 78, width: 132, height: 30))
        outputQuality.addItems(withTitles: ["Preserve", "Compress", "Maximum file size"])
        outputQuality.setAccessibilityLabel("Output quality mode")
        outputQuality.target = self; outputQuality.action = #selector(outputOptionsChanged)
        outputContent.addSubview(outputQuality)

        outputQualityValueLabel = panelFieldLabel("Quality value", x: 0, y: 118,
                                                  parent: outputContent)
        outputPngPaletteLabel = panelFieldLabel("PNG palette", x: 132, y: 118,
                                                parent: outputContent)
        configure(outputQualityValue, frame: NSRect(x: 0, y: 138, width: 120, height: 30),
                  label: "Output quality value", parent: outputContent)
        configure(outputPngPalette, frame: NSRect(x: 132, y: 138, width: 120, height: 30),
                  label: "PNG maximum colors", parent: outputContent)
        outputQualityValue.stringValue = "98"
        outputPngPalette.placeholderString = "Optional"

        outputByteBudgetLabel = panelFieldLabel("Byte budget (minimum 10,000)", x: 0, y: 178,
                                               parent: outputContent)
        configure(outputByteBudget, frame: NSRect(x: 0, y: 198, width: 252, height: 30),
                  label: "Output byte budget", parent: outputContent)
        outputByteBudget.placeholderString = "Required for Maximum"
        outputByteBudget.stringValue = "10000000"
        [outputQualityValue, outputPngPalette, outputByteBudget].forEach {
            $0.formatter = outputIntegerFormatter; $0.delegate = self
        }

        previewOutputButton = button("Preview output", frame: NSRect(x: 0, y: 240, width: 140, height: 34),
                                     parent: outputContent) { [weak self] in self?.previewOutput() }
        previewOutputButton.primary = true
        copyImageButton = button("Copy image", frame: NSRect(x: 152, y: 240, width: 100, height: 34),
                                 parent: outputContent) { [weak self] in self?.copyEditedImage() }
        copyImageButton.setAccessibilityLabel("Copy edited screenshot")
        copyImageButton.toolTip = "Copy full-resolution edited pixels as PNG. Export options are ignored; no file or draft is saved."
        outputPreviewMode = NSSegmentedControl(labels: ["Edited canvas", "Encoded output"],
                                               trackingMode: .selectOne, target: self,
                                               action: #selector(changeOutputPreview))
        outputPreviewMode.frame = NSRect(x: 0, y: 286, width: 252, height: 28)
        outputPreviewMode.selectedSegment = 0
        outputPreviewMode.setAccessibilityLabel("Output preview image")
        outputContent.addSubview(outputPreviewMode)
        outputSize.frame = NSRect(x: 0, y: 326, width: 252, height: 58)
        outputSize.maximumNumberOfLines = 3
        outputSize.setAccessibilityLabel("Encoded output size")
        outputContent.addSubview(outputSize)

        panelLabel("Save new copy", frame: NSRect(x: 0, y: 402, width: 252, height: 24),
                   size: 16, weight: .semibold, parent: outputContent)
        panelLabel("Publish a new file and History item. Existing files are never replaced.",
                   frame: NSRect(x: 0, y: 430, width: 252, height: 38), muted: true,
                   parent: outputContent)
        panelFieldLabel("Filename", x: 0, y: 474, parent: outputContent)
        outputFilename.frame = NSRect(x: 0, y: 494, width: 252, height: 30)
        outputFilename.setAccessibilityLabel("Output filename")
        outputFilename.placeholderString = "Captures_…_edited.png"
        outputContent.addSubview(outputFilename)
        panelFieldLabel("Save location", x: 0, y: 532, parent: outputContent)
        outputLocation.frame = NSRect(x: 0, y: 552, width: 166, height: 24)
        outputLocation.lineBreakMode = .byTruncatingMiddle
        outputLocation.setAccessibilityLabel("Output save location")
        outputContent.addSubview(outputLocation)
        changeOutputDirectoryButton = button("Change…", frame: NSRect(x: 174, y: 548, width: 78, height: 30),
                                             parent: outputContent) {
            [weak self] in self?.chooseOutputDirectory()
        }
        saveNewCopyButton = button("Save new copy", frame: NSRect(x: 0, y: 584, width: 252, height: 34),
                                   parent: outputContent) { [weak self] in self?.saveNewCopy() }
        saveNewCopyButton.primary = true
        updateOutputOptionControls()
    }

    private func buildLayersPanel() {
        let panelScroll = NSScrollView(frame: layersPanel.bounds)
        panelScroll.autoresizingMask = [.width, .height]
        panelScroll.hasVerticalScroller = true; panelScroll.scrollerStyle = .overlay
        panelScroll.drawsBackground = false
        layerContent.frame = NSRect(x: 0, y: 0, width: 272, height: 550)
        panelScroll.documentView = layerContent; layersPanel.addSubview(panelScroll)

        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 272, height: 106))
        scroll.hasVerticalScroller = true; scroll.drawsBackground = false
        layerTable = NSTableView(frame: scroll.bounds)
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("editor-layer"))
        column.width = 252; layerTable.addTableColumn(column); layerTable.headerView = nil
        layerTable.rowHeight = 32; layerTable.dataSource = self; layerTable.delegate = self
        layerTable.allowsEmptySelection = true; layerTable.setAccessibilityLabel("Screenshot layers")
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
        importImageButton = button("Add image…", frame: NSRect(x: 0, y: 494, width: 272, height: 34),
                                   parent: layerContent) { [weak self] in self?.chooseImage() }
        importImageButton.primary = true
        annotationControls = EditorAnnotationControls(tokens: tokens, formatter: editorNumberFormatter)
        annotationControls.apply = { [weak self] patch in
            guard let self, !self.state.busy, let layer = self.selectedLayer else { return }
            self.layerCommand(layer, edit: ["action": "annotation_style", "patch": patch],
                              message: "Applying annotation style…")
        }
        annotationControls.reportError = { [weak self] message in self?.showError(message) }
        annotationControls.resized = { [weak self] height in
            self?.layerContent.frame.size.height = 550 + height
        }
        layerContent.addSubview(annotationControls)
    }

    @objc private func changeSection() {
        cancelDrawing()
        cancelViewportPan()
        geometryPanel.isHidden = sectionControl.selectedSegment != Section.geometry
        layersPanel.isHidden = sectionControl.selectedSegment != Section.layers
        drawPanel.isHidden = sectionControl.selectedSegment != Section.draw
        outputPanel.isHidden = sectionControl.selectedSegment != Section.output
        if sectionControl.selectedSegment == Section.output {
            changeOutputPreview()
        } else {
            preview.image = editedImage
            if let editedImage { viewportCanvasSize = editedImage.size }
            updateViewportGeometry()
        }
        updateDrawing()
    }

    @objc private func changeDrawTool() {
        cancelDrawing()
        drawOverlay.shape = EditorDrawOverlay.Shape.allCases[drawTool.indexOfSelectedItem]
        publishDrawToolControls()
    }

    private func publishDrawToolControls() {
        guard let drawTool, drawTool.indexOfSelectedItem >= 0 else { return }
        let shape = EditorDrawOverlay.Shape.allCases[drawTool.indexOfSelectedItem]
        let wand = shape == .wand
        let brush = shape.isBackgroundBrush
        wandToleranceLabel?.isHidden = !wand
        wandTolerance.isHidden = !wand; wandContiguous.isHidden = !wand
        brushSizeLabel?.isHidden = !brush; brushSize.isHidden = !brush
        brushSoftnessLabel?.isHidden = !brush; brushSoftness.isHidden = !brush
        drawHelper.stringValue = wand
            ? "Wand removes matching pixels from the frontmost visible image."
            : brush
                ? "The outline previews brush size and path only. Pixels apply on release."
                : "This tool creates one annotation layer on release."
    }

    @objc private func outputOptionsChanged() {
        normalizeOutputQuality()
        updateOutputFilenameExtension()
        invalidateOutput(optionsChanged: true)
        updateOutputOptionControls()
        updateControls()
    }

    func controlTextDidChange(_ notification: Notification) {
        guard let field = notification.object as? NSTextField else { return }
        if field === brushSize {
            if let value = Double(field.stringValue), value.isFinite {
                drawOverlay.brushDiameter = CGFloat(min(120, max(4, value)))
            }
            return
        }
        guard
              [outputQualityValue, outputPngPalette, outputByteBudget].contains(where: { $0 === field })
        else { return }
        invalidateOutput(optionsChanged: true)
        updateControls()
    }

    @objc private func changeOutputPreview() {
        if outputPreviewMode.selectedSegment == 1, let encodedOutput {
            preview.image = NSImage(cgImage: encodedOutput.image,
                                    size: NSSize(width: encodedOutput.image.width,
                                                 height: encodedOutput.image.height))
            viewportCanvasSize = NSSize(width: encodedOutput.image.width,
                                        height: encodedOutput.image.height)
        } else {
            outputPreviewMode.selectedSegment = 0
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
                self.outputSize.stringValue = "Exact encoded size: \(self.formatInteger(output.length)) bytes"
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
                } else {
                    self.showError("Couldn’t copy the edited image. The clipboard is unavailable; try again.")
                }
            case .failure(let error):
                guard self.state.fail(generation: generation) else { return }
                self.showError("Couldn’t copy the edited image: \(error.localizedDescription)")
            }
            self.updateControls()
            self.submitPendingImportIfReady()
        }
    }

    private func chooseOutputDirectory() {
        guard let artifactID = state.artifactID else { return }
        let generation = state.generation
        let current = outputDirectory.isEmpty ? nil
            : URL(fileURLWithPath: outputDirectory, isDirectory: true)
        let completion: (URL?) -> Void = { [weak self] selected in
            DispatchQueue.main.async {
                guard let self, let selected,
                      self.state.generation == generation,
                      self.state.artifactID == artifactID else { return }
                self.outputDirectory = selected.path
                self.publishOutputLocation()
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

    private func saveNewCopy() {
        guard let artifactID = state.artifactID,
              let filename = normalizedOutputFilename(),
              let options = outputOptions(),
              let generation = state.beginCommand() else { return }
        outputFilename.stringValue = filename
        let destination = URL(fileURLWithPath: outputDirectory, isDirectory: true)
            .appendingPathComponent(filename, isDirectory: false).path
        let request: [String: Any] = [
            "history_root": historyRoot,
            "destination": destination,
            "options": options,
            "mode": captureMode,
        ]
        status.textColor = tokens.color("text-muted")
        status.stringValue = "Saving new copy…"; updateControls()
        worker.saveNew(request) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let saved):
                guard self.state.completeOutput(generation: generation, artifactID: artifactID) else { return }
                switch saved {
                case .saved(let path):
                    self.status.textColor = self.tokens.color("text-muted")
                    self.status.stringValue = "Saved new copy to \(path)"
                    self.didSaveCopy()
                case .savedWithoutHistory(let path, let warning):
                    self.showError("Saved new copy to \(path), but couldn’t add it to History: \(warning)")
                }
            case .failure(let error):
                guard self.state.fail(generation: generation) else { return }
                self.showError("Couldn’t save new copy: \(error.localizedDescription)")
            }
            self.updateControls()
            self.submitPendingImportIfReady()
        }
    }

    private func defaultOutputFilename(_ date: Date = Date()) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.timeZone = .current
        formatter.dateFormat = "yyyy-MM-dd_HH-mm-ss"
        return "Captures_\(formatter.string(from: date))_edited.\(outputExtension)"
    }

    private var outputExtension: String {
        ["png", "jpg", "webp"][max(0, min(2, outputFormat?.indexOfSelectedItem ?? 0))]
    }

    private func updateOutputFilenameExtension() {
        guard !outputFilename.stringValue.isEmpty else { return }
        let base = (outputFilename.stringValue as NSString).deletingPathExtension
        if !base.isEmpty { outputFilename.stringValue = "\(base).\(outputExtension)" }
    }

    private func normalizedOutputFilename() -> String? {
        let name = outputFilename.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, name != ".", name != "..",
              (name as NSString).lastPathComponent == name else {
            showError("Enter a filename without folders."); return nil
        }
        let base = (name as NSString).deletingPathExtension
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !base.isEmpty else { showError("Enter a filename."); return nil }
        return "\(base).\(outputExtension)"
    }

    private func publishOutputLocation() {
        outputLocation.stringValue = outputDirectory
        outputLocation.toolTip = outputDirectory
    }

    private func outputOptions() -> [String: Any]? {
        let formats = ["png", "jpeg", "webp"]
        let qualities = ["preserve", "compress", "maximum"]
        guard formats.indices.contains(outputFormat.indexOfSelectedItem),
              qualities.indices.contains(outputQuality.indexOfSelectedItem) else { return nil }
        let format = formats[outputFormat.indexOfSelectedItem]
        let quality = qualities[outputQuality.indexOfSelectedItem]
        let qualityValue: UInt64
        if quality == "compress" {
            let minimum: UInt64 = format == "jpeg" ? 40 : 1
            guard let value = outputInteger(outputQualityValue), (minimum...100).contains(value) else {
                showError("Output quality must be a whole number from \(minimum) through 100 for \(format.uppercased()).")
                return nil
            }
            qualityValue = value
        } else {
            qualityValue = 100
        }
        var png: [String: Any] = [:]
        if format == "png", quality == "compress", !outputPngPalette.stringValue.isEmpty {
            guard let colors = outputInteger(outputPngPalette), (1...256).contains(colors) else {
                showError("PNG palette size must be a whole number from 1 through 256."); return nil
            }
            png["max_colors"] = colors
        }
        var options: [String: Any] = [
            "format": format, "quality": quality, "quality_value": qualityValue, "png": png,
        ]
        if quality == "maximum" {
            guard let budget = outputInteger(outputByteBudget), budget >= 10_000 else {
                showError("Enter an output byte budget of at least 10,000."); return nil
            }
            options["max_size_bytes"] = budget
        }
        return options
    }

    private func invalidateOutput(optionsChanged: Bool = false) {
        let hadOutput = encodedOutput != nil
        encodedOutput = nil
        outputPreviewMode?.selectedSegment = 0
        preview.image = editedImage
        if let editedImage { viewportCanvasSize = editedImage.size; updateViewportGeometry() }
        if optionsChanged && hadOutput {
            outputSize.stringValue = "Options changed. Preview output again."
        } else if !optionsChanged {
            outputSize.stringValue = "No encoded preview yet."
        }
    }

    private func updateOutputOptionControls() {
        guard outputFormat != nil, outputQuality != nil else { return }
        let ready = state.snapshot != nil && !state.busy
        let compress = outputQuality.indexOfSelectedItem == 1
        let maximum = outputQuality.indexOfSelectedItem == 2
        let pngPalette = compress && outputFormat.indexOfSelectedItem == 0
        outputQualityValueLabel.isHidden = !compress; outputQualityValue.isHidden = !compress
        outputPngPaletteLabel.isHidden = !pngPalette; outputPngPalette.isHidden = !pngPalette
        outputByteBudgetLabel.isHidden = !maximum; outputByteBudget.isHidden = !maximum
        outputQualityValue.isEnabled = ready && compress
        outputPngPalette.isEnabled = ready && pngPalette
        outputByteBudget.isEnabled = ready && maximum
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
        let request: [String: Any]
        if shape == .pen {
            request = ["operation": "create_freehand_path", "points": points.map { ["x": $0.x, "y": $0.y] }]
        } else {
            request = ["operation": shape == .line || shape == .arrow ? "create_open_shape" : "create_closed_shape",
                       "shape": shape.rawValue,
                       "start": ["x": start.x, "y": start.y], "end": ["x": end.x, "y": end.y]]
        }
        command(request, message: "Drawing \(shape.rawValue)…", createdLayerExistingIDs: Set(layers.map(\.id)))
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
        guard let size = Double(brushSize.stringValue), size >= 4, size <= 120,
              size == size.rounded() else {
            showError("Brush diameter must be a whole number from 4 to 120."); return
        }
        guard let softness = Double(brushSoftness.stringValue), softness >= 0, softness <= 100,
              softness == softness.rounded() else {
            showError("Brush softness must be a whole number from 0 to 100."); return
        }
        brushSize.stringValue = String(Int(size)); brushSoftness.stringValue = String(Int(softness))
        drawOverlay.brushDiameter = CGFloat(size)
        command(["operation": "paint_image_background",
                 "points": points.map { ["x": $0.x, "y": $0.y] },
                 "size": size, "softness": softness, "mode": mode.rawValue],
                message: mode == .erase ? "Erasing image background…" : "Restoring image background…")
    }

    private func cancelDrawing() {
        drawOverlay.cancelGesture()
        selectionOverlay.cancelGesture()
    }

    private var fittedImageRect: NSRect {
        guard viewportCanvasSize.width > 0, viewportCanvasSize.height > 0 else { return .zero }
        let scale = min(viewportBounds.width / viewportCanvasSize.width,
                        viewportBounds.height / viewportCanvasSize.height)
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

    private func handleZoomShortcut(_ event: NSEvent) -> Bool {
        guard event.type == .keyDown, state.snapshot != nil,
              event.modifierFlags.contains(.command) || event.modifierFlags.contains(.control) else { return false }
        let key = event.charactersIgnoringModifiers ?? ""
        if key == "+" || key == "=" || event.keyCode == 24 || event.keyCode == 69 {
            scaleViewport(by: 1.25)
        } else if key == "-" || key == "_" || event.keyCode == 27 || event.keyCode == 78 {
            scaleViewport(by: 1 / 1.25)
        } else if key == "0" || event.keyCode == 82 {
            setViewportZoom(100)
        } else {
            return false
        }
        return true
    }

    @objc private func changeZoomPreset() {
        guard let value = zoomPreset.selectedItem?.representedObject as? Double else { return }
        if value == 0 { fitViewport() } else { setViewportZoom(value) }
    }

    private func publishZoomPreset() {
        let current = viewport.zoomPercent
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
    }
    private func updateViewportGeometry() {
        preview.frame = presentedImageRect
        publishZoomPreset()
        drawOverlay.needsDisplay = true; selectionOverlay.needsDisplay = true
    }

    private func updateDrawing() {
        let active = sectionControl?.selectedSegment == Section.draw
            && state.snapshot != nil && !state.busy
        drawTool?.isEnabled = state.snapshot != nil && !state.busy
        wandTolerance.isEnabled = active; wandContiguous.isEnabled = active
        brushSize.isEnabled = active; brushSoftness.isEnabled = active
        drawOverlay.drawingEnabled = active
        selectionOverlay.selectionEnabled = sectionControl?.selectedSegment == Section.layers
            && state.snapshot != nil && !state.busy && !importLoading
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

    private func duplicateLayer() {
        guard let layer = selectedLayer else { return }
        let newID = UUID().uuidString.lowercased()
        layerCommand(layer, edit: ["action": "duplicate", "new_id": newID],
                     message: "Duplicating layer…", preferredSelection: newID)
    }

    private func deleteLayer() {
        guard let layer = selectedLayer, !layer.locked else { return }
        layerCommand(layer, edit: ["action": "delete"], message: "Deleting layer…")
    }

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

    private func applyCrop() {
        guard let x = number(cropX), let y = number(cropY),
              let width = positive(cropWidth), let height = positive(cropHeight) else {
            showError("Crop values must be finite numbers with positive width and height."); return
        }
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
                         createdLayerExistingIDs: Set<String>? = nil) {
        guard let generation = state.beginCommand() else { return }
        invalidateOutput()
        preferredLayerID = preferredSelection
        status.stringValue = message; updateControls()
        worker.request(object) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let presentation):
                guard self.state.complete(presentation.snapshot, generation: generation) else { return }
                if let createdLayerExistingIDs {
                    self.preferredLayerID = presentation.snapshot.layers
                        .first { !createdLayerExistingIDs.contains($0.id) }?.id
                }
                self.publish(presentation, resetCrop: resetCrop)
                self.status.textColor = self.tokens.color("text-muted")
                self.status.stringValue = presentation.snapshot.unsavedChanges
                    ? "Unsaved changes." : presentation.snapshot.hasDraft ? "Draft saved." : "Original restored."
                if self.closeAfterCommand { self.closeAfterCommand = false; self.closeNow(); return }
            case .failure(let error):
                guard self.state.fail(generation: generation) else { return }
                self.preferredLayerID = nil
                self.closeAfterCommand = false
                self.showError("Editor action failed: \(error.localizedDescription)")
                self.publishSelectedLayerFields()
                self.publishBackgroundFields()
            }
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
        updateViewportGeometry()
        dimensions.stringValue = "\(format(snapshot.width)) × \(format(snapshot.height)) pixels"
        canvasWidth.stringValue = format(snapshot.width); canvasHeight.stringValue = format(snapshot.height)
        publishBackgroundFields()
        if resetCrop || cropWidth.stringValue.isEmpty {
            cropX.stringValue = "0"; cropY.stringValue = "0"
            cropWidth.stringValue = format(snapshot.width); cropHeight.stringValue = format(snapshot.height)
        }
        reconcileLayerSelection(snapshot.layers)
        window.title = snapshot.unsavedChanges ? "Edit screenshot — Unsaved" : "Edit screenshot"
    }

    private func closeNow() {
        cancelDrawing()
        cancelPendingImport()
        closeAfterCommand = false; selectedLayerID = nil; preferredLayerID = nil
        state.close(); editedImage = nil; invalidateOutput(); preview.image = nil
        cancelViewportPan()
        viewport = NativeEditorViewport(); viewportCanvasSize = .zero
        worker.close(); window.orderOut(nil); updateControls()
    }

    private func updateControls() {
        let ready = state.snapshot != nil && !state.busy
        fields.forEach { $0.isEnabled = ready }
        applyCropButton?.isEnabled = ready; resizeButton?.isEnabled = ready
        trimButton?.isEnabled = ready
        backgroundMode?.isEnabled = ready
        backgroundColor.isEnabled = ready && backgroundMode?.indexOfSelectedItem == 0
        backgroundApply?.isEnabled = ready; backgroundReset?.isEnabled = ready
        undoButton?.isEnabled = ready && state.snapshot?.canUndo == true
        redoButton?.isEnabled = ready && state.snapshot?.canRedo == true
        saveButton?.isEnabled = ready && state.snapshot?.unsavedChanges == true
        discardButton?.isEnabled = ready && (state.snapshot?.hasDraft == true || state.snapshot?.unsavedChanges == true)
        sectionControl?.isEnabled = ready
        outputFormat?.isEnabled = ready; outputQuality?.isEnabled = ready
        previewOutputButton?.isEnabled = ready
        copyImageButton?.isEnabled = ready
        outputFilename.isEnabled = ready
        changeOutputDirectoryButton?.isEnabled = ready
        saveNewCopyButton?.isEnabled = ready && !outputDirectory.isEmpty
        outputPreviewMode?.isEnabled = ready && encodedOutput != nil
        viewportButtons.forEach { $0.isEnabled = ready }
        zoomPreset.isEnabled = ready
        updateOutputOptionControls()
        updateDrawing()
        layerTable?.isEnabled = ready
        importImageButton?.isEnabled = ready && !importLoading
        annotationControls?.setReady(ready)
        let layer = ready ? selectedLayer : nil
        let image = layer?.kind == .image
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
        let title = NSTextField(labelWithString: layer.name)
        title.lineBreakMode = .byTruncatingTail; title.toolTip = layer.name
        title.textColor = tokens.color("text")
        let detail = NSTextField(labelWithString:
            "\(layer.kind.rawValue.capitalized)\(layer.visible ? "" : " · Hidden")\(layer.locked ? " · Locked" : "")")
        detail.alignment = .right; detail.font = .systemFont(ofSize: 10)
        detail.textColor = tokens.color("text-muted"); detail.toolTip = detail.stringValue
        return EditorLayerCell(title: title, detail: detail)
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
        status.textColor = tokens.color("text-muted"); dimensions.textColor = tokens.color("text-muted")
        outputSize.textColor = tokens.color("text-muted")
        drawOverlay.fillColor = tokens.color("theme-accent").withAlphaComponent(0.22)
        drawOverlay.strokeColor = tokens.color("theme-accent")
        drawOverlay.brushOutlineColor = tokens.color("text")
        selectionOverlay.strokeColor = tokens.color("theme-accent")
        drawOverlay.needsDisplay = true
        preview.superview?.layer?.backgroundColor = tokens.color("surface-sunken").cgColor
        preview.superview?.layer?.borderColor = tokens.color("border").cgColor
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
