import AppKit
import UniformTypeIdentifiers

final class RecordingTrimHandle: NSView {
    let edge: NativeRecordingTimelineEdge
    weak var timeline: RecordingTrimTimeline?
    var enabled = false { didSet { updateAccessibility(); needsDisplay = true } }

    init(edge: NativeRecordingTimelineEdge, timeline: RecordingTrimTimeline) {
        self.edge = edge; self.timeline = timeline
        super.init(frame: .zero)
        setAccessibilityElement(true)
        setAccessibilityRole(.slider)
        setAccessibilityLabel(edge == .start ? "Recording trim start handle"
                                             : "Recording trim end handle")
        updateAccessibility()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override var acceptsFirstResponder: Bool { enabled }

    override func mouseDown(with event: NSEvent) {
        guard enabled, let timeline else { return }
        window?.makeFirstResponder(self)
        timeline.beginDrag(edge: edge, at: timeline.convert(event.locationInWindow, from: nil).x)
    }

    override func mouseDragged(with event: NSEvent) {
        timeline?.continueDrag(at: timeline?.convert(event.locationInWindow, from: nil).x ?? 0)
    }

    override func mouseUp(with event: NSEvent) {
        guard let timeline else { return }
        timeline.continueDrag(at: timeline.convert(event.locationInWindow, from: nil).x)
        timeline.endDrag()
    }

    override func keyDown(with event: NSEvent) {
        guard let timeline else { return }
        let direction: Int
        let page: Bool
        switch event.keyCode {
        case 123, 125: direction = -1; page = false
        case 124, 126: direction = 1; page = false
        case 116: direction = 1; page = true
        case 121: direction = -1; page = true
        case 53: timeline.endDrag(); return
        default: super.keyDown(with: event); return
        }
        timeline.nudge(edge: edge, direction: direction, page: page)
    }

    override func resignFirstResponder() -> Bool {
        timeline?.endDrag()
        let accepted = super.resignFirstResponder()
        needsDisplay = true
        return accepted
    }

    override func becomeFirstResponder() -> Bool {
        let accepted = super.becomeFirstResponder()
        needsDisplay = true
        return accepted
    }

    override func accessibilityPerformIncrement() -> Bool {
        guard enabled, let timeline else { return false }
        timeline.nudge(edge: edge, direction: 1, page: false); return true
    }

    override func accessibilityPerformDecrement() -> Bool {
        guard enabled, let timeline else { return false }
        timeline.nudge(edge: edge, direction: -1, page: false); return true
    }

    func updateAccessibility() {
        guard let timeline else { return }
        setAccessibilityEnabled(enabled)
        setAccessibilityMinValue(NSNumber(value: edge == .start ? 0 : timeline.startMilliseconds + 1))
        setAccessibilityMaxValue(NSNumber(value: edge == .start
            ? max(0, timeline.endMilliseconds - 1) : timeline.durationMilliseconds))
        setAccessibilityValue(NSNumber(value: edge == .start
            ? timeline.startMilliseconds : timeline.endMilliseconds))
        setAccessibilityValueDescription("\(edge == .start ? timeline.startMilliseconds : timeline.endMilliseconds) milliseconds")
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let timeline else { return }
        let focused = window?.firstResponder === self
        let rect = bounds.insetBy(dx: 4, dy: 2)
        let path = NSBezierPath(roundedRect: rect, xRadius: 4, yRadius: 4)
        timeline.tokens.color(enabled ? "theme-accent" : "text-faint").setFill(); path.fill()
        timeline.tokens.color(focused ? "text" : "theme-accent-ink").setStroke()
        path.lineWidth = focused ? 2 : 1; path.stroke()
    }
}

final class RecordingTrimTimeline: NSView {
    fileprivate let tokens: Tokens
    fileprivate(set) var durationMilliseconds: UInt64 = 1
    fileprivate(set) var startMilliseconds: UInt64 = 0
    fileprivate(set) var endMilliseconds: UInt64 = 1
    var onStage: ((NativeRecordingTimelineEdge, UInt64) -> Void)?
    private var drag: NativeRecordingTimelineDrag?
    private var dragEdge: NativeRecordingTimelineEdge?
    private(set) var editingEnabled = false
    private var thumbnailImage: NSImage?
    private var playbackPositionMilliseconds: UInt64?
    private(set) var thumbnailStateDescription = "Source thumbnails not loaded"
    private lazy var startHandle = RecordingTrimHandle(edge: .start, timeline: self)
    private lazy var endHandle = RecordingTrimHandle(edge: .end, timeline: self)

    init(tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: .zero)
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel("Recording trim range")
        addSubview(startHandle); addSubview(endHandle)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override var isFlipped: Bool { true }
    private var trackRect: NSRect {
        NSRect(x: 10, y: thumbnailImage == nil ? max(0, bounds.height - 8) : bounds.midY - 3,
               width: max(1, bounds.width - 20), height: 6)
    }

    func setValues(start: UInt64, end: UInt64, duration: UInt64) {
        guard duration > 0, start < end, end <= duration else { return }
        durationMilliseconds = duration; startMilliseconds = start; endMilliseconds = end
        updateHandles(); needsDisplay = true
    }

    func setEditingEnabled(_ enabled: Bool) {
        editingEnabled = enabled
        startHandle.enabled = enabled; endHandle.enabled = enabled
        if !enabled { endDrag() }
    }

    func setPlaybackPosition(_ milliseconds: UInt64?) {
        playbackPositionMilliseconds = milliseconds
        needsDisplay = true
    }

    func showThumbnailLoading() {
        thumbnailImage = nil
        thumbnailStateDescription = "Loading source thumbnails…"
        setAccessibilityValueDescription(thumbnailStateDescription)
        needsDisplay = true
    }

    func showThumbnailCancelling() {
        thumbnailStateDescription = "Cancelling source thumbnails…"
        setAccessibilityValueDescription(thumbnailStateDescription)
        needsDisplay = true
    }

    func showThumbnailFailure(cancelled: Bool) {
        thumbnailImage = nil
        thumbnailStateDescription = cancelled ? "Source thumbnails cancelled"
            : "Source thumbnails unavailable"
        setAccessibilityValueDescription(thumbnailStateDescription)
        needsDisplay = true
    }

    func showThumbnails(_ image: CGImage) {
        thumbnailImage = NSImage(cgImage: image,
                                 size: NSSize(width: image.width, height: image.height))
        thumbnailStateDescription = "12 source-relative timeline thumbnails"
        setAccessibilityValueDescription(thumbnailStateDescription)
        needsDisplay = true
    }

    func clearThumbnails() {
        thumbnailImage = nil
        thumbnailStateDescription = "Source thumbnails not loaded"
        setAccessibilityValueDescription(thumbnailStateDescription)
        needsDisplay = true
    }

    @discardableResult func beginDrag(edge: NativeRecordingTimelineEdge, at pointerX: CGFloat) -> Bool {
        guard editingEnabled,
              let value = NativeRecordingTimelineDrag.begin(edge: edge, pointerX: Double(pointerX),
                startMilliseconds: Double(startMilliseconds),
                endMilliseconds: Double(endMilliseconds),
                durationMilliseconds: Double(durationMilliseconds)) else { return false }
        drag = value; dragEdge = edge; return true
    }

    func continueDrag(at pointerX: CGFloat) {
        guard editingEnabled, let edge = dragEdge, var value = drag,
              let milliseconds = value.update(pointerX: Double(pointerX),
                  trackLeft: Double(trackRect.minX),
                  trackWidth: Double(trackRect.width)) else { return }
        drag = value
        stage(edge: edge, milliseconds: UInt64(milliseconds.rounded()))
    }

    func endDrag() { drag = nil; dragEdge = nil }

    func nudge(edge: NativeRecordingTimelineEdge, direction: Int, page: Bool) {
        guard editingEnabled, direction == -1 || direction == 1 else { return }
        let step: UInt64 = page ? 1_000 : durationMilliseconds < 60_000 ? 1 : 10
        let current = edge == .start ? startMilliseconds : endMilliseconds
        let minimum = edge == .start ? 0 : startMilliseconds + 1
        let maximum = edge == .start ? endMilliseconds - 1 : durationMilliseconds
        let next = direction > 0 ? min(maximum, current.addingReportingOverflow(step).overflow
            ? maximum : current + step) : max(minimum, current > step ? current - step : 0)
        stage(edge: edge, milliseconds: next)
    }

    private func stage(edge: NativeRecordingTimelineEdge, milliseconds: UInt64) {
        let next = edge == .start ? min(milliseconds, endMilliseconds - 1)
                                  : max(startMilliseconds + 1, min(milliseconds, durationMilliseconds))
        guard next != (edge == .start ? startMilliseconds : endMilliseconds) else { return }
        if edge == .start { startMilliseconds = next } else { endMilliseconds = next }
        updateHandles(); needsDisplay = true; onStage?(edge, next)
    }

    private func updateHandles() {
        let track = trackRect
        let startRatio = NativeRecordingTimeline.ratio(milliseconds: Double(startMilliseconds),
                                                        durationMilliseconds: Double(durationMilliseconds)) ?? 0
        let endRatio = NativeRecordingTimeline.ratio(milliseconds: Double(endMilliseconds),
                                                      durationMilliseconds: Double(durationMilliseconds)) ?? 1
        startHandle.frame = NSRect(x: track.minX + track.width * CGFloat(startRatio) - 10,
                                   y: 0, width: 20, height: bounds.height)
        endHandle.frame = NSRect(x: track.minX + track.width * CGFloat(endRatio) - 10,
                                 y: 0, width: 20, height: bounds.height)
        startHandle.updateAccessibility(); endHandle.updateAccessibility()
        startHandle.needsDisplay = true; endHandle.needsDisplay = true
    }

    override func setFrameSize(_ newSize: NSSize) {
        endDrag(); super.setFrameSize(newSize); updateHandles()
    }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
        if newWindow == nil { endDrag() }
        super.viewWillMove(toWindow: newWindow)
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        let local = superview.map { convert(point, from: $0) } ?? point
        guard editingEnabled, bounds.contains(local) else { return nil }
        let startDistance = abs(local.x - startHandle.frame.midX)
        let endDistance = abs(local.x - endHandle.frame.midX)
        if min(startDistance, endDistance) <= 10 {
            if startDistance == endDistance, window?.firstResponder === startHandle {
                return startHandle
            }
            return startDistance < endDistance ? startHandle : endHandle
        }
        return nil
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        let track = trackRect
        let strip = NSRect(x: track.minX, y: 2, width: track.width,
                           height: max(1, bounds.height - 4))
        let background = NSBezierPath(roundedRect: strip, xRadius: 4, yRadius: 4)
        tokens.color("surface-sunken").setFill(); background.fill()
        if let thumbnailImage {
            NSGraphicsContext.saveGraphicsState()
            background.addClip()
            let frameCount = 12
            let sourceWidth = thumbnailImage.size.width / CGFloat(frameCount)
            let targetWidth = strip.width / CGFloat(frameCount)
            for index in 0..<frameCount {
                thumbnailImage.draw(
                    in: NSRect(x: strip.minX + CGFloat(index) * targetWidth, y: strip.minY,
                               width: targetWidth + 0.5, height: strip.height),
                    from: NSRect(x: CGFloat(index) * sourceWidth, y: 0,
                                 width: sourceWidth, height: thumbnailImage.size.height),
                    operation: .copy, fraction: 1, respectFlipped: true,
                    hints: [.interpolation: NSImageInterpolation.high])
            }
            NSGraphicsContext.restoreGraphicsState()
        } else {
            let paragraph = NSMutableParagraphStyle(); paragraph.alignment = .center
            (thumbnailStateDescription as NSString).draw(
                in: NSRect(x: strip.minX + 4, y: strip.minY + 1,
                           width: max(1, strip.width - 8), height: max(1, strip.height - 9)),
                withAttributes: [.font: NSFont.systemFont(ofSize: 9, weight: .medium),
                                 .foregroundColor: tokens.color("text-muted"),
                                 .paragraphStyle: paragraph])
        }
        let selected = NSRect(x: startHandle.frame.midX, y: track.minY,
                              width: max(0, endHandle.frame.midX - startHandle.frame.midX),
                              height: track.height)
        let selection = NSBezierPath(roundedRect: selected, xRadius: 3, yRadius: 3)
        tokens.color(editingEnabled ? "theme-accent" : "text-faint")
            .withAlphaComponent(thumbnailImage == nil ? 1 : 0.38).setFill()
        selection.fill()
        tokens.color(editingEnabled ? "theme-accent" : "text-faint").setStroke()
        selection.lineWidth = 2; selection.stroke()
        if let playbackPositionMilliseconds,
           let ratio = NativeRecordingTimeline.ratio(
               milliseconds: Double(playbackPositionMilliseconds),
               durationMilliseconds: Double(durationMilliseconds)) {
            let x = track.minX + track.width * CGFloat(ratio)
            let playhead = NSBezierPath()
            playhead.move(to: NSPoint(x: x, y: strip.minY))
            playhead.line(to: NSPoint(x: x, y: strip.maxY))
            tokens.color("info-text").setStroke()
            playhead.lineWidth = 2; playhead.stroke()
        }
        tokens.color("control-border").setStroke(); background.lineWidth = 1; background.stroke()
    }
}

