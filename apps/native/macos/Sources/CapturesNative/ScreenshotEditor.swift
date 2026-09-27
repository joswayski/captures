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

    /// A background autosave's snapshot: same document, now in the draft.
    /// Ignored once a command is running or the session changed.
    mutating func autosaved(_ value: NativeEditorSnapshot, generation: Int) -> Bool {
        guard self.generation == generation, !busy, artifactID == value.artifactID,
              snapshot != nil else { return false }
        snapshot = value; return true
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

/// Shipping `.screenshot-layer-list li`: a grip, a live preview over the
/// transparency checkerboard, the layer name over its muted kind, and eye,
/// lock and ⋯ quick actions. Hidden layers fade; locked rows dim the grip.
/// The selected row takes the selected surface with an accent border.
private final class EditorLayerCell: NSTableCellView {
    let titleLabel: NSTextField
    let detailLabel: NSTextField
    /// Shipping `.screenshot-layer-copy input`, shown while renaming.
    let renameField: NSTextField
    let iconName: String
    let thumbnail: NSImage?
    let coversPreview: Bool
    let layerLocked: Bool
    let layerVisible: Bool
    var rowSelected = false { didSet { needsDisplay = true } }
    let visibilityButton: CaptureButton
    let lockButton: CaptureButton
    let menuButton: CaptureButton
    private let tokens: Tokens

    init(title: NSTextField, detail: NSTextField, rename: NSTextField, iconName: String,
         thumbnail: NSImage?, coversPreview: Bool, locked: Bool, visible: Bool, tokens: Tokens,
         visibility: CaptureButton, lock: CaptureButton, menu: CaptureButton) {
        titleLabel = title; detailLabel = detail; renameField = rename
        self.iconName = iconName; self.thumbnail = thumbnail; self.coversPreview = coversPreview
        layerLocked = locked; layerVisible = visible; self.tokens = tokens
        visibilityButton = visibility; lockButton = lock; menuButton = menu
        super.init(frame: .zero)
        let views: [NSView] = [title, detail, rename, visibility, lock, menu]
        views.forEach { addSubview($0) }
        textField = title
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override var isFlipped: Bool { true }

    /// `.screenshot-layer-preview`: 44×32 after the 12pt grip column.
    var previewRect: NSRect {
        NSRect(x: 6 + 12 + 8, y: floor((bounds.height - 32) / 2), width: 44, height: 32)
    }

    override func layout() {
        super.layout()
        let trailingEdge = min(bounds.maxX, visibleRect.maxX) - 6
        let action = NSSize(width: 25, height: 28)
        let top = floor((bounds.height - action.height) / 2)
        menuButton.frame = NSRect(x: trailingEdge - action.width, y: top,
                                  width: action.width, height: action.height)
        lockButton.frame = menuButton.frame.offsetBy(dx: -(action.width + 2), dy: 0)
        visibilityButton.frame = lockButton.frame.offsetBy(dx: -(action.width + 2), dy: 0)
        let copyLeft = previewRect.maxX + 8
        let copyWidth = max(0, visibilityButton.frame.minX - 4 - copyLeft)
        titleLabel.frame = NSRect(x: copyLeft, y: bounds.midY - 18, width: copyWidth, height: 17)
        renameField.frame = NSRect(x: copyLeft, y: bounds.midY - 21, width: copyWidth, height: 22)
        detailLabel.frame = NSRect(x: copyLeft, y: bounds.midY + 2, width: copyWidth, height: 15)
    }

    override func draw(_ dirtyRect: NSRect) {
        let radius = tokens.number("r-lg")
        let row = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: radius, yRadius: radius)
        if rowSelected {
            tokens.color("surface-selected").setFill(); row.fill()
            tokens.color("theme-accent").withAlphaComponent(0.4).setStroke(); row.lineWidth = 1; row.stroke()
        }
        super.draw(dirtyRect)
        tokens.color("text-subtle").withAlphaComponent(layerLocked ? 0.35 : 0.5).setStroke()
        ShippingIcons.stroke("grip", in: NSRect(x: 3, y: floor((bounds.height - 18) / 2), width: 18, height: 18))
        let preview = previewRect
        let fade: CGFloat = layerVisible ? 1 : 0.42
        NSGraphicsContext.saveGraphicsState()
        let frame = NSBezierPath(roundedRect: preview, xRadius: tokens.number("r-sm"), yRadius: tokens.number("r-sm"))
        frame.addClip()
        tokens.color("canvas-checker-b").setFill(); preview.fill()
        tokens.color("canvas-checker-a").setFill()
        let tile: CGFloat = 5
        var tileY = preview.minY
        var rowIndex = 0
        while tileY < preview.maxY {
            var tileX = preview.minX + (rowIndex % 2 == 0 ? 0 : tile)
            while tileX < preview.maxX {
                NSRect(x: tileX, y: tileY, width: tile, height: tile).fill()
                tileX += 2 * tile
            }
            tileY += tile; rowIndex += 1
        }
        if let thumbnail, thumbnail.size.width > 0, thumbnail.size.height > 0 {
            let widthScale = preview.width / thumbnail.size.width
            let heightScale = preview.height / thumbnail.size.height
            let scale = coversPreview ? max(widthScale, heightScale) : min(widthScale, heightScale)
            let size = NSSize(width: thumbnail.size.width * scale, height: thumbnail.size.height * scale)
            let target = NSRect(x: preview.midX - size.width / 2, y: preview.midY - size.height / 2,
                                width: size.width, height: size.height)
            thumbnail.draw(in: target, from: .zero, operation: .sourceOver, fraction: fade,
                           respectFlipped: true, hints: nil)
        } else {
            tokens.color("text-muted").withAlphaComponent(fade).setStroke()
            ShippingIcons.stroke(iconName, in: NSRect(x: preview.midX - 9, y: preview.midY - 9, width: 18, height: 18))
        }
        NSGraphicsContext.restoreGraphicsState()
        tokens.color("border").setStroke()
        frame.lineWidth = 1; frame.stroke()
    }
}

/// Decoded shared-session row previews, keyed by their data URL so an
/// unchanged layer reuses its image across snapshots.
enum EditorLayerThumbnails {
    private static var cache: [String: NSImage] = [:]

    static func image(_ dataURL: String?) -> NSImage? {
        guard let dataURL else { return nil }
        if let cached = cache[dataURL] { return cached }
        let prefix = "data:image/png;base64,"
        guard dataURL.hasPrefix(prefix),
              let data = Data(base64Encoded: String(dataURL.dropFirst(prefix.count))),
              let image = NSImage(data: data) else { return nil }
        if cache.count > 256 { cache.removeAll() }
        cache[dataURL] = image
        return image
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

    /// Image file drops onto the canvas. Every visible canvas gesture view
    /// forwards to one controller handler with points in its own coordinates.
    enum FileDropPhase { case hover, exit, drop }
    var fileDropHandler: ((FileDropPhase, [URL], NSPoint) -> Bool)? {
        didSet { if fileDropHandler != nil { registerForDraggedTypes([.fileURL]) } else { unregisterDraggedTypes() } }
    }
    private func draggedFileURLs(_ sender: NSDraggingInfo) -> [URL] {
        sender.draggingPasteboard.readObjects(forClasses: [NSURL.self],
            options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
    }
    private func fileDrag(_ sender: NSDraggingInfo) -> NSDragOperation {
        let urls = draggedFileURLs(sender)
        guard !urls.isEmpty, let fileDropHandler,
              fileDropHandler(.hover, urls, convert(sender.draggingLocation, from: nil)) else { return [] }
        return .copy
    }
    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation { fileDrag(sender) }
    override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation { fileDrag(sender) }
    override func draggingExited(_ sender: NSDraggingInfo?) { _ = fileDropHandler?(.exit, [], .zero) }
    override func prepareForDragOperation(_ sender: NSDraggingInfo) -> Bool { fileDropHandler != nil }
    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        let urls = draggedFileURLs(sender)
        guard !urls.isEmpty, let fileDropHandler else { return false }
        return fileDropHandler(.drop, urls, convert(sender.draggingLocation, from: nil))
    }
    override func concludeDragOperation(_ sender: NSDraggingInfo?) { _ = fileDropHandler?(.exit, [], .zero) }

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
    /// Pointer hover in view coordinates, nil on exit (the Wand loupe).
    var onHover: ((NSPoint?) -> Void)?
    private var hoverTracking: NSTrackingArea?
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

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let hoverTracking { removeTrackingArea(hoverTracking) }
        let tracking = NSTrackingArea(rect: .zero,
            options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
            owner: self, userInfo: nil)
        addTrackingArea(tracking); hoverTracking = tracking
    }

    override func mouseMoved(with event: NSEvent) {
        super.mouseMoved(with: event)
        onHover?(convert(event.locationInWindow, from: nil))
    }

    override func mouseEntered(with event: NSEvent) {
        onHover?(convert(event.locationInWindow, from: nil))
    }

    override func mouseExited(with event: NSEvent) {
        onHover?(nil)
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
    var selectionEnabled = false {
        didSet { if !selectionEnabled { cancelGesture() }; isHidden = !selectionEnabled; layoutExpandButton() }
    }
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
    var dotFill = NSColor.windowBackgroundColor { didSet { needsDisplay = true } }
    var hintFill = NSColor(white: 0.08, alpha: 0.9) { didSet { needsDisplay = true } }
    var hintText = NSColor.white { didSet { needsDisplay = true } }
    var imageRect: (() -> NSRect)?
    var hitTestLayer: ((CGPoint, Double) throws -> String?)?
    var outlineForLayer: ((String) -> [CGPoint]?)?
    var onSelect: ((String?) -> Void)?
    var onMove: ((String, CGFloat, CGFloat, Double) -> Void)?
    var onRotate: ((String, Double) -> Void)?
    var onResize: ((String, String, CGPoint, Double, Bool) -> Void)?
    var onDoubleClick: ((CGPoint, Double) -> Bool)?
    var onError: ((Error) -> Void)?
    /// Curve dots for the selected visible, unlocked line/arrow.
    var curveHandles: NativeCurveHandles? { didSet { needsDisplay = true } }
    var curveHitTest: ((String, CGPoint, Double) -> NativeCurveHit?)?
    var curvePreviewer: ((String, [String: AnyHashable], CGPoint) -> NativeCurveHandles?)?
    var onCurve: ((String, [String: Any]) -> Void)?
    var hoverHintProvider: ((CGPoint, Double) -> String?)?
    /// The selected layer's overflow preview and its Expand canvas action.
    var expandPreview: NativeCanvasExpand? { didSet { layoutExpandButton(); needsDisplay = true } }
    var expandButton: CaptureButton? {
        didSet {
            oldValue?.removeFromSuperview()
            if let expandButton { addSubview(expandButton) }
            layoutExpandButton()
        }
    }
    private(set) var expandArmed = false { didSet { expandClock.update(running: expandArmed) } }
    /// Resolves the armed ghost's breathing and edge loops; nil holds them still.
    var motionTokens: Tokens?
    var reducedMotion: () -> Bool = { NativeMotion.reduceMotion } {
        didSet { expandClock.reducedMotion = reducedMotion }
    }
    private lazy var expandClock = NativeEdgeEffectClock(view: self)
    /// True while the armed Expand canvas ghost schedules redraws.
    var isExpandAnimating: Bool { expandClock.isAnimating }
    private(set) var hoverHint: String?
    private var hoverPoint: CGPoint?
    private var curveLayerID: String?
    private var curveHandle: [String: AnyHashable]?
    private(set) var curvePreview: NativeCurveHandles?
    private var hoverTracking: NSTrackingArea?
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
        layoutExpandButton()
    }
    override func updateTrackingAreas() {
        if let hoverTracking { removeTrackingArea(hoverTracking) }
        let area = NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited,
                                                         .activeInKeyWindow, .inVisibleRect],
                                  owner: self, userInfo: nil)
        addTrackingArea(area); hoverTracking = area
        super.updateTrackingAreas()
    }
    override func mouseMoved(with event: NSEvent) {
        updateHover(at: convert(event.locationInWindow, from: nil))
    }
    override func mouseExited(with event: NSEvent) { updateHover(at: nil) }
    /// Curve hints near the pointer and the armed Expand canvas ghost.
    func updateHover(at point: CGPoint?) {
        let armed = point.map { expandButton?.isHidden == false && expandButton?.frame.contains($0) == true } ?? false
        var hint: String?
        if let point, startPoint == nil, !armed, presentedImageRect.contains(point), canvasSize.width > 0 {
            let scale = presentedImageRect.width / canvasSize.width
            hint = hoverHintProvider?(canvasPoint(for: point), 10 / scale)
        }
        if armed != expandArmed || hint != hoverHint || (hint != nil && point != hoverPoint) {
            expandArmed = armed; hoverHint = hint; hoverPoint = point; needsDisplay = true
        }
    }
    /// Place the action outside the largest overflow gap, kept inside the view.
    func layoutExpandButton() {
        guard let expandButton else { return }
        guard let expandPreview, selectionEnabled, startPoint == nil, canvasSize.width > 0 else {
            expandButton.isHidden = true; expandArmed = false; return
        }
        let image = presentedImageRect, scale = image.width / canvasSize.width
        let anchor = expandPreview.anchor(inset: NativeEditorCanvas.expandInset / max(0.01, scale))
        let center = CGPoint(x: image.minX + anchor.x * scale, y: image.minY + anchor.y * scale)
        let size = expandButton.frame.size
        let inner = bounds.insetBy(dx: 4, dy: 4)
        guard inner.width >= size.width, inner.height >= size.height else { expandButton.isHidden = true; return }
        expandButton.frame.origin = CGPoint(
            x: min(max(center.x - size.width / 2, inner.minX), inner.maxX - size.width),
            y: min(max(center.y - size.height / 2, inner.minY), inner.maxY - size.height))
        expandButton.isHidden = false
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
            var result: (NativeEditorResizeDrag?, Int?) = (nil, nil)
            if resizeEnabled, let id = selectedLayerID, let documentJSON {
                result = try NativeEditorResizeDrag.begin(documentJSON: documentJSON, layerID: id,
                    point: documentPoint, displayScale: scale)
            }
            // Shipping priority: corner resize, then curve dots, then edge resize.
            let corner = result.1?.isMultiple(of: 2) ?? false
            if !corner, curveHandles != nil, let id = selectedLayerID,
               let hit = curveHitTest?(id, documentPoint, 10 / scale), let handle = hit.handle {
                curveLayerID = id; curveHandle = handle; curvePreview = curveHandles
                startPoint = point; currentPoint = point; hoverHint = nil
                expandButton?.isHidden = true; needsDisplay = true; return
            }
            if let drag = result.0, let handle = result.1 {
                resizeDrag = drag; resizeHandle = handle
                startPoint = point; currentPoint = point
                lockResizeAspect = snap && handle.isMultiple(of: 2)
                resizePreview = drag.preview(current: documentPoint, lockAspect: lockResizeAspect)
                needsDisplay = true; return
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
        if let id = curveLayerID, let handle = curveHandle {
            if hypot(point.x - startPoint.x, point.y - startPoint.y) >= 3 {
                curvePreview = curvePreviewer?(id, handle, canvasPoint(for: point)) ?? curvePreview
            }
        } else if rotatingLayerID != nil, let outline = rotationStartOutline {
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
        if let id = curveLayerID, let handle = curveHandle {
            let distance = hypot(point.x - start.x, point.y - start.y)
            let current = canvasPoint(for: point)
            cancelGesture()
            if distance >= 3 {
                onCurve?(id, ["kind": "move", "handle": handle as [String: Any],
                              "point": ["x": Double(current.x), "y": Double(current.y)]])
            }
            return
        }
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
        curveLayerID = nil; curveHandle = nil; curvePreview = nil
        layoutExpandButton()
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
        drawSelection()
        drawCanvasInteractions()
    }
    private func drawSelection() {
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
            // Lines/arrows keep corner grips only so curve dots stay easy to grab.
            for (index, point) in resizeHandlePoints.enumerated() where curveHandles == nil || index.isMultiple(of: 2) {
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
    /// Overflow tint and armed ghost, curve dots (live while dragging) and the
    /// floating curve hint.
    private func drawCanvasInteractions() {
        guard canvasSize.width > 0, canvasSize.height > 0 else { return }
        NSGraphicsContext.saveGraphicsState()
        defer { NSGraphicsContext.restoreGraphicsState() }
        NSBezierPath(rect: bounds).addClip()
        let image = presentedImageRect, scale = image.width / canvasSize.width
        let map: (CGPoint) -> CGPoint = { CGPoint(x: image.minX + $0.x * scale, y: image.minY + $0.y * scale) }
        let mapRect: (CGRect) -> CGRect = {
            CGRect(x: image.minX + $0.minX * scale, y: image.minY + $0.minY * scale,
                   width: $0.width * scale, height: $0.height * scale)
        }
        if let expandPreview, expandButton?.isHidden == false {
            for gap in expandPreview.gaps {
                let rect = mapRect(gap)
                strokeColor.withAlphaComponent(0.16).setFill(); NSBezierPath(rect: rect).fill()
                let border = NSBezierPath(rect: rect.insetBy(dx: 0.5, dy: 0.5))
                strokeColor.withAlphaComponent(0.5).setStroke(); border.lineWidth = 1; border.stroke()
            }
            if expandArmed {
                // `.screenshot-canvas-expand-ghost`: a breathing dashed outline
                // whose crossed sides are brighter, each with an accent edge.
                let reduced = reducedMotion()
                let elapsed = expandClock.elapsed
                let breathe = NativeEdgeEffects.loopOpacity("expand_ghost_breathe", at: elapsed,
                                                            tokens: motionTokens, reduced: reduced)
                let ghost = mapRect(expandPreview.rect)
                let sides: [(NativeCanvasExpand.Edge, CGPoint, CGPoint)] = [
                    (.top, CGPoint(x: ghost.minX, y: ghost.minY), CGPoint(x: ghost.maxX, y: ghost.minY)),
                    (.right, CGPoint(x: ghost.maxX, y: ghost.minY), CGPoint(x: ghost.maxX, y: ghost.maxY)),
                    (.bottom, CGPoint(x: ghost.maxX, y: ghost.maxY), CGPoint(x: ghost.minX, y: ghost.maxY)),
                    (.left, CGPoint(x: ghost.minX, y: ghost.maxY), CGPoint(x: ghost.minX, y: ghost.minY)),
                ]
                for (edge, from, to) in sides {
                    let crossed = expandPreview.edges.contains(edge)
                    let side = NSBezierPath(); side.move(to: from); side.line(to: to)
                    side.lineWidth = crossed ? 2 : 1.5; side.setLineDash([6, 4], count: 2, phase: 0)
                    strokeColor.withAlphaComponent((crossed ? 0.82 : 0.42) * breathe).setStroke(); side.stroke()
                }
                if let context = NSGraphicsContext.current?.cgContext {
                    for edge in expandPreview.edges {
                        NativeEdgeEffects.drawAccentEdge(context, target: ghost, edge: edge,
                            depth: NativeEditorPreviewPaint.snap("bloom", 96), overhang: 0, accent: strokeColor,
                            tokens: motionTokens, elapsed: elapsed, reduced: reduced)
                    }
                }
            }
        }
        if let handles = curvePreview ?? (startPoint == nil ? curveHandles : nil) {
            if curvePreview != nil, handles.path.count >= 2 {
                let path = NSBezierPath()
                for (index, point) in handles.path.enumerated() {
                    if index == 0 { path.move(to: map(point)) } else { path.line(to: map(point)) }
                }
                strokeColor.withAlphaComponent(0.85).setStroke(); path.lineWidth = 1.5; path.stroke()
            }
            func dot(_ point: CGPoint, radius: CGFloat, filled: Bool) {
                let center = map(point)
                let oval = NSBezierPath(ovalIn: NSRect(x: center.x - radius, y: center.y - radius,
                                                       width: radius * 2, height: radius * 2))
                (filled ? strokeColor : dotFill).setFill(); oval.fill()
                strokeColor.setStroke(); oval.lineWidth = filled ? 2 : 1.5; oval.stroke()
            }
            handles.starters.forEach { dot($0, radius: 4.5, filled: false) }
            handles.controls.forEach { dot($0, radius: 5, filled: true) }
            [handles.start, handles.end].forEach { dot($0, radius: 4.5, filled: false) }
        }
        if let hoverHint, let hoverPoint, startPoint == nil {
            let font = NSFont.systemFont(ofSize: 11)
            let text = hoverHint as NSString
            let size = text.size(withAttributes: [.font: font])
            let bubble = CGRect(x: hoverPoint.x + 14, y: hoverPoint.y + 18,
                                width: ceil(size.width) + 16, height: ceil(size.height) + 8)
            let plate = NSBezierPath(roundedRect: bubble, xRadius: 6, yRadius: 6)
            hintFill.setFill(); plate.fill()
            text.draw(at: CGPoint(x: bubble.minX + 8, y: bubble.minY + 4),
                      withAttributes: [.font: font, .foregroundColor: hintText])
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
    private var backgroundSolid: NSButton!
    private var backgroundSwatches: ColorSwatchRow!
    /// Shipping `lastSolid`: restored when Solid background is turned back on.
    private var lastSolidBackground = EditorColors.defaultCanvasBackground
    /// A live background change made while other work runs; only the latest
    /// is applied once the editor is free (one undo step).
    private var queuedBackground: String??
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
    private(set) var curveControls: EditorCurveControls!
    private var annotationControlsHeight: CGFloat = 0
    let dropGuideView = EditorDropGuideView()
    /// Shipping Trim edges hover/focus preview over the canvas.
    lazy var trimPreviewView = EditorTrimPreviewView(tokens: tokens)
    /// Trim edges is hovered or keyboard-focused.
    private(set) var trimHighlighted = false
    /// Shipping Wand colour loupe beside the crosshair.
    lazy var wandLoupeView = EditorWandLoupeView(tokens: tokens)
    /// One loupe sample in flight; the latest hover point waits behind it.
    private var wandLoupeInFlight = false
    private var wandLoupeQueued: (canvas: CGPoint, cursor: CGPoint)?
    private var wandLoupeGeneration = 0
    /// Shipping `DrawToolPreview` above the drawing and brush defaults.
    lazy var drawToolPreview = EditorDrawToolPreviewView(tokens: tokens)
    /// Base y of each draw control below the preview slot, before the preview
    /// pushes them down.
    private var drawControlBaseY: [ObjectIdentifier: CGFloat] = [:]
    /// Shipping `cta-pulse` halo behind Apply crop while a crop is staged.
    lazy var applyCropHalo = EditorCtaHalo(tokens: tokens)
    private(set) var expandCanvasButton: CaptureButton!
    /// Remaining files from one drop; each imports after the previous one.
    private var pendingDropURLs: [URL] = []
    private let rotationSnap = NSTextField()
    private var rotationSnapLabel: NSTextField!
    private let drawPanel = Surface()
    private let exportBar = Surface()
    private let exportSettingsPanel = Surface()
    let drawOverlay = EditorDrawOverlay()
    let selectionOverlay = EditorSelectionOverlay()
    let cropOverlay = EditorCropOverlay()
    /// Shipping `.screenshot-inline-text-frame`: the canvas text box, in the
    /// layer's own face, colour, plate and rotation (`updateInlineTextFrame`).
    private var inlineTextFrame: EditorInlineTextFrame!
    private let inlineTextEditor = EditorInlineTextView()
    /// Shipping image Width/Height/X/Y: live number fields.
    private let layerWidth = TokenNumberField()
    private let layerHeight = TokenNumberField()
    private let layerX = TokenNumberField()
    private let layerY = TokenNumberField()
    private var layerGeometryLabels: [NSTextField] = []
    private var layerGeometryHint: NSTextField!
    /// Properties for the selected layer, below the always-visible Layers list.
    private let layerPropertiesPanel = Surface()
    private let layerPropertiesHeading = NSTextField(labelWithString: "")
    private let layerPropertiesRule = Surface()
    private var layerListScroll: NSScrollView!
    /// Shipping `.screenshot-layer-menu-panel`, opened from a row's ⋯ button.
    private let layerMenuCard = Surface()
    private(set) var layerMenuID: String?
    private var layerMenuMonitor: Any?
    private let layerBlendMode = ClosurePopUpButton(frame: .zero, pullsDown: false)
    private let layerOpacity = TokenSlider(value: 100, minValue: 0, maxValue: 100, target: nil, action: nil)
    private let layerOpacityValue = NSTextField(labelWithString: "100%")
    private var layerMenuSections: [(title: NSTextField, views: [NSView])] = []
    private var layerMenuRules: [Surface] = []
    private let layerMenuFooter = Surface()
    private var bringFrontButton: CaptureButton!
    private var sendBackButton: CaptureButton!
    private var mergeDownButton: CaptureButton!
    private var mergeVisibleButton: CaptureButton!
    private var flattenButton: CaptureButton!
    /// The image layer whose name is being edited inline in its row.
    private(set) var renamingLayerID: String?
    /// Live inspector edits (typing, steppers, sliders, toggles) apply at once
    /// like shipping. Edits sharing a key fold into one undo step in the
    /// session; while another command runs, the newest per key waits here.
    private var liveQueue: [(key: String, request: [String: Any])] = []
    private var liveSerial = 0
    private let outputQualityValue = NSTextField()
    /// Maximum file size as a decimal value in `outputMaximumUnit` (KB/MB/GB).
    private let outputMaximumSize = NSTextField()
    private let outputMaximumUnit = ClosurePopUpButton()
    private var maximumUnit = RecordingFileSizeUnit.megabytes
    private var lastQualityIndex = 0
    private let outputCompressionPreset = ClosurePopUpButton()
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
    /// The drawing tool the rail (and its Shapes flyout or Eraser mode) chose.
    private var drawShape: EditorDrawOverlay.Shape = .rectangle
    /// Shipping `Eraser mode`: Wand, Erase or Restore under the Eraser tool.
    private var eraserMode: NSSegmentedControl!
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
    /// Shipping Eraser `RangeSlider`s: Wand Tolerance (the wand's own 0–255
    /// channel distance, stopped at 120), brush Size and Softness.
    private var wandTolerance: EditorMarkedSlider!
    private let wandContiguous = NSButton(checkboxWithTitle: EditorInspectorCopy.eraser("contiguous"),
                                          target: nil, action: nil)
    private var brushSize: EditorMarkedSlider!
    private var brushSoftness: EditorMarkedSlider!
    private var drawHelper: NSTextField!
    private let createTextPreset = NSPopUpButton()
    private let createTextSize = NSTextField()
    private var createTextColor: ColorSwatchRow!
    private var createTextControls: [NSView] = []
    private var createTextDefaultsPublished = false
    private let drawingStroke = NSButton(checkboxWithTitle: "Stroke", target: nil, action: nil)
    private let drawingFill = NSButton(checkboxWithTitle: "Fill", target: nil, action: nil)
    private var drawingStrokeColor: ColorSwatchRow!
    private var drawingFillColor: ColorSwatchRow!
    private var drawingStrokeColorLabel: NSTextField!
    private let drawingStrokeWidth = NSTextField()
    private let drawingOpacity = NSTextField()
    private var drawingDefaultControls: [NSView] = []
    /// Bottom of the closed-shape Fill color row (before the preview shift).
    private var drawingFillBottom: CGFloat = 0
    private var drawingFillControls: [NSView] = []
    private var drawingDefaultsArtifactID: String?
    private let drawingDropShadow = NSButton(checkboxWithTitle: "Drop shadow", target: nil, action: nil)
    private var drawingShadowControls: [NSView] = []
    private var drawingShadowFields: [String: NSTextField] = [:]
    private var drawingShadowLabels: [String: NSTextField] = [:]
    private var drawingShadowCustomized = false
    private let textEditor = NSTextView()
    private let textSize = NSTextField()
    private var textColor: ColorSwatchRow!
    private let textFamily = NSPopUpButton()
    private var textFormat: EditorTextFormatButtons!
    private let textPlate = NSPopUpButton()
    private var textPlateColor: ColorSwatchRow!
    private var textPlateColorLabel: NSTextField!
    /// Text property rows laid out top to bottom by `layoutTextControls`.
    private var textFamilyLabel: NSTextField!
    private var textContentLabel: NSTextField!
    private var textContentScroll: NSScrollView!
    private var textSizeLabel: NSTextField!
    private var textColorLabel: NSTextField!
    private let textShadow = NSButton(checkboxWithTitle: "Drop shadow", target: nil, action: nil)
    private let textOutline = NSButton(checkboxWithTitle: "Outline", target: nil, action: nil)
    private let textPreset = NSPopUpButton(frame: .zero, pullsDown: true)
    private var textPresetRounded: Bool?
    private let textShadowPanel = Surface()
    private var textShadowFields: [String: NSTextField] = [:]
    private let textShadowNumbers: [(String, KeyPath<NativeTextShadowStyle, Double>)] = [
        ("opacity", \.opacity), ("blur", \.blur), ("offsetX", \.offsetX), ("offsetY", \.offsetY)
    ]
    private var textControls: [NSView] = []
    private var textFieldsID: String?
    private var acceptedTextStyle: NativeTextStyle?
    private var textApplyPending = false
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
    /// The style last applied to the inline editor's text storage.
    private var inlineTextStyleKey: String?
    private var closeAfterTextInput = false
    private var outputFormat: NSPopUpButton!
    private var outputQuality: ClosurePopUpButton!
    private var outputSizeMode: NSPopUpButton!
    private var layerTable: EditorLayerTable!
    private var duplicateButton: CaptureButton!
    private var deleteButton: CaptureButton!
    private var rotateLeftButton: CaptureButton!
    private var rotateRightButton: CaptureButton!
    private var flipHorizontalButton: CaptureButton!
    private var flipVerticalButton: CaptureButton!
    private var undoButton: CaptureButton!
    private var redoButton: CaptureButton!
    private var applyCropButton: CaptureButton!
    private var drawCropButton: CaptureButton!
    private let cropAspect = NSPopUpButton()
    private var cropPrevious: [String]?
    private var trimButton: CaptureButton!
    private var exportDisclosure: CaptureButton!
    private var showComparisonButton: CaptureButton!
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
    /// Shipping's 700 ms draft autosave (`EditorChrome.metric("draft_autosave_ms")`).
    private var autosaveWork: DispatchWorkItem?
    /// Bumps with every published edit; a stale autosave result is ignored.
    private var autosaveSerial = 0
    static var autosaveDelay: TimeInterval {
        let milliseconds = EditorChrome.metric("draft_autosave_ms")
        return TimeInterval(milliseconds > 0 ? milliseconds : 700) / 1_000
    }
    private var selectedLayerID: String?
    private var selectedLayerIndex = 0
    private var preferredLayerID: String?
    private var reconcilingLayerSelection = false
    private var editedImage: NSImage?
    private var drawingPreviewEpoch = 0
    private var drawingPreviewInFlight = false
    private var drawingPreviewPending: [String: Any]?
    /// The automatic before/after comparison: the encoded After side for the
    /// current pixels and options, its pending encode and whether it is hidden.
    private var comparisonOutput: EditorOutputPresentation?
    private(set) var comparisonPending = false
    private var comparisonGeneration = 0
    private var comparisonWork: DispatchWorkItem?
    private var comparisonFailure: String?
    private(set) var comparisonDismissed = false
    private(set) var compareView: CompressionCompareView!
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
    /// Test seam for the Add images panel: reports every chosen file, or none.
    private let imagePicker: ((NSWindow, @escaping ([URL]) -> Void) -> Void)?
    private let imageDecoder: (URL) throws -> EditorDecodedImage
    private static let imageDecodeQueue = DispatchQueue(label: "es.captures.native.editor-image-decode",
                                                        qos: .userInitiated)
    private var importToken = 0
    private var importLoading = false
    private var pendingImport: (image: EditorDecodedImage, generation: Int, artifactID: String, point: CGPoint?)?

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
         imagePicker: ((NSWindow, @escaping ([URL]) -> Void) -> Void)? = nil,
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
        window.isReleasedWhenClosed = false; window.title = EditorWindowTitle.screenshot
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
        if state.artifactID != artifact.id, !liveQueue.isEmpty || inlineTextInput != nil {
            showError("Finish text input before opening another capture.")
            window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
            completion?(false)
            return
        }
        // Edits in the capture being replaced autosave first, as on close.
        if state.artifactID != artifact.id { flushDraft() }
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
        lastSolidBackground = EditorColors.defaultCanvasBackground; queuedBackground = nil
        self.historyRoot = historyRoot
        captureMode = artifact.mode
        self.outputDirectory = outputDirectory ?? URL(fileURLWithPath: historyRoot)
            .deletingLastPathComponent().path
        outputSizeMode?.selectItem(at: 0)
        outputWidth.stringValue = ""; outputHeight.stringValue = ""
        resetExportState(originalBytes: artifact.sizeBytes)
        selectedLayerID = nil; selectedLayerIndex = 0; preferredLayerID = nil
        editedImage = nil; invalidateOutput(); preview.image = nil; window.title = EditorWindowTitle.screenshot
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
                    ? "Draft restored." : "Ready. Changes save automatically as a draft."
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
            backgroundSwatches?.deactivate(); queuedBackground = nil
            state.close(); editedImage = nil; invalidateOutput(); preview.image = nil
            estimateWork?.cancel(); estimateWork = nil; estimateGeneration += 1
            comparisonWork?.cancel(); comparisonWork = nil; comparisonGeneration += 1
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
        guard !state.busy else {
            status.stringValue = "Wait for the current editor action to finish."
            return false
        }
        // Shipping closes without asking; `closeNow` flushes the draft first.
        closeNow()
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
        // Shipping `.screenshot-sidebar` rows: minmax(188px, 40%) for Layers,
        // the rest for the tool's Properties.
        let column = max(0, statusY - 14 - top - gap)
        let layersHeight = min(column, max(188, (column * 0.4).rounded()))
        layersPanel.frame = NSRect(x: inspectorX, y: top + gap, width: 272, height: layersHeight)
        for panel in [geometryPanel, layerPropertiesPanel, drawPanel] {
            panel.frame = NSRect(x: inspectorX, y: layersPanel.frame.maxY, width: 272,
                                 height: max(0, column - layersHeight))
        }
        if layerMenuID != nil { closeLayerMenu() }
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
    /// above 1040pt), the zoom group and Add images on the right. Drafts autosave.
    /// Controls are placed first; the Canvas toolbar takes what remains.
    private func layoutHeader() {
        guard let addImagesButton, let fitButton else { return }
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
        let addWidth = 2 * pad + 16 + tokens.number("s-3") + titleWidth(addImagesButton.title)
        addImagesButton.frame = NSRect(x: width - pad - addWidth, y: y,
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
            + (outputQuality?.indexOfSelectedItem == 2 ? ["maximum"] : [])
            + ["estimate"]
            + (outputQuality?.indexOfSelectedItem != 0 && comparisonDismissed ? ["comparison"] : [])
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
        drawOverlay.onHover = { [weak self] point in self?.wandHover(point) }
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
            guard let self else { return false }
            if self.curveDoubleClick(at: point, radius: tolerance * 10 / 8) { return true }
            return self.beginExistingTextInput(at: point, tolerance: tolerance)
        }
        selectionOverlay.curveHitTest = { [weak self] id, point, radius in
            guard let json = self?.state.snapshot?.documentJSON else { return nil }
            return try? NativeEditorCanvas.curveHit(documentJSON: json, layerID: id, point: point, radius: radius)
        }
        selectionOverlay.curvePreviewer = { [weak self] id, handle, point in
            guard let json = self?.state.snapshot?.documentJSON else { return nil }
            return try? NativeEditorCanvas.curvePreview(documentJSON: json, layerID: id, handle: handle, point: point)
        }
        selectionOverlay.onCurve = { [weak self] id, edit in self?.curveCanvasLayer(id, edit: edit) }
        selectionOverlay.hoverHintProvider = { [weak self] point, radius in
            self?.curveHoverHint(at: point, radius: radius)
        }
        expandCanvasButton = CaptureButton(NativeEditorCanvas.expandCanvas,
                                           frame: NSRect(x: 0, y: 0, width: 118, height: 28),
                                           tokens: tokens) { [weak self] in self?.expandSelectedCanvas() }
        expandCanvasButton.primary = true
        expandCanvasButton.setAccessibilityLabel(NativeEditorCanvas.expandCanvas)
        selectionOverlay.expandButton = expandCanvasButton
        selectionOverlay.onError = { [weak self] error in self?.showError("Layer interaction failed: \(error.localizedDescription)") }
        viewportInput.addSubview(selectionOverlay)
        cropOverlay.frame = viewportInput.bounds
        cropOverlay.autoresizingMask = [.width, .height]
        cropOverlay.setAccessibilityLabel("Screenshot crop canvas")
        cropOverlay.toolTip = "Drag to choose a crop. Hold Shift to lock the ratio. Escape cancels; Apply crop commits."
        cropOverlay.onChange = { [weak self] rect in self?.setCropFields(rect) }
        cropOverlay.onCancel = { [weak self] in self?.cancelCrop() }
        viewportInput.addSubview(cropOverlay)
        trimPreviewView.frame = viewportInput.bounds
        trimPreviewView.autoresizingMask = [.width, .height]
        trimPreviewView.imageRect = { [weak self] in self?.presentedImageRect ?? .zero }
        viewportInput.addSubview(trimPreviewView)
        // Above the canvas overlays: only its handle, bottom strip and Hide
        // take the pointer; every other press reaches the canvas below.
        compareView = CompressionCompareView(tokens: tokens)
        compareView.frame = viewportInput.bounds
        compareView.autoresizingMask = [.width, .height]
        compareView.isHidden = true
        compareView.onDismiss = { [weak self] in self?.dismissComparison() }
        viewportInput.addSubview(compareView)
        dropGuideView.frame = viewportInput.bounds
        dropGuideView.autoresizingMask = [.width, .height]
        dropGuideView.isHidden = true
        dropGuideView.imageRect = { [weak self] in self?.presentedImageRect ?? .zero }
        viewportInput.addSubview(dropGuideView)
        let inlineFrame = EditorInlineTextFrame(tokens: tokens)
        inlineFrame.isHidden = true
        inlineTextEditor.isRichText = false
        inlineTextEditor.drawsBackground = false
        inlineTextEditor.isHorizontallyResizable = false
        inlineTextEditor.isVerticallyResizable = false
        inlineTextEditor.allowsUndo = true
        inlineTextEditor.delegate = self
        inlineTextEditor.textContainerInset = .zero
        inlineTextEditor.textContainer?.lineFragmentPadding = 0
        inlineTextEditor.textContainer?.widthTracksTextView = false
        inlineTextEditor.focusRingType = .none
        inlineTextEditor.setAccessibilityLabel(EditorInspectorCopy.textFormat("inline_label"))
        // Shipping: Escape and clicking away commit; Return inserts a line.
        inlineTextEditor.onEscape = { [weak self] in self?.finishInlineTextInput(commit: true) }
        inlineTextEditor.onBlur = { [weak self] in self?.finishInlineTextInput(commit: true) }
        inlineFrame.addSubview(inlineTextEditor)
        viewportInput.addSubview(inlineFrame)
        inlineTextFrame = inlineFrame
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

        // Shipping sidebar: Layers always sits above the tool's Properties.
        layersPanel.frame = NSRect(x: 688, y: 66, width: 272, height: 188)
        geometryPanel.frame = NSRect(x: 688, y: 254, width: 272, height: 346)
        layerPropertiesPanel.frame = geometryPanel.frame; layerPropertiesPanel.isHidden = true
        drawPanel.frame = geometryPanel.frame; drawPanel.isHidden = true
        geometryPanel.setAccessibilityLabel("Geometry controls")
        layersPanel.setAccessibilityLabel("Layer controls")
        layerPropertiesPanel.setAccessibilityLabel("Layer properties")
        drawPanel.setAccessibilityLabel("Drawing controls")
        // layoutEditor() places the inspector above the export bar.
        root.addSubview(geometryPanel); root.addSubview(layersPanel)
        root.addSubview(layerPropertiesPanel); root.addSubview(drawPanel)

        let geometryScroll = NSScrollView(frame: geometryPanel.bounds)
        geometryScroll.autoresizingMask = [.width, .height]
        geometryScroll.hasVerticalScroller = true; geometryScroll.drawsBackground = false
        geometryScroll.useTokenScrollers(tokens)
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
        // Shipping `.screenshot-property-actions button.primary.cta-pulse`.
        applyCropButton.primary = true
        applyCropHalo.surround(applyCropButton)
        geometryContent.addSubview(applyCropHalo, positioned: .below, relativeTo: applyCropButton)

        buildLayersPanel()
        buildDrawPanel()

        status.frame = NSRect(x: 688, y: 524, width: 272, height: 96)
        status.maximumNumberOfLines = 5; status.setAccessibilityLabel("Screenshot editor status")
        root.addSubview(status)
        buildExportBar()
        // Floating chrome stays above the canvas, inspector and export bar.
        root.addSubview(backgroundCard)
        root.addSubview(layerMenuCard)
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
        // Hover or keyboard focus previews the cut on the canvas.
        trimButton.highlightChanged = { [weak self] _, highlighted in
            self?.trimHighlighted = highlighted
            self?.refreshTrimPreview()
        }
        backgroundButton.toolTip = EditorColors.text("background_tooltip")
        backgroundButton.swatch = tokens.color("surface-raised")

        // Shipping canvas background card: a Solid toggle and the compact swatch
        // row. Every change applies at once as its own undo step.
        let cardWidth = EditorColors.metric("menu_width")
        let cardPadding = tokens.number("s-5")
        let swatchWidth = cardWidth - 2 * cardPadding
        let swatchHeight = ColorSwatchRow.height(width: swatchWidth, compact: true, tokens: tokens)
        backgroundCard.frame = NSRect(x: 0, y: 0, width: cardWidth,
                                      height: 2 * cardPadding + 24 + tokens.number("s-4") + swatchHeight)
        backgroundCard.layer?.cornerRadius = tokens.number("r-xl")
        backgroundCard.layer?.borderWidth = 1
        backgroundCard.setAccessibilityLabel(EditorColors.text("canvas_background"))
        backgroundCard.isHidden = true
        backgroundSolid = NSButton(checkboxWithTitle: EditorColors.text("solid_background"),
                                   target: self, action: #selector(toggleSolidBackground))
        backgroundSolid.font = .systemFont(ofSize: tokens.number("text-sm"))
        backgroundSolid.frame = NSRect(x: cardPadding, y: cardPadding, width: swatchWidth, height: 24)
        backgroundSolid.setAccessibilityLabel(EditorColors.text("solid_background"))
        backgroundCard.addSubview(backgroundSolid)
        backgroundSwatches = ColorSwatchRow(tokens: tokens, label: EditorColors.text("canvas_background"),
                                            compact: true)
        backgroundSwatches.frame = NSRect(x: cardPadding, y: backgroundSolid.frame.maxY + tokens.number("s-4"),
                                          width: swatchWidth, height: swatchHeight)
        backgroundSwatches.changed = { [weak self] color in self?.setBackground(color) }
        backgroundCard.addSubview(backgroundSwatches)

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
        if backgroundCard.isHidden { backgroundSwatches.deactivate() }
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
        scroll.useTokenScrollers(tokens)
        let content = Surface(frame: NSRect(x: 0, y: 0, width: 252, height: 390))
        scroll.documentView = content; drawPanel.addSubview(scroll)
        drawHeading = panelLabel("Draw", frame: NSRect(x: 0, y: 0, width: 272, height: 24),
                   size: 16, weight: .semibold, parent: content)
        panelLabel("Draw annotations, or click with Wand to remove pixels from an image.",
                   frame: NSRect(x: 0, y: 28, width: 252, height: 42), muted: true,
                   parent: content)
        // The rail alone picks the tool. Eraser adds shipping's mode group.
        eraserMode = NSSegmentedControl(labels: ["Wand", "Erase", "Restore"], trackingMode: .selectOne,
                                        target: self, action: #selector(changeEraserMode))
        eraserMode.frame = NSRect(x: 0, y: 100, width: 252, height: 30)
        eraserMode.segmentDistribution = .fillEqually
        eraserMode.setAccessibilityLabel("Eraser mode")
        eraserMode.isHidden = true
        content.addSubview(eraserMode)
        // Shipping Eraser sliders (`editor_chrome::eraser`): Tolerance values
        // are the wand's 0–255 channel distance, stopped at 120 like shipping.
        let tolerance = EditorMarkedSlider(
            tokens: tokens, title: EditorInspectorCopy.eraser("tolerance"),
            accessibilityLabel: EditorInspectorCopy.eraser("tolerance_label"),
            range: EditorInspectorCopy.eraserRange("tolerance_range", fallback: 0...120), value: 36,
            marks: EditorInspectorCopy.eraserMarks("tolerance_marks")) { "\(Int($0))" }
        tolerance.frame = NSRect(x: 0, y: 146, width: 252, height: EditorMarkedSlider.height)
        content.addSubview(tolerance); wandTolerance = tolerance
        wandContiguous.frame = NSRect(x: 0, y: 216, width: 252, height: 24)
        wandContiguous.state = .on; wandContiguous.setAccessibilityLabel("Wand contiguous only")
        wandContiguous.target = self; wandContiguous.action = #selector(wandContiguousChanged)
        content.addSubview(wandContiguous)
        let size = EditorMarkedSlider(
            tokens: tokens, title: EditorInspectorCopy.eraser("size"),
            accessibilityLabel: EditorInspectorCopy.eraser("size_label"),
            range: EditorInspectorCopy.eraserRange("size_range", fallback: 4...120), value: 28,
            marks: EditorInspectorCopy.eraserMarks("size_marks")) { "\(Int($0)) px" }
        size.frame = NSRect(x: 0, y: 146, width: 252, height: EditorMarkedSlider.height)
        size.changed = { [weak self] value in
            self?.drawOverlay.brushDiameter = CGFloat(value)
            self?.refreshDrawToolPreview()
        }
        content.addSubview(size); brushSize = size
        let softness = EditorMarkedSlider(
            tokens: tokens, title: EditorInspectorCopy.eraser("softness"),
            accessibilityLabel: EditorInspectorCopy.eraser("softness_label"),
            range: EditorInspectorCopy.eraserRange("softness_range", fallback: 0...100), value: 18,
            marks: EditorInspectorCopy.eraserMarks("softness_marks")) { "\(Int($0))%" }
        softness.frame = NSRect(x: 0, y: 218, width: 252, height: EditorMarkedSlider.height)
        softness.changed = { [weak self] _ in self?.refreshDrawToolPreview() }
        content.addSubview(softness); brushSoftness = softness
        drawHelper = panelLabel("Other tools create one annotation layer on release.",
                                frame: NSRect(x: 0, y: 408, width: 252, height: 42), muted: true,
                                parent: content)
        buildDrawingDefaultControls(in: content)
        buildCreateTextControls(in: content)
        // Controls below the preview slot move down while it shows.
        for view in content.subviews where view !== drawHelper && view.frame.minY >= Self.drawPreviewTop {
            drawControlBaseY[ObjectIdentifier(view)] = view.frame.minY
        }
        drawToolPreview.frame = NSRect(x: 0, y: Self.drawPreviewTop, width: 252,
                                       height: EditorDrawToolPreviewView.height)
        drawToolPreview.isHidden = true
        content.addSubview(drawToolPreview)
        publishDrawToolControls()
    }

    /// Top of the `DrawToolPreview` slot, below the Eraser mode row.
    static let drawPreviewTop: CGFloat = 140
    /// The preview's height plus one row gap.
    private var drawPreviewShift: CGFloat { EditorDrawToolPreviewView.height + 12 }

    /// Height of a full-width shipping `ColorField` swatch grid in Properties.
    private var panelSwatchHeight: CGFloat { ColorSwatchRow.height(width: 252, compact: false, tokens: tokens) }

    /// A shipping `ColorField` in Properties: the legend, then the swatch row.
    private func panelColorField(_ legend: String, y: CGFloat, parent: NSView,
                                 changed: @escaping (String) -> Void) -> (NSTextField, ColorSwatchRow) {
        let label = panelFieldLabel(legend, x: 0, y: y, parent: parent)
        label.frame.size.width = 252
        label.setAccessibilityElement(false)
        let swatches = ColorSwatchRow(tokens: tokens, label: legend, compact: false)
        swatches.frame = NSRect(x: 0, y: y + 22, width: 252, height: panelSwatchHeight)
        swatches.changed = changed
        parent.addSubview(swatches)
        return (label, swatches)
    }

    private func buildDrawingDefaultControls(in content: NSView) {
        // Shipping order: Stroke color (Color for open tools), Size and
        // Opacity, the Stroke/Fill toggles, Fill color, then Drop shadow.
        let swatchRow = 22 + panelSwatchHeight + 8
        let (strokeColorLabel, strokeSwatches) = panelColorField(
            EditorColors.text("stroke_color"), y: 146, parent: content) { [weak self] _ in
            self?.updateDrawingPreviewStyle()
        }
        drawingStrokeColor = strokeSwatches; drawingStrokeColorLabel = strokeColorLabel
        let numbersY = 146 + swatchRow
        let widthLabel = panelFieldLabel("Width (2–40)", x: 0, y: numbersY, parent: content)
        let opacityLabel = panelFieldLabel("Opacity (0–100)", x: 132, y: numbersY, parent: content)
        configure(drawingStrokeWidth, frame: NSRect(x: 0, y: numbersY + 22, width: 120, height: 30),
                  label: "New drawing stroke width", parent: content)
        configure(drawingOpacity, frame: NSRect(x: 132, y: numbersY + 22, width: 120, height: 30),
                  label: "New drawing opacity", parent: content)
        [drawingStrokeWidth, drawingOpacity].forEach { $0.delegate = self }
        let togglesY = numbersY + 62
        drawingStroke.frame = NSRect(x: 0, y: togglesY, width: 120, height: 24)
        drawingFill.frame = NSRect(x: 132, y: togglesY, width: 120, height: 24)
        drawingStroke.setAccessibilityLabel("New drawing stroke")
        drawingFill.setAccessibilityLabel("New drawing fill")
        drawingStroke.target = self; drawingStroke.action = #selector(drawingDefaultsChanged(_:))
        drawingFill.target = self; drawingFill.action = #selector(drawingDefaultsChanged(_:))
        content.addSubview(drawingStroke); content.addSubview(drawingFill)
        let (fillColorLabel, fillSwatches) = panelColorField(
            EditorColors.text("fill_color"), y: togglesY + 32, parent: content) { [weak self] _ in
            self?.updateDrawingPreviewStyle()
        }
        drawingFillColor = fillSwatches
        drawingDefaultControls = [strokeColorLabel, fillColorLabel, strokeSwatches, fillSwatches,
                                  widthLabel, opacityLabel, drawingStrokeWidth, drawingOpacity,
                                  drawingStroke, drawingFill]
        drawingFillControls = [fillColorLabel, fillSwatches, drawingFill]
        drawingFillBottom = togglesY + 32 + swatchRow
        drawingDropShadow.frame = NSRect(x: 0, y: drawingFillBottom, width: 252, height: 24)
        drawingDropShadow.setAccessibilityLabel("New drawing drop shadow")
        drawingDropShadow.target = self; drawingDropShadow.action = #selector(drawingDefaultsChanged(_:))
        content.addSubview(drawingDropShadow)
        drawingDefaultControls.append(drawingDropShadow)
        for (index, item) in [("color", "Shadow color"), ("opacity", "Shadow opacity"),
                              ("blur", "Blur (0–100)"), ("offsetX", "X offset"),
                              ("offsetY", "Y offset")].enumerated() {
            let x = CGFloat(index % 2) * 132
            let y = CGFloat(index / 2) * 62 + drawingFillBottom + 34
            let label = panelFieldLabel(item.1, x: x, y: y, parent: content)
            let field = NSTextField()
            configure(field, frame: NSRect(x: x, y: y + 22, width: 120, height: 30),
                      label: "New drawing shadow \(item.0)", parent: content)
            field.formatter = nil; field.delegate = self
            drawingShadowFields[item.0] = field
            drawingShadowLabels[item.0] = label
            drawingShadowControls.append(contentsOf: [label, field])
        }
    }

    private func publishInitialDrawingDefaults(_ snapshot: NativeEditorSnapshot) {
        guard drawingDefaultsArtifactID != snapshot.artifactID, let style = snapshot.initialAnnotationStyle else { return }
        drawingDefaultsArtifactID = snapshot.artifactID
        drawingStrokeColor?.selectedHex = style.color
        drawingFillColor?.selectedHex = style.fill ?? style.color
        drawingStrokeWidth.stringValue = format(style.strokeWidth)
        drawingOpacity.stringValue = "100"
        drawingStroke.state = style.strokeEnabled ? .on : .off
        drawingFill.state = style.fill == nil ? .off : .on
        drawingDropShadow.state = style.dropShadow ? .on : .off
        drawingShadowCustomized = false
        drawingDefaultsChanged()
    }

    @objc private func drawingDefaultsChanged(_ sender: Any? = nil) {
        if sender as? NSButton === drawingFill, drawingFill.state == .on, let stroke = drawingStrokeColor {
            drawingFillColor?.selectedHex = stroke.selectedHex
        }
        updateDrawingPreviewStyle()
        publishDrawToolControls()
    }

    private func updateDrawingPreviewStyle() {
        if let color = drawingStrokeColor.flatMap({ NSColor(hex: $0.selectedHex) }) { drawOverlay.strokeColor = color }
        if let color = drawingFillColor.flatMap({ NSColor(hex: $0.selectedHex) }) { drawOverlay.fillColor = color }
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
        refreshDrawToolPreview()
    }

    private func buildCreateTextControls(in content: NSView) {
        let styleLabel = panelFieldLabel("Style", x: 0, y: 146, parent: content)
        createTextPreset.frame = NSRect(x: 0, y: 168, width: 252, height: 30)
        createTextPreset.setAccessibilityLabel("New text style")
        content.addSubview(createTextPreset)
        let sizeLabel = panelFieldLabel("Size (8–512)", x: 0, y: 208, parent: content)
        sizeLabel.frame.size.width = 118
        configure(createTextSize, frame: NSRect(x: 0, y: 230, width: 118, height: 30),
                  label: "New text size", parent: content)
        createTextSize.stringValue = format(24)
        // Shipping `ColorField label="Color"` for new text.
        let (colorLabel, colorSwatches) = panelColorField(EditorColors.text("color"), y: 268,
                                                          parent: content) { _ in }
        colorSwatches.selectedHex = "#ff3b5c"
        createTextColor = colorSwatches
        createTextControls = [styleLabel, createTextPreset, sizeLabel, colorLabel,
                              createTextSize, colorSwatches]
    }

    private func publishCreateTextDefaults() {
        let selectedID = createTextPreset.selectedItem?.representedObject as? String
        createTextPreset.removeAllItems()
        createTextPreset.addItem(withTitle: "Plain")
        for preset in state.snapshot?.textStylePresets ?? [] {
            createTextPreset.addItem(withTitle: preset.label)
            createTextPreset.lastItem?.representedObject = preset.id
            // Shipping `TextStylePicker` preview chip, also shown on the trigger.
            createTextPreset.lastItem?.image = TextStyleChip.image(for: preset, tokens: tokens)
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

    /// Selected text properties under Select (shipping `selected?.kind ===
    /// "text"`). Every change applies live; typing in one field is one undo step.
    private func buildTextControls(in content: NSView) {
        textPreset.frame = NSRect(x: 0, y: 322, width: 252, height: 30)
        textPreset.setAccessibilityLabel("Text style preset")
        textPreset.target = self; textPreset.action = #selector(stageTextPreset)
        content.addSubview(textPreset)
        textFamilyLabel = panelFieldLabel("Font", x: 0, y: 356, parent: content)
        textFamily.frame = NSRect(x: 0, y: 378, width: 252, height: 30)
        textFamily.setAccessibilityLabel("Text font")
        textFamily.target = self; textFamily.action = #selector(textControlToggled)
        content.addSubview(textFamily)
        textContentLabel = panelFieldLabel("Content", x: 0, y: 416, parent: content)
        let textScroll = NSScrollView(frame: NSRect(x: 0, y: 438, width: 252, height: 82))
        textScroll.hasVerticalScroller = true; textScroll.borderType = .lineBorder
        textScroll.useTokenScrollers(tokens)
        textEditor.frame = NSRect(x: 0, y: 0, width: 234, height: 82)
        textEditor.isRichText = false; textEditor.isVerticallyResizable = true
        textEditor.allowsUndo = true
        textEditor.isHorizontallyResizable = false; textEditor.textContainer?.widthTracksTextView = true
        textEditor.delegate = self
        textEditor.setAccessibilityLabel("Text content"); textScroll.documentView = textEditor
        content.addSubview(textScroll); textContentScroll = textScroll
        textSizeLabel = panelFieldLabel("Size (8–512)", x: 0, y: 528, parent: content)
        configure(textSize, frame: NSRect(x: 0, y: 550, width: 78, height: 30), label: "Text size", parent: content)
        textSize.formatter = nil; textSize.stringValue = "32"; textSize.delegate = self
        // Shipping `.screenshot-format-buttons`: B, I and the alignment icons.
        let format = EditorTextFormatButtons(tokens: tokens)
        format.frame = NSRect(x: 0, y: 588, width: 252, height: 32)
        format.changed = { [weak self] in self?.textControlsChanged(field: nil) }
        content.addSubview(format); textFormat = format
        let colorField = panelColorField(EditorColors.text("text_color"), y: 628, parent: content) {
            [weak self] _ in self?.textControlsChanged(field: "color")
        }
        textColorLabel = colorField.0; textColor = colorField.1
        textColorLabel.frame.size.width = 118
        textOutline.frame = NSRect(x: 126, y: 625, width: 126, height: 22)
        textOutline.target = self; textOutline.action = #selector(textControlToggled)
        textOutline.setAccessibilityLabel("Text outline"); content.addSubview(textOutline)
        textPlate.frame = NSRect(x: 0, y: 740, width: 120, height: 30)
        textPlate.addItems(withTitles: ["No plate", "Square plate", "Rounded plate"])
        textPlate.target = self; textPlate.action = #selector(textControlToggled)
        textPlate.setAccessibilityLabel("Text plate"); content.addSubview(textPlate)
        // Shipping `ColorField label="Background color"` under a plate.
        let plateField = panelColorField(EditorColors.text("background"), y: 778, parent: content) {
            [weak self] _ in self?.textControlsChanged(field: "background")
        }
        textPlateColorLabel = plateField.0; textPlateColor = plateField.1
        textPlateColor.selectedHex = "#ffffff"
        textShadow.frame = NSRect(x: 132, y: 740, width: 120, height: 30)
        textShadow.setAccessibilityLabel("Text drop shadow"); content.addSubview(textShadow)
        textShadow.target = self; textShadow.action = #selector(textShadowChanged)
        textShadowPanel.frame = NSRect(x: 0, y: 900, width: 252, height: 0)
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
            field.formatter = nil; field.delegate = self
            textShadowFields[row.0] = field
        }
        textControls = [textPreset, textFamilyLabel, textFamily, textContentLabel, textScroll, textSizeLabel,
                        textSize, format, textColorLabel, colorField.1, textPlate, textPlateColorLabel,
                        plateField.1, textShadow, textOutline, textShadowPanel]
    }

    /// Shipping text properties top to bottom from `top`; the Background
    /// color row shows only under a plate. Returns the last row's bottom.
    private func layoutTextControls(top: CGFloat) -> CGFloat {
        guard let textFormat, let textColor, let textPlateColor else { return top }
        var y = top
        textPreset.frame.origin.y = y; y += 34
        textFamilyLabel?.frame.origin.y = y; textFamily.frame.origin.y = y + 22; y += 60
        textContentLabel?.frame.origin.y = y; textContentScroll?.frame.origin.y = y + 22; y += 112
        textSizeLabel?.frame.origin.y = y; textSize.frame.origin.y = y + 22; y += 60
        textFormat.frame.origin.y = y; y += 40
        textColorLabel?.frame.origin.y = y; textOutline.frame.origin.y = y - 3
        textColor.frame.origin.y = y + 22; y += 22 + panelSwatchHeight + 8
        textPlate.frame.origin.y = y; textShadow.frame.origin.y = y; y += 38
        let plated = textPlate.indexOfSelectedItem != 0 && !textPlate.isHidden
        textPlateColorLabel?.isHidden = !plated; textPlateColor.isHidden = !plated
        if plated {
            textPlateColorLabel?.frame.origin.y = y; textPlateColor.frame.origin.y = y + 22
            y += 22 + panelSwatchHeight + 8
        }
        textShadowPanel.frame.origin.y = y
        return max(y - 8, textShadowPanel.isHidden ? 0 : textShadowPanel.frame.maxY)
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

        outputQuality = ClosurePopUpButton()
        outputQuality.tokens = tokens
        outputQuality.addItems(withTitles: ["Preserve quality", "Compress", "Maximum file size"])
        outputQuality.setAccessibilityLabel("Save quality")
        outputQuality.target = self; outputQuality.action = #selector(outputOptionsChanged)
        add(outputQuality)
        outputCompressionPreset.tokens = tokens
        outputCompressionPreset.addItems(withTitles: Self.outputCompressionPresets.map { $0.name })
        outputCompressionPreset.setAccessibilityLabel("Output compression preset")
        outputCompressionPreset.target = self
        outputCompressionPreset.action = #selector(outputCompressionPresetChanged)
        add(outputCompressionPreset)
        configure(outputQualityValue, frame: .zero, label: "Output quality value", parent: exportSettingsPanel)
        configure(outputMaximumSize, frame: .zero, label: "Maximum file size", parent: exportSettingsPanel)
        outputQualityValue.stringValue = "98"
        outputQualityValue.formatter = outputIntegerFormatter; outputQualityValue.delegate = self
        // Shipping's screenshot default: 10 MB, decimal units.
        outputMaximumSize.placeholderString = "Required"
        outputMaximumSize.stringValue = maximumUnit.value(10_000_000)
        outputMaximumSize.delegate = self
        outputMaximumUnit.tokens = tokens
        outputMaximumUnit.addItems(withTitles: RecordingFileSizeUnit.allCases.map { $0.label })
        outputMaximumUnit.selectItem(at: maximumUnit.rawValue)
        outputMaximumUnit.setAccessibilityLabel("Screenshot file size unit")
        outputMaximumUnit.target = self; outputMaximumUnit.action = #selector(outputMaximumUnitChanged)
        add(outputMaximumUnit)
        exportEstimateValue.setAccessibilityLabel("Estimated size")
        exportEstimateValue.font = .systemFont(ofSize: tokens.number("text-sm"), weight: .semibold)
        exportEstimateValue.toolTip = "Estimated export file size for the current format, quality, and output size"
        add(exportEstimateValue)
        exportEstimateDelta.setAccessibilityLabel("Estimated size change")
        exportEstimateDelta.font = .systemFont(ofSize: tokens.number("text-xs"), weight: .medium)
        exportEstimateDelta.toolTip = "Change versus the original image, before this export"
        add(exportEstimateDelta)
        // Shipping `.screenshot-show-comparison`, while a hidden comparison applies.
        showComparisonButton = button(CompressionCompareCopy.copy.show, frame: .zero,
                                      parent: exportSettingsPanel) { [weak self] in self?.showComparison() }
        func group(_ key: String, _ text: String, _ views: [(NSView, CGFloat, CGFloat)], _ width: CGFloat) {
            exportGroups[key] = (caption(text), views, width)
        }
        group("size", "Output size", [(outputSizeMode as NSView, 0, 110), (outputDimensions as NSView, 118, 110)], 228)
        group("custom", "Width × height", [(outputWidth as NSView, 0, 64), (outputHeight as NSView, 72, 64),
                                           (outputAspectLock as NSView, 144, 72)], 216)
        group("quality", "Save quality", [(outputQuality as NSView, 0, 150)], 150)
        group("preset", "Quality", [(outputCompressionPreset as NSView, 0, 100),
                                    (outputQualityValue as NSView, 108, 56)], 164)
        group("maximum", "Maximum file size", [(outputMaximumSize as NSView, 0, 96),
                                               (outputMaximumUnit as NSView, 104, 72)], 176)
        group("estimate", "Est. size", [(exportEstimateValue as NSView, 0, 92),
                                        (exportEstimateDelta as NSView, 96, 56)], 152)
        group("comparison", CompressionCompareCopy.copy.showCaption,
              [(showComparisonButton as NSView, 0, 150)], 150)

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

    /// Shipping sidebar Layers section, shown above Properties whatever the
    /// tool: the heading (title, count pill, Add image layer), then 54pt rows
    /// with a grip, live preview, name and kind and eye/lock/⋯ quick actions.
    /// Rows drag to reorder and image rows rename on double-click. The
    /// selected layer's properties live in their own panel below.
    private func buildLayersPanel() {
        let headingHeight = Self.layersHeadingHeight
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
        addLayerButton = button("", frame: NSRect(x: 272 - 30, y: (headingHeight - 30) / 2, width: 30, height: 30),
                                parent: layersPanel) { [weak self] in self?.chooseImage() }
        addLayerButton.quiet = true; addLayerButton.icon = .shipping("plus")
        addLayerButton.autoresizingMask = [.minXMargin]
        addLayerButton.setAccessibilityLabel(EditorChrome.text("layers", "add"))
        addLayerButton.toolTip = EditorChrome.text("layers", "add")
        layerHeadingRule.wantsLayer = true
        layerHeadingRule.frame = NSRect(x: 0, y: headingHeight - 1, width: 272, height: 1)
        layerHeadingRule.autoresizingMask = [.width]
        layersPanel.addSubview(layerHeadingRule)

        let listTop = headingHeight + tokens.number("s-3")
        let scroll = NSScrollView(frame: NSRect(x: 0, y: listTop, width: 272,
                                                height: max(0, layersPanel.bounds.height - listTop)))
        scroll.autoresizingMask = [.width, .height]
        // Overlay scrollers, as in Properties: a legacy scroller would narrow
        // the clip below the 272pt rows and cover their lock/⋯ quick actions.
        scroll.hasVerticalScroller = true; scroll.scrollerStyle = .overlay
        scroll.drawsBackground = false
        scroll.useTokenScrollers(tokens)
        layerTable = EditorLayerTable(frame: scroll.bounds)
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("editor-layer"))
        column.width = 272; layerTable.addTableColumn(column); layerTable.headerView = nil
        // 54pt rows with shipping's 2pt margins (a 58pt pitch).
        layerTable.rowHeight = 56; layerTable.intercellSpacing = NSSize(width: 0, height: 2)
        layerTable.selectionHighlightStyle = .none
        layerTable.backgroundColor = .clear
        layerTable.dataSource = self; layerTable.delegate = self
        layerTable.allowsEmptySelection = true; layerTable.setAccessibilityLabel("Screenshot layers")
        layerTable.contextMenu = { [weak self] row in self?.layerContextMenu(row: row) }
        layerTable.target = self; layerTable.doubleAction = #selector(layerRowDoubleClicked)
        layerTable.registerForDraggedTypes([Self.layerDragType])
        layerTable.setDraggingSourceOperationMask(.move, forLocal: true)
        layerTable.draggingDestinationFeedbackStyle = .gap
        scroll.documentView = layerTable; layersPanel.addSubview(scroll)
        layerListScroll = scroll

        let panelScroll = NSScrollView(frame: layerPropertiesPanel.bounds)
        panelScroll.autoresizingMask = [.width, .height]
        panelScroll.hasVerticalScroller = true; panelScroll.scrollerStyle = .overlay
        panelScroll.useTokenScrollers(tokens)
        panelScroll.drawsBackground = false
        layerContent.frame = NSRect(x: 0, y: 0, width: 272, height: 400)
        panelScroll.documentView = layerContent; layerPropertiesPanel.addSubview(panelScroll)
        // Shipping `.screenshot-properties-heading`: the selected layer's label.
        layerPropertiesHeading.font = .systemFont(ofSize: tokens.number("text-md"), weight: .semibold)
        layerPropertiesHeading.lineBreakMode = .byTruncatingTail
        layerPropertiesHeading.frame = NSRect(x: 0, y: 14, width: 252, height: 20)
        layerPropertiesHeading.setAccessibilityLabel("Properties heading")
        layerContent.addSubview(layerPropertiesHeading)
        layerPropertiesRule.wantsLayer = true
        layerPropertiesRule.frame = NSRect(x: 0, y: 47, width: 272, height: 1)
        layerContent.addSubview(layerPropertiesRule)

        // Shipping image `.screenshot-number-pair` rows: live Width/Height/X/Y.
        let geometry: [(String, TokenNumberField, String)] = [
            (EditorChrome.layerGeometry("width"), layerWidth, EditorChrome.layerGeometry("width_label")),
            (EditorChrome.layerGeometry("height"), layerHeight, EditorChrome.layerGeometry("height_label")),
            ("X", layerX, EditorChrome.layerGeometry("x_label")),
            ("Y", layerY, EditorChrome.layerGeometry("y_label")),
        ]
        for (index, (text, field, label)) in geometry.enumerated() {
            let x = CGFloat(index % 2) * 134
            let y = 56 + CGFloat(index / 2) * 58
            layerGeometryLabels.append(panelFieldLabel(text, x: x, y: y, parent: layerContent))
            configure(field, frame: NSRect(x: x, y: y + 22, width: 118, height: 30), label: label,
                      parent: layerContent)
            field.tokens = tokens; field.delegate = self
            if index < 2 {
                field.minimum = { 1 }
                field.maximum = { Self.maximumLayerSize }
            }
            field.stepped = { [weak self] changed in self?.layerGeometryChanged(changed) }
        }
        layerGeometryHint = panelLabel(EditorChrome.layerGeometry("proportional"),
                                       frame: NSRect(x: 0, y: 172, width: 252, height: 34), muted: true,
                                       parent: layerContent)
        buildTextControls(in: layerContent)

        rotationSnapLabel = panelFieldLabel("Shift rotation snap (1–180°)", x: 0, y: 550, parent: layerContent)
        rotationSnapLabel.frame.size.width = 252
        configure(rotationSnap, frame: NSRect(x: 0, y: 574, width: 252, height: 30),
                  label: "Shift rotation snap", parent: layerContent)
        rotationSnap.stringValue = "15"
        rotationSnap.toolTip = "Hold Shift while dragging the rotate handle. Does not edit the document."
        rotationSnap.delegate = self
        rotationSnap.target = self; rotationSnap.action = #selector(rotationSnapChanged)
        annotationControls = EditorAnnotationControls(tokens: tokens, formatter: editorNumberFormatter)
        // Shipping applies style changes live; a burst in one field is one undo step.
        annotationControls.apply = { [weak self] patch, field in
            guard let self, let layer = self.selectedLayer else { return }
            let key = field.map { "style:\(layer.id):\($0)" } ?? self.liveOnceKey("style")
            self.liveEdit(key: key, request: ["operation": "layer", "id": layer.id,
                "edit": ["action": "annotation_style", "patch": patch]])
        }
        annotationControls.opacityChanged = { [weak self] opacity in
            guard let self, let layer = self.selectedLayer else { return }
            self.liveEdit(key: "opacity:\(layer.id)", request: ["operation": "layer", "id": layer.id,
                "edit": ["action": "opacity", "opacity": opacity]])
        }
        annotationControls.reportError = { [weak self] message in self?.showError(message) }
        annotationControls.resized = { [weak self] height in
            self?.annotationControlsHeight = height
            self?.layoutLayerInspectorTail()
        }
        layerContent.addSubview(annotationControls)
        curveControls = EditorCurveControls(tokens: tokens)
        curveControls.apply = { [weak self] edit in
            guard let self, let id = self.selectedLayer?.id else { return }
            self.curveCanvasLayer(id, edit: edit)
        }
        curveControls.resized = { [weak self] _ in self?.layoutLayerInspectorTail() }
        layerContent.addSubview(curveControls)
        buildLayerMenu()
        layoutLayerInspectorTail()
    }

    static let layersHeadingHeight: CGFloat = 48
    /// Shipping `MAX_SCREENSHOT_OUTPUT_DIMENSION` for layer Width/Height.
    static let maximumLayerSize = 16_384.0
    static let layerDragType = NSPasteboard.PasteboardType("es.captur.editor-layer")

    /// Shipping `.screenshot-layer-menu-panel`: Appearance (blend mode and
    /// opacity), image Transform tiles, Arrange, Combine, then Duplicate and
    /// Delete. It floats beside the inspector at the row's ⋯ button.
    private func buildLayerMenu() {
        layerMenuCard.wantsLayer = true
        layerMenuCard.layer?.cornerRadius = tokens.number("r-lg")
        layerMenuCard.layer?.borderWidth = 1
        layerMenuCard.isHidden = true
        layerMenuCard.frame = NSRect(x: 0, y: 0, width: Self.layerMenuWidth, height: 400)
        func sectionTitle(_ key: String) -> NSTextField {
            let label = NSTextField(labelWithString: EditorChrome.layerMenu(key).uppercased())
            label.font = .systemFont(ofSize: tokens.number("text-2xs"), weight: .semibold)
            label.textColor = tokens.color("text-subtle")
            layerMenuCard.addSubview(label)
            return label
        }
        func fieldLabel(_ key: String) -> NSTextField {
            let label = NSTextField(labelWithString: EditorChrome.layerMenu(key))
            label.font = .systemFont(ofSize: tokens.number("text-sm"), weight: .medium)
            label.textColor = tokens.color("text-muted")
            layerMenuCard.addSubview(label)
            return label
        }
        func action(_ key: String, icon: String, parent: NSView? = nil,
                    perform: @escaping () -> Void) -> CaptureButton {
            let control = button(EditorChrome.layerMenu(key), frame: .zero, parent: parent ?? layerMenuCard,
                                 action: perform)
            control.icon = .shipping(icon)
            control.toolTip = EditorChrome.layerMenu(key + "_tip")
            control.setAccessibilityLabel(EditorChrome.layerMenu(key))
            return control
        }
        let blendLabel = fieldLabel("blend_mode")
        layerBlendMode.tokens = tokens
        for mode in EditorChrome.blendModes {
            layerBlendMode.addItem(withTitle: mode.label)
            layerBlendMode.lastItem?.representedObject = mode.value
        }
        layerBlendMode.setAccessibilityLabel(EditorChrome.layerMenu("blend_mode"))
        layerBlendMode.bindChange { [weak self] _ in self?.layerBlendModeChanged() }
        layerMenuCard.addSubview(layerBlendMode)
        let opacityLabel = fieldLabel("opacity")
        layerOpacityValue.font = .monospacedDigitSystemFont(ofSize: tokens.number("text-xs"), weight: .regular)
        layerOpacityValue.alignment = .right
        layerOpacityValue.textColor = tokens.color("text-muted")
        layerMenuCard.addSubview(layerOpacityValue)
        layerOpacity.tokens = tokens
        layerOpacity.isContinuous = true
        layerOpacity.target = self; layerOpacity.action = #selector(layerOpacityChanged)
        layerOpacity.setAccessibilityLabel(EditorChrome.layerMenu("opacity_label"))
        layerMenuCard.addSubview(layerOpacity)
        rotateLeftButton = action("rotate_left", icon: "rotate-counterclockwise") { [weak self] in
            self?.transformLayer("rotate-counterclockwise", message: "Rotating layer left…")
        }
        rotateRightButton = action("rotate_right", icon: "rotate-clockwise") { [weak self] in
            self?.transformLayer("rotate-clockwise", message: "Rotating layer right…")
        }
        flipHorizontalButton = action("flip_horizontal", icon: "flip-horizontal") { [weak self] in
            self?.transformLayer("flip-horizontal", message: "Flipping layer horizontally…")
        }
        flipVerticalButton = action("flip_vertical", icon: "flip-vertical") { [weak self] in
            self?.transformLayer("flip-vertical", message: "Flipping layer vertically…")
        }
        bringFrontButton = action("bring_front", icon: "bring-front") { [weak self] in
            self?.arrangeLayer(front: true)
        }
        sendBackButton = action("send_back", icon: "send-back") { [weak self] in
            self?.arrangeLayer(front: false)
        }
        mergeDownButton = action("merge_down", icon: "merge-down") { [weak self] in
            self?.closeLayerMenu(); self?.combine("merge_down", id: self?.selectedLayer?.id)
        }
        mergeVisibleButton = action("merge_visible", icon: "merge-visible") { [weak self] in
            self?.closeLayerMenu(); self?.combine("merge_visible")
        }
        flattenButton = action("flatten", icon: "flatten") { [weak self] in
            self?.closeLayerMenu(); self?.combine("flatten")
        }
        for control in [bringFrontButton, sendBackButton, mergeDownButton, mergeVisibleButton, flattenButton] {
            control?.quiet = true
        }
        layerMenuFooter.wantsLayer = true
        layerMenuCard.addSubview(layerMenuFooter)
        duplicateButton = action("duplicate", icon: "duplicate", parent: layerMenuFooter) { [weak self] in
            self?.closeLayerMenu(); self?.duplicateLayer()
        }
        deleteButton = action("delete", icon: "trash", parent: layerMenuFooter) { [weak self] in
            self?.closeLayerMenu(); self?.deleteLayer()
        }
        deleteButton.signal = true
        layerMenuSections = [
            (sectionTitle("appearance"), [blendLabel, layerBlendMode, opacityLabel, layerOpacityValue, layerOpacity]),
            (sectionTitle("transform"), [rotateLeftButton!, rotateRightButton!, flipHorizontalButton!,
                                         flipVerticalButton!]),
            (sectionTitle("arrange"), [bringFrontButton!, sendBackButton!]),
            (sectionTitle("combine"), [mergeDownButton!, mergeVisibleButton!, flattenButton!]),
        ]
        for _ in layerMenuSections {
            let rule = Surface(); rule.wantsLayer = true
            layerMenuCard.addSubview(rule); layerMenuRules.append(rule)
        }
        layerMenuCard.setAccessibilityLabel("Layer settings")
    }

    static let layerMenuWidth: CGFloat = 280

    /// Lay out the popover for `image` layers (Transform is image-only) and
    /// return its height.
    @discardableResult private func layoutLayerMenu(image: Bool) -> CGFloat {
        let pad = tokens.number("s-5")
        let inner = Self.layerMenuWidth - 2 * pad
        let gap = tokens.number("s-4")
        var y: CGFloat = 0
        for (index, section) in layerMenuSections.enumerated() {
            let visible = index != 1 || image
            section.title.isHidden = !visible
            section.views.forEach { $0.isHidden = !visible }
            layerMenuRules[index].isHidden = !visible
            guard visible else { continue }
            y += pad
            section.title.frame = NSRect(x: pad, y: y, width: inner, height: 12)
            y += 12 + gap
            switch index {
            case 0:
                let views = section.views
                views[0].frame = NSRect(x: pad, y: y, width: inner, height: 16); y += 16 + tokens.number("s-3")
                views[1].frame = NSRect(x: pad, y: y, width: inner, height: tokens.number("h-md"))
                y += tokens.number("h-md") + gap
                views[2].frame = NSRect(x: pad, y: y, width: inner / 2, height: 16)
                views[3].frame = NSRect(x: pad + inner / 2, y: y, width: inner / 2, height: 16)
                y += 16 + tokens.number("s-3")
                views[4].frame = NSRect(x: pad, y: y, width: inner, height: 20); y += 20
            case 1:
                let tile = (inner - tokens.number("s-3")) / 2
                for (tileIndex, view) in section.views.enumerated() {
                    view.frame = NSRect(x: pad + CGFloat(tileIndex % 2) * (tile + tokens.number("s-3")),
                                        y: y + CGFloat(tileIndex / 2) * (36 + tokens.number("s-3")),
                                        width: tile, height: 36)
                }
                y += 2 * 36 + tokens.number("s-3")
            default:
                for (actionIndex, view) in section.views.enumerated() {
                    view.frame = NSRect(x: pad, y: y + CGFloat(actionIndex) * 36, width: inner, height: 34)
                }
                y += CGFloat(section.views.count) * 36 - 2
            }
            y += pad
            layerMenuRules[index].frame = NSRect(x: 0, y: y - 1, width: Self.layerMenuWidth, height: 1)
        }
        let footerPad = tokens.number("s-4")
        duplicateButton.frame = NSRect(x: footerPad, y: footerPad, width: Self.layerMenuWidth - 2 * footerPad,
                                       height: 34)
        deleteButton.frame = duplicateButton.frame.offsetBy(dx: 0, dy: 34 + tokens.number("s-2"))
        layerMenuFooter.frame = NSRect(x: 0, y: y, width: Self.layerMenuWidth,
                                       height: deleteButton.frame.maxY + footerPad)
        return layerMenuFooter.frame.maxY
    }

    /// Open (or, for the same layer, close) the ⋯ settings popover beside the
    /// inspector, top-aligned with the row, like shipping.
    func toggleLayerMenu(id: String) {
        if layerMenuID == id { closeLayerMenu(); return }
        guard let layers = state.snapshot?.layers, let row = layers.firstIndex(where: { $0.id == id }) else { return }
        finishLayerRename(commit: true)
        activateTool(section: Section.layers, shape: nil)
        selectLayerRow(id: id)
        layerMenuID = id
        publishLayerMenu()
        let height = layoutLayerMenu(image: layers[row].kind == .image)
        let rowRect = root.convert(layerTable.rect(ofRow: row), from: layerTable)
        let maximumTop = root.bounds.height - 8 - height
        let top = max(chromeTop + 8, min(rowRect.minY, maximumTop))
        let left = max(8, layersPanel.frame.minX - 8 - Self.layerMenuWidth)
        layerMenuCard.frame = NSRect(x: left, y: top, width: Self.layerMenuWidth, height: height)
        layerMenuCard.isHidden = false
        layerTable.reloadData(forRowIndexes: IndexSet(integer: row), columnIndexes: IndexSet(integer: 0))
        if layerMenuMonitor == nil {
            layerMenuMonitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .keyDown]) {
                [weak self] event in
                guard let self, self.layerMenuID != nil, event.window === self.window else { return event }
                if event.type == .keyDown {
                    guard event.keyCode == 53 else { return event }
                    self.closeLayerMenu(); return nil
                }
                let point = self.root.convert(event.locationInWindow, from: nil)
                if !self.layerMenuCard.frame.contains(point) && !self.layerMenuTriggerContains(point) {
                    self.closeLayerMenu()
                }
                return event
            }
        }
    }

    private func layerMenuTriggerContains(_ point: NSPoint) -> Bool {
        guard let id = layerMenuID, let row = state.snapshot?.layers.firstIndex(where: { $0.id == id }),
              let cell = layerTable.view(atColumn: 0, row: row, makeIfNecessary: false) as? EditorLayerCell else {
            return false
        }
        return root.convert(cell.menuButton.bounds, from: cell.menuButton).contains(point)
    }

    func closeLayerMenu() {
        guard layerMenuID != nil else { return }
        let id = layerMenuID
        layerMenuID = nil
        layerMenuCard.isHidden = true
        if let layerMenuMonitor { NSEvent.removeMonitor(layerMenuMonitor) }
        layerMenuMonitor = nil
        if let row = state.snapshot?.layers.firstIndex(where: { $0.id == id }) {
            layerTable.reloadData(forRowIndexes: IndexSet(integer: row), columnIndexes: IndexSet(integer: 0))
        }
    }

    /// Current values for the open popover.
    private func publishLayerMenu() {
        guard let id = layerMenuID, let layer = state.snapshot?.layers.first(where: { $0.id == id }) else {
            closeLayerMenu(); return
        }
        if let item = layerBlendMode.itemArray.firstIndex(where: { $0.representedObject as? String == layer.blendMode }) {
            layerBlendMode.selectItem(at: item)
        }
        if window.firstResponder !== layerOpacity { layerOpacity.doubleValue = layer.opacity }
        layerOpacityValue.stringValue = "\(Int(layerOpacity.doubleValue.rounded()))%"
        publishLayerMenuStates()
    }

    /// Enabled states for the popover's actions: its layer while open, else
    /// the selection (Command-D and Delete use Duplicate and Delete).
    private func publishLayerMenuStates() {
        guard let duplicateButton else { return }
        let snapshot = state.snapshot
        let id = layerMenuID ?? selectedLayerID
        let index = id.flatMap { id in snapshot?.layers.firstIndex { $0.id == id } }
        let layer = index.flatMap { snapshot?.layers[$0] }
        let ready = layerActionsReady
        layerBlendMode.isEnabled = ready && layer != nil
        layerOpacity.isEnabled = snapshot != nil && inlineTextInput == nil && layer != nil
        for control in [rotateLeftButton, rotateRightButton, flipHorizontalButton, flipVerticalButton] {
            control?.isEnabled = ready && layer?.kind == .image
        }
        // Shipping disables arrange for locked layers and the layer already there.
        let count = snapshot?.layers.count ?? 0
        bringFrontButton.isEnabled = ready && layer?.locked == false && (index ?? 0) > 0
        sendBackButton.isEnabled = ready && layer?.locked == false && (index ?? count) < count - 1
        mergeDownButton.isEnabled = ready && layer.map { snapshot?.mergeDownIDs.contains($0.id) == true } == true
        mergeVisibleButton.isEnabled = ready && snapshot?.canMergeVisible == true
        flattenButton.isEnabled = ready && snapshot?.canFlatten == true
        duplicateButton.isEnabled = ready && layer != nil
        deleteButton.isEnabled = ready && layer?.locked == false
    }

    @objc private func layerBlendModeChanged() {
        guard let layer = selectedLayer, layerMenuID == layer.id,
              let mode = layerBlendMode.selectedItem?.representedObject as? String,
              mode != layer.blendMode, layerActionsReady else { return }
        layerCommand(layer, edit: ["action": "blend_mode", "blend_mode": mode],
                     message: "Changing blend mode…", preferredSelection: layer.id)
    }

    @objc private func layerOpacityChanged() {
        guard let layer = selectedLayer, layerMenuID == layer.id else { return }
        let opacity = layerOpacity.doubleValue.rounded()
        layerOpacityValue.stringValue = "\(Int(opacity))%"
        liveEdit(key: "opacity:\(layer.id)", request: ["operation": "layer", "id": layer.id,
            "edit": ["action": "opacity", "opacity": opacity]])
    }

    private func arrangeLayer(front: Bool) {
        guard layerActionsReady, let layer = selectedLayer, !layer.locked else { return }
        layerCommand(layer, edit: ["action": "arrange", "front": front],
                     message: front ? "Bringing layer to front…" : "Sending layer to back…",
                     preferredSelection: layer.id)
    }

    /// Select a row as a click does: the Select tool and that layer.
    private func selectLayerRow(id: String) {
        guard let row = state.snapshot?.layers.firstIndex(where: { $0.id == id }) else { return }
        if layerTable.selectedRow != row {
            layerTable.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        }
    }

    @objc private func layerRowDoubleClicked() {
        guard let layers = state.snapshot?.layers, layers.indices.contains(layerTable.clickedRow) else { return }
        beginLayerRename(id: layers[layerTable.clickedRow].id)
    }

    /// Shipping double-click rename: image layers only, in place in the row.
    func beginLayerRename(id: String) {
        guard layerActionsReady, let layers = state.snapshot?.layers,
              let row = layers.firstIndex(where: { $0.id == id }), layers[row].kind == .image else { return }
        closeLayerMenu()
        activateTool(section: Section.layers, shape: nil)
        selectLayerRow(id: id)
        renamingLayerID = id
        layerTable.reloadData(forRowIndexes: IndexSet(integer: row), columnIndexes: IndexSet(integer: 0))
        if let cell = layerTable.view(atColumn: 0, row: row, makeIfNecessary: true) as? EditorLayerCell {
            window.makeFirstResponder(cell.renameField)
            cell.renameField.currentEditor()?.selectAll(nil)
        }
    }

    /// Enter or leaving the field renames; Escape cancels.
    func finishLayerRename(commit: Bool) {
        guard let id = renamingLayerID else { return }
        let row = state.snapshot?.layers.firstIndex(where: { $0.id == id })
        let field = row.flatMap { layerTable.view(atColumn: 0, row: $0, makeIfNecessary: false) as? EditorLayerCell }?
            .renameField
        let name = (field?.currentEditor()?.string ?? field?.stringValue ?? "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        renamingLayerID = nil
        if let row {
            layerTable.reloadData(forRowIndexes: IndexSet(integer: row), columnIndexes: IndexSet(integer: 0))
        }
        if field?.currentEditor() != nil { window.makeFirstResponder(layerTable) }
        guard commit, !name.isEmpty, let layer = state.snapshot?.layers.first(where: { $0.id == id }),
              name != layer.name else { return }
        layerCommand(layer, edit: ["action": "rename", "name": name], message: "Renaming layer…",
                     preferredSelection: id)
    }

    /// Shipping live Width/Height/X/Y: each field's burst is one undo step.
    private func layerGeometryChanged(_ field: NSTextField) {
        guard let layer = selectedLayer, layer.kind == .image, !layer.locked,
              let value = number(field), value.isFinite else { return }
        let name: String
        if field === layerWidth { name = "width" } else if field === layerHeight { name = "height" }
        else if field === layerX { name = "x" } else if field === layerY { name = "y" } else { return }
        if name == "width" || name == "height" {
            guard (1...Self.maximumLayerSize).contains(value) else { return }
        }
        liveEdit(key: "geometry:\(layer.id):\(name)", request: ["operation": "layer", "id": layer.id,
            "edit": ["action": "geometry", name: value]])
    }

    private func liveEdit(key: String, request: [String: Any]) {
        if let last = liveQueue.last, last.key == key {
            liveQueue[liveQueue.count - 1].request = request
        } else {
            liveQueue.append((key, request))
        }
        flushLiveQueue()
    }

    /// A key for one discrete live change: its own undo step, in order.
    private func liveOnceKey(_ kind: String) -> String {
        liveSerial += 1
        return "\(kind):once:\(liveSerial)"
    }

    private func flushLiveQueue() {
        guard !liveQueue.isEmpty, state.snapshot != nil, !state.busy, inlineTextInput == nil else { return }
        let next = liveQueue.removeFirst()
        command(["operation": "live", "key": next.key, "request": next.request],
                message: "Applying changes…", preferredSelection: selectedLayerID,
                preserveStagedTextOnFailure: true)
    }

    @objc private func changeSection() {
        cancelCrop()
        cancelDrawing()
        cancelViewportPan()
        geometryPanel.isHidden = sectionControl.selectedSegment != Section.geometry
        layerPropertiesPanel.isHidden = sectionControl.selectedSegment != Section.layers
        drawPanel.isHidden = sectionControl.selectedSegment != Section.draw
        if sectionControl.selectedSegment != Section.layers { closeLayerMenu(); finishLayerRename(commit: true) }
        layoutLayerInspectorTail()
        // The export bar and its encoded preview do not depend on the section.
        changeOutputPreview()
        updateDrawing()
    }

    @objc private func changeEraserMode() {
        let modes: [EditorDrawOverlay.Shape] = [.wand, .erase, .restore]
        guard modes.indices.contains(eraserMode.selectedSegment) else { return }
        activateTool(section: Section.draw, shape: modes[eraserMode.selectedSegment])
    }

    /// Choose a drawing tool as the rail, its Shapes flyout or Eraser mode do.
    func selectDrawTool(_ shape: EditorDrawOverlay.Shape) {
        guard state.snapshot != nil else { return }
        activateTool(section: Section.draw, shape: shape)
    }

    private func changeDrawTool() {
        cancelDrawing()
        drawOverlay.shape = drawShape
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
            let shapeIndex = EditorDrawOverlay.Shape.allCases.firstIndex(of: drawShape)
            button.menu?.items.forEach { $0.state = $0.tag == shapeIndex ? .on : .off }
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

    @objc private func wandContiguousChanged() { publishDrawToolControls() }

    private func publishDrawToolControls() {
        guard let eraserMode else { return }
        let shape = drawShape
        // Shipping's properties heading names the active tool.
        drawHeading?.stringValue = EditorChrome.toolLabel(
            shape == .text ? "t" : shape == .arrow ? "a" : shape.rawValue)
        let wand = shape == .wand
        let brush = shape.isBackgroundBrush
        eraserMode.isHidden = !(wand || brush)
        eraserMode.selectedSegment = wand ? 0 : shape == .erase ? 1 : shape == .restore ? 2 : -1
        let creatingText = shape == .text
        let creatingDrawing = !wand && !brush && !creatingText
        wandTolerance?.isHidden = !wand; wandContiguous.isHidden = !wand
        brushSize?.isHidden = !brush; brushSoftness?.isHidden = !brush
        createTextControls.forEach { $0.isHidden = !creatingText }
        drawingDefaultControls.forEach { $0.isHidden = !creatingDrawing }
        let closed = [.rectangle, .ellipse, .triangle, .diamond, .star].contains(shape)
        drawingStroke.isHidden = !creatingDrawing || !closed
        drawingFillControls.forEach { $0.isHidden = !creatingDrawing || !closed }
        let drawingShadowVisible = creatingDrawing && drawingDropShadow.state == .on
        drawingShadowControls.forEach { $0.isHidden = !drawingShadowVisible }
        // Shipping names the stroke color "Color" for open tools.
        let strokeLegend = EditorColors.text(closed ? "stroke_color" : "color")
        drawingStrokeColorLabel?.stringValue = strokeLegend
        drawingStrokeColor?.relabel(strokeLegend)
        // Open tools have no Fill row: Drop shadow and its fields move up.
        let fillHeight = closed ? 0 : 22 + panelSwatchHeight + 8
        let shadowBase = drawingFillBottom - fillHeight
        drawControlBaseY[ObjectIdentifier(drawingDropShadow)] = shadowBase
        for (index, key) in ["color", "opacity", "blur", "offsetX", "offsetY"].enumerated() {
            guard let field = drawingShadowFields[key] else { continue }
            let y = CGFloat(index / 2) * 62 + shadowBase + 34
            if let label = drawingShadowLabels[key] { drawControlBaseY[ObjectIdentifier(label)] = y }
            drawControlBaseY[ObjectIdentifier(field)] = y + 22
        }
        drawHelper.stringValue = wand
            ? EditorInspectorCopy.eraser(wandContiguous.state == .on ? "wand_contiguous_hint" : "wand_everywhere_hint")
            : brush
                ? EditorInspectorCopy.eraser(shape == .erase ? "erase_hint" : "restore_hint")
                : shape == .text
                    ? "Click once to create empty auto-width text, then type on the canvas."
                    : drawingShadowVisible ? "Drawing pixels update in the background while dragging."
                    : "This tool creates one annotation layer on release."
        // Shipping shows `DrawToolPreview` for drawing tools and brushes.
        let previewing = creatingDrawing || brush
        let shift = previewing ? drawPreviewShift : 0
        drawToolPreview.isHidden = !previewing
        for view in drawToolPreview.superview?.subviews ?? [] {
            guard let base = drawControlBaseY[ObjectIdentifier(view)] else { continue }
            view.frame.origin.y = base + shift
        }
        refreshDrawToolPreview()
        if shape != .wand { hideWandLoupe() }
        // Each tool's rows end at a fixed, font-independent offset.
        let helperY: CGFloat
        if wand {
            helperY = 216 + 24 + 8
        } else if brush {
            helperY = 218 + EditorMarkedSlider.height + 8
        } else if creatingText {
            helperY = 290 + panelSwatchHeight + 8
        } else if drawingShadowVisible {
            helperY = shadowBase + 34 + 3 * 62
        } else {
            helperY = shadowBase + 32
        }
        drawHelper.frame.origin.y = helperY + shift
        drawHelper.superview?.frame.size.height = drawHelper.frame.maxY + 8
    }

    /// Shipping `DrawToolPreview`: the new stroke/shape with its colour, fill
    /// and opacity, or the Erase/Restore brush dab.
    private func refreshDrawToolPreview() {
        let shape = drawShape
        guard !drawToolPreview.isHidden else { return }
        if shape.isBackgroundBrush {
            let size = brushSize?.value ?? 28
            let softness = brushSoftness?.value ?? 18
            drawToolPreview.update(NativeDrawToolPreview.brush(size: size, softness: softness),
                                   stroke: .white, fill: nil, opacity: 1)
            return
        }
        let closed = [.rectangle, .ellipse, .triangle, .diamond, .star].contains(shape)
        let width = min(40, max(2, number(drawingStrokeWidth) ?? 8))
        let opacity = min(100, max(0, number(drawingOpacity) ?? 100))
        let tool = shape == .pen ? "pen" : shape.rawValue
        drawToolPreview.update(
            NativeDrawToolPreview.stroke(tool: tool, strokeWidth: width,
                                         strokeEnabled: !closed || drawingStroke.state == .on),
            stroke: drawingStrokeColor.flatMap({ NSColor(hex: $0.selectedHex) }) ?? tokens.color("text"),
            fill: closed && drawingFill.state == .on ? drawingFillColor.flatMap({ NSColor(hex: $0.selectedHex) }) : nil,
            opacity: CGFloat(opacity / 100))
    }

    /// Shipping Trim edges preview: shown while the enabled button is hovered
    /// or focused, following each new snapshot.
    func refreshTrimPreview() {
        let snapshot = state.snapshot
        trimPreviewView.canvasSize = snapshot.map { NSSize(width: $0.width, height: $0.height) } ?? .zero
        trimPreviewView.trimPreview = trimHighlighted && trimButton?.isEnabled == true
            ? snapshot?.trimPreview : nil
    }

    /// Shipping `WandColorLoupe`: sample the image under the Wand crosshair on
    /// the session queue (one request at a time, latest point wins).
    private func wandHover(_ point: NSPoint?) {
        guard let point, drawShape == .wand, drawOverlay.drawingEnabled, state.snapshot != nil,
              !drawOverlay.isViewportPanning, drawOverlay.presentedImageRect.contains(point) else {
            hideWandLoupe(); return
        }
        let request = (canvas: drawOverlay.canvasPoint(for: point),
                       cursor: root.convert(point, from: drawOverlay))
        if wandLoupeInFlight { wandLoupeQueued = request; return }
        requestWandLoupe(request)
    }

    private func requestWandLoupe(_ request: (canvas: CGPoint, cursor: CGPoint)) {
        wandLoupeInFlight = true
        let generation = wandLoupeGeneration
        worker.wandLoupe(at: request.canvas) { [weak self] loupe in
            guard let self else { return }
            self.wandLoupeInFlight = false
            if generation == self.wandLoupeGeneration {
                if let loupe { self.showWandLoupe(loupe, cursor: request.cursor) } else { self.wandLoupeView.hide() }
            }
            if let next = self.wandLoupeQueued {
                self.wandLoupeQueued = nil
                self.requestWandLoupe(next)
            }
        }
    }

    private func showWandLoupe(_ loupe: NativeWandLoupe, cursor: CGPoint) {
        guard drawShape == .wand, drawOverlay.drawingEnabled else { hideWandLoupe(); return }
        if wandLoupeView.superview !== root { root.addSubview(wandLoupeView) }
        let origin = NativeWandLoupe.position(cursor: cursor, viewport: root.bounds.size) ?? cursor
        wandLoupeView.show(loupe, circleOrigin: origin)
    }

    private func hideWandLoupe() {
        wandLoupeGeneration += 1
        wandLoupeQueued = nil
        wandLoupeView.hide()
    }

    @objc private func outputOptionsChanged() {
        if outputQuality.indexOfSelectedItem != lastQualityIndex {
            // Shipping `applyQualityMode`: a new mode shows the comparison again.
            lastQualityIndex = outputQuality.indexOfSelectedItem
            comparisonDismissed = false
        }
        normalizeOutputQuality()
        synchronizeOutputCompressionPreset()
        invalidateOutput()
        updateOutputOptionControls()
        // A different encoding is no longer the file that was just saved.
        exportInputsChanged(clearsSaved: true)
        updateControls()
    }

    /// Switching units converts the typed value, as shipping does.
    @objc private func outputMaximumUnitChanged() {
        guard let unit = RecordingFileSizeUnit(rawValue: outputMaximumUnit.indexOfSelectedItem),
              unit != maximumUnit else { return }
        if let bytes = maximumUnit.bytes(outputMaximumSize.stringValue) {
            outputMaximumSize.stringValue = unit.value(bytes)
        }
        maximumUnit = unit
        invalidateOutput()
        exportInputsChanged(clearsSaved: false)
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
        if let field = notification.object as? NSTextField,
           field.identifier == Self.layerRenameIdentifier, renamingLayerID != nil {
            finishLayerRename(commit: true)
        }
        // A field left mid-edit shows the accepted value again.
        if let field = notification.object as? NSTextField,
           [layerWidth, layerHeight, layerX, layerY].contains(where: { $0 === field }) {
            publishLayerGeometry()
        }
    }

    /// Escape in the inline rename field cancels the rename.
    func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
        guard control.identifier == Self.layerRenameIdentifier,
              commandSelector == #selector(NSResponder.cancelOperation(_:)) else { return false }
        finishLayerRename(commit: false)
        return true
    }

    static let layerRenameIdentifier = NSUserInterfaceItemIdentifier("editor-layer-rename")

    func controlTextDidChange(_ notification: Notification) {
        guard let field = notification.object as? NSTextField else { return }
        if [layerWidth, layerHeight, layerX, layerY].contains(where: { $0 === field }) {
            layerGeometryChanged(field)
            return
        }
        if field === textSize
            || textShadowFields.values.contains(where: { $0 === field }) {
            textControlsChanged(field: field.accessibilityLabel() ?? "field")
            return
        }
        if drawingShadowFields.values.contains(where: { $0 === field }) {
            drawingShadowCustomized = true
            return
        }
        if [drawingStrokeWidth, drawingOpacity].contains(where: { $0 === field }) {
            updateDrawingPreviewStyle()
            return
        }
        if [cropX, cropY, cropWidth, cropHeight].contains(where: { $0 === field }) {
            publishCropSelection()
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
        guard [outputQualityValue, outputMaximumSize].contains(where: { $0 === field }) else { return }
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
        outputOptionsChanged()
    }

    /// The canvas always shows the edited frame; the comparison covers it
    /// with the encoded After side while it applies.
    @objc private func changeOutputPreview() {
        preview.image = editedImage
        if let editedImage { viewportCanvasSize = editedImage.size }
        updateViewportGeometry()
        publishComparison()
    }

    /// Shipping's automatic comparison: Compress or Maximum with Export
    /// settings open, until Hide.
    var comparisonVisible: Bool {
        state.snapshot != nil && (outputQuality?.indexOfSelectedItem ?? 0) != 0
            && exportSettingsOpen && !comparisonDismissed
    }

    /// Encode the After side once edits or options settle (shipping's 280 ms
    /// refresh), off the session queue, exactly as Save would.
    private func scheduleComparison() {
        comparisonWork?.cancel(); comparisonWork = nil
        comparisonGeneration += 1
        comparisonOutput = nil; comparisonFailure = nil
        guard comparisonVisible else {
            comparisonPending = false; publishComparison(); return
        }
        comparisonPending = true
        let generation = comparisonGeneration
        let work = DispatchWorkItem { [weak self] in self?.runComparison(generation: generation) }
        comparisonWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + CompressionCompareCopy.screenshotRefreshDelay,
                                      execute: work)
        publishComparison()
    }

    private func runComparison(generation: Int) {
        guard generation == comparisonGeneration, comparisonVisible else { return }
        let result = outputOptionsResult()
        guard let options = result.options, result.error == nil else {
            comparisonPending = false
            comparisonFailure = result.error ?? exportBarState?.error
            publishComparison(); return
        }
        worker.compare(options) { [weak self] result in
            guard let self, generation == self.comparisonGeneration else { return }
            self.comparisonPending = false
            switch result {
            case .success(let output): self.comparisonOutput = output; self.comparisonFailure = nil
            case .failure(let error): self.comparisonOutput = nil; self.comparisonFailure = error.localizedDescription
            }
            self.publishComparison()
        }
    }

    private func showComparison() {
        comparisonDismissed = false
        layoutExportSettings()
        scheduleComparison()
    }

    private func dismissComparison() {
        comparisonDismissed = true
        comparisonWork?.cancel(); comparisonWork = nil
        comparisonGeneration += 1; comparisonPending = false
        layoutExportSettings()
        publishComparison()
    }

    /// Before is the lossless flattened edit (the estimate's baseline while
    /// compressing), After the encoded file.
    private func publishComparison() {
        guard let compareView else { return }
        let suppressed = inlineTextInput != nil || drawingPreviewInFlight || drawingPreviewPending != nil
        compareView.isHidden = !comparisonVisible || suppressed || editedImage == nil
        guard !compareView.isHidden else { return }
        compareView.mediaRect = compareView.convert(presentedImageRect, from: viewportInput)
        compareView.afterImage = comparisonOutput?.image
        compareView.processing = comparisonPending
        compareView.failureMessage = comparisonFailure
        compareView.badges = CompressionCompareCopy.badges(
            before: estimate?.baselineBytes, after: comparisonOutput.map { UInt64($0.length) },
            processing: comparisonPending)
        let drawing = sectionControl?.selectedSegment == Section.draw
        compareView.stripEnabled = !drawing && cropPrevious == nil
        compareView.afterHint = drawing ? CompressionCompareCopy.copy.afterHint : nil
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
        comparisonWork?.cancel(); comparisonWork = nil; comparisonGeneration += 1
        comparisonOutput = nil; comparisonPending = false; comparisonFailure = nil
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
            // Shipping JPEG option description while JPEG drops transparency.
            outputFormat.item(at: 1)?.toolTip = bar.hintWarning ? "Fills in transparent areas." : nil
        }
        saveAsNewSwitch.isHidden = bar?.formatRequiresCopy ?? true
        saveAsNewLabel.isHidden = saveAsNewSwitch.isHidden
        saveAsNewSwitch.state = bar?.savingCopy == false ? .off : .on
        if let bar {
            // Shipping `CustomSelect` descriptions for the current format.
            for (index, mode) in bar.qualityModes.enumerated() {
                outputQuality.item(at: index)?.toolTip = mode.description
            }
            for preset in bar.qualityPresets {
                outputCompressionPreset.item(withTitle: preset.label)?.toolTip = preset.description
            }
            exportGroups["maximum"]?.caption.toolTip = bar.maximumHelp
            outputMaximumSize.toolTip = bar.maximumHelp
        }
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
        publishComparison()
    }

    /// Output option or pixel changes: refresh the summary and re-estimate.
    private func exportInputsChanged(clearsSaved: Bool) {
        if clearsSaved { clearSavedResult() }
        exportError = nil
        refreshExportBar()
        scheduleEstimate()
        scheduleComparison()
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
        scheduleComparison()
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
        // Shared encoding derives the PNG palette from the Compress preset.
        let png: [String: Any] = [:]
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
            guard let budget = maximumUnit.bytes(outputMaximumSize.stringValue), budget >= 10_000 else {
                return (nil, "Enter a maximum file size of at least 10 KB.")
            }
            options["max_size_bytes"] = budget
        }
        return (options, nil)
    }

    /// Drop the comparison's After side; it and Est. size re-encode on their
    /// own schedules once the change is accepted.
    private func invalidateOutput() {
        comparisonOutput = nil
        preview.image = editedImage
        if let editedImage { viewportCanvasSize = editedImage.size; updateViewportGeometry() }
        publishComparison()
    }

    private func updateOutputOptionControls() {
        guard outputFormat != nil, outputQuality != nil else { return }
        let ready = state.snapshot != nil && !state.busy
        let compress = outputQuality.indexOfSelectedItem == 1
        let maximum = outputQuality.indexOfSelectedItem == 2
        outputQualityValue.isEnabled = ready && compress
        outputMaximumSize.isEnabled = ready && maximum
        outputMaximumUnit.isEnabled = ready && maximum
        showComparisonButton?.isEnabled = state.snapshot != nil
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
        let preset = Self.outputCompressionPresets.first(where: { $0.value == value })
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
        guard let color = PreferencesController.normalizeHex(drawingStrokeColor?.selectedHex ?? ""),
              let width = number(drawingStrokeWidth), (2...40).contains(width) else {
            if reportErrors { showError("Choose a drawing color and enter a stroke width from 2 to 40.") }
            return nil
        }
        let closed = [.rectangle, .ellipse, .triangle, .diamond, .star].contains(shape)
        var fill: Any = NSNull()
        if closed && drawingFill.state == .on {
            guard let color = PreferencesController.normalizeHex(drawingFillColor?.selectedHex ?? "") else {
                if reportErrors { showError("Choose a drawing fill color.") }
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
        guard let color = createTextColor?.selectedHex, !color.isEmpty else {
            showError("Enter a text color."); return
        }
        let families = state.snapshot?.fontFamilies ?? [:]
        let family = families["sans"] != nil ? "sans" : families.keys.sorted().first ?? "sans"
        var create: [String: Any] = [
            "point": ["x": point.x, "y": point.y], "text": "",
            "fontSize": size, "fontFamily": family, "color": color,
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
        guard liveQueue.isEmpty else {
            showError("Wait for property changes to apply before editing text inline.")
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
                    self.showError("Couldn’t start inline text input: invalid shared response. Press Escape to retry.")
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
                self.showError("Couldn’t start inline text input: \(error.localizedDescription). Press Escape to retry.")
                self.showInlineTextEditor()
            }
            self.updateControls()
        }
    }

    func textDidChange(_ notification: Notification) {
        if notification.object as? NSTextView === textEditor {
            textControlsChanged(field: "content")
            return
        }
        guard notification.object as? NSTextView === inlineTextEditor,
              var input = inlineTextInput, !input.finishInFlight else { return }
        input.bufferedText = inlineTextEditor.string
        inlineTextInput = input
        updateInlineTextFrame() // Auto-width boxes grow as you type.
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
                    self.showError("Inline text preview returned a stale token. Press Escape to retry.")
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
                self.showError("Inline text preview failed: \(error.localizedDescription). Press Escape to retry.")
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
            // Shipping has no Cancel: after a failed Begin, finishing retries
            // unless the box is blank, which leaves the document untouched.
            let blank = input.bufferedText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            if commit && !blank { sendBeginTextInput() }
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
                    self.showError("Inline text finish returned an active token. Press Escape to retry.")
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
                    ? (presentation.snapshot.unsavedChanges ? "Changes save automatically." : "Text finished.")
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
                self.showError("Couldn’t finish inline text: \(error.localizedDescription). Press Escape to retry.")
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
            && textColor?.selectedHex == style.color
            && textFormat?.bold == style.bold && textFormat?.italic == style.italic
            && textFormat?.align == style.align
            && textPlate.indexOfSelectedItem == (style.background == nil ? 0 : style.roundedBackground ? 2 : 1)
            && (style.background == nil || textPlateColor?.selectedHex == style.background)
            && (textPlate.indexOfSelectedItem != 0 || (textPresetRounded ?? style.roundedBackground) == style.roundedBackground)
            && (textShadow.state == .on) == style.dropShadow
            && (textOutline.state == .on) == style.outlined
    }

    @objc private func textShadowChanged() {
        updateTextShadowControls()
        layoutLayerInspectorTail()
        textControlsChanged(field: nil)
    }

    /// Menus, segments and checkboxes: each change is its own undo step.
    @objc private func textControlToggled() {
        layoutLayerInspectorTail()
        textControlsChanged(field: nil)
    }

    @objc private func stageTextPreset() {
        let index = textPreset.indexOfSelectedItem - 1
        guard let presets = state.snapshot?.textStylePresets,
              presets.indices.contains(index) else { return }
        let preset = presets[index]
        guard let family = textFamily.itemArray.firstIndex(where: {
            $0.representedObject as? String == preset.fontFamily
        }) else { return }
        textFamily.selectItem(at: family)
        if let color = preset.background {
            if textPlate.indexOfSelectedItem == 0 { textPlateColor?.selectedHex = color }
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
        layoutLayerInspectorTail()
        textControlsChanged(field: nil)
    }

    private func updateTextShadowControls() {
        let expanded = selectedLayer?.kind == .text && textShadow.state == .on
            && acceptedTextStyle?.shadowStyle != nil
        textShadowPanel.isHidden = !expanded
        textShadowPanel.frame.size.height = expanded ? 190 : 0
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
        for preset in state.snapshot?.textStylePresets ?? [] {
            textPreset.addItem(withTitle: preset.label)
            textPreset.lastItem?.image = TextStyleChip.image(for: preset, tokens: tokens)
        }
        if preserve { return }
        textPresetRounded = nil
        // Live edits republish the accepted values; never disturb a field
        // the user is typing in (its caret and selection stay put).
        func show(_ field: NSTextField, _ value: String) {
            if field.currentEditor() == nil && field.stringValue != value { field.stringValue = value }
        }
        if textEditor.string != style.text && window.firstResponder !== textEditor { textEditor.string = style.text }
        show(textSize, format(style.fontSize))
        textColor?.selectedHex = style.color
        textPlateColor?.selectedHex = style.background ?? "#ffffff"
        textFormat?.bold = style.bold; textFormat?.italic = style.italic
        textFormat?.align = style.align
        textPlate.selectItem(at: style.background == nil ? 0 : style.roundedBackground ? 2 : 1)
        textShadow.state = style.dropShadow ? .on : .off
        textOutline.state = style.outlined ? .on : .off
        if let field = textShadowFields["color"] { show(field, style.shadowStyle?.color ?? "") }
        for (key, path) in textShadowNumbers {
            if let field = textShadowFields[key] {
                show(field, style.shadowStyle.map { format($0[keyPath: path]) } ?? "")
            }
        }
        updateTextShadowControls()
        textFamily.removeAllItems()
        let families = state.snapshot?.fontFamilies ?? [:]
        for option in state.snapshot?.fontFamilyOptions ?? [] {
            textFamily.addItem(withTitle: option.label)
            textFamily.lastItem?.representedObject = option.key
        }
        if families[style.fontFamily] == nil {
            textFamily.addItem(withTitle: "Saved font: \(style.fontFamily)")
            textFamily.lastItem?.representedObject = style.fontFamily
        }
        textFamily.selectItem(at: textFamily.itemArray.firstIndex {
            $0.representedObject as? String == style.fontFamily
        } ?? 0)
    }

    /// Shipping applies text properties as they change. A typing burst in one
    /// field (`field`) is one undo step; other changes are each their own.
    private func textControlsChanged(field: String?) {
        guard let layer = selectedLayer, layer.kind == .text, inlineTextInput == nil,
              let patch = stagedTextPatch(), !patch.isEmpty else { return }
        let key = field.map { "text:\(layer.id):\($0)" } ?? liveOnceKey("text")
        liveEdit(key: key, request: ["operation": "edit_text", "id": layer.id, "patch": patch])
    }

    /// The text fields' changes from the accepted style, or nil while a field
    /// holds a value that cannot apply yet (an empty or partial color, say).
    private func stagedTextPatch() -> [String: Any]? {
        guard let style = selectedLayer?.textStyle else { return nil }
        guard let size = Double(textSize.stringValue), size.isFinite,
              size == style.fontSize || (8...512).contains(size) else { return nil }
        let color = textColor?.selectedHex ?? style.color
        guard PreferencesController.normalizeHex(color) != nil || color == style.color else { return nil }
        var patch: [String: Any] = [:]
        if textEditor.string != style.text { patch["text"] = textEditor.string }
        if size != style.fontSize { patch["fontSize"] = size }
        if let family = textFamily.selectedItem?.representedObject as? String, family != style.fontFamily {
            patch["fontFamily"] = family
        }
        if let bold = textFormat?.bold, bold != style.bold { patch["bold"] = bold }
        if let italic = textFormat?.italic, italic != style.italic { patch["italic"] = italic }
        if let align = textFormat?.align, align != style.align { patch["align"] = align }
        if color != style.color { patch["color"] = color }
        let plateColor = textPlateColor?.selectedHex ?? style.background ?? "#ffffff"
        let background = textPlate.indexOfSelectedItem == 0 ? nil : plateColor
        if background != nil {
            guard PreferencesController.normalizeHex(plateColor) != nil
                    || plateColor == style.background else { return nil }
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
                guard let value = PreferencesController.normalizeHex(color) else { return nil }
                shadowPatch["color"] = value
            }
            for (key, path) in textShadowNumbers {
                guard let field = textShadowFields[key] else { continue }
                // Formatting unchanged display values must not round authored precision.
                if field.stringValue != format(shadow[keyPath: path]) {
                    guard let value = number(field) else { return nil }
                    shadowPatch[key] = value
                }
            }
            if !shadowPatch.isEmpty { patch["dropShadowStyle"] = shadowPatch }
        }
        return patch
    }

    private func removeImageBackground(at point: NSPoint) {
        guard !state.busy else { return }
        // The slider's whole-number value is the wand's own 0–255 channel
        // distance; shipping stops the slider at 120.
        let tolerance = wandTolerance?.value ?? 36
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
        // The sliders only produce whole numbers inside the shipping ranges.
        let size = brushSize?.value ?? 28
        let softness = brushSoftness?.value ?? 18
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
        view.fileDropHandler = { [weak self, weak view] phase, urls, point in
            guard let self, let view else { return false }
            return self.handleFileDrop(phase, urls: urls, at: view.convert(point, to: self.viewportInput))
        }
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
            drawShape = shape
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
        publishComparison()
        selectionOverlay.layoutExpandButton(); dropGuideView.needsDisplay = true
        trimPreviewView.needsDisplay = true
    }

    private func publishTextInputPresentation(_ presentation: EditorPresentation) {
        publish(presentation, resetCrop: false)
        updateInlineTextFrame()
    }

    private func showInlineTextEditor(selectAtEnd: Bool = false) {
        guard inlineTextInput != nil, let inlineTextFrame else { return }
        inlineTextFrame.isHidden = false
        updateInlineTextFrame()
        publishComparison() // Inline text fades the comparison away.
        window.makeFirstResponder(inlineTextEditor)
        if selectAtEnd {
            inlineTextEditor.setSelectedRange(NSRange(location: inlineTextEditor.string.utf16.count, length: 0))
        }
    }

    private func hideInlineTextEditor() {
        defer { publishComparison() }
        inlineTextFrame?.isHidden = true
        inlineTextStyleKey = nil
    }

    /// The layer the inline editor draws: the session's transient layer once
    /// Begin is accepted, else the existing layer or the layer a Text click
    /// creates (`inline_text_layout`, shared with the wgpu host).
    private func inlineTextLayout() -> NativeInlineTextLayout? {
        guard let input = inlineTextInput else { return nil }
        let target = input.beginTarget
        let existing = target?["kind"] as? String == "existing" ? target?["id"] as? String : nil
        if let id = input.layerID ?? existing, let element = documentTextElement(id),
           let layout = NativeInlineTextLayout.resolve(element: element) {
            return layout
        }
        if let create = target?["create"] as? [String: Any], let layout = NativeInlineTextLayout.resolve(create: create) {
            return layout
        }
        // A layer the shared layout cannot read still gets a usable box at
        // the click, in the default face.
        return .resolve(create: [
            "point": ["x": Double(input.anchor.x), "y": Double(input.anchor.y)], "text": "",
            "fontSize": min(512, max(8, input.fontSize)), "fontFamily": "sans", "color": "#111318",
        ])
    }

    private func documentTextElement(_ id: String) -> [String: Any]? {
        guard let json = state.snapshot?.documentJSON,
              let document = try? JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any],
              let elements = document["elements"] as? [[String: Any]] else { return nil }
        return elements.first { $0["id"] as? String == id && $0["kind"] as? String == "text" }
    }

    /// Shipping inline editor geometry: the layer's plate or glyph box (at
    /// least 48 × 28 pt) scaled to the canvas, its padding, face, colour,
    /// outline and alignment, rotated about the frame centre. The published
    /// preview omits this layer while the input is active.
    private func updateInlineTextFrame() {
        guard let inlineTextFrame, !inlineTextFrame.isHidden, let snapshot = state.snapshot,
              let layout = inlineTextLayout() else { return }
        let image = presentedImageRect
        guard image.width > 0, image.height > 0, snapshot.width > 0,
              image.minX.isFinite, image.minY.isFinite, image.width.isFinite else { return }
        let scale = image.width / CGFloat(snapshot.width)
        let pad = layout.padding.map { $0 * scale } // top, right, bottom, left
        let alpha = min(1, max(0, layout.opacity / 100))
        let color = NSColor(hex: String(layout.color.prefix(7))) ?? tokens.color("text")
        let font = EditorTextFaces.font(family: layout.fontFamily, bold: layout.bold, italic: layout.italic,
                                        size: max(1, layout.fontSize * scale))
        let lineHeight = max(1, layout.lineHeight * scale)
        let paragraph = NSMutableParagraphStyle()
        paragraph.alignment = layout.align == "center" ? .center : layout.align == "right" ? .right : .left
        paragraph.minimumLineHeight = lineHeight; paragraph.maximumLineHeight = lineHeight
        paragraph.lineBreakMode = layout.autoWidth ? .byClipping : .byWordWrapping
        var attributes: [NSAttributedString.Key: Any] = [
            .font: font, .paragraphStyle: paragraph,
            // CSS line-height centres the glyphs in each line box.
            .baselineOffset: max(0, (lineHeight - (font.ascender - font.descender)) / 2),
            .foregroundColor: layout.outlined ? NSColor.clear : color.withAlphaComponent(alpha),
        ]
        if layout.outlined, layout.fontSize > 0 {
            // `-webkit-text-stroke` over a transparent fill; a positive width strokes only.
            attributes[.strokeColor] = color.withAlphaComponent(alpha)
            attributes[.strokeWidth] = layout.outlineWidth / layout.fontSize * 100
        }
        let text = inlineTextEditor.string
        let measured = NSAttributedString(string: text.isEmpty ? " " : text, attributes: attributes)
        let acceptedWidth = max(0, layout.frame.width * scale - pad[1] - pad[3])
        let wrapWidth = layout.autoWidth ? CGFloat.greatestFiniteMagnitude : max(1, acceptedWidth)
        let bounds = measured.boundingRect(with: NSSize(width: wrapWidth, height: .greatestFiniteMagnitude),
                                           options: [.usesLineFragmentOrigin])
        // Auto-width labels grow with the buffer before the session refits
        // them; the accepted box keeps its left edge, centre or right edge.
        let contentWidth = layout.autoWidth ? ceil(bounds.width) + 2 : acceptedWidth
        let trailingLine: CGFloat = text.hasSuffix("\n") ? lineHeight : 0
        let contentHeight = max(lineHeight, ceil(bounds.height) + trailingLine)
        let width = max(48, contentWidth + pad[1] + pad[3])
        let height = max(28, contentHeight + pad[0] + pad[2])
        let acceptedLeft = image.minX + layout.frame.minX * scale
        let acceptedRight = acceptedLeft + layout.frame.width * scale
        let left: CGFloat
        if layout.autoWidth && layout.align == "center" {
            left = (acceptedLeft + acceptedRight) / 2 - width / 2
        } else if layout.autoWidth && layout.align == "right" {
            left = acceptedRight - width
        } else {
            left = acceptedLeft
        }
        let textFrame = NSRect(x: left, y: image.minY + layout.frame.minY * scale, width: width, height: height)
        guard textFrame.minX.isFinite, textFrame.minY.isFinite else { return }
        let outset = inlineTextFrame.outset
        inlineTextFrame.frameCenterRotation = 0
        inlineTextFrame.frame = textFrame.insetBy(dx: -outset, dy: -outset)
        // The superview is flipped (y down), so a positive angle turns
        // clockwise like the document's rotation.
        inlineTextFrame.frameCenterRotation = layout.rotation * 180 / .pi
        inlineTextFrame.plateColor = layout.background
            .flatMap { NSColor(hex: String($0.prefix(7))) }?.withAlphaComponent(alpha)
        inlineTextFrame.plateRadius = layout.plateRadius * scale
        let editorFrame = NSRect(x: outset + pad[3], y: outset + pad[0],
                                 width: max(1, width - pad[1] - pad[3]), height: max(1, height - pad[0] - pad[2]))
        inlineTextEditor.frame = editorFrame
        inlineTextEditor.textContainer?.containerSize = NSSize(width: editorFrame.width,
                                                               height: CGFloat.greatestFiniteMagnitude)
        inlineTextEditor.insertionPointColor = color
        inlineTextEditor.selectedTextAttributes = [
            .backgroundColor: tokens.color("theme-accent").withAlphaComponent(0.2),
        ]
        // Restyling the storage would end an IME composition; wait for it.
        let key = "\(font.fontName):\(font.pointSize):\(lineHeight):\(layout.align):\(layout.color):"
            + "\(layout.outlined):\(alpha)"
        inlineTextEditor.typingAttributes = attributes
        if key != inlineTextStyleKey, !inlineTextEditor.hasMarkedText(), let storage = inlineTextEditor.textStorage {
            storage.setAttributes(attributes, range: NSRange(location: 0, length: storage.length))
            inlineTextStyleKey = key
        }
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
        eraserMode?.isEnabled = state.snapshot != nil && !state.busy && inputResolved
        wandTolerance?.isEnabled = active; wandContiguous.isEnabled = active
        brushSize?.isEnabled = active; brushSoftness?.isEnabled = active
        createTextPreset.isEnabled = active && createTextPreset.numberOfItems > 1
        createTextSize.isEnabled = active; createTextColor?.isEnabled = active
        // Selected text edits live under Select and stay editable while an
        // edit applies; changes made meanwhile queue (see `liveEdit`).
        let textReady = sectionControl?.selectedSegment == Section.layers && state.snapshot != nil
            && inputResolved && selectedLayer?.kind == .text
        textEditor.isEditable = textReady
        textFamily.isEnabled = textReady && textFamily.numberOfItems > 1
        textPreset.isEnabled = textReady && textPreset.numberOfItems > 1
        let textFields: [NSControl] = [textSize, textPlate, textShadow, textOutline]
        textFields.forEach { $0.isEnabled = textReady }
        textFormat?.isEnabled = textReady
        textColor?.isEnabled = textReady; textPlateColor?.isEnabled = textReady
        textShadowFields.values.forEach { $0.isEnabled = textReady }
        // Freeze the native responder only during an accepted Finish and restore
        // its normal state once that input has resolved.
        inlineTextEditor.isEditable = inlineTextInput?.finishInFlight != true
        drawOverlay.drawingEnabled = active
        if !active || drawShape != .wand { hideWandLoupe() }
        selectionOverlay.selectionEnabled = sectionControl?.selectedSegment == Section.layers
            && state.snapshot != nil && !state.busy && inputResolved && !importLoading
    }

    /// Properties for the selected layer, top to bottom: the heading, then
    /// image Width/Height/X/Y or the text fields, annotation style, Curve and
    /// the Shift rotation snap. Hidden groups take no space.
    private func layoutLayerInspectorTail() {
        guard let curveControls, let layerGeometryHint else { return }
        let layer = selectedLayer
        layerPropertiesHeading.stringValue = layer.map { $0.kind == .image ? $0.name : $0.rowKind } ?? ""
        layerPropertiesHeading.isHidden = layer == nil
        layerPropertiesRule.isHidden = layer == nil
        var y: CGFloat = 56
        let image = layer?.kind == .image
        let geometryViews: [NSView] = layerGeometryLabels + [layerWidth, layerHeight, layerX, layerY, layerGeometryHint]
        geometryViews.forEach { $0.isHidden = !image }
        if image {
            for (index, field) in [layerWidth, layerHeight, layerX, layerY].enumerated() {
                let x = CGFloat(index % 2) * 134
                let rowY = y + CGFloat(index / 2) * 58
                layerGeometryLabels[index].frame.origin = NSPoint(x: x, y: rowY)
                field.frame.origin = NSPoint(x: x, y: rowY + 22)
            }
            layerGeometryHint.stringValue = EditorChrome.layerGeometry(layer?.locked == true ? "locked" : "proportional")
            layerGeometryHint.frame.origin.y = y + 116
            y = layerGeometryHint.frame.maxY + 12
        }
        let text = layer?.kind == .text
        for control in textControls where control !== textShadowPanel { control.isHidden = !text }
        if text {
            updateTextShadowControls()
            y = layoutTextControls(top: y) + 12
        } else {
            textShadowPanel.isHidden = true
        }
        annotationControls.frame.origin.y = y
        if annotationControlsHeight > 0 { y += annotationControlsHeight + 8 }
        curveControls.frame.origin = CGPoint(x: 0, y: y)
        if !curveControls.isHidden { y += curveControls.frame.height + 8 }
        rotationSnapLabel.isHidden = layer == nil; rotationSnap.isHidden = layer == nil
        rotationSnapLabel.frame.origin.y = y + 8
        rotationSnap.frame.origin.y = y + 30
        layerContent.frame.size.height = layer == nil ? 0 : rotationSnap.frame.maxY + 16
    }

    /// Accepted Width/Height/X/Y for fields the user is not typing in.
    private func publishLayerGeometry() {
        let layer = selectedLayer
        for (field, value) in [(layerWidth, layer?.width), (layerHeight, layer?.height),
                               (layerX, layer?.x), (layerY, layer?.y)] {
            guard field.currentEditor() == nil else { continue }
            field.stringValue = value.map { format($0.rounded()) } ?? ""
        }
    }

    private func curveCanvasLayer(_ id: String, edit: [String: Any]) {
        guard !state.busy, let layer = state.snapshot?.layers.first(where: { $0.id == id }),
              !layer.locked else { return }
        command(["operation": "layer", "id": id, "edit": ["action": "curve", "edit": edit]],
                message: "Editing curve…", preferredSelection: id)
    }

    private func expandSelectedCanvas() {
        guard !state.busy, let id = selectedLayer?.id, state.snapshot?.canvasExpand[id] != nil else { return }
        command(["operation": "layer", "id": id, "edit": ["action": "expand_canvas"]],
                message: "Expanding canvas…", preferredSelection: id)
    }

    /// Shipping double-click on a line/arrow: remove a control dot, add a point
    /// on the selected path, or select an unselected path and add a point.
    private func curveDoubleClick(at point: CGPoint, radius: Double) -> Bool {
        guard !state.busy, let json = state.snapshot?.documentJSON else { return false }
        if let layer = selectedLayer, layer.visible, !layer.locked, state.snapshot?.curveHandles[layer.id] != nil,
           let hit = try? NativeEditorCanvas.curveHit(documentJSON: json, layerID: layer.id,
                                                      point: point, radius: radius) {
            if hit.handleKind == "control", let index = hit.handleIndex {
                curveCanvasLayer(layer.id, edit: ["kind": "remove", "index": index]); return true
            }
            if hit.handle != nil { return true }
            if hit.onPath, let closest = hit.closest {
                curveCanvasLayer(layer.id, edit: ["kind": "insert",
                    "point": ["x": Double(closest.x), "y": Double(closest.y)]])
                return true
            }
        }
        guard let id = try? NativeEditorHitTesting.hit(documentJSON: json, point: point, tolerance: radius),
              id != selectedLayer?.id, state.snapshot?.curveHandles[id] != nil,
              let hit = try? NativeEditorCanvas.curveHit(documentJSON: json, layerID: id, point: point, radius: radius),
              hit.onPath, let closest = hit.closest else { return false }
        curveCanvasLayer(id, edit: ["kind": "insert", "point": ["x": Double(closest.x), "y": Double(closest.y)]])
        return true
    }

    private func curveHoverHint(at point: CGPoint, radius: Double) -> String? {
        guard !state.busy, let json = state.snapshot?.documentJSON else { return nil }
        if let layer = selectedLayer, layer.visible, !layer.locked, state.snapshot?.curveHandles[layer.id] != nil,
           let hint = (try? NativeEditorCanvas.curveHit(documentJSON: json, layerID: layer.id,
                                                        point: point, radius: radius))?.hint {
            return hint
        }
        guard let id = try? NativeEditorHitTesting.hit(documentJSON: json, point: point, tolerance: radius),
              id != selectedLayer?.id, state.snapshot?.curveHandles[id] != nil,
              let hit = try? NativeEditorCanvas.curveHit(documentJSON: json, layerID: id, point: point, radius: radius),
              hit.onPath else { return nil }
        return "Click to select · double-click path to add curve points"
    }

    /// File drags over any canvas gesture view. Points are in viewport coordinates.
    func handleFileDrop(_ phase: EditorViewportGestureView.FileDropPhase, urls: [URL], at point: NSPoint) -> Bool {
        switch phase {
        case .exit:
            dropGuideView.active = false; dropGuideView.guide = nil
            return true
        case .hover:
            guard let snapshot = state.snapshot, state.artifactID != nil, inlineTextInput == nil else {
                dropGuideView.active = false; dropGuideView.guide = nil; return false
            }
            let image = presentedImageRect
            let documentPoint = image.contains(point) && image.width > 0
                ? CGPoint(x: (point.x - image.minX) * snapshot.width / image.width,
                          y: (point.y - image.minY) * snapshot.height / image.height) : nil
            dropGuideView.canvasSize = NSSize(width: snapshot.width, height: snapshot.height)
            dropGuideView.guide = try? NativeEditorCanvas.dropGuide(documentJSON: snapshot.documentJSON,
                selectedID: selectedLayerID, point: documentPoint)
            dropGuideView.active = true
            return true
        case .drop:
            let guide = dropGuideView.guide
            dropGuideView.active = false; dropGuideView.guide = nil
            guard let artifactID = state.artifactID, state.snapshot != nil, inlineTextInput == nil,
                  !importLoading else { return false }
            let images = urls.filter(NativeEditorCanvas.isSupportedImage)
            guard let first = images.first else { showError(NativeEditorCanvas.dropUnsupported); return false }
            selectionOverlay.cancelGesture()
            importToken += 1
            pendingDropURLs = Array(images.dropFirst())
            decodeImport(first, point: guide?.point, token: importToken,
                         generation: state.generation, artifactID: artifactID)
            return true
        }
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

    private func chooseImage() {
        guard let artifactID = state.artifactID, state.snapshot != nil, !state.busy,
              !importLoading else { return }
        selectionOverlay.cancelGesture()
        importToken += 1
        let token = importToken
        let generation = state.generation
        pendingDropURLs = []
        // Shipping's `<input type="file" multiple>` feeds the canvas-drop
        // import: the first image takes the default placement and each later
        // one stacks below the layer the previous one created.
        let completion: ([URL]) -> Void = { [weak self] urls in
            DispatchQueue.main.async {
                guard let self, !urls.isEmpty, self.importToken == token else { return }
                let images = urls.filter(NativeEditorCanvas.isSupportedImage)
                guard let first = images.first else { self.showError(NativeEditorCanvas.dropUnsupported); return }
                self.pendingDropURLs = Array(images.dropFirst())
                self.decodeImport(first, point: nil, token: token, generation: generation, artifactID: artifactID)
            }
        }
        if let imagePicker {
            imagePicker(window, completion)
            return
        }
        let panel = Self.imagePanel()
        panel.beginSheetModal(for: window) { response in
            completion(response == .OK ? panel.urls : [])
        }
    }

    /// Decode off the main thread, then import as one undoable edit. A drop
    /// point places the image where the drop guide showed.
    private func decodeImport(_ url: URL, point: CGPoint?, token: Int, generation: Int, artifactID: String) {
        guard importToken == token, state.generation == generation, state.artifactID == artifactID else { return }
        importLoading = true
        status.textColor = tokens.color("text-muted")
        status.stringValue = "Reading image…"
        updateControls()
        let decoder = imageDecoder
        Self.imageDecodeQueue.async {
            let result = Result { try decoder(url) }
            DispatchQueue.main.async { [weak self] in
                guard let self, self.importToken == token,
                      self.state.generation == generation,
                      self.state.artifactID == artifactID else { return }
                switch result {
                case .success(let image):
                    self.pendingImport = (image, generation, artifactID, point)
                    self.submitPendingImportIfReady()
                case .failure(let error):
                    self.importLoading = false
                    self.pendingDropURLs = []
                    self.showError("Couldn’t import image: \(error.localizedDescription)")
                    self.updateControls()
                }
            }
        }
    }

    static func imagePanel() -> NSOpenPanel {
        let panel = NSOpenPanel()
        panel.title = "Choose images"
        panel.message = "Choose images to add as new layers"
        panel.prompt = "Add Images"
        panel.allowedContentTypes = [.image]
        panel.canChooseDirectories = false
        panel.canChooseFiles = true
        panel.allowsMultipleSelection = true
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
        worker.importImage(pendingImport.image, selectedID: selectedID, point: pendingImport.point) { [weak self] result in
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
                self.status.stringValue = "Image added. Changes save automatically."
                if !self.pendingDropURLs.isEmpty, let artifactID = self.state.artifactID {
                    // Later files in one drop stack below the layer just added.
                    let next = self.pendingDropURLs.removeFirst()
                    self.updateControls()
                    self.decodeImport(next, point: nil, token: token,
                                      generation: self.state.generation, artifactID: artifactID)
                    return
                }
            case .failure(let error):
                guard self.state.fail(generation: generation) else { return }
                self.pendingDropURLs = []
                self.showError("Couldn’t import image: \(error.localizedDescription)")
            }
            self.updateControls()
        }
    }

    private func cancelPendingImport() {
        importToken += 1
        importLoading = false
        pendingImport = nil
        pendingDropURLs = []
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

    @objc private func toggleSolidBackground() {
        setBackground(backgroundSolid.state == .on ? lastSolidBackground : nil)
    }

    /// The background the card shows: a queued live change, else the document's.
    private var shownBackground: String? {
        if let queuedBackground { return queuedBackground }
        return state.snapshot?.background
    }

    private func publishBackgroundFields() {
        guard state.snapshot != nil else { return }
        if let color = state.snapshot?.background, queuedBackground == nil { lastSolidBackground = color }
        let shown = shownBackground
        backgroundSolid?.state = shown == nil ? .off : .on
        backgroundSwatches?.selectedHex = shown ?? lastSolidBackground
        backgroundButton?.swatch = shown.flatMap { NSColor(hex: $0) } ?? .clear
        backgroundButton?.setAccessibilityLabel(EditorColors.backgroundLabel(shown))
    }

    /// Shipping applies each background change at once as its own undo step;
    /// an unchanged value adds none (the session skips identical commits).
    private func setBackground(_ color: String?) {
        guard state.snapshot != nil, inlineTextInput == nil else { return }
        if let color { lastSolidBackground = color }
        if state.busy {
            queuedBackground = .some(color); publishBackgroundFields(); return
        }
        queuedBackground = nil
        command(["operation": "set_background", "color": color.map { $0 as Any } ?? NSNull()],
                message: "Changing canvas background…")
    }

    private func flushQueuedBackground() {
        guard let queued = queuedBackground, state.snapshot != nil, !state.busy,
              inlineTextInput == nil else { return }
        queuedBackground = nil
        command(["operation": "set_background", "color": queued.map { $0 as Any } ?? NSNull()],
                message: "Changing canvas background…")
    }

    private func discardEdits() {
        autosaveWork?.cancel(); autosaveWork = nil
        command(["operation": "discard_draft"], message: "Discarding edits…", resetCrop: true)
    }

    /// Restart shipping's autosave timer after an accepted change.
    private func scheduleAutosave(_ snapshot: NativeEditorSnapshot) {
        autosaveSerial += 1
        autosaveWork?.cancel(); autosaveWork = nil
        guard snapshot.unsavedChanges else { return }
        armAutosave()
    }

    private func armAutosave() {
        let work = DispatchWorkItem { [weak self] in self?.autosaveNow() }
        autosaveWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.autosaveDelay, execute: work)
    }

    /// Save in the background: the editor stays usable and the save runs behind
    /// any accepted edit on the session queue.
    private func autosaveNow() {
        autosaveWork = nil
        guard state.snapshot?.unsavedChanges == true else { return }
        // A running edit or inline text publishes (and re-arms) when it lands.
        guard !state.busy, inlineTextInput == nil, liveQueue.isEmpty else { armAutosave(); return }
        let serial = autosaveSerial, generation = state.generation
        worker.autosaveDraft { [weak self] result in
            // A failed autosave stays quiet, as in shipping: the next edit or
            // the close flush retries.
            guard let self, case .success(let saved) = result, self.autosaveSerial == serial,
                  self.state.autosaved(saved, generation: generation) else { return }
            if !saved.hasDraft && self.draftRestored { self.draftRestored = false; self.layoutEditor() }
        }
    }

    /// Closing or switching captures: write a pending or unsaved draft now.
    private func flushDraft() {
        let pending = autosaveWork != nil
        autosaveWork?.cancel(); autosaveWork = nil
        autosaveSerial += 1
        guard pending || state.snapshot?.unsavedChanges == true else { return }
        worker.autosaveDraft { _ in }
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
                        ? "Changes save automatically." : presentation.snapshot.hasDraft ? "Draft saved." : "Original restored."
                }
                if selectToolOnSuccess { self.activateTool(section: Section.layers, shape: nil) }
            case .failure(let error):
                guard self.state.fail(generation: generation) else { return }
                self.preferredLayerID = nil
                // A rejected live edit ends its burst; queued edits built on it drop too.
                self.liveQueue.removeAll()
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
        // Shipping keeps one title; drafts autosave instead of flagging it.
        window.title = EditorWindowTitle.screenshot
        scheduleAutosave(snapshot)
        // New pixels or dimensions: refresh the summary, re-estimate the export
        // and re-encode the comparison's After side.
        refreshExportBar()
        scheduleEstimate()
        scheduleComparison()
    }

    private func closeNow() {
        cancelCrop()
        cancelDrawing()
        cancelPendingImport()
        // Shipping flushes the draft on close; it runs before the session is freed.
        flushDraft()
        closeAfterTextInput = false
        inlineTextInput = nil; hideInlineTextEditor()
        selectedLayerID = nil; preferredLayerID = nil
        backgroundSwatches?.deactivate(); queuedBackground = nil
        state.close(); editedImage = nil; invalidateOutput(); preview.image = nil
        estimateWork?.cancel(); estimateWork = nil; estimateGeneration += 1
        comparisonWork?.cancel(); comparisonWork = nil; comparisonGeneration += 1
        cancelViewportPan()
        viewport = NativeEditorViewport(); viewportCanvasSize = .zero
        worker.close(); window.orderOut(nil); updateControls()
        publishPresence()
    }

    private func updateControls() {
        let ready = state.snapshot != nil && !state.busy && inlineTextInput == nil
        fields.forEach { $0.isEnabled = ready }
        applyCropButton?.isEnabled = ready
        if let applyCropButton {
            applyCropHalo.surround(applyCropButton)
            applyCropHalo.pulsing = ready && cropPrevious != nil
        }
        // Shipping `disabled={!canTrimEdges}`: nothing to trim greys it out.
        trimButton?.isEnabled = ready && state.snapshot?.canTrim == true
        backgroundButton?.isEnabled = ready
        refreshTrimPreview()
        // The card stays live while a change applies, like shipping; changes
        // made meanwhile queue (see `setBackground`).
        let backgroundLive = state.snapshot != nil && inlineTextInput == nil
        backgroundSolid?.isEnabled = backgroundLive
        backgroundSwatches?.isEnabled = backgroundLive
        if ready && queuedBackground != nil {
            DispatchQueue.main.async { [weak self] in self?.flushQueuedBackground() }
        }
        undoButton?.isEnabled = ready && state.snapshot?.canUndo == true
        redoButton?.isEnabled = ready && state.snapshot?.canRedo == true
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
        fitButton?.isEnabled = ready
        publishZoomLimits(ready: ready)
        zoomPreset.isEnabled = ready
        zoomSlider.isEnabled = ready
        updateOutputOptionControls()
        updateDrawing()
        layerTable?.isEnabled = ready
        if ready && !liveQueue.isEmpty {
            DispatchQueue.main.async { [weak self] in self?.flushLiveQueue() }
        }
        curveControls?.setReady(ready)
        // Live fields stay editable while an edit applies; their edits queue.
        let live = state.snapshot != nil && inlineTextInput == nil
        annotationControls?.setReady(live)
        let layer = live ? selectedLayer : nil
        rotationSnap.isEnabled = layer != nil
        let resizable = layer?.kind == .image && layer?.locked == false
        [layerWidth, layerHeight, layerX, layerY].forEach { $0.isEnabled = resizable }
        if layerMenuID != nil { publishLayerMenu() } else { publishLayerMenuStates() }
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
        // Queued live style edits keep the fields the user is changing.
        let layerID = selectedLayer?.id
        if !liveQueue.contains(where: { layerID != nil && $0.key.hasPrefix("style:\(layerID!):") }) {
            annotationControls?.setStyle(selectedLayer?.annotation)
        }
        if !liveQueue.contains(where: { layerID != nil && $0.key == "opacity:\(layerID!)" }) {
            annotationControls?.setOpacity(selectedLayer?.opacity)
        }
        publishTextFields(preserveStaged: true)
        publishDrawToolControls()
        selectionOverlay.documentJSON = state.snapshot?.documentJSON
        selectionOverlay.selectedOutline = selectedLayer?.selectionOutline
        selectionOverlay.selectedLayerID = selectedLayer?.id
        selectionOverlay.selectedRotation = selectedLayer?.rotation ?? 0
        selectionOverlay.rotationEnabled = selectedLayer?.visible == true && selectedLayer?.locked == false
        selectionOverlay.resizeEnabled = selectedLayer?.visible == true && selectedLayer?.locked == false
        let curve = selectedLayer.flatMap { state.snapshot?.curveHandles[$0.id] }
        selectionOverlay.curveHandles = selectionOverlay.resizeEnabled ? curve : nil
        selectionOverlay.expandPreview = selectedLayer.flatMap { state.snapshot?.canvasExpand[$0.id] }
        curveControls?.setHandles(selectedLayer?.locked == false ? curve : nil)
        publishLayerGeometry()
        if layerMenuID != nil && layerMenuID != selectedLayerID { closeLayerMenu() }
        if renamingLayerID != nil && renamingLayerID != selectedLayerID { finishLayerRename(commit: true) }
        layoutLayerInspectorTail()
        updateControls()
    }

    func numberOfRows(in tableView: NSTableView) -> Int { state.snapshot?.layers.count ?? 0 }

    /// Shipping drag to reorder: unlocked rows drag; the drop is one undo step.
    func tableView(_ tableView: NSTableView, pasteboardWriterForRow row: Int) -> NSPasteboardWriting? {
        guard tableView === layerTable, renamingLayerID == nil, layerActionsReady,
              let layers = state.snapshot?.layers, layers.indices.contains(row), !layers[row].locked else {
            return nil
        }
        closeLayerMenu()
        let item = NSPasteboardItem()
        item.setString(layers[row].id, forType: Self.layerDragType)
        return item
    }

    func tableView(_ tableView: NSTableView, validateDrop info: NSDraggingInfo, proposedRow row: Int,
                   proposedDropOperation dropOperation: NSTableView.DropOperation) -> NSDragOperation {
        guard tableView === layerTable,
              info.draggingPasteboard.string(forType: Self.layerDragType) != nil else { return [] }
        if dropOperation == .on { tableView.setDropRow(row, dropOperation: .above) }
        return .move
    }

    func tableView(_ tableView: NSTableView, acceptDrop info: NSDraggingInfo, row: Int,
                   dropOperation: NSTableView.DropOperation) -> Bool {
        guard tableView === layerTable,
              let id = info.draggingPasteboard.string(forType: Self.layerDragType) else { return false }
        return dropLayer(id, aboveRow: row)
    }

    /// Drop a dragged layer between rows: `row` is the front-to-back display
    /// index it lands above (the row count drops it at the back).
    @discardableResult func dropLayer(_ id: String, aboveRow row: Int) -> Bool {
        guard layerActionsReady, let layers = state.snapshot?.layers,
              let index = layers.firstIndex(where: { $0.id == id }), !layers[index].locked,
              row >= 0, row <= layers.count, row != index, row != index + 1 else { return false }
        let target = row < layers.count ? (layers[row].id, "before") : (layers[layers.count - 1].id, "after")
        cancelDrawing(); cancelViewportPan()
        layerCommand(layers[index], edit: ["action": "reorder", "target_id": target.0, "placement": target.1],
                     message: "Reordering layer…", preferredSelection: id)
        return true
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard !reconcilingLayerSelection else { return }
        guard let layers = state.snapshot?.layers, layers.indices.contains(layerTable.selectedRow) else {
            selectedLayerID = nil; publishSelectedLayerFields(); return
        }
        selectedLayerIndex = layerTable.selectedRow
        selectedLayerID = layers[selectedLayerIndex].id
        refreshLayerRowSelection()
        // Choosing a row chooses the Select tool, like shipping.
        if !reconcilingLayerSelection { activateTool(section: Section.layers, shape: nil) }
        publishSelectedLayerFields()
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let layers = state.snapshot?.layers, layers.indices.contains(row) else { return nil }
        let layer = layers[row]
        let title = NSTextField(labelWithString: layer.rowName)
        title.lineBreakMode = .byTruncatingTail
        title.toolTip = layer.kind == .image ? EditorChrome.text("layers", "rename") : layer.rowName
        title.font = .systemFont(ofSize: tokens.number("text-sm"), weight: .medium)
        title.textColor = tokens.color("text")
        let detail = NSTextField(labelWithString: layer.rowKind)
        detail.lineBreakMode = .byTruncatingTail
        detail.font = .systemFont(ofSize: tokens.number("text-xs"))
        detail.textColor = tokens.color("text-subtle"); detail.toolTip = layer.rowKind
        // Shipping fades hidden layers' name and preview.
        if !layer.visible { title.alphaValue = 0.42; detail.alphaValue = 0.42 }
        let rename = NSTextField(string: layer.name)
        rename.identifier = Self.layerRenameIdentifier
        rename.font = .systemFont(ofSize: tokens.number("text-sm"), weight: .medium)
        rename.setAccessibilityLabel(EditorChrome.layerMenu("rename_label"))
        rename.delegate = self
        let renaming = renamingLayerID == layer.id
        rename.isHidden = !renaming; title.isHidden = renaming
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
        let menu = CaptureButton("", frame: .zero, tokens: tokens) { [weak self] in
            self?.toggleLayerMenu(id: id)
        }
        menu.icon = .shipping("more")
        menu.setAccessibilityLabel(String(format: "Layer settings for %@", layer.rowName))
        menu.toolTip = EditorChrome.text("layers", "menu")
        for (control, on) in [(visibility, !layer.visible), (lock, layer.locked), (menu, layerMenuID == id)] {
            control.quiet = true; control.iconSide = 14
            control.cornerRadius = tokens.number("r-sm"); control.selected = on
            control.isEnabled = state.snapshot != nil && !state.busy && inlineTextInput == nil
        }
        let cell = EditorLayerCell(title: title, detail: detail, rename: rename, iconName: layer.rowIcon,
                                   thumbnail: EditorLayerThumbnails.image(layer.thumbnail),
                                   coversPreview: layer.kind == .image, locked: layer.locked,
                                   visible: layer.visible, tokens: tokens,
                                   visibility: visibility, lock: lock, menu: menu)
        cell.rowSelected = layer.id == selectedLayerID
        cell.toolTip = layer.locked ? EditorChrome.text("layers", "locked") : EditorChrome.text("layers", "drag")
        return cell
    }

    /// Cells draw shipping's selected row; the table's own highlight is off.
    private func refreshLayerRowSelection() {
        guard let layerTable else { return }
        for row in 0..<layerTable.numberOfRows {
            (layerTable.view(atColumn: 0, row: row, makeIfNecessary: false) as? EditorLayerCell)?.rowSelected =
                state.snapshot?.layers.indices.contains(row) == true
                && state.snapshot?.layers[row].id == selectedLayerID
        }
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
        restyleTokenScrollers(in: root, tokens)
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
        // The inline editor draws in the layer's own style; only its outline
        // and selection follow the theme.
        inlineTextFrame?.tokens = tokens
        inlineTextStyleKey = nil
        updateInlineTextFrame()
        for slider in [wandTolerance, brushSize, brushSoftness] { slider?.tokens = tokens }
        textFormat?.tokens = tokens
        for swatches in [createTextColor, drawingStrokeColor, drawingFillColor, textColor, textPlateColor] {
            swatches?.tokens = tokens
        }
        cropOverlay.tokens = tokens
        drawOverlay.fillColor = tokens.color("theme-accent").withAlphaComponent(0.22)
        drawOverlay.strokeColor = tokens.color("theme-accent")
        drawOverlay.brushOutlineColor = tokens.color("text")
        selectionOverlay.strokeColor = tokens.color("theme-accent")
        selectionOverlay.dotFill = tokens.color("surface-raised")
        selectionOverlay.hintFill = tokens.color("glass-strong")
        selectionOverlay.hintText = tokens.color("glass-text")
        selectionOverlay.motionTokens = tokens
        dropGuideView.motionTokens = tokens
        dropGuideView.accent = tokens.color("theme-accent")
        dropGuideView.glassFill = tokens.color("glass-strong")
        dropGuideView.glassText = tokens.color("glass-text")
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
        backgroundSwatches?.tokens = tokens
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
        layerPropertiesRule.layer?.backgroundColor = border
        layerPropertiesHeading.textColor = tokens.color("text")
        layerMenuCard.layer?.backgroundColor = tokens.color("surface-overlay").cgColor
        layerMenuCard.layer?.borderColor = tokens.color("border").cgColor
        layerMenuFooter.layer?.backgroundColor = tokens.color("surface-sunken").cgColor
        layerMenuRules.forEach { $0.layer?.backgroundColor = border }
        layerBlendMode.tokens = tokens; layerOpacity.tokens = tokens
        for field in [layerWidth, layerHeight, layerX, layerY] { field.tokens = tokens }
        for section in layerMenuSections { section.title.textColor = tokens.color("text-subtle") }
        let menuControls: [CaptureButton?] = [rotateLeftButton, rotateRightButton, flipHorizontalButton,
                                              flipVerticalButton, bringFrontButton, sendBackButton,
                                              mergeDownButton, mergeVisibleButton, flattenButton,
                                              duplicateButton, deleteButton]
        for control in menuControls.compactMap({ $0 }) { control.tokens = tokens; control.needsDisplay = true }
        layerTable?.reloadData()
        let controls: [CaptureButton?] = [trimButton, backgroundButton, undoButton, redoButton, fitButton,
                                          zoomOutButton, zoomInButton, addImagesButton,
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