final class RecordingCropHandle: NSView {
    let kind: NativeRecordingCropDragHandle
    weak var overlay: RecordingCropOverlay?
    var enabled = false { didSet { needsDisplay = true; setAccessibilityEnabled(enabled) } }

    init(kind: NativeRecordingCropDragHandle, overlay: RecordingCropOverlay) {
        self.kind = kind; self.overlay = overlay
        super.init(frame: .zero)
        setAccessibilityElement(true); setAccessibilityRole(.slider)
        setAccessibilityLabel("Recording crop \(kind.accessibilityName) handle")
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override var acceptsFirstResponder: Bool { enabled }

    override func mouseDown(with event: NSEvent) {
        guard enabled, let overlay else { return }
        window?.makeFirstResponder(self)
        overlay.beginDrag(kind, at: overlay.convert(event.locationInWindow, from: nil))
    }
    override func mouseDragged(with event: NSEvent) {
        guard let overlay else { return }
        overlay.continueDrag(at: overlay.convert(event.locationInWindow, from: nil))
    }
    override func mouseUp(with event: NSEvent) {
        guard let overlay else { return }
        overlay.continueDrag(at: overlay.convert(event.locationInWindow, from: nil))
        overlay.endDrag()
    }

    override func keyDown(with event: NSEvent) {
        guard let overlay else { return }
        let step = event.modifierFlags.contains(.shift) ? 10.0 : 1.0
        switch event.keyCode {
        case 123: overlay.nudge(kind, deltaX: -step, deltaY: 0)
        case 124: overlay.nudge(kind, deltaX: step, deltaY: 0)
        case 125: overlay.nudge(kind, deltaX: 0, deltaY: step)
        case 126: overlay.nudge(kind, deltaX: 0, deltaY: -step)
        case 53: overlay.endDrag()
        default: super.keyDown(with: event); return
        }
    }

    override func becomeFirstResponder() -> Bool {
        let accepted = super.becomeFirstResponder(); needsDisplay = true; return accepted
    }
    override func resignFirstResponder() -> Bool {
        overlay?.endDrag()
        let accepted = super.resignFirstResponder(); needsDisplay = true; return accepted
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let overlay else { return }
        let focused = window?.firstResponder === self
        let rect = bounds.insetBy(dx: 5, dy: 5)
        let path = NSBezierPath(ovalIn: rect)
        overlay.tokens.color(enabled ? "theme-accent" : "text-faint").setFill(); path.fill()
        overlay.tokens.color(focused ? "glass-text" : "theme-accent-ink").setStroke()
        path.lineWidth = focused ? 3 : 2; path.stroke()
    }
}

final class RecordingCropOverlay: NSView {
    fileprivate var tokens: Tokens
    private let imageInset: CGFloat = 12
    var presentedImageRect: NSRect? {
        didSet { endDrag(); updateHandles(); needsDisplay = true }
    }
    var sourceSize = NativeRecordingDimensions(width: 2, height: 2) {
        didSet { endDrag(); updateHandles(); needsDisplay = true }
    }
    var crop = NativeRecordingCropRect(x: 0, y: 0, width: 2, height: 2) {
        didSet { updateHandles(); updateAccessibility(); needsDisplay = true }
    }
    var lockAspect = true
    var onStage: ((NativeRecordingCropRect) -> Void)?
    var onCommitPendingInput: (() -> Void)?
    private(set) var editingEnabled = false
    var interceptsPendingInput = false
    private var initialCrop: NativeRecordingCropRect?
    private var initialPoint: NSPoint?
    private var handles: [NativeRecordingCropDragHandle: RecordingCropHandle] = [:]

    init(tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: .zero)
        setAccessibilityElement(true); setAccessibilityRole(.group)
        setAccessibilityLabel("Recording crop canvas")
        for kind in NativeRecordingCropDragHandle.allCases where kind != .move {
            let handle = RecordingCropHandle(kind: kind, overlay: self)
            handles[kind] = handle; addSubview(handle)
        }
        isHidden = true
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { editingEnabled }

    var fittedImageRect: NSRect {
        if let presentedImageRect { return presentedImageRect }
        let width = CGFloat(sourceSize.width), height = CGFloat(sourceSize.height)
        let canvas = bounds.insetBy(dx: imageInset, dy: imageInset)
        guard width > 0, height > 0, canvas.width > 0, canvas.height > 0 else { return .zero }
        let scale = min(canvas.width / width, canvas.height / height)
        let size = NSSize(width: width * scale, height: height * scale)
        return NSRect(x: canvas.midX - size.width / 2, y: canvas.midY - size.height / 2,
                      width: size.width, height: size.height)
    }

    var displayedCropRect: NSRect {
        let image = fittedImageRect
        guard image.width > 0, image.height > 0 else { return .zero }
        let scaleX = image.width / CGFloat(sourceSize.width)
        let scaleY = image.height / CGFloat(sourceSize.height)
        return NSRect(x: image.minX + CGFloat(crop.x) * scaleX,
                      y: image.minY + CGFloat(crop.y) * scaleY,
                      width: CGFloat(crop.width) * scaleX,
                      height: CGFloat(crop.height) * scaleY)
    }

    func setEditingEnabled(_ enabled: Bool) {
        editingEnabled = enabled
        handles.values.forEach { $0.enabled = enabled }
        if !enabled { endDrag() }
        window?.invalidateCursorRects(for: self)
        needsDisplay = true
    }

    func beginDrag(_ kind: NativeRecordingCropDragHandle, at point: NSPoint) {
        guard editingEnabled, displayedCropRect.contains(point) || kind != .move else { return }
        activeHandle = kind; initialCrop = crop; initialPoint = point
    }

    func continueDrag(at point: NSPoint) {
        guard editingEnabled, let initialCrop, let initialPoint else { return }
        let image = fittedImageRect
        guard image.width > 0, image.height > 0 else { return }
        let deltaX = Double((point.x - initialPoint.x) * CGFloat(sourceSize.width) / image.width)
        let deltaY = Double((point.y - initialPoint.y) * CGFloat(sourceSize.height) / image.height)
        let kind = activeHandle ?? .move
        guard let value = NativeRecordingGeometry.afterDrag(initialCrop, source: sourceSize,
            handle: kind, deltaX: deltaX, deltaY: deltaY, lockAspect: lockAspect),
            value != crop else { return }
        crop = value; onStage?(value)
    }

    func endDrag() { initialCrop = nil; initialPoint = nil; activeHandle = nil }

    func nudge(_ kind: NativeRecordingCropDragHandle, deltaX: Double, deltaY: Double) {
        guard editingEnabled,
              let value = NativeRecordingGeometry.afterDrag(crop, source: sourceSize,
                handle: kind, deltaX: deltaX, deltaY: deltaY, lockAspect: lockAspect),
              value != crop else { return }
        crop = value; onStage?(value)
    }

    private var activeHandle: NativeRecordingCropDragHandle?
    private func handlePoint(_ kind: NativeRecordingCropDragHandle, rect: NSRect) -> NSPoint {
        switch kind {
        case .north: NSPoint(x: rect.midX, y: rect.minY)
        case .northEast: NSPoint(x: rect.maxX, y: rect.minY)
        case .east: NSPoint(x: rect.maxX, y: rect.midY)
        case .southEast: NSPoint(x: rect.maxX, y: rect.maxY)
        case .south: NSPoint(x: rect.midX, y: rect.maxY)
        case .southWest: NSPoint(x: rect.minX, y: rect.maxY)
        case .west: NSPoint(x: rect.minX, y: rect.midY)
        case .northWest: NSPoint(x: rect.minX, y: rect.minY)
        case .move: NSPoint(x: rect.midX, y: rect.midY)
        }
    }

    private func updateHandles() {
        let rect = displayedCropRect
        for (kind, handle) in handles {
            let point = handlePoint(kind, rect: rect)
            handle.frame = NSRect(x: point.x - 12, y: point.y - 12, width: 24, height: 24)
            handle.needsDisplay = true
        }
    }

    private func updateAccessibility() {
        setAccessibilityValueDescription(
            "X \(crop.x), Y \(crop.y), width \(crop.width), height \(crop.height)")
        for (kind, handle) in handles {
            handle.setAccessibilityValueDescription(
                "\(kind.accessibilityName), X \(crop.x), Y \(crop.y), width \(crop.width), height \(crop.height)")
        }
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        let local = superview.map { convert(point, from: $0) } ?? point
        guard !isHidden, bounds.contains(local), visibleRect.contains(local) else { return nil }
        if interceptsPendingInput {
            return fittedImageRect.contains(local) ? self : nil
        }
        guard editingEnabled else { return nil }
        let ordered = NativeRecordingCropDragHandle.allCases.reversed()
        if let kind = ordered.first(where: { handles[$0]?.frame.contains(local) == true }),
           let handle = handles[kind] { return handle }
        return displayedCropRect.contains(local) ? self : nil
    }

    override func mouseDown(with event: NSEvent) {
        if interceptsPendingInput {
            onCommitPendingInput?()
            return
        }
        guard editingEnabled else { return }
        window?.makeFirstResponder(self)
        beginDrag(.move, at: convert(event.locationInWindow, from: nil))
    }
    override func mouseDragged(with event: NSEvent) {
        continueDrag(at: convert(event.locationInWindow, from: nil))
    }
    override func mouseUp(with event: NSEvent) {
        continueDrag(at: convert(event.locationInWindow, from: nil)); endDrag()
    }
    override func keyDown(with event: NSEvent) {
        let step = event.modifierFlags.contains(.shift) ? 10.0 : 1.0
        switch event.keyCode {
        case 123: nudge(.move, deltaX: -step, deltaY: 0)
        case 124: nudge(.move, deltaX: step, deltaY: 0)
        case 125: nudge(.move, deltaX: 0, deltaY: step)
        case 126: nudge(.move, deltaX: 0, deltaY: -step)
        case 53: endDrag()
        default: super.keyDown(with: event)
        }
    }
    override func resignFirstResponder() -> Bool {
        endDrag(); return super.resignFirstResponder()
    }
    override func setFrameSize(_ newSize: NSSize) {
        if newSize != frame.size { endDrag() }
        super.setFrameSize(newSize); updateHandles()
    }
    override func resetCursorRects() {
        if editingEnabled { addCursorRect(displayedCropRect, cursor: .openHand) }
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard !isHidden else { return }
        let image = fittedImageRect, selection = displayedCropRect
        guard image.width > 0, image.height > 0, selection.width > 0, selection.height > 0 else { return }
        NSGraphicsContext.saveGraphicsState(); defer { NSGraphicsContext.restoreGraphicsState() }
        NSBezierPath(rect: image.intersection(bounds)).addClip()
        tokens.color("glass-veil-heavy").setFill()
        for area in [
            NSRect(x: image.minX, y: image.minY, width: image.width,
                   height: max(0, selection.minY - image.minY)),
            NSRect(x: image.minX, y: selection.maxY, width: image.width,
                   height: max(0, image.maxY - selection.maxY)),
            NSRect(x: image.minX, y: selection.minY,
                   width: max(0, selection.minX - image.minX), height: selection.height),
            NSRect(x: selection.maxX, y: selection.minY,
                   width: max(0, image.maxX - selection.maxX), height: selection.height),
        ] { NSBezierPath(rect: area).fill() }
        let border = NSBezierPath(rect: selection)
        tokens.color(editingEnabled ? "theme-accent" : "text-faint").setStroke()
        border.lineWidth = 2; border.stroke()
    }
}

private extension NativeRecordingCropDragHandle {
    var accessibilityName: String {
        switch self {
        case .move: "move"
        case .north: "north"
        case .northEast: "north-east"
        case .east: "east"
        case .southEast: "south-east"
        case .south: "south"
        case .southWest: "south-west"
        case .west: "west"
        case .northWest: "north-west"
        }
    }
}

private enum RecordingPlaybackState {
    case idle
    case playing
    case pausing
}

final class RecordingEditorController: NSObject, NSWindowDelegate, NSTextFieldDelegate {
    let window: NSWindow
    let root = Surface()
    private let worker: RecordingEditorWorking
    private let reportError: (String) -> Void
    private let didSaveCopy: () -> Void
    private let confirmDiscard: () -> Bool
    private let requestTermination: () -> Void
    private var tokens: Tokens
    private var generation = 0
    private var artifactID: String?
    private var presentation: RecordingEditorPresentation?
    private var savedEdit: Data?
    private var savedExport: Data?
    private var busy = false
    private var pickerOpen = false
    private var activeCancel: NativeRecordingEditorCancel?
    private var thumbnailCancel: NativeRecordingEditorCancel?
    private var thumbnailRetryAvailable = false
    private var estimate: RecordingEditorEstimate?
    private var playbackState = RecordingPlaybackState.idle
    private var playbackCancel: NativeRecordingEditorCancel?
    private var playbackPositionMilliseconds: UInt64?
    private var playbackReachedEOF = false
    private var playbackFramePresented = false
    private var playbackLoopEnabled = false
    private var playbackSoundEnabled = false
    private var playbackAudioEnabled: Bool?
    private var gifFramesPerSecond: UInt16 = 15
    private var playbackLoopControl: RecordingPlaybackLoopControl?
    private var sourceFrameCache: RecordingSourceImage?
    private var sourceFrameCancel: NativeRecordingEditorCancel?
    private var cropAdjustmentActive = false
    private var cropAdjustmentPriorImage: NSImage?
    private var previewActualSize = false
    private var playbackStopActions: [() -> Void] = []
    private var closeAfterPlayback = false
    private var terminateAfterPlayback = false
    private var switchAfterPlayback: String?

    private let previewPanel = Surface()
    private let previewTitle = NSTextField(labelWithString: "Preview")
    private let previewScroll = NSScrollView()
    private let previewCanvas = Surface()
    private let preview = NSImageView()
    private let cropOverlay: RecordingCropOverlay
    private let geometryPanel = Surface()
    private let cropEnabled = NSButton(checkboxWithTitle: "Crop recording", target: nil, action: nil)
    private let cropLock = NSButton(checkboxWithTitle: "Lock aspect ratio", target: nil, action: nil)
    private let cropX = NSTextField()
    private let cropY = NSTextField()
    private let cropWidth = NSTextField()
    private let cropHeight = NSTextField()
    private let outputMode = NSPopUpButton()
    private let outputWidth = NSTextField()
    private let outputHeight = NSTextField()
    private var stagedCrop: NativeRecordingCropRect?
    private var cropAspectUnlocked = false
    private var resolutionPreset = NativeRecordingResolutionPreset.original
    private var customOutput = false
    private let sourceLabel = NSTextField(labelWithString: "Opening recording…")
    private let seekSlider = NSSlider(value: 0, minValue: 0, maxValue: 1,
                                      target: nil, action: nil)
    private let seekLabel = NSTextField(labelWithString: "0:00.000 / 0:00.000")
    private let trimPanel = Surface()
    private let trimStart = NSTextField()
    private let trimEnd = NSTextField()
    private let trimTimeline: RecordingTrimTimeline
    private let audioPanel = Surface()
    private let systemVolume = NSTextField()
    private let microphoneVolume = NSTextField()
    private let systemMute = NSButton(checkboxWithTitle: "Mute", target: nil, action: nil)
    private let microphoneMute = NSButton(checkboxWithTitle: "Mute", target: nil, action: nil)
    private let monoOutput = NSButton(checkboxWithTitle: "Mono output", target: nil, action: nil)
    private var systemAudioLabel: NSTextField!
    private var microphoneAudioLabel: NSTextField!
    private var audioNote: NSTextField!
    private let format = NSPopUpButton()
    private let quality = NSPopUpButton()
    private let gifFrameRateLabel = NSTextField(labelWithString: "GIF FPS")
    private let gifFrameRate = NSPopUpButton()
    private let destination = NSTextField()
    private let status = NSTextField(wrappingLabelWithString: "")
    private let estimateLabel = NSTextField(labelWithString: "Size not estimated")
    private let progress = NSProgressIndicator()
    private var applyButton: CaptureButton!
    private var estimateButton: CaptureButton!
    private var saveButton: CaptureButton!
    private var cancelButton: CaptureButton!
    private var changeButton: CaptureButton!
    private var thumbnailRetryButton: CaptureButton!
    private var playbackButton: CaptureButton!
    private var cropAdjustmentButton: CaptureButton!
    private var previewFitButton: CaptureButton!
    private var previewActualButton: CaptureButton!
    private let playbackLoop = NSButton(checkboxWithTitle: "Loop", target: nil, action: nil)
    private let playbackSound = NSButton(checkboxWithTitle: "Sound", target: nil, action: nil)

    init(tokens: Tokens, worker: RecordingEditorWorking = RecordingEditorWorker(),
         reportError: @escaping (String) -> Void = { _ in },
         didSaveCopy: @escaping () -> Void = {},
         confirmDiscard: (() -> Bool)? = nil,
         requestTermination: @escaping () -> Void = { NSApp.terminate(nil) }) {
        self.tokens = tokens; self.worker = worker; self.reportError = reportError
        self.didSaveCopy = didSaveCopy
        self.confirmDiscard = confirmDiscard ?? RecordingEditorController.confirmDiscardAlert
        self.requestTermination = requestTermination
        trimTimeline = RecordingTrimTimeline(tokens: tokens)
        cropOverlay = RecordingCropOverlay(tokens: tokens)
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 960, height: 760),
                          styleMask: [.titled, .closable, .miniaturizable, .resizable],
                          backing: .buffered, defer: false)
        super.init()
        window.isReleasedWhenClosed = false
        window.title = "Recording editor"
        window.minSize = NSSize(width: 760, height: 540)
        window.appearance = NSAppearance(named: tokens.color("text").brightnessComponent > 0.5
            ? .darkAqua : .aqua)
        window.delegate = self
        root.frame = window.contentView?.bounds ?? NSRect(x: 0, y: 0, width: 960, height: 760)
        root.autoresizingMask = [.width, .height]
        root.wantsLayer = true
        window.contentView = root
        buildUI()
        layout()
    }

    func present(artifact: CaptureArtifact, historyRoot: String, outputDirectory: String) {
        if artifactID == artifact.id {
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return
        }
        if playbackState != .idle {
            guard switchAfterPlayback == nil else { return }
            switchAfterPlayback = artifact.id
            pausePlayback { [weak self] in
                guard let self else { return }
                self.switchAfterPlayback = nil
                self.present(artifact: artifact, historyRoot: historyRoot,
                             outputDirectory: outputDirectory)
            }
            return
        }
        if artifactID != nil, busy || pickerOpen || dirty {
            showError("Finish, cancel, save, or discard the current recording edits first.")
            window.makeKeyAndOrderFront(nil); return
        }
        generation += 1
        let current = generation
        artifactID = artifact.id; presentation = nil; savedEdit = nil; savedExport = nil
        estimate = nil; activeCancel = nil; thumbnailCancel = nil; busy = true; pickerOpen = false
        playbackPositionMilliseconds = nil; playbackReachedEOF = false; playbackFramePresented = false
        playbackLoopEnabled = false; playbackLoopControl = nil; playbackLoop.state = .off
        playbackSoundEnabled = false; playbackAudioEnabled = nil; playbackSound.state = .off
        gifFramesPerSecond = 15; gifFrameRate.selectItem(withTitle: "15 FPS")
        sourceFrameCache = nil; sourceFrameCancel = nil
        cropAdjustmentActive = false; cropAdjustmentPriorImage = nil
        previewActualSize = false
        cropOverlay.isHidden = true; cropOverlay.setEditingEnabled(false)
        stagedCrop = nil; cropAspectUnlocked = false
        resolutionPreset = .original; customOutput = false
        setPreviewImage(nil)
        trimTimeline.clearThumbnails()
        destination.stringValue = URL(fileURLWithPath: outputDirectory)
            .appendingPathComponent("recording-edit-\(artifact.id.prefix(8)).mp4").path
        sourceLabel.stringValue = "Opening recording…"
        status.stringValue = "Decoding the first source-relative frame…"
        progress.isHidden = true
        updateControls(); layout()
        window.center(); window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        worker.open(historyRoot: historyRoot, artifactID: artifact.id) { [weak self] result in
            guard let self, self.generation == current, self.artifactID == artifact.id else { return }
            switch result {
            case .success(let value):
                self.busy = false
                self.publish(value, initialize: true)
                self.status.stringValue = "Original remains unchanged. Save creates a new copy."
                self.generateThumbnails()
            case .failure(let error):
                self.busy = false
                self.showError("Couldn’t open recording: \(error.localizedDescription)")
            }
            self.updateControls(); self.layout()
        }
    }

    var dirty: Bool {
        guard let snapshot = presentation?.snapshot else { return false }
        return stagedDiffers || canonicalEdit(snapshot.edit) != savedEdit
            || canonical(snapshot.export) != savedExport
    }

    func prepareForTermination() -> Bool {
        if playbackState != .idle {
            if !terminateAfterPlayback {
                terminateAfterPlayback = true
                pausePlayback { [weak self] in
                    guard let self else { return }
                    self.terminateAfterPlayback = false
                    self.requestTermination()
                }
            }
            showError("Pausing recording playback before quitting…")
            window.makeKeyAndOrderFront(nil); return false
        }
        if busy || pickerOpen {
            showError("Cancel or wait for the recording operation before quitting.")
            window.makeKeyAndOrderFront(nil); return false
        }
        if dirty {
            showError("Save or discard recording edits before quitting. Recording drafts are not available.")
            window.makeKeyAndOrderFront(nil); return false
        }
        closeSession(); return true
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if playbackState != .idle {
            if !closeAfterPlayback {
                closeAfterPlayback = true
                pausePlayback { [weak self] in
                    guard let self else { return }
                    self.closeAfterPlayback = false
                    if self.windowShouldClose(self.window) { self.window.close() }
                }
            }
            return false
        }
        if busy || pickerOpen {
            showError("Cancel or wait for the recording operation before closing.")
            return false
        }
        if dirty && !confirmDiscard() { return false }
        closeSession(); return true
    }

    func windowDidResize(_ notification: Notification) { cropOverlay.endDrag(); layout() }
    func windowDidResignKey(_ notification: Notification) {
        trimTimeline.endDrag(); cropOverlay.endDrag(); pausePlayback()
    }
    func windowDidMiniaturize(_ notification: Notification) { pausePlayback() }
    func controlTextDidChange(_ notification: Notification) {
        if let field = notification.object as? NSTextField,
           [cropX, cropY, cropWidth, cropHeight].contains(where: { $0 === field }) {
            estimate = nil; updateControls(); return
        }
        estimate = nil; syncTimelineFromFields(); updateControls()
    }
    func controlTextDidEndEditing(_ notification: Notification) {
        guard let field = notification.object as? NSTextField,
              [cropX, cropY, cropWidth, cropHeight].contains(where: { $0 === field }) else { return }
        commitCropField(field)
    }

    private func buildUI() {
        root.layer?.backgroundColor = tokens.color("surface-canvas").cgColor
        _ = label("Edit recording", size: 22, weight: .semibold)
        let note = label("Playback uses accepted recording edits only", muted: true)
        note.identifier = NSUserInterfaceItemIdentifier("recording-editor-note")
        previewPanel.wantsLayer = true
        previewPanel.layer?.backgroundColor = tokens.color("surface-sunken").cgColor
        previewPanel.layer?.cornerRadius = tokens.number("r-md")
        previewTitle.font = .systemFont(ofSize: 12, weight: .semibold)
        previewTitle.textColor = tokens.color("text")
        previewPanel.addSubview(previewTitle)
        previewFitButton = button("Fit") { [weak self] in self?.setPreviewActualSize(false) }
        previewActualButton = button("100%") { [weak self] in self?.setPreviewActualSize(true) }
        previewFitButton.toolTip = "Fit the decoded frame within the preview."
        previewActualButton.toolTip = "One decoded image pixel per screen point. Scroll to see overflow; playback may use a reduced-size frame."
        previewPanel.addSubview(previewFitButton); previewPanel.addSubview(previewActualButton)
        previewScroll.drawsBackground = false; previewScroll.borderType = .noBorder
        previewScroll.scrollerStyle = .overlay; previewScroll.autohidesScrollers = true
        previewScroll.contentView.drawsBackground = false
        previewScroll.contentView.postsBoundsChangedNotifications = true
        previewScroll.setAccessibilityLabel("Recording preview viewport")
        previewScroll.documentView = previewCanvas
        previewPanel.addSubview(previewScroll)
        preview.imageScaling = .scaleProportionallyUpOrDown
        preview.setAccessibilityLabel("Decoded recording frame")
        previewCanvas.addSubview(preview)
        cropOverlay.toolTip = "Drag inside to move. Drag a handle to resize. Arrow keys move a focused handle by 1 source pixel; Shift moves 10."
        cropOverlay.onStage = { [weak self] crop in self?.stageGraphicalCrop(crop) }
        cropOverlay.onCommitPendingInput = { [weak self] in
            guard let self else { return }
            self.window.makeFirstResponder(nil)
            _ = self.commitPendingCropInput()
            self.updateControls()
        }
        previewCanvas.addSubview(cropOverlay)
        root.addSubview(previewPanel)
        NotificationCenter.default.addObserver(self, selector: #selector(previewDidScroll(_:)),
            name: NSView.boundsDidChangeNotification, object: previewScroll.contentView)

        geometryPanel.wantsLayer = true
        geometryPanel.layer?.backgroundColor = tokens.color("surface-raised").cgColor
        geometryPanel.layer?.cornerRadius = tokens.number("r-md")
        root.addSubview(geometryPanel)
        geometryPanel.addSubview(label("Crop & output", size: 14, weight: .semibold,
                                       parent: geometryPanel))
        cropEnabled.target = self; cropEnabled.action = #selector(cropEnabledChanged)
        cropEnabled.setAccessibilityLabel("Crop recording")
        cropLock.target = self; cropLock.action = #selector(cropLockChanged)
        cropLock.setAccessibilityLabel("Lock recording crop aspect ratio")
        geometryPanel.addSubview(cropEnabled); geometryPanel.addSubview(cropLock)
        cropAdjustmentButton = button("Adjust crop") { [weak self] in self?.toggleCropAdjustment() }
        cropAdjustmentButton.setAccessibilityLabel("Adjust recording crop graphically")
        geometryPanel.addSubview(cropAdjustmentButton)
        for (field, accessibilityLabel) in [
            (cropX, "Recording crop X"), (cropY, "Recording crop Y"),
            (cropWidth, "Recording crop width"), (cropHeight, "Recording crop height"),
            (outputWidth, "Recording output width"), (outputHeight, "Recording output height"),
        ] {
            configureNumberField(field, label: accessibilityLabel)
            field.alignment = .right
            if [cropX, cropY, cropWidth, cropHeight].contains(where: { $0 === field }) {
                field.target = self; field.action = #selector(cropFieldCommitted(_:))
            }
            geometryPanel.addSubview(field)
        }
        for title in ["X", "Y", "W", "H", "Output", "×"] {
            geometryPanel.addSubview(label(title, muted: true, parent: geometryPanel))
        }
        for preset in NativeRecordingResolutionPreset.allCases { outputMode.addItem(withTitle: preset.title) }
        outputMode.addItem(withTitle: "Custom")
        outputMode.target = self; outputMode.action = #selector(outputModeChanged)
        outputMode.setAccessibilityLabel("Recording output size")
        geometryPanel.addSubview(outputMode)

        sourceLabel.textColor = tokens.color("text-muted")
        sourceLabel.font = .systemFont(ofSize: 12)
        sourceLabel.setAccessibilityLabel("Recording source details")
        root.addSubview(sourceLabel)
        seekSlider.target = self; seekSlider.action = #selector(seekChanged)
        seekSlider.setAccessibilityLabel("Recording frame position")
        root.addSubview(seekSlider)
        seekLabel.textColor = tokens.color("text-muted"); seekLabel.alignment = .right
        root.addSubview(seekLabel)
        playbackButton = button("Play") { [weak self] in self?.togglePlayback() }
        playbackButton.setAccessibilityLabel("Play silent recording preview")
        playbackLoop.target = self; playbackLoop.action = #selector(playbackLoopChanged)
        playbackLoop.setAccessibilityLabel("Loop recording preview")
        playbackSound.target = self; playbackSound.action = #selector(playbackSoundChanged)
        playbackSound.setAccessibilityLabel("Preview accepted recording audio")
        root.addSubview(playbackLoop)
        root.addSubview(playbackSound)
        trimPanel.wantsLayer = true; trimPanel.layer?.backgroundColor = tokens.color("surface-raised").cgColor
        trimPanel.layer?.cornerRadius = tokens.number("r-md")
        root.addSubview(trimPanel)
        configureNumberField(trimStart, label: "Trim start milliseconds")
        configureNumberField(trimEnd, label: "Trim end milliseconds")
        trimPanel.addSubview(label("Trim (milliseconds)", size: 14, weight: .semibold))
        trimTimeline.onStage = { [weak self] edge, milliseconds in
            guard let self else { return }
            (edge == .start ? self.trimStart : self.trimEnd).stringValue = String(milliseconds)
            self.estimate = nil; self.updateControls()
        }
        trimPanel.addSubview(trimTimeline)
        thumbnailRetryButton = button("Retry") { [weak self] in self?.generateThumbnails() }
        thumbnailRetryButton.setAccessibilityLabel("Retry recording thumbnails")
        trimPanel.addSubview(thumbnailRetryButton)
        trimPanel.addSubview(label("Start", muted: true)); trimPanel.addSubview(trimStart)
        trimPanel.addSubview(label("End", muted: true)); trimPanel.addSubview(trimEnd)
        applyButton = button("Apply edits") { [weak self] in self?.applyEdits() }
        trimPanel.addSubview(applyButton)

        audioPanel.wantsLayer = true
        audioPanel.layer?.backgroundColor = tokens.color("surface-raised").cgColor
        audioPanel.layer?.cornerRadius = tokens.number("r-md")
        root.addSubview(audioPanel)
        audioPanel.addSubview(label("Audio", size: 14, weight: .semibold, parent: audioPanel))
        audioNote = label("Sound preview off", muted: true, parent: audioPanel)
        configureVolumeField(systemVolume, label: "System audio volume percent")
        configureVolumeField(microphoneVolume, label: "Microphone volume percent")
        systemAudioLabel = label("System", muted: true, parent: audioPanel)
        microphoneAudioLabel = label("Microphone", muted: true, parent: audioPanel)
        for control in [systemMute, microphoneMute, monoOutput] {
            control.target = self; control.action = #selector(stageChanged)
            audioPanel.addSubview(control)
        }
        systemMute.setAccessibilityLabel("Mute system audio")
        microphoneMute.setAccessibilityLabel("Mute microphone")
        monoOutput.setAccessibilityLabel("Mono audio output")
        audioPanel.addSubview(systemVolume); audioPanel.addSubview(microphoneVolume)

        format.addItems(withTitles: ["MP4", "GIF"])
        format.target = self; format.action = #selector(formatChanged)
        format.setAccessibilityLabel("Recording export format")
        quality.addItems(withTitles: ["Preserve", "Highest", "High", "Standard", "Small", "Tiny"])
        quality.target = self; quality.action = #selector(stageChanged)
        quality.setAccessibilityLabel("Recording export quality")
        gifFrameRate.addItems(withTitles: ["8 FPS", "10 FPS", "12 FPS", "15 FPS",
                                              "20 FPS", "24 FPS", "30 FPS"])
        gifFrameRate.selectItem(withTitle: "15 FPS")
        gifFrameRate.target = self; gifFrameRate.action = #selector(gifFrameRateChanged)
        gifFrameRate.setAccessibilityLabel("GIF frame rate")
        gifFrameRateLabel.textColor = tokens.color("text-muted")
        destination.delegate = self; destination.setAccessibilityLabel("Recording destination")
        root.addSubview(format); root.addSubview(quality); root.addSubview(destination)
        root.addSubview(gifFrameRateLabel); root.addSubview(gifFrameRate)
        changeButton = button("Change…") { [weak self] in self?.chooseDestination() }
        estimateButton = button("Estimate size") { [weak self] in self?.estimateSize() }
        saveButton = button("Save new copy") { [weak self] in self?.saveNewCopy() }
        saveButton.primary = true
        cancelButton = button("Cancel operation") { [weak self] in self?.cancelActiveOperation() }
        cancelButton.signal = true
        status.textColor = tokens.color("text-muted"); status.maximumNumberOfLines = 2
        status.setAccessibilityLabel("Recording editor status")
        estimateLabel.textColor = tokens.color("text-muted")
        estimateLabel.setAccessibilityLabel("Recording size estimate")
        progress.minValue = 0; progress.maxValue = 1000; progress.isIndeterminate = false
        progress.setAccessibilityLabel("Recording export progress")
        root.addSubview(status); root.addSubview(estimateLabel); root.addSubview(progress)
    }

    private func layout() {
        let width = root.bounds.width, height = root.bounds.height
        guard width > 0, height > 0 else { return }
        root.subviews.first { ($0 as? NSTextField)?.stringValue == "Edit recording" }?.frame =
            NSRect(x: 24, y: 18, width: width - 48, height: 28)
        root.subviews.first { $0.identifier?.rawValue == "recording-editor-note" }?.frame =
            NSRect(x: 24, y: 48, width: width - 48, height: 20)
        let saveHeight: CGFloat = 150
        let trimHeight: CGFloat = 116
        let previewHeight = max(150, height - saveHeight - trimHeight - 116)
        let availableWidth = width - 48
        let geometryWidth = max(328, min(380, availableWidth * 0.42))
        previewPanel.frame = NSRect(x: 24, y: 76,
                                    width: availableWidth - geometryWidth - 12,
                                    height: previewHeight)
        previewTitle.frame = NSRect(x: 12, y: 8, width: 80, height: 20)
        previewActualButton.frame = NSRect(x: previewPanel.bounds.width - 66, y: 5,
                                           width: 54, height: 26)
        previewFitButton.frame = NSRect(x: previewActualButton.frame.minX - 50, y: 5,
                                        width: 46, height: 26)
        previewScroll.frame = NSRect(x: 0, y: 34, width: previewPanel.bounds.width,
                                     height: max(0, previewPanel.bounds.height - 34))
        refreshPreviewLayout(resetScroll: false)
        geometryPanel.frame = NSRect(x: previewPanel.frame.maxX + 12, y: 76,
                                     width: geometryWidth, height: previewHeight)
        let geometryLabels = geometryPanel.subviews.compactMap { $0 as? NSTextField }
            .filter { !$0.isEditable }
        geometryLabels.first { $0.stringValue == "Crop & output" }?.frame =
            NSRect(x: 14, y: 12, width: 130, height: 20)
        cropAdjustmentButton.frame = NSRect(x: geometryPanel.bounds.width - 122, y: 7,
                                            width: 108, height: 28)
        cropEnabled.frame = NSRect(x: 14, y: 36, width: 124, height: 24)
        cropLock.frame = NSRect(x: 142, y: 36, width: 150, height: 24)
        for (index, title) in ["X", "Y", "W", "H"].enumerated() {
            let x = CGFloat(14 + index * 72)
            geometryLabels.first { $0.stringValue == title }?.frame =
                NSRect(x: x, y: 69, width: 14, height: 18)
            [cropX, cropY, cropWidth, cropHeight][index].frame =
                NSRect(x: x + 16, y: 63, width: 50, height: 28)
        }
        geometryLabels.first { $0.stringValue == "Output" }?.frame =
            NSRect(x: 14, y: 105, width: 48, height: 18)
        geometryLabels.first { $0.stringValue == "×" }?.frame =
            NSRect(x: 275, y: 104, width: 12, height: 18)
        outputMode.frame = NSRect(x: 62, y: 98, width: 146, height: 28)
        outputWidth.frame = NSRect(x: 216, y: 98, width: 56, height: 28)
        outputHeight.frame = NSRect(x: 286, y: 98,
                                    width: max(42, geometryPanel.bounds.width - 300), height: 28)
        let seekY = previewPanel.frame.maxY + 10
        sourceLabel.frame = NSRect(x: 24, y: seekY, width: width * 0.27 - 24, height: 20)
        playbackButton.frame = NSRect(x: width * 0.27, y: seekY - 4, width: 104, height: 28)
        seekLabel.frame = NSRect(x: width * 0.77, y: seekY, width: width * 0.2 - 24, height: 20)
        let seekGap = tokens.number("s-2")
        playbackLoop.frame = NSRect(x: playbackButton.frame.maxX + seekGap, y: seekY - 2,
                                    width: 64, height: 24)
        playbackSound.frame = NSRect(x: playbackLoop.frame.maxX + seekGap, y: seekY - 2,
                                     width: 74, height: 24)
        let seekX = playbackSound.frame.maxX + seekGap
        seekSlider.frame = NSRect(x: seekX, y: seekY,
                                  width: max(0, seekLabel.frame.minX - seekGap - seekX), height: 20)
        let controlGap: CGFloat = 12
        let controlsWidth = width - 48
        let audioWidth = max(304, min(360, controlsWidth * 0.4))
        trimPanel.frame = NSRect(x: 24, y: seekY + 30,
                                 width: controlsWidth - audioWidth - controlGap, height: trimHeight)
        audioPanel.frame = NSRect(x: trimPanel.frame.maxX + controlGap, y: seekY + 30,
                                  width: audioWidth, height: trimHeight)
        let labels = trimPanel.subviews.compactMap { $0 as? NSTextField }.filter { !$0.isEditable }
        labels.first { $0.stringValue == "Trim (milliseconds)" }?.frame = NSRect(x: 14, y: 12, width: 150, height: 20)
        trimTimeline.frame = NSRect(x: 154, y: 8, width: trimPanel.bounds.width - 168, height: 28)
        thumbnailRetryButton.frame = NSRect(x: trimPanel.bounds.width - 88, y: 40,
                                            width: 74, height: 28)
        labels.first { $0.stringValue == "Start" }?.frame = NSRect(x: 14, y: 82, width: 42, height: 18)
        labels.first { $0.stringValue == "End" }?.frame = NSRect(x: 138, y: 82, width: 34, height: 18)
        trimStart.frame = NSRect(x: 52, y: 76, width: 78, height: 28)
        trimEnd.frame = NSRect(x: 172, y: 76, width: 78, height: 28)
        applyButton.frame = NSRect(x: trimPanel.bounds.width - 112, y: 76, width: 98, height: 30)
        let explanation = labels.first { $0.stringValue.hasPrefix("Apply before") }
            ?? label("Apply before seeking or saving. The original is immutable.", muted: true,
                     parent: trimPanel)
        explanation.stringValue = thumbnailRetryAvailable
            ? "Apply before seeking or saving."
            : "Apply before seeking or saving. The original is immutable."
        explanation.frame = NSRect(x: 14, y: 46,
            width: trimPanel.bounds.width - (thumbnailRetryAvailable ? 116 : 28), height: 20)

        let audioLabels = audioPanel.subviews.compactMap { $0 as? NSTextField }.filter { !$0.isEditable }
        audioLabels.first { $0.stringValue == "Audio" }?.frame = NSRect(x: 14, y: 12, width: 54, height: 20)
        audioNote.frame = NSRect(x: 68, y: 12, width: audioPanel.bounds.width - 194, height: 20)
        systemAudioLabel.frame = NSRect(x: 14, y: 48, width: 78, height: 18)
        microphoneAudioLabel.frame = NSRect(x: 14, y: 82, width: 78, height: 18)
        systemVolume.frame = NSRect(x: 94, y: 42, width: 68, height: 28)
        microphoneVolume.frame = NSRect(x: 94, y: 76, width: 68, height: 28)
        systemMute.frame = NSRect(x: 170, y: 44, width: 72, height: 24)
        microphoneMute.frame = NSRect(x: 170, y: 78, width: 72, height: 24)
        monoOutput.frame = NSRect(x: audioPanel.bounds.width - 112, y: 8, width: 102, height: 24)

        let barY = height - saveHeight
        status.frame = NSRect(x: 24, y: barY + 8, width: width - 48, height: 36)
        progress.frame = NSRect(x: 24, y: barY + 44, width: width - 174, height: 16)
        cancelButton.frame = NSRect(x: width - 140, y: barY + 38, width: 116, height: 28)
        changeButton.frame = NSRect(x: width - 116, y: barY + 70, width: 92, height: 30)
        gifFrameRate.frame = NSRect(x: changeButton.frame.minX - 86, y: barY + 72,
                                    width: 78, height: 28)
        gifFrameRateLabel.frame = NSRect(x: gifFrameRate.frame.minX - 62, y: barY + 77,
                                         width: 58, height: 20)
        let destinationEnd = format.indexOfSelectedItem == 1
            ? gifFrameRateLabel.frame.minX - 8 : changeButton.frame.minX - 10
        destination.frame = NSRect(x: 24, y: barY + 72,
                                   width: max(0, destinationEnd - 24), height: 28)
        format.frame = NSRect(x: 24, y: barY + 110, width: 92, height: 28)
        quality.frame = NSRect(x: 124, y: barY + 110, width: 116, height: 28)
        estimateLabel.frame = NSRect(x: 252, y: barY + 114, width: 190, height: 20)
        estimateButton.frame = NSRect(x: width - 296, y: barY + 106, width: 126, height: 32)
        saveButton.frame = NSRect(x: width - 160, y: barY + 106, width: 136, height: 32)
    }

    private func setPreviewActualSize(_ actualSize: Bool) {
        guard previewActualSize != actualSize else { return }
        previewActualSize = actualSize
        cropOverlay.endDrag()
        refreshPreviewLayout(resetScroll: true)
        updateControls()
    }

    @objc private func previewDidScroll(_ notification: Notification) {
        guard let clipView = notification.object as? NSClipView,
              clipView === previewScroll.contentView else { return }
        cropOverlay.endDrag()
    }

    private func setPreviewImage(_ image: NSImage?) {
        preview.image = image
        refreshPreviewLayout(resetScroll: false)
    }

    private func refreshPreviewLayout(resetScroll: Bool) {
        let viewport = previewScroll.contentSize
        guard viewport.width > 0, viewport.height > 0 else { return }
        let imageSize = preview.image?.size ?? .zero
        let actualSize = previewActualSize && imageSize.width > 0 && imageSize.height > 0
        let margin: CGFloat = actualSize && cropAdjustmentActive ? 12 : 0
        let canvasSize = actualSize
            ? NSSize(width: max(viewport.width, imageSize.width + margin * 2),
                     height: max(viewport.height, imageSize.height + margin * 2))
            : viewport
        previewCanvas.frame = NSRect(origin: .zero, size: canvasSize)
        let imageRect: NSRect
        if actualSize {
            imageRect = NSRect(x: (canvasSize.width - imageSize.width) / 2,
                               y: (canvasSize.height - imageSize.height) / 2,
                               width: imageSize.width, height: imageSize.height)
        } else {
            imageRect = previewCanvas.bounds.insetBy(dx: 12, dy: 12)
        }
        preview.frame = imageRect
        cropOverlay.frame = previewCanvas.bounds
        cropOverlay.presentedImageRect = actualSize ? imageRect : nil
        previewScroll.hasHorizontalScroller = actualSize && canvasSize.width > viewport.width
        previewScroll.hasVerticalScroller = actualSize && canvasSize.height > viewport.height
        if resetScroll || !actualSize {
            previewScroll.contentView.scroll(to: .zero)
            previewScroll.reflectScrolledClipView(previewScroll.contentView)
        }
    }

    private func publish(_ value: RecordingEditorPresentation, initialize: Bool = false) {
        let old = presentation?.snapshot
        if old?.artifactID != value.snapshot.artifactID
            || old?.positionMilliseconds != value.snapshot.positionMilliseconds {
            sourceFrameCache = nil
        }
        presentation = value
        playbackPositionMilliseconds = nil; playbackReachedEOF = false; playbackFramePresented = false
        playbackAudioEnabled = nil
        trimTimeline.setPlaybackPosition(nil)
        setPreviewImage(NSImage(cgImage: value.image,
                                size: NSSize(width: CGFloat(value.image.width),
                                             height: CGFloat(value.image.height))))
        sourceLabel.stringValue = "\(value.snapshot.width) × \(value.snapshot.height) source frame"
        seekSlider.maxValue = Double(max(1, value.snapshot.durationMilliseconds))
        seekSlider.doubleValue = Double(value.snapshot.positionMilliseconds)
        seekLabel.stringValue = "\(time(value.snapshot.positionMilliseconds)) / \(time(value.snapshot.durationMilliseconds))"
        let start = (value.snapshot.edit["trim_start_ms"] as? NSNumber)?.uint64Value ?? 0
        let end = (value.snapshot.edit["trim_end_ms"] as? NSNumber)?.uint64Value
            ?? value.snapshot.durationMilliseconds
        trimStart.stringValue = String(start)
        trimEnd.stringValue = String(end)
        trimTimeline.setValues(start: start, end: end,
                               duration: value.snapshot.durationMilliseconds)
        let audio = value.snapshot.edit["audio"] as? [String: Any] ?? [:]
        systemVolume.stringValue = volumePercent(audio["system_volume"])
        microphoneVolume.stringValue = volumePercent(audio["microphone_volume"])
        systemMute.state = (audio["mute_system_audio"] as? Bool ?? false) ? .on : .off
        microphoneMute.state = (audio["mute_microphone"] as? Bool ?? false) ? .on : .off
        monoOutput.state = (audio["mono_output"] as? Bool ?? false) ? .on : .off
        if let source = sourceDimensions(value.snapshot) {
            stagedCrop = NativeRecordingCropRect(value: value.snapshot.edit["crop"],
                                                  sourceWidth: source.width,
                                                  sourceHeight: source.height)
            cropEnabled.state = stagedCrop == nil ? .off : .on
            cropLock.state = cropAspectUnlocked ? .off : .on
            if initialize {
                if let output = editOutputDimensions(value.snapshot.edit) {
                    customOutput = true
                    outputMode.selectItem(withTitle: "Custom")
                    outputWidth.stringValue = String(output.width)
                    outputHeight.stringValue = String(output.height)
                } else {
                    customOutput = false; resolutionPreset = .original
                    outputMode.selectItem(withTitle: resolutionPreset.title)
                }
            } else if customOutput, let output = editOutputDimensions(value.snapshot.edit) {
                outputWidth.stringValue = String(output.width)
                outputHeight.stringValue = String(output.height)
            }
            refreshGeometryFields(source: source, preserveCustom: customOutput)
        }
        select(format, value: value.snapshot.export["format"] as? String ?? "mp4")
        select(quality, value: value.snapshot.export["quality"] as? String ?? "preserve")
        if format.indexOfSelectedItem == 1 {
            let accepted = (value.snapshot.export["frames_per_second"] as? NSNumber)?.uint16Value
            let supported: [UInt16] = [8, 10, 12, 15, 20, 24, 30]
            gifFramesPerSecond = accepted.flatMap { supported.contains($0) ? $0 : nil } ?? 15
            gifFrameRate.selectItem(withTitle: "\(gifFramesPerSecond) FPS")
        }
        if initialize {
            savedEdit = canonicalEdit(value.snapshot.edit); savedExport = canonical(value.snapshot.export)
        } else if canonicalEdit(old?.edit) != canonicalEdit(value.snapshot.edit)
                    || canonical(old?.export) != canonical(value.snapshot.export) {
            estimate = nil
        }
        updateControls()
    }

    private var stagedEdit: [String: Any]? {
        guard let accepted = presentation?.snapshot.edit,
              let start = UInt64(trimStart.stringValue), let end = UInt64(trimEnd.stringValue),
              let duration = presentation?.snapshot.durationMilliseconds,
              let source = presentation?.snapshot,
              let sourceSize = sourceDimensions(source),
              let outputSize = stagedOutputDimensions(source: sourceSize),
              start < end, end <= duration else { return nil }
        var edit = accepted
        edit["trim_start_ms"] = start
        edit["trim_end_ms"] = end == duration ? NSNull() : end
        edit["crop"] = stagedCrop == nil ? NSNull() : stagedCrop!.dictionary
        if customOutput || resolutionPreset != .original {
            edit["output_width"] = outputSize.width
            edit["output_height"] = outputSize.height
        } else {
            edit["output_width"] = NSNull()
            edit["output_height"] = NSNull()
        }
        var audio = accepted["audio"] as? [String: Any] ?? [:]
        if presentation?.snapshot.hasSystemAudio == true {
            guard let volume = volume(systemVolume) else { return nil }
            audio["system_volume"] = volume
            audio["mute_system_audio"] = systemMute.state == .on
        }
        if presentation?.snapshot.hasMicrophoneAudio == true {
            guard let volume = volume(microphoneVolume) else { return nil }
            audio["microphone_volume"] = volume
            audio["mute_microphone"] = microphoneMute.state == .on
        }
        audio["mono_output"] = monoOutput.state == .on
        audio["source_has_system_audio"] = presentation?.snapshot.hasSystemAudio == true
        audio["source_has_microphone_audio"] = presentation?.snapshot.hasMicrophoneAudio == true
        edit["audio"] = audio
        return edit
    }

    private var stagedExport: [String: Any]? {
        guard presentation != nil else { return nil }
        var value = presentation!.snapshot.export
        value["format"] = format.indexOfSelectedItem == 1 ? "gif" : "mp4"
        value["quality"] = quality.titleOfSelectedItem?.lowercased() ?? "preserve"
        value["frames_per_second"] = format.indexOfSelectedItem == 1
            ? NSNumber(value: gifFramesPerSecond) : NSNull()
        return value
    }

    private var stagedDiffers: Bool {
        guard let snapshot = presentation?.snapshot else { return false }
        return hasPendingCropInput
            || canonicalEdit(stagedEdit) != canonicalEdit(snapshot.edit)
            || canonical(stagedExport) != canonical(snapshot.export)
    }

    private func applyEdits() {
        guard commitPendingCropInput() else {
            showError("Enter valid trim, crop, audio, and output values."); return
        }
        window.makeFirstResponder(nil)
        guard !busy, let edit = stagedEdit, let export = stagedExport else {
            showError("Enter valid trim, crop, audio, and output values."); return
        }
        let finishCropOnSuccess = cropAdjustmentActive
        if !finishCropOnSuccess { restoreAcceptedPresentation() }
        request(["operation": "update_preview", "edit": edit, "export": export],
                activity: "Applying edits and decoding preview…",
                finishCropOnSuccess: finishCropOnSuccess)
    }

    @objc private func seekChanged() {
        guard !busy, !stagedDiffers else {
            seekSlider.doubleValue = Double(presentation?.snapshot.positionMilliseconds ?? 0)
            if stagedDiffers { showError("Apply staged recording changes before seeking.") }
            return
        }
        let position = UInt64(seekSlider.doubleValue.rounded())
        let finishCropOnSuccess = cropAdjustmentActive
        if !finishCropOnSuccess { restoreAcceptedPresentation() }
        request(["operation": "seek", "position_ms": position],
                activity: "Decoding source-relative frame…",
                finishCropOnSuccess: finishCropOnSuccess)
    }

    private func request(_ object: [String: Any], activity: String,
                         finishCropOnSuccess: Bool = false) {
        guard !busy else { return }
        let current = generation; busy = true; status.stringValue = activity
        updateControls()
        worker.request(object) { [weak self] result in
            guard let self, self.generation == current else { return }
            self.busy = false
            switch result {
            case .success(let value):
                if finishCropOnSuccess { self.finishCropAdjustment(restorePriorImage: false) }
                self.publish(value); self.status.stringValue = "Preview updated."
            case .failure(let error):
                self.seekSlider.doubleValue = Double(self.presentation?.snapshot.positionMilliseconds ?? 0)
                self.showError("Recording preview failed: \(error.localizedDescription)")
            }
            self.updateControls()
        }
    }

    private func togglePlayback() {
        switch playbackState {
        case .idle: startPlayback()
        case .playing: pausePlayback()
        case .pausing: break
        }
    }

    @objc private func playbackLoopChanged() {
        guard presentation != nil, !busy, !pickerOpen, playbackState != .pausing else {
            playbackLoop.state = playbackLoopEnabled ? .on : .off
            return
        }
        playbackLoopEnabled = playbackLoop.state == .on
        playbackLoopControl?.isEnabled = playbackLoopEnabled
        updateControls()
    }

    @objc private func playbackSoundChanged() {
        guard presentation != nil, !busy, !pickerOpen, playbackState == .idle else {
            playbackSound.state = playbackSoundEnabled ? .on : .off
            return
        }
        playbackSoundEnabled = playbackSound.state == .on
        playbackAudioEnabled = nil
        updateControls()
    }

    private func startPlayback() {
        guard !busy, !pickerOpen, playbackState == .idle,
              pendingCropInputValid, stagedEdit != nil, stagedExport != nil,
              !stagedDiffers, let snapshot = presentation?.snapshot,
              let cancel = NativeRecordingEditorCancel() else { return }
        window.makeFirstResponder(nil)
        guard !stagedDiffers else { return }
        let trimStart = (snapshot.edit["trim_start_ms"] as? NSNumber)?.uint64Value ?? 0
        let position = playbackReachedEOF ? trimStart
            : playbackPositionMilliseconds ?? snapshot.positionMilliseconds
        let loop = RecordingPlaybackLoopControl(enabled: playbackLoopEnabled)
        let soundEnabled = playbackSoundEnabled
        let current = generation
        playbackState = .playing; playbackCancel = cancel; playbackLoopControl = loop
        playbackReachedEOF = false; playbackAudioEnabled = nil
        status.textColor = tokens.color("text-muted")
        status.stringValue = soundEnabled ? "Starting playback with sound…"
            : "Starting silent playback…"
        updateControls()
        worker.playback(positionMilliseconds: position, loopStartMilliseconds: trimStart,
            soundEnabled: soundEnabled, loop: loop, cancel: cancel,
            started: { [weak self] metadata in
                guard let self, self.generation == current,
                      self.playbackCancel === cancel,
                      self.playbackState == .playing,
                      !cancel.isCancelled else { return }
                self.playbackAudioEnabled = metadata.audioEnabled
                let mode = metadata.audioEnabled ? "Sound playback" : "Silent playback"
                self.status.stringValue = "\(mode) · \(metadata.width) × \(metadata.height) · \(metadata.framesPerSecond) fps"
                self.updateControls()
            }, frame: { [weak self] value in
                guard let self, self.generation == current,
                      self.playbackCancel === cancel,
                      self.playbackState != .idle else { return }
                self.playbackPositionMilliseconds = value.positionMilliseconds
                self.playbackFramePresented = true
                self.setPreviewImage(NSImage(cgImage: value.image,
                    size: NSSize(width: CGFloat(value.image.width), height: CGFloat(value.image.height))))
                self.updatePlaybackPosition(value.positionMilliseconds)
            }, completion: { [weak self] result in
                guard let self, self.generation == current,
                      self.playbackCancel === cancel else { return }
                self.playbackCancel = nil; self.playbackLoopControl = nil
                self.playbackState = .idle
                switch result {
                case .success(.eof):
                    self.playbackReachedEOF = true
                    self.status.textColor = self.tokens.color("text-muted")
                    self.status.stringValue = "\(self.playbackStatusName) ended. Play restarts at the accepted trim start."
                case .success(.empty):
                    self.playbackReachedEOF = true
                    self.status.textColor = self.tokens.color("text-muted")
                    self.status.stringValue = "\(self.playbackStatusName) ended without frames. Loop did not restart."
                case .success(.cancelled):
                    self.playbackReachedEOF = false
                    self.status.textColor = self.tokens.color("text-muted")
                    self.status.stringValue = "\(self.playbackStatusName) paused."
                case .failure(let error):
                    self.restoreAcceptedPresentation()
                    self.playbackAudioEnabled = nil
                    let mode = soundEnabled ? "Sound playback" : "Silent playback"
                    self.showError("\(mode) failed: \(error.localizedDescription). The accepted preview was restored.")
                }
                self.updateControls()
                let actions = self.playbackStopActions
                self.playbackStopActions.removeAll()
                actions.forEach { $0() }
            })
    }

    private func pausePlayback(after action: (() -> Void)? = nil) {
        if let action { playbackStopActions.append(action) }
        guard playbackState != .idle else {
            let actions = playbackStopActions; playbackStopActions.removeAll()
            actions.forEach { $0() }
            return
        }
        guard playbackState == .playing else { return }
        playbackState = .pausing
        playbackCancel?.cancel()
        status.textColor = tokens.color("text-muted")
        status.stringValue = "Pausing \(playbackStatusName.lowercased())…"
        updateControls()
    }

    private var playbackStatusName: String {
        playbackAudioEnabled == true || (playbackAudioEnabled == nil && playbackSoundEnabled)
            ? "Sound playback" : "Silent playback"
    }

    private func updatePlaybackPosition(_ milliseconds: UInt64) {
        guard let duration = presentation?.snapshot.durationMilliseconds else { return }
        seekSlider.doubleValue = Double(milliseconds)
        seekLabel.stringValue = "\(time(milliseconds)) / \(time(duration))"
        trimTimeline.setPlaybackPosition(milliseconds)
    }

    private func restoreAcceptedPresentation() {
        guard let presentation else { return }
        let restoreFrame = playbackFramePresented
        playbackPositionMilliseconds = nil; playbackReachedEOF = false; playbackFramePresented = false
        trimTimeline.setPlaybackPosition(nil)
        if restoreFrame {
            setPreviewImage(NSImage(cgImage: presentation.image,
                                    size: NSSize(width: CGFloat(presentation.image.width),
                                                 height: CGFloat(presentation.image.height))))
        }
        sourceLabel.stringValue = "\(presentation.snapshot.width) × \(presentation.snapshot.height) source frame"
        seekSlider.doubleValue = Double(presentation.snapshot.positionMilliseconds)
        seekLabel.stringValue = "\(time(presentation.snapshot.positionMilliseconds)) / \(time(presentation.snapshot.durationMilliseconds))"
    }

    private func estimateSize() {
        guard !busy, !stagedDiffers, presentation != nil,
              let cancel = NativeRecordingEditorCancel() else { return }
        let current = generation; busy = true; activeCancel = cancel; estimate = nil
        status.stringValue = "Estimating accepted recording settings…"; updateControls()
        worker.estimate(cancel: cancel) { [weak self] result in
            guard let self, self.generation == current else { return }
            self.busy = false; self.activeCancel = nil
            switch result {
            case .success(let value): self.estimate = value; self.status.stringValue = "Estimate ready."
            case .failure(let error): self.showError("Size estimate failed: \(error.localizedDescription)")
            }
            self.updateControls()
        }
    }

    private func generateThumbnails() {
        guard !busy, !pickerOpen, presentation != nil,
              let cancel = NativeRecordingEditorCancel() else { return }
        let current = generation
        busy = true; activeCancel = cancel; thumbnailCancel = cancel
        thumbnailRetryAvailable = false
        trimTimeline.showThumbnailLoading()
        status.textColor = tokens.color("text-muted")
        status.stringValue = "Generating immutable source timeline thumbnails…"
        updateControls(); layout()
        worker.thumbnails(cancel: cancel) { [weak self] result in
            guard let self, self.generation == current,
                  self.thumbnailCancel === cancel else { return }
            self.busy = false; self.activeCancel = nil; self.thumbnailCancel = nil
            switch result {
            case .success(let image):
                self.trimTimeline.showThumbnails(image)
                self.thumbnailRetryAvailable = false
                self.status.textColor = self.tokens.color("text-muted")
                self.status.stringValue = "Source thumbnails ready. The original remains unchanged."
            case .failure(let error):
                let cancelled = cancel.isCancelled
                self.trimTimeline.showThumbnailFailure(cancelled: cancelled)
                self.thumbnailRetryAvailable = true
                if cancelled {
                    self.status.textColor = self.tokens.color("text-muted")
                    self.status.stringValue = "Source thumbnails cancelled. Editing remains available."
                } else {
                    self.showError("Source thumbnails unavailable: \(error.localizedDescription). Editing remains available.")
                }
            }
            self.updateControls(); self.layout()
        }
    }

    private func cancelActiveOperation() {
        guard let activeCancel else { return }
        activeCancel.cancel()
        if thumbnailCancel === activeCancel {
            trimTimeline.showThumbnailCancelling()
            status.textColor = tokens.color("text-muted")
            status.stringValue = "Cancelling source thumbnail generation…"
        } else if sourceFrameCancel === activeCancel {
            status.textColor = tokens.color("text-muted")
            status.stringValue = "Cancelling full-source crop frame…"
        }
    }

    private func saveNewCopy() {
        guard !busy, !stagedDiffers, let export = presentation?.snapshot.export,
              !destination.stringValue.isEmpty,
              let cancel = NativeRecordingEditorCancel() else { return }
        let current = generation; busy = true; activeCancel = cancel
        progress.doubleValue = 0; progress.isHidden = false
        status.stringValue = "Saving new copy…"; updateControls()
        worker.save(destination: destination.stringValue, export: export, cancel: cancel,
            progress: { [weak self] value in
                guard let self, self.generation == current else { return }
                self.progress.doubleValue = Double(value.completedPerMille)
                self.status.stringValue = value.message
            }, completion: { [weak self] result in
                guard let self, self.generation == current else { return }
                self.busy = false; self.activeCancel = nil; self.progress.isHidden = true
                switch result {
                case .success(let saved):
                    if let snapshot = self.presentation?.snapshot {
                        self.savedEdit = self.canonicalEdit(snapshot.edit)
                        self.savedExport = self.canonical(snapshot.export)
                    }
                    switch saved {
                    case .saved(let path):
                        self.status.stringValue = "Saved new copy: \(path)"; self.didSaveCopy()
                    case .savedWithoutHistory(let path, let warning):
                        self.showError("Saved new copy: \(path). History could not be updated: \(warning)")
                    }
                case .failure(let error): self.showError("Save failed: \(error.localizedDescription)")
                }
                self.updateControls()
            })
    }

    private func chooseDestination() {
        guard !busy else { return }
        pickerOpen = true; updateControls()
        let panel = NSSavePanel(); panel.title = "Save recording as new copy"
        panel.canCreateDirectories = true; panel.nameFieldStringValue = URL(fileURLWithPath: destination.stringValue).lastPathComponent
        panel.directoryURL = URL(fileURLWithPath: destination.stringValue).deletingLastPathComponent()
        panel.allowedContentTypes = format.indexOfSelectedItem == 1 ? [.gif] : [.mpeg4Movie]
        panel.beginSheetModal(for: window) { [weak self] response in
            guard let self else { return }
            self.pickerOpen = false
            if response == .OK, let url = panel.url { self.destination.stringValue = url.path }
            self.updateControls()
        }
    }

    @objc private func cropEnabledChanged() {
        guard let snapshot = presentation?.snapshot,
              let source = sourceDimensions(snapshot) else { return }
        if cropEnabled.state == .on {
            stagedCrop = stagedCrop ?? NativeRecordingCropRect(x: 0, y: 0,
                width: source.width, height: source.height)
        } else {
            stagedCrop = nil
            if cropAdjustmentActive { finishCropAdjustment(restorePriorImage: true) }
        }
        estimate = nil; refreshGeometryFields(source: source, preserveCustom: customOutput)
        updateControls()
    }

    @objc private func cropLockChanged() {
        cropAspectUnlocked = cropLock.state != .on
        cropOverlay.lockAspect = !cropAspectUnlocked
        updateControls()
    }

    @objc private func cropFieldCommitted(_ sender: NSTextField) {
        commitCropField(sender)
    }

    private func commitCropField(_ field: NSTextField) {
        guard [cropX, cropY, cropWidth, cropHeight].contains(where: { $0 === field }) else { return }
        _ = commitPendingCropInput()
        updateControls()
    }

    private var pendingCropFields: [NSTextField] {
        guard let crop = stagedCrop else { return [] }
        return [(cropX, crop.x), (cropY, crop.y), (cropWidth, crop.width),
                (cropHeight, crop.height)].compactMap { field, value in
            field.stringValue == String(value) ? nil : field
        }
    }

    private var hasPendingCropInput: Bool { !pendingCropFields.isEmpty }

    private var pendingCropInputValid: Bool {
        !hasPendingCropInput || pendingCropCandidate() != nil
    }

    private func pendingCropCandidate() -> NativeRecordingCropRect? {
        guard var crop = stagedCrop, let snapshot = presentation?.snapshot,
              let source = sourceDimensions(snapshot) else { return nil }
        for field in pendingCropFields {
            guard let value = parseUInt32(field.stringValue) else { return nil }
            if field === cropX {
                guard UInt64(value) + UInt64(crop.width) <= UInt64(source.width) else { return nil }
                crop.x = value
            } else if field === cropY {
                guard UInt64(value) + UInt64(crop.height) <= UInt64(source.height) else { return nil }
                crop.y = value
            } else if field === cropWidth {
                if cropAspectUnlocked {
                    crop.width = min(max(2, value), source.width - crop.x)
                } else if let resized = NativeRecordingGeometry.resizeLocked(
                    crop, source: source, axis: .width, value: value) {
                    crop = resized
                } else { return nil }
            } else if field === cropHeight {
                if cropAspectUnlocked {
                    crop.height = min(max(2, value), source.height - crop.y)
                } else if let resized = NativeRecordingGeometry.resizeLocked(
                    crop, source: source, axis: .height, value: value) {
                    crop = resized
                } else { return nil }
            }
        }
        return crop
    }

    @discardableResult private func commitPendingCropInput() -> Bool {
        guard hasPendingCropInput else { return true }
        guard let crop = pendingCropCandidate(), let snapshot = presentation?.snapshot,
              let source = sourceDimensions(snapshot) else {
            estimate = nil; return false
        }
        stagedCrop = crop; estimate = nil
        refreshGeometryFields(source: source, preserveCustom: customOutput)
        return true
    }

    private func toggleCropAdjustment() {
        if cropAdjustmentActive {
            finishCropAdjustment(restorePriorImage: true); updateControls(); return
        }
        guard !busy, !pickerOpen, playbackState == .idle, cropEnabled.state == .on,
              stagedCrop != nil, !hasPendingCropInput,
              let snapshot = presentation?.snapshot,
              let source = sourceDimensions(snapshot) else { return }
        window.makeFirstResponder(nil)
        guard !hasPendingCropInput else { return }
        cropAdjustmentPriorImage = preview.image
        if let cached = sourceFrameCache,
           cached.positionMilliseconds == snapshot.positionMilliseconds,
           cached.image.width == Int(source.width), cached.image.height == Int(source.height) {
            beginCropAdjustment(with: cached, source: source)
            updateControls(); return
        }
        guard let cancel = NativeRecordingEditorCancel() else { return }
        let current = generation
        busy = true; activeCancel = cancel; sourceFrameCancel = cancel
        status.textColor = tokens.color("text-muted")
        status.stringValue = "Loading immutable full-source crop frame…"
        updateControls()
        worker.sourceFrame(cancel: cancel) { [weak self] result in
            guard let self, self.generation == current,
                  self.sourceFrameCancel === cancel else { return }
            self.busy = false; self.activeCancel = nil; self.sourceFrameCancel = nil
            guard !cancel.isCancelled else {
                self.cropAdjustmentPriorImage = nil
                self.status.textColor = self.tokens.color("text-muted")
                self.status.stringValue = "Full-source crop frame cancelled. Editing remains available."
                self.updateControls()
                return
            }
            switch result {
            case .success(let value):
                guard let snapshot = self.presentation?.snapshot,
                      value.positionMilliseconds == snapshot.positionMilliseconds,
                      value.image.width == snapshot.width,
                      value.image.height == snapshot.height,
                      let source = self.sourceDimensions(snapshot) else {
                    self.cropAdjustmentPriorImage = nil
                    self.showError("The full-source crop frame no longer matches this recording position.")
                    self.updateControls(); return
                }
                self.sourceFrameCache = value
                self.beginCropAdjustment(with: value, source: source)
            case .failure(let error):
                self.cropAdjustmentPriorImage = nil
                self.showError("Full-source crop frame unavailable: \(error.localizedDescription). Editing remains available.")
            }
            self.updateControls()
        }
    }

    private func beginCropAdjustment(with sourceImage: RecordingSourceImage,
                                     source: NativeRecordingDimensions) {
        guard let crop = stagedCrop else { return }
        cropAdjustmentActive = true
        setPreviewImage(NSImage(cgImage: sourceImage.image,
            size: NSSize(width: CGFloat(sourceImage.image.width),
                         height: CGFloat(sourceImage.image.height))))
        cropOverlay.sourceSize = source; cropOverlay.crop = crop
        cropOverlay.lockAspect = !cropAspectUnlocked
        cropOverlay.isHidden = false
        sourceLabel.stringValue = "Full source · \(time(sourceImage.positionMilliseconds))"
        status.textColor = tokens.color("text-muted")
        status.stringValue = "Adjust the source crop, then Apply edits or choose Done cropping."
    }

    private func finishCropAdjustment(restorePriorImage: Bool) {
        cropOverlay.endDrag(); cropOverlay.setEditingEnabled(false); cropOverlay.isHidden = true
        cropAdjustmentActive = false
        if restorePriorImage {
            setPreviewImage(cropAdjustmentPriorImage ?? presentation.map {
                NSImage(cgImage: $0.image,
                        size: NSSize(width: CGFloat($0.image.width),
                                     height: CGFloat($0.image.height)))
            })
        }
        cropAdjustmentPriorImage = nil
        if let snapshot = presentation?.snapshot {
            sourceLabel.stringValue = "\(snapshot.width) × \(snapshot.height) source frame"
        }
    }

    private func stageGraphicalCrop(_ crop: NativeRecordingCropRect) {
        guard cropAdjustmentActive, !hasPendingCropInput,
              let snapshot = presentation?.snapshot,
              let source = sourceDimensions(snapshot) else { return }
        stagedCrop = crop; estimate = nil
        refreshGeometryFields(source: source, preserveCustom: customOutput)
        updateControls()
    }

    @objc private func outputModeChanged() {
        guard let snapshot = presentation?.snapshot,
              let source = sourceDimensions(snapshot) else { return }
        let index = outputMode.indexOfSelectedItem
        if index == NativeRecordingResolutionPreset.allCases.count {
            if !customOutput {
                let current = resolvedPresetDimensions(source: source)
                outputWidth.stringValue = String(current.width)
                outputHeight.stringValue = String(current.height)
            }
            customOutput = true
        } else if index >= 0, let preset = NativeRecordingResolutionPreset(rawValue: UInt8(index)) {
            customOutput = false; resolutionPreset = preset
            refreshGeometryFields(source: source, preserveCustom: false,
                                  preservePendingCrop: true)
        }
        estimate = nil; updateControls()
    }

    @objc private func formatChanged() {
        let ext = format.indexOfSelectedItem == 1 ? "gif" : "mp4"
        if !destination.stringValue.isEmpty {
            destination.stringValue = URL(fileURLWithPath: destination.stringValue)
                .deletingPathExtension().appendingPathExtension(ext).path
        }
        estimate = nil; updateControls(); layout()
    }
    @objc private func gifFrameRateChanged() {
        let title = gifFrameRate.titleOfSelectedItem ?? "15 FPS"
        gifFramesPerSecond = UInt16(title.split(separator: " ").first.map(String.init) ?? "15") ?? 15
        estimate = nil; updateControls()
    }
    @objc private func stageChanged() { estimate = nil; updateControls() }

    private func sourceDimensions(_ snapshot: NativeRecordingEditorSnapshot)
        -> NativeRecordingDimensions? {
        guard snapshot.width > 0, snapshot.height > 0,
              snapshot.width <= Int(UInt32.max), snapshot.height <= Int(UInt32.max) else { return nil }
        return NativeRecordingDimensions(width: UInt32(snapshot.width),
                                         height: UInt32(snapshot.height))
    }

    private func editOutputDimensions(_ edit: [String: Any]) -> NativeRecordingDimensions? {
        guard let width = parseUInt32(edit["output_width"]),
              let height = parseUInt32(edit["output_height"]),
              width >= 2, height >= 2 else { return nil }
        return NativeRecordingDimensions(width: width, height: height)
    }

    private func parseUInt32(_ value: Any?) -> UInt32? {
        if let text = value as? String {
            guard let raw = UInt64(text.trimmingCharacters(in: .whitespacesAndNewlines)),
                  raw <= UInt64(UInt32.max) else { return nil }
            return UInt32(raw)
        }
        guard let number = value as? NSNumber else { return nil }
        let raw = number.uint64Value
        return raw <= UInt64(UInt32.max) ? UInt32(raw) : nil
    }

    private func cropInputDimensions(source: NativeRecordingDimensions) -> NativeRecordingDimensions {
        stagedCrop.map { NativeRecordingDimensions(width: $0.width, height: $0.height) } ?? source
    }

    private func resolvedPresetDimensions(source: NativeRecordingDimensions)
        -> NativeRecordingDimensions {
        let input = cropInputDimensions(source: source)
        if resolutionPreset == .original { return input }
        return NativeRecordingGeometry.constrain(input, preset: resolutionPreset) ?? input
    }

    private func stagedOutputDimensions(source: NativeRecordingDimensions)
        -> NativeRecordingDimensions? {
        if customOutput {
            guard let width = parseUInt32(outputWidth.stringValue),
                  let height = parseUInt32(outputHeight.stringValue),
                  width >= 2, height >= 2 else { return nil }
            return NativeRecordingDimensions(width: width, height: height)
        }
        return resolvedPresetDimensions(source: source)
    }

    private func refreshGeometryFields(source: NativeRecordingDimensions,
                                       preserveCustom: Bool,
                                       preservePendingCrop: Bool = false) {
        let pendingCrop = preservePendingCrop
            ? pendingCropFields.map { ($0, $0.stringValue) } : []
        let crop = stagedCrop ?? NativeRecordingCropRect(x: 0, y: 0,
            width: source.width, height: source.height)
        cropX.stringValue = String(crop.x); cropY.stringValue = String(crop.y)
        cropWidth.stringValue = String(crop.width); cropHeight.stringValue = String(crop.height)
        pendingCrop.forEach { $0.0.stringValue = $0.1 }
        cropEnabled.state = stagedCrop == nil ? .off : .on
        cropLock.state = cropAspectUnlocked ? .off : .on
        cropOverlay.lockAspect = !cropAspectUnlocked
        if let stagedCrop { cropOverlay.crop = stagedCrop }
        if !customOutput || !preserveCustom {
            let output = resolvedPresetDimensions(source: source)
            outputWidth.stringValue = String(output.width)
            outputHeight.stringValue = String(output.height)
        }
        outputMode.selectItem(withTitle: customOutput ? "Custom" : resolutionPreset.title)
    }

    private func syncTimelineFromFields() {
        guard let duration = presentation?.snapshot.durationMilliseconds,
              let start = UInt64(trimStart.stringValue), let end = UInt64(trimEnd.stringValue),
              start < end, end <= duration else { return }
        trimTimeline.setValues(start: start, end: end, duration: duration)
    }

    private func updateControls() {
        let available = presentation != nil && !busy && !pickerOpen && playbackState == .idle
        let valid = pendingCropInputValid && stagedEdit != nil && stagedExport != nil
        [trimStart, trimEnd, format, quality, destination].forEach { $0.isEnabled = available }
        let gif = format.indexOfSelectedItem == 1
        gifFrameRateLabel.isHidden = !gif; gifFrameRate.isHidden = !gif
        gifFrameRate.isEnabled = available && gif
        cropEnabled.isEnabled = available
        cropLock.isEnabled = available && stagedCrop != nil
        [cropX, cropY, cropWidth, cropHeight].forEach {
            $0.isEnabled = available && stagedCrop != nil
        }
        outputMode.isEnabled = available
        outputWidth.isEnabled = available && customOutput
        outputHeight.isEnabled = available && customOutput
        let hasSystem = presentation?.snapshot.hasSystemAudio == true
        let hasMicrophone = presentation?.snapshot.hasMicrophoneAudio == true
        let hasAudio = hasSystem || hasMicrophone
        systemAudioLabel.isHidden = !hasSystem; systemVolume.isHidden = !hasSystem
        systemMute.isHidden = !hasSystem
        microphoneAudioLabel.isHidden = !hasMicrophone; microphoneVolume.isHidden = !hasMicrophone
        microphoneMute.isHidden = !hasMicrophone
        monoOutput.isHidden = !hasAudio
        if !playbackSoundEnabled {
            audioNote.stringValue = !hasAudio ? "No audio tracks"
                : gif ? "GIF · MP4 kept · Sound off"
                : "Sound preview off"
        } else if playbackAudioEnabled == true {
            audioNote.stringValue = playbackState == .playing ? "Sound active"
                : "Audio used"
        } else if gif {
            audioNote.stringValue = "GIF · no audio"
        } else if !hasAudio {
            audioNote.stringValue = "No audio tracks"
        } else if playbackAudioEnabled == false {
            audioNote.stringValue = "Mix silent"
        } else {
            audioNote.stringValue = "Uses accepted mix"
        }
        systemVolume.isEnabled = available && !gif && systemMute.state != .on
        microphoneVolume.isEnabled = available && !gif && microphoneMute.state != .on
        systemMute.isEnabled = available && !gif
        microphoneMute.isEnabled = available && !gif
        monoOutput.isEnabled = available && !gif
        trimTimeline.setEditingEnabled(available && stagedEdit != nil)
        thumbnailRetryButton?.isHidden = !thumbnailRetryAvailable
        thumbnailRetryButton?.isEnabled = available && thumbnailRetryAvailable
        cropAdjustmentButton?.title = cropAdjustmentActive ? "Done cropping" : "Adjust crop"
        cropAdjustmentButton?.setAccessibilityLabel(cropAdjustmentActive
            ? "Done adjusting recording crop" : "Adjust recording crop graphically")
        cropAdjustmentButton?.isEnabled = available && (cropAdjustmentActive
            || (stagedCrop != nil && !hasPendingCropInput))
        previewFitButton?.selected = !previewActualSize
        previewActualButton?.selected = previewActualSize
        previewFitButton?.needsDisplay = true
        previewActualButton?.needsDisplay = true
        previewFitButton?.isEnabled = preview.image != nil
        previewActualButton?.isEnabled = preview.image != nil
        cropOverlay.interceptsPendingInput = cropAdjustmentActive && available
            && hasPendingCropInput
        cropOverlay.setEditingEnabled(cropAdjustmentActive && available && !hasPendingCropInput)
        playbackLoop.isEnabled = presentation != nil && !busy && !pickerOpen
            && playbackState != .pausing
        playbackSound.isEnabled = presentation != nil && !busy && !pickerOpen
            && playbackState == .idle
        switch playbackState {
        case .idle:
            playbackButton?.title = "Play"
            playbackButton?.setAccessibilityLabel(playbackSoundEnabled
                ? "Play recording preview with sound" : "Play silent recording preview")
            playbackButton?.isEnabled = available && valid && !stagedDiffers
                && !cropAdjustmentActive
        case .playing:
            playbackButton?.title = "Pause"
            playbackButton?.setAccessibilityLabel("Pause recording preview")
            playbackButton?.isEnabled = true
        case .pausing:
            playbackButton?.title = "Pausing…"
            playbackButton?.setAccessibilityLabel("Pausing recording preview")
            playbackButton?.isEnabled = false
        }
        applyButton?.isEnabled = available && valid && stagedDiffers
        seekSlider.isEnabled = available && valid && !stagedDiffers
        changeButton?.isEnabled = available
        estimateButton?.isEnabled = available && valid && !stagedDiffers
        saveButton?.isEnabled = available && valid && !stagedDiffers && !destination.stringValue.isEmpty
        cancelButton?.isHidden = activeCancel == nil
        cancelButton?.isEnabled = activeCancel != nil
        if stagedDiffers { estimateLabel.stringValue = "Apply edits to estimate size" }
        else if let estimate {
            estimateLabel.stringValue = "\(estimate.exact ? "" : "≈ ")\(ByteCountFormatter.string(fromByteCount: Int64(estimate.sizeBytes), countStyle: .file))"
        } else { estimateLabel.stringValue = "Size not estimated" }
    }

    private func closeSession() {
        generation += 1; artifactID = nil; presentation = nil; activeCancel = nil
        thumbnailCancel = nil; thumbnailRetryAvailable = false
        playbackCancel = nil; playbackState = .idle
        playbackPositionMilliseconds = nil; playbackReachedEOF = false; playbackFramePresented = false
        playbackLoopEnabled = false; playbackLoopControl = nil; playbackLoop.state = .off
        playbackSoundEnabled = false; playbackAudioEnabled = nil; playbackSound.state = .off
        gifFramesPerSecond = 15; gifFrameRate.selectItem(withTitle: "15 FPS")
        sourceFrameCache = nil; sourceFrameCancel = nil
        cropAdjustmentActive = false; cropAdjustmentPriorImage = nil
        previewActualSize = false
        cropOverlay.setEditingEnabled(false); cropOverlay.isHidden = true
        playbackStopActions.removeAll(); closeAfterPlayback = false
        terminateAfterPlayback = false; switchAfterPlayback = nil
        trimTimeline.setPlaybackPosition(nil)
        trimTimeline.clearThumbnails()
        worker.close()
    }

    private func showError(_ message: String) {
        status.textColor = tokens.color("danger-text"); status.stringValue = message
        reportError(message)
    }

    private func canonical(_ value: [String: Any]?) -> Data? {
        value.flatMap { try? JSONSerialization.data(withJSONObject: $0, options: [.sortedKeys]) }
    }

    private func canonicalEdit(_ value: [String: Any]?) -> Data? {
        guard var value else { return nil }
        if var audio = value["audio"] as? [String: Any] {
            for key in ["system_volume", "microphone_volume"] {
                if let number = audio[key] as? NSNumber { audio[key] = number.floatValue }
            }
            value["audio"] = audio
        }
        return canonical(value)
    }

    private func time(_ milliseconds: UInt64) -> String {
        String(format: "%llu:%02llu.%03llu", milliseconds / 60_000,
               (milliseconds / 1_000) % 60, milliseconds % 1_000)
    }

    private func select(_ popup: NSPopUpButton, value: String) {
        let title = value == "mp4" ? "MP4" : value == "gif" ? "GIF"
            : value.prefix(1).uppercased() + String(value.dropFirst())
        popup.selectItem(withTitle: title)
    }

    private func configureNumberField(_ field: NSTextField, label: String) {
        field.delegate = self; field.setAccessibilityLabel(label)
        field.font = .monospacedDigitSystemFont(ofSize: 13, weight: .regular)
    }

    private func configureVolumeField(_ field: NSTextField, label: String) {
        configureNumberField(field, label: label)
        field.alignment = .right
        field.placeholderString = "%"
    }

    private func volume(_ field: NSTextField) -> Float? {
        let text = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "%"))
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard let percent = Double(text), percent.isFinite,
              (0...200).contains(percent) else { return nil }
        return Float(percent / 100)
    }

    private func volumePercent(_ value: Any?) -> String {
        let gain = (value as? NSNumber)?.floatValue ?? 1
        let percent = Double(gain) * 100
        for digits in 1...9 {
            let text = String(format: "%.*g", locale: Locale(identifier: "en_US_POSIX"),
                              digits, percent)
            if let parsed = Double(text), Float(parsed / 100) == gain,
               let decimal = Decimal(string: text, locale: Locale(identifier: "en_US_POSIX")) {
                return "\(NSDecimalNumber(decimal: decimal).stringValue)%"
            }
        }
        return "\(percent)%"
    }

    @discardableResult private func label(_ text: String, size: CGFloat = 12,
                                          weight: NSFont.Weight = .regular,
                                          muted: Bool = false, parent: NSView? = nil) -> NSTextField {
        let value = NSTextField(labelWithString: text)
        value.font = .systemFont(ofSize: size, weight: weight)
        value.textColor = tokens.color(muted ? "text-muted" : "text")
        (parent ?? root).addSubview(value); return value
    }

    @discardableResult private func button(_ title: String,
                                            action: @escaping () -> Void) -> CaptureButton {
        let value = CaptureButton(title, frame: .zero, tokens: tokens, action: action)
        root.addSubview(value); return value
    }

    private static func confirmDiscardAlert() -> Bool {
        let alert = NSAlert()
        alert.messageText = "Discard unsaved recording edits?"
        alert.informativeText = "Recording drafts are not available. The original recording is unchanged."
        alert.addButton(withTitle: "Keep editing")
        alert.addButton(withTitle: "Discard edits")
        return alert.runModal() == .alertSecondButtonReturn
    }
}
