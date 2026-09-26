import AppKit
import UniformTypeIdentifiers

final class RecordingComparisonView: NSView {
    let tokens: Tokens
    init(tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: .zero)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    var comparison: RecordingEditorComparison? { didSet { needsDisplay = true } }
    var split: CGFloat = 0.5 { didSet { needsDisplay = true } }
    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        guard let comparison, bounds.width > 0, bounds.height > 0 else { return }
        let before = NSImage(cgImage: comparison.before,
                             size: NSSize(width: comparison.before.width,
                                          height: comparison.before.height))
        let after = NSImage(cgImage: comparison.after,
                            size: NSSize(width: comparison.after.width,
                                         height: comparison.after.height))
        let scale = min(bounds.width / before.size.width, bounds.height / before.size.height)
        let rect = NSRect(x: (bounds.width - before.size.width * scale) / 2,
                          y: (bounds.height - before.size.height * scale) / 2,
                          width: before.size.width * scale, height: before.size.height * scale)
        before.draw(in: rect, from: .zero, operation: .sourceOver, fraction: 1,
                    respectFlipped: true, hints: nil)
        guard let context = NSGraphicsContext.current else { return }
        context.saveGraphicsState()
        NSRect(x: rect.minX + rect.width * split, y: rect.minY,
               width: rect.width * (1 - split), height: rect.height).clip()
        after.draw(in: rect, from: .zero, operation: .sourceOver, fraction: 1,
                   respectFlipped: true, hints: nil)
        context.restoreGraphicsState()
        let dividerWidth = tokens.number("s-1")
        let divider = NSRect(x: rect.minX + rect.width * split - dividerWidth / 2,
                             y: rect.minY, width: dividerWidth, height: rect.height)
        tokens.color("theme-accent").setFill(); divider.fill()
    }
}

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
        let tokens = timeline.tokens
        let focused = window?.firstResponder === self
        // `.timeline-trim-handle`: an accent bar with two grip lines.
        let barWidth = tokens.number("s-4")
        let bar = NSRect(x: bounds.midX - barWidth / 2, y: 0, width: barWidth, height: bounds.height)
        let path = NSBezierPath(roundedRect: bar, xRadius: tokens.number("r-xs"),
                                yRadius: tokens.number("r-xs"))
        tokens.color(enabled ? "theme-accent" : "text-faint").setFill(); path.fill()
        tokens.color("theme-accent-ink").withAlphaComponent(0.45).setStroke()
        for offset in [-tokens.number("s-1"), tokens.number("s-1")] {
            let grip = NSBezierPath()
            grip.move(to: NSPoint(x: bar.midX + offset, y: bar.midY - 7))
            grip.line(to: NSPoint(x: bar.midX + offset, y: bar.midY + 7))
            grip.lineWidth = 1; grip.stroke()
        }
        if focused {
            let ring = NSBezierPath(roundedRect: bar.insetBy(dx: -2, dy: 1),
                                    xRadius: tokens.number("r-xs"), yRadius: tokens.number("r-xs"))
            tokens.color("text").setStroke(); ring.lineWidth = 2; ring.stroke()
        }
    }
}

/// Shipping `.timeline-track` height; the view adds a little room for the playhead cap.
let recordingTimelineTrackHeight: CGFloat = 76

/// `.recording-export-progress`: a thin accent bar on the save footer's top edge.
final class RecordingProgressBar: NSView {
    var tokens: Tokens? { didSet { needsDisplay = true } }
    var doubleValue: Double = 0 { didSet { needsDisplay = true } }
    let maxValue: Double = 1_000

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        setAccessibilityElement(true)
        setAccessibilityRole(.progressIndicator)
        setAccessibilityLabel("Recording export progress")
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        guard let tokens else { return }
        tokens.color("surface-sunken").setFill(); bounds.fill()
        let fraction = CGFloat(min(max(doubleValue / maxValue, 0), 1))
        tokens.color("theme-accent").setFill()
        NSRect(x: 0, y: 0, width: bounds.width * fraction, height: bounds.height).fill()
    }
}

final class RecordingTrimTimeline: NSView {
    fileprivate let tokens: Tokens
    fileprivate(set) var durationMilliseconds: UInt64 = 1
    fileprivate(set) var startMilliseconds: UInt64 = 0
    fileprivate(set) var endMilliseconds: UInt64 = 1
    var onStage: ((NativeRecordingTimelineEdge, UInt64) -> Void)?
    /// Clicking the track outside the grips seeks the accepted preview there.
    var onSeek: ((UInt64) -> Void)?
    var seekEnabled = false
    private var drag: NativeRecordingTimelineDrag?
    private var dragEdge: NativeRecordingTimelineEdge?
    private(set) var editingEnabled = false
    private var thumbnailImage: NSImage?
    private var playbackPositionMilliseconds: UInt64?
    private var acceptedPositionMilliseconds: UInt64?
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
        NSRect(x: 10, y: 3, width: max(1, bounds.width - 20), height: max(1, bounds.height - 6))
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

    /// The accepted still's position, drawn as the playhead while not playing.
    func setAcceptedPosition(_ milliseconds: UInt64?) {
        acceptedPositionMilliseconds = milliseconds
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
        if newSize != frame.size { endDrag() }
        super.setFrameSize(newSize); updateHandles()
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
        return seekEnabled && onSeek != nil && trackRect.contains(local) ? self : nil
    }

    override func mouseDown(with event: NSEvent) {
        guard seekEnabled, let onSeek else { return }
        let x = convert(event.locationInWindow, from: nil).x
        guard durationMilliseconds > 0,
              let time = NativeRecordingTimeline.time(atX: Double(x), trackLeft: Double(trackRect.minX),
                  trackWidth: Double(trackRect.width),
                  durationMilliseconds: Double(durationMilliseconds)) else { return }
        onSeek(min(UInt64(max(0, time).rounded()), durationMilliseconds - 1))
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        let track = trackRect
        // `.timeline-track`: a sunken, bordered frame around the filmstrip.
        let frame = bounds.insetBy(dx: 0, dy: 3)
        let background = NSBezierPath(roundedRect: frame, xRadius: tokens.number("r-md"),
                                      yRadius: tokens.number("r-md"))
        tokens.color("surface-sunken").setFill(); background.fill()
        let strip = NSRect(x: track.minX, y: frame.minY + tokens.number("s-2"),
                           width: track.width, height: max(1, frame.height - tokens.number("s-2") * 2))
        let stripPath = NSBezierPath(roundedRect: strip, xRadius: tokens.number("r-xs"),
                                     yRadius: tokens.number("r-xs"))
        if let thumbnailImage {
            NSGraphicsContext.saveGraphicsState()
            stripPath.addClip()
            let frameCount = 12
            let sourceWidth = thumbnailImage.size.width / CGFloat(frameCount)
            let targetWidth = strip.width / CGFloat(frameCount)
            // Center-crop each frame vertically to the strip, preserving aspect.
            let sourceHeight = min(thumbnailImage.size.height,
                                   sourceWidth * strip.height / max(1, targetWidth))
            let sourceY = (thumbnailImage.size.height - sourceHeight) / 2
            for index in 0..<frameCount {
                thumbnailImage.draw(
                    in: NSRect(x: strip.minX + CGFloat(index) * targetWidth, y: strip.minY,
                               width: targetWidth + 0.5, height: strip.height),
                    from: NSRect(x: CGFloat(index) * sourceWidth, y: sourceY,
                                 width: sourceWidth, height: sourceHeight),
                    operation: .copy, fraction: 1, respectFlipped: true,
                    hints: [.interpolation: NSImageInterpolation.high])
            }
            NSGraphicsContext.restoreGraphicsState()
        } else {
            tokens.color("n-5").setFill(); stripPath.fill()
            let paragraph = NSMutableParagraphStyle(); paragraph.alignment = .center
            let font = NSFont.systemFont(ofSize: tokens.number("text-sm"))
            (thumbnailStateDescription as NSString).draw(
                in: NSRect(x: strip.minX + 4, y: strip.midY - font.pointSize * 0.7,
                           width: max(1, strip.width - 8), height: font.pointSize * 1.5),
                withAttributes: [.font: font, .foregroundColor: tokens.color("text-muted"),
                                 .paragraphStyle: paragraph])
        }
        // `.timeline-excluded`: trimmed-away time dims toward the canvas.
        tokens.color("surface-canvas").withAlphaComponent(0.72).setFill()
        for area in [
            NSRect(x: frame.minX, y: frame.minY,
                   width: max(0, startHandle.frame.midX - frame.minX), height: frame.height),
            NSRect(x: endHandle.frame.midX, y: frame.minY,
                   width: max(0, frame.maxX - endHandle.frame.midX), height: frame.height),
        ] where area.width > 0 {
            NSBezierPath(roundedRect: area, xRadius: tokens.number("r-md"),
                         yRadius: tokens.number("r-md")).fill()
        }
        tokens.color("border").setStroke(); background.lineWidth = 1; background.stroke()
        // `.timeline-playhead`: a 2pt line with a cap over the filmstrip.
        if let position = playbackPositionMilliseconds ?? acceptedPositionMilliseconds,
           let ratio = NativeRecordingTimeline.ratio(
               milliseconds: Double(min(position, durationMilliseconds)),
               durationMilliseconds: Double(durationMilliseconds)) {
            let x = track.minX + track.width * CGFloat(ratio)
            tokens.color("text").setFill()
            NSRect(x: x - 1, y: 0, width: 2, height: bounds.height).fill()
            NSBezierPath(roundedRect: NSRect(x: x - 5, y: 0, width: 10, height: 8),
                         xRadius: 3, yRadius: 3).fill()
        }
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
        didSet {
            guard presentedImageRect != oldValue else { return }
            endDrag(); updateHandles(); needsDisplay = true
        }
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

enum RecordingFileSizeUnit: Int, CaseIterable {
    case kilobytes
    case megabytes
    case gigabytes

    var label: String {
        switch self {
        case .kilobytes: "KB"
        case .megabytes: "MB"
        case .gigabytes: "GB"
        }
    }

    private var digits: Int {
        switch self {
        case .kilobytes: 3
        case .megabytes: 6
        case .gigabytes: 9
        }
    }

    func bytes(_ text: String) -> UInt64? {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let parts = text.split(separator: ".", omittingEmptySubsequences: false)
        guard !parts.isEmpty, parts.count <= 2 else { return nil }
        let wholeText = String(parts[0])
        let fractionText = parts.count == 2 ? String(parts[1]) : ""
        guard !(wholeText.isEmpty && fractionText.isEmpty),
              wholeText.utf8.allSatisfy({ $0 >= 48 && $0 <= 57 }),
              fractionText.utf8.allSatisfy({ $0 >= 48 && $0 <= 57 }) else { return nil }
        let whole = wholeText.isEmpty ? 0 : UInt64(wholeText)
        guard let whole else { return nil }
        let factor = Self.powerOfTen(digits)
        let (wholeBytes, overflow) = whole.multipliedReportingOverflow(by: factor)
        guard !overflow else { return nil }
        let kept = String(fractionText.prefix(digits))
        let fraction = kept.isEmpty ? 0 : UInt64(kept)
        guard let fraction else { return nil }
        let fractionBytes = fraction * Self.powerOfTen(digits - kept.count)
        let (bytes, additionOverflow) = wholeBytes.addingReportingOverflow(fractionBytes)
        return additionOverflow ? nil : bytes
    }

    func value(_ bytes: UInt64) -> String {
        let factor = Self.powerOfTen(digits)
        let rawFraction = String(format: "%0*llu", digits, bytes % factor)
        let fraction = rawFraction.replacingOccurrences(of: "0+$", with: "",
                                                         options: .regularExpression)
        return fraction.isEmpty ? String(bytes / factor)
            : "\(bytes / factor).\(fraction)"
    }

    private static func powerOfTen(_ exponent: Int) -> UInt64 {
        (0..<exponent).reduce(1) { value, _ in value * 10 }
    }
}

func formatRecordingFileSizeDelta(estimatedBytes: UInt64?, originalBytes: UInt64) -> String? {
    guard let estimatedBytes, originalBytes > 0 else { return nil }
    let change = (Double(estimatedBytes) / Double(originalBytes) - 1) * 100
    let rounded = change.rounded()
    let percent = change < 0 && abs(change - rounded) == 0.5 ? rounded + 1 : rounded
    guard percent != 0 else { return nil }
    let magnitude = String(format: "%.0f", abs(percent))
    return percent < 0 ? "−\(magnitude)%" : "+\(magnitude)%"
}

final class RecordingEditorController: NSObject, NSWindowDelegate, NSTextFieldDelegate {
    let window: NSWindow
    let root = Surface()
    private let worker: RecordingEditorWorking
    private let reportError: (String) -> Void
    private let didSaveCopy: () -> Void
    private let didReplaceOriginal: (String) -> Void
    private let confirmDiscard: () -> Bool
    private let confirmReplaceOriginal: (NSWindow, String, @escaping (Bool) -> Void) -> Void
    private let requestTermination: () -> Void
    private var tokens: Tokens
    private var generation = 0
    private var artifactID: String?
    private var presentation: RecordingEditorPresentation?
    private var savedEdit: Data?
    private var savedExport: Data?
    private var busy = false
    private var pickerOpen = false
    private var awaitingReplaceConfirmation = false
    private var originalPath: String?
    private var requiresReopen = false
    private var activeCancel: NativeRecordingEditorCancel?
    private var thumbnailCancel: NativeRecordingEditorCancel?
    private var thumbnailRetryAvailable = false
    private var estimate: RecordingEditorEstimate?
    private var comparison: RecordingEditorComparison?
    private var comparisonCancel: NativeRecordingEditorCancel?
    private var comparisonStatusMessage: String?
    private var playbackState = RecordingPlaybackState.idle
    private var playbackCancel: NativeRecordingEditorCancel?
    private var playbackPositionMilliseconds: UInt64?
    private var playbackReachedEOF = false
    private var playbackFramePresented = false
    private var playbackLoopEnabled = false
    private var playbackSoundEnabled = false
    private var playbackAudioEnabled: Bool?
    private var gifFramesPerSecond: UInt16 = 15
    private var gifMaximumWidth: UInt32 = 800
    private var maximumSizeEnabled = false
    private var maximumSizeUnit = RecordingFileSizeUnit.megabytes
    /// The palette preset value (`preserve`, `highest` … `tiny`); Maximum keeps
    /// the last preset so GIF palettes follow the user's quality choice.
    private var qualityPreference = "preserve"
    /// Preserve quality mode (MP4 default); otherwise Compress unless Maximum.
    private var preserveQuality = true
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

    // Shipping page: a scrolling column of cards above a fixed save footer.
    private let pageScroll = NSScrollView()
    private let page = Surface()
    private let footer = Surface()
    private let footerDivider = Surface()
    private let titleLabel = NSTextField(labelWithString: "Edit recording")
    /// `.recording-editor-warning` for sources that dropped frames.
    private let droppedFramesBand = Surface()
    private let droppedFramesLabel = NSTextField(wrappingLabelWithString: "")
    private let previewPanel = Surface()
    private let previewTitle = NSTextField(labelWithString: "Preview")
    /// Toolbar playback-mode note beside "Preview".
    private let audioNote = NSTextField(labelWithString: "Silent playback")
    private let previewToolbar = Surface()
    private let previewDivider = Surface()
    private let previewViewport = Surface()
    private let previewSizeTrack = Surface()
    private let previewScroll = NSScrollView()
    private let previewCanvas = Surface()
    private let preview = NSImageView()
    private let previewNote = NSTextField(labelWithString:
        "First-attempt preview. Size-limited saves may reduce resolution, frame rate or audio quality.")
    private let comparisonView: RecordingComparisonView
    private let comparisonSlider = NSSlider(value: 50, minValue: 0, maxValue: 100,
                                             target: nil, action: nil)
    private let comparisonBeforeLabel = NSTextField(labelWithString: "Before")
    private let comparisonAfterLabel = NSTextField(labelWithString: "Encoded")
    private var comparisonButton: CaptureButton!
    private var comparisonHideButton: CaptureButton!
    private let cropOverlay: RecordingCropOverlay
    private let geometryPanel = Surface()
    private let geometryTitle = NSTextField(labelWithString: "Crop & size")
    private let cropEnabled = NSButton(checkboxWithTitle: "Crop recording", target: nil, action: nil)
    private let cropLock = NSButton(checkboxWithTitle: "Lock aspect ratio", target: nil, action: nil)
    private let cropX = NSTextField()
    private let cropY = NSTextField()
    private let cropWidth = NSTextField()
    private let cropHeight = NSTextField()
    private var cropFieldLabels: [NSTextField] = []
    private let outputModeLabel = NSTextField(labelWithString: "Output resolution")
    private let outputMode = NSPopUpButton()
    private let outputWidthLabel = NSTextField(labelWithString: "Width")
    private let outputHeightLabel = NSTextField(labelWithString: "Height")
    private let outputWidth = NSTextField()
    private let outputHeight = NSTextField()
    private let geometryHelp = NSTextField(wrappingLabelWithString:
        "Apply previews even-pixel sizes for the selected format and quality.")
    private var stagedCrop: NativeRecordingCropRect?
    private var cropAspectUnlocked = false
    private var resolutionPreset = NativeRecordingResolutionPreset.original
    private var customOutput = false
    /// Preview caption: position, source and accepted output identity.
    private let sourceLabel = NSTextField(labelWithString: "Opening recording…")
    private let seekSlider = NSSlider(value: 0, minValue: 0, maxValue: 1,
                                      target: nil, action: nil)
    private let seekLabel = NSTextField(labelWithString: "0:00.000")
    private let positionLabel = NSTextField(labelWithString: "Position")
    private let trimPanel = Surface()
    private let trimRangeLabel = NSTextField(labelWithString: "")
    private let trimSelectedLabel = NSTextField(labelWithString: "")
    private let thumbnailStatusLabel = NSTextField(labelWithString: "Source thumbnails unavailable.")
    private let trimStartLabel = NSTextField(labelWithString: "Start (ms)")
    private let trimEndLabel = NSTextField(labelWithString: "End (ms)")
    private let trimStart = NSTextField()
    private let trimEnd = NSTextField()
    private let trimTimeline: RecordingTrimTimeline
    private var resetTrimButton: CaptureButton!
    private let gifPanel = Surface()
    private let gifTitle = NSTextField(labelWithString: "GIF settings")
    private let audioPanel = Surface()
    private let audioTitle = NSTextField(labelWithString: "Audio")
    private let gifAudioNote = NSTextField(wrappingLabelWithString: "GIFs do not include recorded audio.")
    private let systemVolume = NSTextField()
    private let microphoneVolume = NSTextField()
    private let systemVolumeSlider = NSSlider(value: 100, minValue: 0, maxValue: 200,
                                              target: nil, action: nil)
    private let microphoneVolumeSlider = NSSlider(value: 100, minValue: 0, maxValue: 200,
                                                  target: nil, action: nil)
    /// Checked includes the track, like shipping's audio rows.
    private let systemAudio = NSButton(checkboxWithTitle: "System audio", target: nil, action: nil)
    private let microphoneAudio = NSButton(checkboxWithTitle: "Microphone", target: nil, action: nil)
    private let monoOutput = NSButton(checkboxWithTitle: "Convert to mono", target: nil, action: nil)
    private let format = NSPopUpButton()
    private let qualityPanel = Surface()
    private let qualityTitle = NSTextField(labelWithString: "Save quality")
    private let qualityModeLabel = NSTextField(labelWithString: "Quality mode")
    private let qualityMode = NSPopUpButton()
    private let qualityModeHelp = NSTextField(wrappingLabelWithString: "")
    private let qualityLabel = NSTextField(labelWithString: "Quality")
    private let quality = NSPopUpButton()
    private let gifFrameRateLabel = NSTextField(labelWithString: "Frame rate")
    private let gifFrameRate = NSPopUpButton()
    private let gifMaximumWidthLabel = NSTextField(labelWithString: "Maximum width")
    private let gifMaximumWidthControl = NSPopUpButton()
    private let maximumSizeLabel = NSTextField(labelWithString: "Maximum file size")
    private let maximumSizeValue = NSTextField()
    private let maximumSizeUnits = NSPopUpButton()
    private let maximumSizeInvalid = NSTextField(labelWithString: "Enter at least 100 KB (decimal units).")
    private let maximumSizeWarning = NSTextField(wrappingLabelWithString:
        "Preserve quality with a hard limit. Save fails if no retry fits; the original stays unchanged.")
    private let estimateTitle = NSTextField(labelWithString: "Est. size")
    private let estimateLabel = NSTextField(labelWithString: "—")
    private let estimateDelta = NSTextField(labelWithString: "")
    private let filenameLabel = NSTextField(labelWithString: "Filename")
    private let savingToLabel = NSTextField(labelWithString: "Saving to")
    /// The folder a new copy is saved in; the filename and format add the rest.
    private let destination = NSTextField(labelWithString: "")
    private let filenameField = NSTextField()
    private let status = NSTextField(wrappingLabelWithString: "")
    private let progress = RecordingProgressBar()
    private var applyButton: CaptureButton!
    private var estimateButton: CaptureButton!
    private var saveButton: CaptureButton!
    private var replaceButton: CaptureButton!
    private var cancelButton: CaptureButton!
    private var changeButton: CaptureButton!
    private var thumbnailRetryButton: CaptureButton!
    private var playbackButton: CaptureButton!
    private var cropAdjustmentButton: CaptureButton!
    private var previewFitButton: CaptureButton!
    private var previewActualButton: CaptureButton!
    private let playbackLoop = NSButton(checkboxWithTitle: "Loop preview", target: nil, action: nil)
    private let playbackSound = NSButton(checkboxWithTitle: "Sound", target: nil, action: nil)
    private var destinationDirectory = ""
    private var compressQuality = "highest"
    private var estimating = false
    private var replacing = false
    private var layoutSignature = ""
    private var lastPreviewImageRect = NSRect.zero
    private var showInFolderButton: CaptureButton!
    /// The last new copy this editor saved, for shipping's Show in Folder.
    private var lastSavedPath: String?

    /// A user close of the editor window; quitting does not report closes.
    var didClose: (String) -> Void = { _ in }
    /// Reveals a saved copy in Finder; replaceable for tests.
    var revealFiles: ([URL]) -> Void = { NSWorkspace.shared.activateFileViewerSelecting($0) }

    init(tokens: Tokens, worker: RecordingEditorWorking = RecordingEditorWorker(),
         reportError: @escaping (String) -> Void = { _ in },
         didSaveCopy: @escaping () -> Void = {},
         didReplaceOriginal: @escaping (String) -> Void = { _ in },
         confirmDiscard: (() -> Bool)? = nil,
         confirmReplaceOriginal: ((NSWindow, String, @escaping (Bool) -> Void) -> Void)? = nil,
         requestTermination: @escaping () -> Void = { NSApp.terminate(nil) }) {
        self.tokens = tokens; self.worker = worker; self.reportError = reportError
        self.didSaveCopy = didSaveCopy; self.didReplaceOriginal = didReplaceOriginal
        self.confirmDiscard = confirmDiscard ?? RecordingEditorController.confirmDiscardAlert
        self.confirmReplaceOriginal = confirmReplaceOriginal
            ?? RecordingEditorController.presentReplaceOriginalConfirmation
        self.requestTermination = requestTermination
        trimTimeline = RecordingTrimTimeline(tokens: tokens)
        cropOverlay = RecordingCropOverlay(tokens: tokens)
        comparisonView = RecordingComparisonView(tokens: tokens)
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

    func present(artifact: CaptureArtifact, historyRoot: String, outputDirectory: String,
                 completion: ((Bool) -> Void)? = nil) {
        if artifactID == artifact.id, presentation != nil {
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            completion?(true)
            return
        }
        if playbackState != .idle {
            guard switchAfterPlayback == nil else { completion?(false); return }
            switchAfterPlayback = artifact.id
            pausePlayback { [weak self] in
                guard let self else { completion?(false); return }
                self.switchAfterPlayback = nil
                self.present(artifact: artifact, historyRoot: historyRoot,
                             outputDirectory: outputDirectory, completion: completion)
            }
            return
        }
        if artifactID != nil, busy || pickerOpen || awaitingReplaceConfirmation
            || dirty || cropAdjustmentActive {
            showError("Finish, cancel, save, or discard the current recording edits first.")
            window.makeKeyAndOrderFront(nil)
            completion?(false)
            return
        }
        generation += 1
        let current = generation
        artifactID = artifact.id; presentation = nil; savedEdit = nil; savedExport = nil
        originalPath = nil; requiresReopen = false
        estimate = nil; activeCancel = nil; thumbnailCancel = nil; busy = true; pickerOpen = false
        invalidateComparison()
        playbackPositionMilliseconds = nil; playbackReachedEOF = false; playbackFramePresented = false
        playbackLoopEnabled = false; playbackLoopControl = nil; playbackLoop.state = .off
        playbackSoundEnabled = false; playbackAudioEnabled = nil; playbackSound.state = .off
        gifFramesPerSecond = 15; gifFrameRate.selectItem(withTitle: "15 FPS")
        gifMaximumWidth = 800; gifMaximumWidthControl.selectItem(withTitle: "800 px")
        if gifMaximumWidthControl.item(withTitle: "Original") != nil {
            gifMaximumWidthControl.removeItem(withTitle: "Original")
        }
        maximumSizeEnabled = false; preserveQuality = true; estimating = false; replacing = false
        lastSavedPath = nil; droppedFramesBand.isHidden = true
        maximumSizeUnit = .megabytes; maximumSizeUnits.selectItem(withTitle: "MB")
        maximumSizeValue.stringValue = "10"
        qualityPreference = "preserve"; compressQuality = "highest"
        sourceFrameCache = nil; sourceFrameCancel = nil
        cropAdjustmentActive = false; cropAdjustmentPriorImage = nil
        previewActualSize = false
        cropOverlay.isHidden = true; cropOverlay.setEditingEnabled(false)
        stagedCrop = nil; cropAspectUnlocked = false
        resolutionPreset = .original; customOutput = false
        setPreviewImage(nil)
        trimTimeline.clearThumbnails()
        setDestinationDirectory(outputDirectory)
        filenameField.stringValue = Self.defaultFilenameStem()
        titleLabel.stringValue = RecordingEditorCopy.title(
            mimeType: artifact.kind == "gif" ? "image/gif" : "video/mp4")
        sourceLabel.stringValue = "Opening recording…"
        status.stringValue = "Decoding the first source-relative frame…"
        progress.isHidden = true
        updateControls(); layout()
        window.center(); window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        worker.open(historyRoot: historyRoot, artifactID: artifact.id) { [weak self] result in
            guard let self, self.generation == current, self.artifactID == artifact.id else {
                completion?(false); return
            }
            let accepted: Bool
            switch result {
            case .success(let value):
                accepted = true
                self.busy = false
                self.originalPath = value.originalSavePath
                self.publish(value, initialize: true)
                self.status.stringValue = "Original remains unchanged. Save creates a new copy."
                self.generateThumbnails(completion: { completion?(true) })
            case .failure(let error):
                accepted = false
                self.busy = false
                self.showError("Couldn’t open recording: \(error.localizedDescription)")
            }
            self.updateControls(); self.layout()
            if !accepted { completion?(false) }
        }
    }

    var dirty: Bool {
        guard let snapshot = presentation?.snapshot else { return false }
        return stagedDiffers || canonicalEdit(snapshot.edit) != savedEdit
            || canonical(snapshot.saveExport) != savedExport
    }

    var activeArtifactID: String? { window.isVisible ? artifactID : nil }

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
        if busy || pickerOpen || awaitingReplaceConfirmation {
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
        if busy || pickerOpen || awaitingReplaceConfirmation {
            showError("Cancel or wait for the recording operation before closing.")
            return false
        }
        if dirty && !confirmDiscard() { return false }
        let closedArtifactID = artifactID
        closeSession()
        if let closedArtifactID { didClose(closedArtifactID) }
        return true
    }

    func windowDidResize(_ notification: Notification) { cropOverlay.endDrag(); layout() }
    func windowDidResignKey(_ notification: Notification) {
        trimTimeline.endDrag(); cropOverlay.endDrag(); pausePlayback()
    }
    func windowDidMiniaturize(_ notification: Notification) { pausePlayback() }
    func controlTextDidChange(_ notification: Notification) {
        if notification.object as? NSTextField === filenameField {
            // Editing the name clears a prior filename error, as shipping does.
            if status.textColor == tokens.color("danger-text") {
                status.stringValue = ""; status.textColor = tokens.color("text-subtle")
            }
            updateControls(); return
        }
        invalidateComparison()
        if let field = notification.object as? NSTextField,
           field === systemVolume || field === microphoneVolume {
            syncVolumeSlider(field === systemVolume ? systemVolumeSlider : microphoneVolumeSlider,
                             from: field)
            estimate = nil; updateControls(); return
        }
        if let field = notification.object as? NSTextField,
           [cropX, cropY, cropWidth, cropHeight].contains(where: { $0 === field }) {
            estimate = nil; updateControls(); return
        }
        if notification.object as? NSTextField === maximumSizeValue {
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
        pageScroll.drawsBackground = false; pageScroll.borderType = .noBorder
        pageScroll.hasVerticalScroller = true; pageScroll.autohidesScrollers = true
        pageScroll.scrollerStyle = .overlay
        pageScroll.contentView.drawsBackground = false
        pageScroll.documentView = page
        pageScroll.setAccessibilityLabel("Recording editor page")
        root.addSubview(pageScroll)
        footer.wantsLayer = true
        footer.layer?.backgroundColor = tokens.color("surface-raised").cgColor
        footerDivider.wantsLayer = true
        footerDivider.layer?.backgroundColor = tokens.color("border-subtle").cgColor
        footer.addSubview(footerDivider)
        root.addSubview(footer)

        style(titleLabel, size: "text-2xl", color: "text", weight: .semibold, parent: page)
        titleLabel.setAccessibilityLabel("Recording editor title")
        droppedFramesBand.wantsLayer = true
        droppedFramesBand.layer?.backgroundColor = tokens.color("caution-surface").cgColor
        droppedFramesBand.layer?.cornerRadius = tokens.number("r-md")
        droppedFramesBand.isHidden = true
        page.addSubview(droppedFramesBand)
        style(droppedFramesLabel, size: "text-sm", color: "caution-text", parent: droppedFramesBand)
        droppedFramesLabel.setAccessibilityLabel("Dropped frames warning")

        // `.recording-editor-preview`: toolbar, sunken viewport and caption.
        card(previewPanel, parent: page)
        previewPanel.layer?.masksToBounds = true
        // The viewport's rounded top edge tucks under a square toolbar band,
        // leaving only its lower corners rounded.
        previewViewport.wantsLayer = true
        previewViewport.layer?.backgroundColor = tokens.color("surface-sunken").cgColor
        previewViewport.layer?.cornerRadius = tokens.number("r-xl")
        previewPanel.addSubview(previewViewport)
        previewToolbar.wantsLayer = true
        previewToolbar.layer?.backgroundColor = tokens.color("surface-raised").cgColor
        previewPanel.addSubview(previewToolbar)
        previewDivider.wantsLayer = true
        previewDivider.layer?.backgroundColor = tokens.color("border-subtle").cgColor
        previewPanel.addSubview(previewDivider)
        style(previewTitle, size: "text-md", color: "text-muted", weight: .medium, parent: previewPanel)
        style(audioNote, size: "text-sm", color: "text-subtle", parent: previewPanel)
        audioNote.setAccessibilityLabel("Recording preview mode")
        audioNote.lineBreakMode = .byTruncatingTail
        previewSizeTrack.wantsLayer = true
        previewSizeTrack.layer?.backgroundColor = tokens.color("surface-sunken").cgColor
        previewSizeTrack.layer?.borderColor = tokens.color("border-subtle").cgColor
        previewSizeTrack.layer?.borderWidth = 1
        previewSizeTrack.layer?.cornerRadius = tokens.number("r-lg")
        previewPanel.addSubview(previewSizeTrack)
        previewFitButton = button("Fit", parent: previewSizeTrack) { [weak self] in
            self?.setPreviewActualSize(false)
        }
        previewActualButton = button("100%", parent: previewSizeTrack) { [weak self] in
            self?.setPreviewActualSize(true)
        }
        previewFitButton.toolTip = "Fit the decoded frame within the preview."
        previewActualButton.toolTip = "One decoded image pixel per screen point. Scroll to see overflow; playback may use a reduced-size frame."
        previewScroll.drawsBackground = false; previewScroll.borderType = .noBorder
        previewScroll.scrollerStyle = .overlay; previewScroll.autohidesScrollers = true
        previewScroll.contentView.drawsBackground = false
        previewScroll.contentView.postsBoundsChangedNotifications = true
        previewScroll.setAccessibilityLabel("Recording preview viewport")
        previewScroll.documentView = previewCanvas
        previewViewport.addSubview(previewScroll)
        preview.imageScaling = .scaleProportionallyUpOrDown
        preview.setAccessibilityLabel("Decoded recording frame")
        previewCanvas.addSubview(preview)
        comparisonView.isHidden = true
        comparisonView.setAccessibilityLabel("Encoded before and after recording frame")
        previewCanvas.addSubview(comparisonView)
        comparisonButton = button("Compare", parent: previewPanel) { [weak self] in
            self?.compareAcceptedFrame()
        }
        comparisonButton.setAccessibilityLabel("Compare encoded recording before and after")
        comparisonButton.toolTip = "Encode a sample at the accepted still frame, not the paused playback position. Before is spatially edited; Encoded includes compression, GIF palette and cadence. First attempt only: a Maximum-size save may differ. Apply staged edits and seek inside the accepted trim first."
        comparisonHideButton = button("Hide", parent: previewPanel) { [weak self] in
            self?.invalidateComparison()
        }
        comparisonHideButton.setAccessibilityLabel("Hide recording comparison")
        comparisonSlider.target = self; comparisonSlider.action = #selector(comparisonSplitChanged)
        comparisonSlider.setAccessibilityLabel("Recording before and after split")
        comparisonSlider.setAccessibilityHelp("Left is before encoding; right is the encoded first attempt at the accepted source-relative position. Output cadence may select a neighboring frame.")
        comparisonSlider.isHidden = true
        style(comparisonBeforeLabel, size: "text-xs", color: "text-subtle", parent: previewPanel)
        style(comparisonAfterLabel, size: "text-xs", color: "text-subtle", parent: previewPanel)
        comparisonBeforeLabel.isHidden = true; comparisonAfterLabel.isHidden = true
        previewPanel.addSubview(comparisonSlider)
        cropOverlay.toolTip = "Drag inside to move. Drag a handle to resize. Arrow keys move a focused handle by 1 source pixel; Shift moves 10."
        cropOverlay.onStage = { [weak self] crop in self?.stageGraphicalCrop(crop) }
        cropOverlay.onCommitPendingInput = { [weak self] in
            guard let self else { return }
            self.window.makeFirstResponder(nil)
            _ = self.commitPendingCropInput()
            self.updateControls()
        }
        previewCanvas.addSubview(cropOverlay)
        // `.recording-preview-overlay-play`: an accent circle over the media.
        playbackButton = button("Play", parent: previewViewport) { [weak self] in
            self?.togglePlayback()
        }
        playbackButton.primary = true; playbackButton.circular = true
        playbackButton.iconOnly = true; playbackButton.icon = .shipping("resume")
        playbackButton.setAccessibilityLabel("Play silent recording preview")
        playbackLoop.target = self; playbackLoop.action = #selector(playbackLoopChanged)
        playbackLoop.setAccessibilityLabel("Loop recording preview")
        playbackLoop.toolTip = "Repeat the accepted trim until paused. This changes only playback, not the saved recording."
        playbackSound.target = self; playbackSound.action = #selector(playbackSoundChanged)
        playbackSound.setAccessibilityLabel("Preview accepted recording audio")
        playbackSound.toolTip = "Preview accepted MP4 audio on the default output device. Change only while stopped. This never changes the export."
        previewPanel.addSubview(playbackLoop); previewPanel.addSubview(playbackSound)
        style(sourceLabel, size: "text-xs", color: "text-subtle", parent: previewPanel)
        sourceLabel.setAccessibilityLabel("Recording source details")
        sourceLabel.lineBreakMode = .byTruncatingTail
        style(previewNote, size: "text-xs", color: "text-faint", parent: previewPanel)
        previewNote.lineBreakMode = .byTruncatingTail
        NotificationCenter.default.addObserver(self, selector: #selector(previewDidScroll(_:)),
            name: NSView.boundsDidChangeNotification, object: previewScroll.contentView)

        // `.recording-timeline`: summary, track and trim/position fields.
        card(trimPanel, parent: page)
        style(trimRangeLabel, size: "text-sm", color: "text", weight: .semibold, parent: trimPanel)
        trimRangeLabel.setAccessibilityLabel("Recording trim range summary")
        style(trimSelectedLabel, size: "text-sm", color: "text-subtle", parent: trimPanel)
        trimSelectedLabel.alignment = .right
        trimSelectedLabel.setAccessibilityLabel("Recording selected duration")
        style(thumbnailStatusLabel, size: "text-xs", color: "text-subtle", parent: trimPanel)
        thumbnailStatusLabel.alignment = .right
        trimTimeline.onStage = { [weak self] edge, milliseconds in
            guard let self else { return }
            (edge == .start ? self.trimStart : self.trimEnd).stringValue = String(milliseconds)
            self.invalidateComparison()
            self.estimate = nil; self.updateControls()
        }
        trimTimeline.onSeek = { [weak self] milliseconds in self?.seek(to: milliseconds) }
        trimPanel.addSubview(trimTimeline)
        thumbnailRetryButton = button("Retry", parent: trimPanel) { [weak self] in
            self?.generateThumbnails()
        }
        thumbnailRetryButton.setAccessibilityLabel("Retry recording thumbnails")
        configureNumberField(trimStart, label: "Trim start milliseconds")
        configureNumberField(trimEnd, label: "Trim end milliseconds")
        for (text, field) in [(trimStartLabel, trimStart), (trimEndLabel, trimEnd)] {
            style(text, size: "text-xs", color: "text-subtle", parent: trimPanel)
            field.alignment = .right
            trimPanel.addSubview(field)
        }
        resetTrimButton = button("Reset trim", parent: trimPanel) { [weak self] in self?.resetTrim() }
        style(positionLabel, size: "text-xs", color: "text-subtle", parent: trimPanel)
        seekSlider.target = self; seekSlider.action = #selector(seekChanged)
        seekSlider.setAccessibilityLabel("Recording frame position")
        trimPanel.addSubview(seekSlider)
        style(seekLabel, size: "text-xs", color: "text-subtle", parent: trimPanel)
        seekLabel.font = .monospacedDigitSystemFont(ofSize: tokens.number("text-xs"), weight: .regular)
        seekLabel.alignment = .right

        // `.editor-output-card`: GIF settings.
        card(gifPanel, parent: page)
        style(gifTitle, size: "text-lg", color: "text", weight: .semibold, parent: gifPanel)
        style(gifFrameRateLabel, size: "text-xs", color: "text-subtle", parent: gifPanel)
        style(gifMaximumWidthLabel, size: "text-xs", color: "text-subtle", parent: gifPanel)
        gifFrameRate.addItems(withTitles: ["8 FPS", "10 FPS", "12 FPS", "15 FPS",
                                              "20 FPS", "24 FPS", "30 FPS"])
        gifFrameRate.selectItem(withTitle: "15 FPS")
        gifFrameRate.target = self; gifFrameRate.action = #selector(gifFrameRateChanged)
        gifFrameRate.setAccessibilityLabel("GIF frame rate")
        gifMaximumWidthControl.addItems(withTitles: ["320 px", "480 px", "640 px", "800 px",
                                                        "1200 px"])
        gifMaximumWidthControl.selectItem(withTitle: "800 px")
        gifMaximumWidthControl.target = self
        gifMaximumWidthControl.action = #selector(gifMaximumWidthChanged)
        gifMaximumWidthControl.setAccessibilityLabel("GIF maximum width")
        gifPanel.addSubview(gifFrameRate); gifPanel.addSubview(gifMaximumWidthControl)

        // Crop & size.
        card(geometryPanel, parent: page)
        style(geometryTitle, size: "text-lg", color: "text", weight: .semibold, parent: geometryPanel)
        cropEnabled.target = self; cropEnabled.action = #selector(cropEnabledChanged)
        cropEnabled.setAccessibilityLabel("Crop recording")
        cropLock.target = self; cropLock.action = #selector(cropLockChanged)
        cropLock.setAccessibilityLabel("Lock recording crop aspect ratio")
        geometryPanel.addSubview(cropEnabled); geometryPanel.addSubview(cropLock)
        cropAdjustmentButton = button("Adjust crop", parent: geometryPanel) { [weak self] in
            self?.toggleCropAdjustment()
        }
        cropAdjustmentButton.setAccessibilityLabel("Adjust recording crop graphically")
        cropAdjustmentButton.toolTip = "Drag the crop on the full source frame. Arrow keys move a focused handle by 1 source pixel; Shift moves 10."
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
        cropFieldLabels = ["X", "Y", "Width", "Height"].map { title in
            let value = NSTextField(labelWithString: title)
            style(value, size: "text-xs", color: "text-subtle", parent: geometryPanel)
            return value
        }
        for value in [outputModeLabel, outputWidthLabel, outputHeightLabel] {
            style(value, size: "text-xs", color: "text-subtle", parent: geometryPanel)
        }
        outputMode.target = self; outputMode.action = #selector(outputModeChanged)
        outputMode.setAccessibilityLabel("Recording output size")
        geometryPanel.addSubview(outputMode)
        rebuildOutputModeMenu(base: nil)
        style(geometryHelp, size: "text-xs", color: "text-subtle", parent: geometryPanel)

        // Save quality.
        card(qualityPanel, parent: page)
        style(qualityTitle, size: "text-lg", color: "text", weight: .semibold, parent: qualityPanel)
        for value in [qualityModeLabel, qualityLabel, maximumSizeLabel, estimateTitle] {
            style(value, size: "text-xs", color: "text-subtle", parent: qualityPanel)
        }
        qualityMode.target = self; qualityMode.action = #selector(qualityModeChanged)
        qualityMode.setAccessibilityLabel("Save quality")
        quality.target = self; quality.action = #selector(qualityChanged)
        quality.setAccessibilityLabel("Recording export quality")
        let menus = RecordingEditorCopy.menus(gif: false, baseWidth: 2, baseHeight: 2)
        for choice in menus["quality_modes"] ?? [] {
            qualityMode.addItem(withTitle: choice.label)
            qualityMode.lastItem?.representedObject = choice.value
            qualityMode.lastItem?.toolTip = choice.description
        }
        for choice in menus["quality_presets"] ?? [] {
            quality.addItem(withTitle: choice.label)
            quality.lastItem?.representedObject = choice.value
            quality.lastItem?.toolTip = choice.description
        }
        qualityPanel.addSubview(qualityMode); qualityPanel.addSubview(quality)
        style(qualityModeHelp, size: "text-xs", color: "text-subtle", parent: qualityPanel)
        configureNumberField(maximumSizeValue, label: "Maximum recording file size value")
        maximumSizeValue.stringValue = "10"
        maximumSizeValue.placeholderString = "At least 100 KB"
        maximumSizeUnits.addItems(withTitles: RecordingFileSizeUnit.allCases.map { $0.label })
        maximumSizeUnits.selectItem(withTitle: maximumSizeUnit.label)
        maximumSizeUnits.target = self; maximumSizeUnits.action = #selector(maximumSizeUnitChanged)
        maximumSizeUnits.setAccessibilityLabel("Maximum recording file size unit")
        qualityPanel.addSubview(maximumSizeValue); qualityPanel.addSubview(maximumSizeUnits)
        style(maximumSizeInvalid, size: "text-xs", color: "danger-text", parent: qualityPanel)
        style(maximumSizeWarning, size: "text-xs", color: "text-subtle", parent: qualityPanel)
        maximumSizeWarning.setAccessibilityLabel("Maximum recording file size preview warning")
        style(estimateLabel, size: "text-sm", color: "text", parent: qualityPanel)
        estimateLabel.font = .monospacedDigitSystemFont(ofSize: tokens.number("text-sm"), weight: .regular)
        estimateLabel.setAccessibilityLabel("Recording size estimate")
        estimateLabel.toolTip = "Estimated saved file size for the current edits and settings"
        estimateLabel.setAccessibilityHelp(estimateLabel.toolTip)
        style(estimateDelta, size: "text-xs", color: "positive-text", weight: .semibold, parent: qualityPanel)
        estimateDelta.alignment = .center
        estimateDelta.wantsLayer = true
        estimateDelta.layer?.cornerRadius = tokens.number("r-sm")
        estimateDelta.setAccessibilityLabel("Recording size estimate change")
        estimateDelta.toolTip = "Change versus the original recording file"
        estimateButton = button("Estimate size", parent: qualityPanel) { [weak self] in
            self?.estimateSize()
        }
        estimateButton.toolTip = "Percentage change compares the accepted estimate with the original recording file. Longer recordings use approximate encoded samples. No History entry or saved file is created."

        // Audio.
        card(audioPanel, parent: page)
        style(audioTitle, size: "text-lg", color: "text", weight: .semibold, parent: audioPanel)
        style(gifAudioNote, size: "text-sm", color: "caution-text", parent: audioPanel)
        for control in [systemAudio, microphoneAudio, monoOutput] {
            control.target = self; control.action = #selector(stageChanged)
            audioPanel.addSubview(control)
        }
        systemAudio.setAccessibilityLabel("System audio")
        microphoneAudio.setAccessibilityLabel("Microphone")
        monoOutput.setAccessibilityLabel("Mono audio output")
        configureVolumeField(systemVolume, label: "System audio volume percent")
        configureVolumeField(microphoneVolume, label: "Microphone volume percent")
        for (slider, name) in [(systemVolumeSlider, "System audio volume"),
                               (microphoneVolumeSlider, "Microphone volume")] {
            slider.target = self; slider.action = #selector(volumeSliderChanged(_:))
            slider.setAccessibilityLabel(name)
            audioPanel.addSubview(slider)
        }
        audioPanel.addSubview(systemVolume); audioPanel.addSubview(microphoneVolume)

        // `.recording-save-footer`.
        progress.tokens = tokens
        progress.isHidden = true
        footer.addSubview(progress)
        style(filenameLabel, size: "text-xs", color: "text-subtle", parent: footer)
        style(savingToLabel, size: "text-2xs", color: "text-faint", parent: footer)
        style(destination, size: "text-2xs", color: "text-subtle", parent: footer)
        destination.font = .monospacedSystemFont(ofSize: tokens.number("text-2xs"), weight: .regular)
        destination.lineBreakMode = .byTruncatingMiddle
        destination.alignment = .right
        destination.setAccessibilityLabel("Recording destination")
        changeButton = button("Change…", parent: footer) { [weak self] in self?.chooseDestination() }
        changeButton.setAccessibilityLabel("Change save location")
        changeButton.toolTip = "Choose the folder for the new copy."
        filenameField.delegate = self
        filenameField.setAccessibilityLabel("Saved filename")
        filenameField.font = .systemFont(ofSize: tokens.number("text-sm"))
        filenameField.lineBreakMode = .byTruncatingTail
        footer.addSubview(filenameField)
        format.addItems(withTitles: [".mp4", ".gif"])
        format.item(at: 0)?.toolTip = "MP4"; format.item(at: 1)?.toolTip = "GIF"
        format.target = self; format.action = #selector(formatChanged)
        format.setAccessibilityLabel("Recording export format")
        footer.addSubview(format)
        status.textColor = tokens.color("text-subtle"); status.maximumNumberOfLines = 2
        status.font = .systemFont(ofSize: tokens.number("text-xs"))
        status.alignment = .right
        status.setAccessibilityLabel("Recording editor status")
        footer.addSubview(status)
        cancelButton = button("Cancel", parent: footer) { [weak self] in self?.cancelActiveOperation() }
        replaceButton = button("Replace original…", parent: footer) { [weak self] in
            self?.confirmReplace()
        }
        showInFolderButton = button("Show in Folder", parent: footer) { [weak self] in
            self?.revealSavedCopy()
        }
        showInFolderButton.icon = .shipping("folder")
        applyButton = button("Apply edits", parent: footer) { [weak self] in self?.applyEdits() }
        applyButton.toolTip = "Update the preview before scrubbing or saving"
        saveButton = button("Save new copy", parent: footer) { [weak self] in self?.saveNewCopy() }
        saveButton.primary = true
        saveButton.icon = .shipping("save")
        saveButton.toolTip = "Creates a separate copy. The original and existing files are never replaced."
    }

    /// A shipping `.editor-card`: raised, hairline border, large radius.
    private func card(_ view: Surface, parent: NSView) {
        view.wantsLayer = true
        view.layer?.backgroundColor = tokens.color("surface-raised").cgColor
        view.layer?.borderColor = tokens.color("border-subtle").cgColor
        view.layer?.borderWidth = 1
        view.layer?.cornerRadius = tokens.number("r-xl")
        parent.addSubview(view)
    }

    private func style(_ field: NSTextField, size: String, color: String,
                       weight: NSFont.Weight = .regular, parent: NSView) {
        field.font = .systemFont(ofSize: tokens.number(size), weight: weight)
        field.textColor = tokens.color(color)
        parent.addSubview(field)
    }

    private func rebuildOutputModeMenu(base: NativeRecordingDimensions?) {
        let menus = RecordingEditorCopy.menus(gif: format.indexOfSelectedItem == 1,
                                              baseWidth: base?.width ?? 0,
                                              baseHeight: base?.height ?? 0)
        let choices = menus["resolutions"] ?? []
        let selected = outputMode.indexOfSelectedItem
        if outputMode.numberOfItems != choices.count {
            outputMode.removeAllItems()
            choices.forEach { outputMode.addItem(withTitle: $0.label) }
        }
        for (index, choice) in choices.enumerated() {
            guard let item = outputMode.item(at: index) else { continue }
            item.title = choice.label
            item.representedObject = choice.value
            item.toolTip = choice.description
        }
        if selected >= 0, selected < outputMode.numberOfItems { outputMode.selectItem(at: selected) }
    }

    private func layout() {
        let width = root.bounds.width, height = root.bounds.height
        guard width > 0, height > 0 else { return }
        let footerHeight = layoutFooter(width: width)
        footer.frame = NSRect(x: 0, y: max(0, height - footerHeight), width: width,
                              height: footerHeight)
        pageScroll.frame = NSRect(x: 0, y: 0, width: width, height: max(0, height - footerHeight))
        let pageWidth = pageScroll.contentSize.width
        let contentHeight = layoutPage(width: pageWidth, windowHeight: height)
        page.frame = NSRect(x: 0, y: 0, width: pageWidth,
                            height: max(pageScroll.contentSize.height, contentHeight))
        refreshPreviewLayout(resetScroll: false)
    }

    /// Lays out the scrolling page and returns its content height.
    private func layoutPage(width: CGFloat, windowHeight: CGFloat) -> CGFloat {
        let pad = tokens.number("s-8"), gap = tokens.number("s-5")
        let side = max(pad, (width - 1_220) / 2)
        let inner = max(0, width - side * 2)
        var y = pad
        titleLabel.frame = NSRect(x: side, y: y, width: inner, height: 30)
        y += 30 + gap
        if !droppedFramesBand.isHidden {
            let inset = NSSize(width: tokens.number("s-5"), height: tokens.number("s-4"))
            droppedFramesBand.frame = NSRect(x: side, y: y, width: inner, height: 20 + inset.height * 2)
            droppedFramesLabel.frame = NSRect(x: inset.width, y: inset.height,
                                              width: max(0, inner - inset.width * 2), height: 20)
            y = droppedFramesBand.frame.maxY + gap
        }

        // Preview card.
        let toolbarHeight: CGFloat = 46
        let viewportHeight = min(480, max(180, windowHeight * 0.46))
        let captionTop = toolbarHeight + viewportHeight
        let comparisonRow: CGFloat = comparison == nil ? 0 : 26
        let noteRow: CGFloat = previewNote.isHidden ? 0 : 16
        let captionHeight = tokens.number("s-4") + comparisonRow + 18 + noteRow + tokens.number("s-4")
        previewPanel.frame = NSRect(x: side, y: y, width: inner,
                                    height: captionTop + captionHeight)
        layoutPreviewCard(width: inner, toolbarHeight: toolbarHeight,
                          viewportHeight: viewportHeight, comparisonRow: comparisonRow)
        y = previewPanel.frame.maxY + gap

        // Timeline card.
        layoutTimelineCard(width: inner)
        trimPanel.frame.origin = NSPoint(x: side, y: y)
        y = trimPanel.frame.maxY + gap

        // GIF settings, then Crop & size beside Save quality, then Audio.
        if !gifPanel.isHidden {
            layoutGifCard(width: inner)
            gifPanel.frame.origin = NSPoint(x: side, y: y)
            y = gifPanel.frame.maxY + gap
        }
        let columns = inner >= 700
        let column = columns ? (inner - gap) / 2 : inner
        layoutGeometryCard(width: column)
        layoutQualityCard(width: column)
        geometryPanel.frame.origin = NSPoint(x: side, y: y)
        if columns {
            qualityPanel.frame.origin = NSPoint(x: side + column + gap, y: y)
            y = max(geometryPanel.frame.maxY, qualityPanel.frame.maxY) + gap
        } else {
            qualityPanel.frame.origin = NSPoint(x: side, y: geometryPanel.frame.maxY + gap)
            y = qualityPanel.frame.maxY + gap
        }
        if !audioPanel.isHidden {
            layoutAudioCard(width: inner)
            audioPanel.frame.origin = NSPoint(x: side, y: y)
            y = audioPanel.frame.maxY + gap
        }
        return y - gap + pad
    }

    private func layoutPreviewCard(width: CGFloat, toolbarHeight: CGFloat,
                                   viewportHeight: CGFloat, comparisonRow: CGFloat) {
        let padding = tokens.number("s-5"), itemGap = tokens.number("s-4")
        let radius = tokens.number("r-xl")
        previewToolbar.frame = NSRect(x: 0, y: 0, width: width, height: toolbarHeight)
        previewDivider.frame = NSRect(x: 0, y: toolbarHeight - 1, width: width, height: 1)
        previewViewport.frame = NSRect(x: 0, y: toolbarHeight - radius, width: width,
                                       height: viewportHeight + radius)
        let controlY = (toolbarHeight - tokens.number("h-sm")) / 2
        // Right to left: Fit | 100%, Loop preview, Compare/Hide, Sound.
        var right = width - padding
        let segmentHeight = tokens.number("h-sm") + 6
        previewSizeTrack.frame = NSRect(x: right - 132, y: (toolbarHeight - segmentHeight) / 2,
                                        width: 132, height: segmentHeight)
        previewFitButton.frame = NSRect(x: 3, y: 3, width: 63, height: segmentHeight - 6)
        previewActualButton.frame = NSRect(x: 66, y: 3, width: 63, height: segmentHeight - 6)
        right = previewSizeTrack.frame.minX - itemGap
        playbackLoop.frame = NSRect(x: right - 112, y: controlY, width: 112, height: tokens.number("h-sm"))
        right = playbackLoop.frame.minX - itemGap
        comparisonHideButton.frame = NSRect(x: right - 56, y: controlY, width: 56,
                                            height: tokens.number("h-sm"))
        if !comparisonHideButton.isHidden { right = comparisonHideButton.frame.minX - itemGap }
        comparisonButton.frame = NSRect(x: right - 84, y: controlY, width: 84,
                                        height: tokens.number("h-sm"))
        right = comparisonButton.frame.minX - itemGap
        playbackSound.frame = NSRect(x: right - 72, y: controlY, width: 72, height: tokens.number("h-sm"))
        right = playbackSound.frame.minX - itemGap
        previewTitle.frame = NSRect(x: padding, y: (toolbarHeight - 18) / 2, width: 56, height: 18)
        audioNote.frame = NSRect(x: previewTitle.frame.maxX + itemGap, y: (toolbarHeight - 16) / 2,
                                 width: max(0, right - previewTitle.frame.maxX - itemGap), height: 16)
        // Viewport: the media fits inside a padded, sunken well.
        previewScroll.frame = NSRect(x: padding, y: radius + padding,
                                     width: max(0, width - padding * 2),
                                     height: max(0, viewportHeight - padding * 2))
        // Caption below the viewport.
        var y = toolbarHeight + viewportHeight + tokens.number("s-4")
        if comparisonRow > 0 {
            comparisonBeforeLabel.frame = NSRect(x: padding, y: y + 4, width: 48, height: 16)
            comparisonAfterLabel.frame = NSRect(x: width - padding - 60, y: y + 4, width: 60, height: 16)
            comparisonAfterLabel.alignment = .right
            comparisonSlider.frame = NSRect(x: padding + 52, y: y,
                                            width: max(0, width - padding * 2 - 116), height: 24)
            y += comparisonRow
        }
        sourceLabel.frame = NSRect(x: padding, y: y, width: max(0, width - padding * 2), height: 18)
        y += 18
        previewNote.frame = NSRect(x: padding, y: y, width: max(0, width - padding * 2), height: 16)
    }

    /// Positions the overlay play control over the presented media.
    private func layoutOverlayPlay(imageRect: NSRect) {
        let size = tokens.number("s-12") - tokens.number("s-4")
        let visible = previewScroll.convert(imageRect.intersection(previewCanvas.visibleRect),
                                            from: previewCanvas)
        let viewportRect = previewViewport.convert(visible, from: previewScroll)
        let bounds = previewViewport.convert(previewScroll.bounds, from: previewScroll)
        var center = NSPoint(x: viewportRect.midX, y: viewportRect.midY)
        if comparison != nil {
            // Keep play clear of the centered comparison divider, as shipping does.
            center.y = viewportRect.maxY - tokens.number("s-5") - tokens.number("s-10") - size / 2
        }
        if viewportRect.isEmpty { center = NSPoint(x: bounds.midX, y: bounds.midY) }
        center.x = min(max(center.x, bounds.minX + size / 2), max(bounds.minX + size / 2, bounds.maxX - size / 2))
        center.y = min(max(center.y, bounds.minY + size / 2), max(bounds.minY + size / 2, bounds.maxY - size / 2))
        playbackButton.frame = NSRect(x: center.x - size / 2, y: center.y - size / 2,
                                      width: size, height: size)
    }

    private func layoutTimelineCard(width: CGFloat) {
        let padding = tokens.number("s-6"), itemGap = tokens.number("s-3")
        let fieldHeight = tokens.number("h-sm")
        var y = tokens.number("s-5")
        let contentWidth = max(0, width - padding * 2)
        trimRangeLabel.frame = NSRect(x: padding, y: y, width: contentWidth / 2, height: 18)
        trimSelectedLabel.frame = NSRect(x: width - padding - 180, y: y, width: 180, height: 18)
        var right = trimSelectedLabel.frame.minX - itemGap
        thumbnailRetryButton.frame = NSRect(x: right - 64, y: y - 5, width: 64, height: 26)
        if !thumbnailRetryButton.isHidden { right = thumbnailRetryButton.frame.minX - itemGap }
        thumbnailStatusLabel.frame = NSRect(x: max(padding, right - 200), y: y + 1,
                                            width: min(200, max(0, right - padding)), height: 16)
        y += 18 + tokens.number("s-4")
        trimTimeline.frame = NSRect(x: padding, y: y, width: contentWidth,
                                    height: recordingTimelineTrackHeight + 6)
        y = trimTimeline.frame.maxY + tokens.number("s-4")
        var x = padding
        let rowY = y
        func place(_ view: NSView, width: CGFloat, height: CGFloat, dy: CGFloat) {
            view.frame = NSRect(x: x, y: rowY + dy, width: width, height: height)
            x = view.frame.maxX + itemGap
        }
        place(trimStartLabel, width: 60, height: 16, dy: 6)
        place(trimStart, width: 72, height: fieldHeight, dy: 0)
        x += tokens.number("s-3")
        place(trimEndLabel, width: 54, height: 16, dy: 6)
        place(trimEnd, width: 72, height: fieldHeight, dy: 0)
        place(resetTrimButton, width: 92, height: fieldHeight, dy: 0)
        x += tokens.number("s-5")
        place(positionLabel, width: 52, height: 16, dy: 6)
        let labelWidth: CGFloat = 76
        seekLabel.frame = NSRect(x: width - padding - labelWidth, y: y + 6, width: labelWidth, height: 16)
        seekSlider.frame = NSRect(x: x, y: y + 4, width: max(0, seekLabel.frame.minX - itemGap - x),
                                  height: 20)
        trimPanel.frame.size = NSSize(width: width, height: y + fieldHeight + tokens.number("s-6"))
    }

    private func layoutGifCard(width: CGFloat) {
        let padding = tokens.number("s-6"), gap = tokens.number("s-4")
        gifTitle.frame = NSRect(x: padding, y: padding, width: max(0, width - padding * 2), height: 20)
        let column = max(0, (width - padding * 2 - gap) / 2)
        let labelY = gifTitle.frame.maxY + tokens.number("s-5")
        gifFrameRateLabel.frame = NSRect(x: padding, y: labelY, width: column, height: 16)
        gifMaximumWidthLabel.frame = NSRect(x: padding + column + gap, y: labelY, width: column, height: 16)
        let controlY = labelY + 16 + tokens.number("s-2")
        gifFrameRate.frame = NSRect(x: padding, y: controlY, width: column, height: tokens.number("h-sm"))
        gifMaximumWidthControl.frame = NSRect(x: padding + column + gap, y: controlY, width: column,
                                              height: tokens.number("h-sm"))
        gifPanel.frame.size = NSSize(width: width, height: gifFrameRate.frame.maxY + padding)
    }

    private func layoutGeometryCard(width: CGFloat) {
        let padding = tokens.number("s-6"), gap = tokens.number("s-4")
        let content = max(0, width - padding * 2)
        let fieldHeight = tokens.number("h-sm")
        geometryTitle.frame = NSRect(x: padding, y: padding, width: content, height: 20)
        var y = geometryTitle.frame.maxY + tokens.number("s-5")
        cropEnabled.frame = NSRect(x: padding, y: y, width: min(content, 160), height: 20)
        y += 20 + tokens.number("s-5")
        let column = max(0, (content - gap * 3) / 4)
        for (index, field) in [cropX, cropY, cropWidth, cropHeight].enumerated() {
            let x = padding + CGFloat(index) * (column + gap)
            cropFieldLabels[index].frame = NSRect(x: x, y: y, width: column, height: 16)
            field.frame = NSRect(x: x, y: y + 16 + tokens.number("s-2"), width: column,
                                 height: fieldHeight)
        }
        y += 16 + tokens.number("s-2") + fieldHeight + tokens.number("s-5")
        cropLock.frame = NSRect(x: padding, y: y + 4, width: 150, height: 20)
        cropAdjustmentButton.frame = NSRect(x: cropLock.frame.maxX + gap, y: y, width: 112,
                                            height: fieldHeight)
        y += fieldHeight + tokens.number("s-5")
        outputModeLabel.frame = NSRect(x: padding, y: y, width: content, height: 16)
        y += 16 + tokens.number("s-2")
        outputMode.frame = NSRect(x: padding, y: y, width: min(content, 430), height: fieldHeight)
        y += fieldHeight + tokens.number("s-5")
        if customOutput {
            let half = max(0, (content - gap) / 2)
            outputWidthLabel.frame = NSRect(x: padding, y: y, width: half, height: 16)
            outputHeightLabel.frame = NSRect(x: padding + half + gap, y: y, width: half, height: 16)
            y += 16 + tokens.number("s-2")
            outputWidth.frame = NSRect(x: padding, y: y, width: half, height: fieldHeight)
            outputHeight.frame = NSRect(x: padding + half + gap, y: y, width: half, height: fieldHeight)
            y += fieldHeight + tokens.number("s-5")
        }
        geometryHelp.frame = NSRect(x: padding, y: y, width: content, height: 30)
        geometryPanel.frame.size = NSSize(width: width, height: geometryHelp.frame.maxY + padding)
    }

    private func layoutQualityCard(width: CGFloat) {
        let padding = tokens.number("s-6"), gap = tokens.number("s-4")
        let content = max(0, width - padding * 2)
        let fieldHeight = tokens.number("h-sm")
        let controlWidth = min(content, 430)
        qualityTitle.frame = NSRect(x: padding, y: padding, width: content, height: 20)
        var y = qualityTitle.frame.maxY + tokens.number("s-5")
        qualityModeLabel.frame = NSRect(x: padding, y: y, width: content, height: 16)
        y += 16 + tokens.number("s-2")
        qualityMode.frame = NSRect(x: padding, y: y, width: controlWidth, height: fieldHeight)
        y += fieldHeight + tokens.number("s-3")
        qualityModeHelp.frame = NSRect(x: padding, y: y, width: content, height: 30)
        y += 30 + tokens.number("s-3")
        if !quality.isHidden {
            qualityLabel.frame = NSRect(x: padding, y: y, width: content, height: 16)
            y += 16 + tokens.number("s-2")
            quality.frame = NSRect(x: padding, y: y, width: controlWidth, height: fieldHeight)
            y += fieldHeight + tokens.number("s-5")
        }
        if !maximumSizeValue.isHidden {
            maximumSizeLabel.frame = NSRect(x: padding, y: y, width: content, height: 16)
            y += 16 + tokens.number("s-2")
            maximumSizeValue.frame = NSRect(x: padding, y: y, width: 96, height: fieldHeight)
            maximumSizeUnits.frame = NSRect(x: maximumSizeValue.frame.maxX + gap, y: y, width: 72,
                                            height: fieldHeight)
            y += fieldHeight + tokens.number("s-3")
            if !maximumSizeInvalid.isHidden {
                maximumSizeInvalid.frame = NSRect(x: padding, y: y, width: content, height: 16)
                y += 16 + tokens.number("s-2")
            }
            maximumSizeWarning.frame = NSRect(x: padding, y: y, width: content, height: 30)
            y += 30 + tokens.number("s-3")
        }
        estimateTitle.frame = NSRect(x: padding, y: y, width: content, height: 16)
        y += 16 + tokens.number("s-2")
        let estimateWidth = min(max(56, estimateLabel.intrinsicContentSize.width + 4),
                                max(56, content - 190))
        estimateLabel.frame = NSRect(x: padding, y: y + 6, width: estimateWidth, height: 18)
        var x = estimateLabel.frame.maxX + gap
        if !estimateDelta.isHidden {
            let deltaWidth = estimateDelta.intrinsicContentSize.width + tokens.number("s-3") * 2
            estimateDelta.frame = NSRect(x: x, y: y + 6, width: deltaWidth, height: 18)
            x = estimateDelta.frame.maxX + gap
        }
        estimateButton.frame = NSRect(x: x, y: y, width: 112, height: fieldHeight)
        qualityPanel.frame.size = NSSize(width: width, height: y + fieldHeight + padding)
    }

    private func layoutAudioCard(width: CGFloat) {
        let padding = tokens.number("s-6"), gap = tokens.number("s-4")
        let content = max(0, width - padding * 2)
        audioTitle.frame = NSRect(x: padding, y: padding, width: content, height: 20)
        var y = audioTitle.frame.maxY + tokens.number("s-5")
        if !gifAudioNote.isHidden {
            gifAudioNote.frame = NSRect(x: padding, y: y, width: content, height: 18)
            audioPanel.frame.size = NSSize(width: width, height: gifAudioNote.frame.maxY + padding)
            return
        }
        for (toggle, slider, field) in [(systemAudio, systemVolumeSlider, systemVolume),
                                        (microphoneAudio, microphoneVolumeSlider, microphoneVolume)]
            where !toggle.isHidden {
            toggle.frame = NSRect(x: padding, y: y + 4, width: 130, height: 20)
            field.frame = NSRect(x: width - padding - 72, y: y, width: 72, height: tokens.number("h-sm"))
            slider.frame = NSRect(x: toggle.frame.maxX + gap, y: y + 4,
                                  width: max(0, field.frame.minX - gap - toggle.frame.maxX - gap),
                                  height: 20)
            y += tokens.number("h-sm") + tokens.number("s-4")
        }
        monoOutput.frame = NSRect(x: padding, y: y, width: 180, height: 20)
        audioPanel.frame.size = NSSize(width: width, height: monoOutput.frame.maxY + padding)
    }

    /// Lays out the fixed save footer and returns its height.
    private func layoutFooter(width: CGFloat) -> CGFloat {
        let marginX = tokens.number("s-7"), marginY = tokens.number("s-6")
        let gap = tokens.number("s-4"), buttonGap = tokens.number("s-3")
        let available = max(0, width - marginX * 2)
        let fieldHeight = tokens.number("h-md")
        // Right to left, as shipping: Save, then the native actions and Cancel.
        let actions: [(CaptureButton, CGFloat)] = [(saveButton, 132), (applyButton, 104),
                                                   (replaceButton, 146), (showInFolderButton, 146),
                                                   (cancelButton, 156)]
        let visible = actions.filter { !$0.0.isHidden }
        let actionsWidth = visible.reduce(CGFloat(0)) { $0 + $1.1 }
            + buttonGap * CGFloat(max(0, visible.count - 1))
        // Filename and actions share one row when both fit, else they stack.
        let beside = available - actionsWidth - tokens.number("s-6")
        let wide = beside >= 280
        let filenameWidth = wide ? min(420, beside) : available
        // Filename heading: label, then "Saving to <folder> Change…" on the right.
        var y = marginY
        filenameLabel.frame = NSRect(x: marginX, y: y, width: 64, height: 16)
        changeButton.frame = NSRect(x: marginX + filenameWidth - 72, y: y - 2, width: 72, height: 20)
        let folderLeft = filenameLabel.frame.maxX + gap + 56
        destination.frame = NSRect(x: folderLeft, y: y + 1,
                                   width: max(0, changeButton.frame.minX - gap - folderLeft),
                                   height: 14)
        let folderWidth = min(destination.frame.width, destination.intrinsicContentSize.width)
        destination.frame = NSRect(x: destination.frame.maxX - folderWidth, y: y + 1,
                                   width: folderWidth, height: 14)
        savingToLabel.frame = NSRect(x: destination.frame.minX - 52, y: y + 1, width: 48, height: 14)
        savingToLabel.alignment = .right
        y += 16 + tokens.number("s-2")
        // `.recording-filename-input`: the stem with its format attached.
        let formatWidth: CGFloat = 84
        filenameField.frame = NSRect(x: marginX, y: y, width: max(0, filenameWidth - formatWidth),
                                     height: fieldHeight)
        format.frame = NSRect(x: filenameField.frame.maxX, y: y + 2, width: formatWidth,
                              height: fieldHeight - 4)
        var actionsTop = marginY
        if !wide { actionsTop = filenameField.frame.maxY + tokens.number("s-5") }
        let actionsLeft = wide ? marginX + filenameWidth + tokens.number("s-6") : marginX
        let actionsRight = width - marginX
        status.frame = NSRect(x: actionsLeft, y: actionsTop, width: max(0, actionsRight - actionsLeft),
                              height: 16)
        let buttonsY = actionsTop + 16 + tokens.number("s-2")
        var right = actionsRight
        for (control, controlWidth) in actions {
            control.frame = NSRect(x: right - controlWidth, y: buttonsY, width: controlWidth,
                                   height: fieldHeight)
            if !control.isHidden { right = control.frame.minX - buttonGap }
        }
        let height = buttonsY + fieldHeight + marginY
        progress.frame = NSRect(x: 0, y: 0, width: width, height: 3)
        footerDivider.frame = NSRect(x: 0, y: 0, width: width, height: 1)
        return height
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
        layoutOverlayPlay(imageRect: lastPreviewImageRect)
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
        comparisonView.frame = imageRect
        cropOverlay.frame = previewCanvas.bounds
        cropOverlay.presentedImageRect = actualSize ? imageRect : nil
        previewScroll.hasHorizontalScroller = actualSize && canvasSize.width > viewport.width
        previewScroll.hasVerticalScroller = actualSize && canvasSize.height > viewport.height
        if resetScroll || !actualSize {
            previewScroll.contentView.scroll(to: .zero)
            previewScroll.reflectScrolledClipView(previewScroll.contentView)
        }
        lastPreviewImageRect = imageRect
        layoutOverlayPlay(imageRect: imageRect)
    }

    private func publish(_ value: RecordingEditorPresentation, initialize: Bool = false) {
        invalidateComparison()
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
        if let mimeType = value.snapshot.source["mime_type"] as? String {
            titleLabel.stringValue = RecordingEditorCopy.title(mimeType: mimeType)
        }
        let droppedWarning = RecordingEditorCopy.droppedFramesWarning(value.snapshot.droppedFrames)
        droppedFramesLabel.stringValue = droppedWarning ?? ""
        droppedFramesBand.isHidden = droppedWarning == nil
        seekSlider.maxValue = Double(max(1, value.snapshot.durationMilliseconds))
        seekSlider.doubleValue = Double(value.snapshot.positionMilliseconds)
        seekLabel.stringValue = time(value.snapshot.positionMilliseconds)
        trimTimeline.setAcceptedPosition(value.snapshot.positionMilliseconds)
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
        syncVolumeSlider(systemVolumeSlider, from: systemVolume)
        syncVolumeSlider(microphoneVolumeSlider, from: microphoneVolume)
        systemAudio.state = (audio["mute_system_audio"] as? Bool ?? false) ? .off : .on
        microphoneAudio.state = (audio["mute_microphone"] as? Bool ?? false) ? .off : .on
        monoOutput.state = (audio["mono_output"] as? Bool ?? false) ? .on : .off
        let acceptedExport = value.snapshot.saveExport
        if let source = sourceDimensions(value.snapshot) {
            stagedCrop = NativeRecordingCropRect(value: value.snapshot.edit["crop"],
                                                  sourceWidth: source.width,
                                                  sourceHeight: source.height)
            cropEnabled.state = stagedCrop == nil ? .off : .on
            cropLock.state = cropAspectUnlocked ? .off : .on
            if initialize {
                if let output = editOutputDimensions(value.snapshot.edit) {
                    customOutput = true
                    outputMode.selectItem(at: NativeRecordingResolutionPreset.allCases.count)
                    outputWidth.stringValue = String(output.width)
                    outputHeight.stringValue = String(output.height)
                } else {
                    customOutput = false; resolutionPreset = .original
                    outputMode.selectItem(at: Int(resolutionPreset.rawValue))
                }
            } else if customOutput, acceptedExport["format"] as? String != "gif",
                      let output = editOutputDimensions(value.snapshot.edit) {
                outputWidth.stringValue = String(output.width)
                outputHeight.stringValue = String(output.height)
            }
            refreshGeometryFields(source: source, preserveCustom: customOutput)
        }
        format.selectItem(at: acceptedExport["format"] as? String == "gif" ? 1 : 0)
        let acceptedMaximum = (acceptedExport["max_size_bytes"] as? NSNumber)?.uint64Value
        maximumSizeEnabled = acceptedMaximum != nil
        if let acceptedMaximum {
            if maximumSizeUnit.bytes(maximumSizeValue.stringValue) != acceptedMaximum {
                maximumSizeValue.stringValue = maximumSizeUnit.value(acceptedMaximum)
            }
            if initialize { qualityPreference = "preserve"; preserveQuality = false }
        } else {
            let acceptedQuality = acceptedExport["quality"] as? String ?? "preserve"
            preserveQuality = acceptedQuality == "preserve"
            if !preserveQuality {
                compressQuality = acceptedQuality
                selectQualityPreset(acceptedQuality)
            }
            qualityPreference = acceptedQuality
            if initialize {
                maximumSizeUnit = .megabytes
                maximumSizeUnits.selectItem(withTitle: maximumSizeUnit.label)
                maximumSizeValue.stringValue = "10"
            }
        }
        if format.indexOfSelectedItem == 1 {
            let accepted = (acceptedExport["frames_per_second"] as? NSNumber)?.uint16Value
            let supported: [UInt16] = [8, 10, 12, 15, 20, 24, 30]
            gifFramesPerSecond = accepted.flatMap { supported.contains($0) ? $0 : nil } ?? 15
            gifFrameRate.selectItem(withTitle: "\(gifFramesPerSecond) FPS")
            if initialize && editOutputDimensions(value.snapshot.edit) == nil {
                if gifMaximumWidthControl.item(withTitle: "Original") == nil {
                    gifMaximumWidthControl.insertItem(withTitle: "Original", at: 0)
                }
                gifMaximumWidthControl.selectItem(withTitle: "Original")
            }
        }
        if initialize {
            savedEdit = canonicalEdit(value.snapshot.edit); savedExport = canonical(acceptedExport)
        } else if canonicalEdit(old?.edit) != canonicalEdit(value.snapshot.edit)
                    || canonical(old?.saveExport) != canonical(acceptedExport) {
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
              let baseOutputSize = stagedOutputDimensions(source: sourceSize),
              start < end, end <= duration else { return nil }
        let gif = format.indexOfSelectedItem == 1
        let normalizedGifBase: NativeRecordingDimensions
        if gif {
            normalizedGifBase = NativeRecordingGeometry.constrain(
                baseOutputSize, preset: .original) ?? baseOutputSize
        } else {
            normalizedGifBase = baseOutputSize
        }
        let originalGifWidth = gifMaximumWidthControl.titleOfSelectedItem == "Original"
        let outputSize = gif
            ? (originalGifWidth ? normalizedGifBase
                : dimensionsAtMaximumWidth(normalizedGifBase, maximumWidth: gifMaximumWidth))
            : baseOutputSize
        var edit = accepted
        edit["trim_start_ms"] = start
        edit["trim_end_ms"] = end == duration ? NSNull() : end
        edit["crop"] = stagedCrop == nil ? NSNull() : stagedCrop!.dictionary
        if (gif && !originalGifWidth)
            || customOutput || resolutionPreset != .original {
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
            audio["mute_system_audio"] = systemAudio.state != .on
        }
        if presentation?.snapshot.hasMicrophoneAudio == true {
            guard let volume = volume(microphoneVolume) else { return nil }
            audio["microphone_volume"] = volume
            audio["mute_microphone"] = microphoneAudio.state != .on
        }
        audio["mono_output"] = monoOutput.state == .on
        audio["source_has_system_audio"] = presentation?.snapshot.hasSystemAudio == true
        audio["source_has_microphone_audio"] = presentation?.snapshot.hasMicrophoneAudio == true
        edit["audio"] = audio
        return edit
    }

    private var stagedExport: [String: Any]? {
        guard presentation != nil else { return nil }
        var value = presentation!.snapshot.saveExport
        let gif = format.indexOfSelectedItem == 1
        value["format"] = gif ? "gif" : "mp4"
        value["quality"] = maximumSizeEnabled || preserveQuality ? "preserve" : selectedQualityPreset
        value["max_size_bytes"] = maximumSizeEnabled
            ? NSNumber(value: maximumSizeBytes ?? 0) : NSNull()
        let accepted = presentation?.snapshot.saveExport
        let acceptedGif = accepted?["format"] as? String == "gif"
        value["frames_per_second"] = gif && (!acceptedGif
            || accepted?["frames_per_second"] is NSNumber
            || gifFramesPerSecond != 15) ? NSNumber(value: gifFramesPerSecond) : NSNull()
        value["gif_max_colors"] = gif && (!acceptedGif
            || accepted?["gif_max_colors"] is NSNumber
            || gifMaxColors != 256) ? NSNumber(value: gifMaxColors) : NSNull()
        return value
    }

    private var gifMaxColors: Int {
        switch qualityPreference {
        case "tiny": 64
        case "small": 96
        case "standard": 128
        default: 256
        }
    }

    private var stagedDiffers: Bool {
        guard let snapshot = presentation?.snapshot else { return false }
        return hasPendingCropInput
            || canonicalEdit(stagedEdit) != canonicalEdit(snapshot.edit)
            || canonical(stagedExport) != canonical(snapshot.saveExport)
    }

    private func applyEdits() {
        invalidateComparison()
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
        seek(to: UInt64(max(0, seekSlider.doubleValue).rounded()))
    }

    /// Decodes the accepted preview at `position`, from the slider or a
    /// timeline click (shipping scrubs its video from the track).
    private func seek(to position: UInt64) {
        invalidateComparison()
        guard !busy, !stagedDiffers else {
            seekSlider.doubleValue = Double(presentation?.snapshot.positionMilliseconds ?? 0)
            if stagedDiffers { showError("Apply staged recording changes before seeking.") }
            return
        }
        let finishCropOnSuccess = cropAdjustmentActive
        if !finishCropOnSuccess { restoreAcceptedPresentation() }
        request(["operation": "seek", "position_ms": position],
                activity: "Decoding source-relative frame…",
                finishCropOnSuccess: finishCropOnSuccess)
    }

    private func request(_ object: [String: Any], activity: String,
                         finishCropOnSuccess: Bool = false) {
        guard !busy else { return }
        let current = generation; busy = true
        status.textColor = tokens.color("text-muted"); status.stringValue = activity
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
        invalidateComparison()
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
        seekSlider.doubleValue = Double(min(milliseconds, duration))
        seekLabel.stringValue = time(milliseconds)
        trimTimeline.setPlaybackPosition(milliseconds)
        refreshCaption()
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
        seekSlider.doubleValue = Double(presentation.snapshot.positionMilliseconds)
        seekLabel.stringValue = time(presentation.snapshot.positionMilliseconds)
        refreshCaption()
    }

    private func estimateSize() {
        guard !busy, !stagedDiffers, presentation != nil,
              (presentation?.snapshot.saveExport["max_size_bytes"] as? NSNumber) == nil,
              let cancel = NativeRecordingEditorCancel() else { return }
        let current = generation; busy = true; activeCancel = cancel; estimate = nil
        estimating = true
        status.textColor = tokens.color("text-muted")
        status.stringValue = "Estimating accepted recording settings…"; updateControls()
        worker.estimate(cancel: cancel) { [weak self] result in
            guard let self, self.generation == current else { return }
            self.busy = false; self.activeCancel = nil; self.estimating = false
            switch result {
            case .success(let value): self.estimate = value; self.status.stringValue = "Estimate ready."
            case .failure(let error): self.showError("Size estimate failed: \(error.localizedDescription)")
            }
            self.updateControls()
        }
    }

    @objc private func comparisonSplitChanged() {
        comparisonView.split = CGFloat(comparisonSlider.doubleValue / 100)
    }

    private func invalidateComparison() {
        comparisonCancel?.cancel()
        if status.stringValue == comparisonStatusMessage {
            status.textColor = tokens.color("text-muted")
            status.stringValue = "Accepted recording preview."
        }
        comparisonStatusMessage = nil
        let wasShowing = comparison != nil
        comparison = nil
        comparisonView.comparison = nil
        comparisonView.isHidden = true
        comparisonSlider.isHidden = true
        comparisonBeforeLabel.isHidden = true; comparisonAfterLabel.isHidden = true
        comparisonHideButton?.isHidden = true
        // Only a shown comparison changes the card height; skipping relayout
        // otherwise keeps in-progress trim and crop drags intact.
        if wasShowing { layoutComparisonViewport() }
    }

    private func layoutComparisonViewport() {
        guard root.bounds.width > 0 else { return }
        layout()
    }

    private func compareAcceptedFrame() {
        guard !busy, !pickerOpen, playbackState == .idle, !cropAdjustmentActive,
              !stagedDiffers, pendingCropInputValid, stagedEdit != nil, stagedExport != nil,
              let snapshot = presentation?.snapshot,
              let cancel = NativeRecordingEditorCancel() else { return }
        let current = generation
        let expectedRevision = snapshot.revision
        let expectedPosition = snapshot.positionMilliseconds
        let expectedExport = canonical(snapshot.export)
        invalidateComparison()
        restoreAcceptedPresentation()
        busy = true; activeCancel = cancel; comparisonCancel = cancel
        status.textColor = tokens.color("text-muted")
        status.stringValue = "Encoding before/after at accepted frame \(time(expectedPosition))…"
        comparisonStatusMessage = status.stringValue
        updateControls()
        worker.comparison(cancel: cancel) { [weak self] result in
            guard let self, self.generation == current,
                  self.comparisonCancel === cancel else { return }
            self.busy = false; self.activeCancel = nil; self.comparisonCancel = nil
            guard !cancel.isCancelled else {
                self.status.stringValue = "Comparison cancelled. Compare to retry."
                self.comparisonStatusMessage = self.status.stringValue
                self.updateControls(); return
            }
            guard let accepted = self.presentation?.snapshot,
                  accepted.revision == expectedRevision,
                  accepted.positionMilliseconds == expectedPosition,
                  self.canonical(accepted.export) == expectedExport,
                  !self.stagedDiffers, !self.cropAdjustmentActive,
                  self.playbackState == .idle else {
                self.status.stringValue = "Comparison no longer matches the accepted frame."
                self.comparisonStatusMessage = self.status.stringValue
                self.updateControls(); return
            }
            switch result {
            case .success(let value):
                guard value.revision == expectedRevision,
                      value.positionMilliseconds == expectedPosition,
                      self.canonical(value.export) == expectedExport else {
                    self.showError("Encoded comparison no longer matches the accepted frame.")
                    break
                }
                self.comparison = value
                self.comparisonView.comparison = value
                self.comparisonView.isHidden = false
                self.comparisonBeforeLabel.isHidden = false
                self.comparisonAfterLabel.isHidden = false
                self.layoutComparisonViewport()
                self.status.stringValue = accepted.saveExport["max_size_bytes"] is NSNumber
                    ? "Encoded first attempt at accepted \(self.time(expectedPosition)); final capped save may differ."
                    : "Encoded before/after at accepted \(self.time(expectedPosition)); playback time is independent."
                self.comparisonStatusMessage = self.status.stringValue
            case .failure(let error):
                self.showError("Comparison unavailable: \(error.localizedDescription). Compare to retry.")
                self.comparisonStatusMessage = self.status.stringValue
            }
            self.updateControls()
        }
    }

    private func generateThumbnails(completion: (() -> Void)? = nil) {
        guard !busy, !pickerOpen, presentation != nil,
              let cancel = NativeRecordingEditorCancel() else { completion?(); return }
        let current = generation
        busy = true; activeCancel = cancel; thumbnailCancel = cancel
        thumbnailRetryAvailable = false
        trimTimeline.showThumbnailLoading()
        status.textColor = tokens.color("text-muted")
        status.stringValue = "Generating immutable source timeline thumbnails…"
        updateControls(); layout()
        worker.thumbnails(cancel: cancel) { [weak self] result in
            guard let self, self.generation == current,
                  self.thumbnailCancel === cancel else { completion?(); return }
            self.busy = false; self.activeCancel = nil; self.thumbnailCancel = nil
            switch result {
            case .success(let image):
                self.trimTimeline.showThumbnails(image)
                self.thumbnailRetryAvailable = false
                self.status.textColor = self.tokens.color("text-muted")
                self.status.stringValue = "Source thumbnails ready."
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
            completion?()
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
        guard !busy, !stagedDiffers, let export = presentation?.snapshot.saveExport else { return }
        if let error = RecordingEditorCopy.filenameError(filenameField.stringValue) {
            showError(error); return
        }
        guard !destinationDirectory.isEmpty,
              let cancel = NativeRecordingEditorCancel() else { return }
        let gif = export["format"] as? String == "gif"
        let current = generation; busy = true; activeCancel = cancel; lastSavedPath = nil
        progress.doubleValue = 0; progress.isHidden = false
        status.textColor = tokens.color("text-subtle")
        status.stringValue = RecordingEditorCopy.stage("preparing") ?? "Preparing…"; updateControls()
        worker.save(destination: destinationPath, export: export, cancel: cancel,
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
                        self.savedExport = self.canonical(snapshot.saveExport)
                    }
                    switch saved {
                    case .saved(let path):
                        let size = (try? FileManager.default.attributesOfItem(atPath: path))?[.size]
                            as? NSNumber
                        self.status.textColor = self.tokens.color("positive-text")
                        self.status.stringValue = size.flatMap {
                            RecordingEditorCopy.saved(gif: gif, sizeBytes: $0.uint64Value)
                        } ?? "Saved new copy: \(path)"
                        self.status.toolTip = path
                        self.lastSavedPath = path
                        self.didSaveCopy()
                    case .savedWithoutHistory(let path, let warning):
                        self.lastSavedPath = path
                        self.showError("Saved new copy: \(path). History could not be updated: \(warning)")
                    }
                case .failure(let error): self.showError("Save failed: \(error.localizedDescription)")
                }
                self.updateControls()
            })
    }

    private static func presentReplaceOriginalConfirmation(
        window: NSWindow, path: String, completion: @escaping (Bool) -> Void
    ) {
        let alert = NSAlert()
        alert.messageText = "Replace the original recording?"
        alert.informativeText = "This replaces the saved file at:\n\(path)\n\nThe existing History item will be updated. This cannot be undone."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Replace")
        alert.addButton(withTitle: "Cancel")
        alert.beginSheetModal(for: window) { completion($0 == .alertFirstButtonReturn) }
    }

    private func confirmReplace() {
        guard !busy, !pickerOpen, !awaitingReplaceConfirmation, !requiresReopen,
              playbackState == .idle, !stagedDiffers, !cropAdjustmentActive,
              let snapshot = presentation?.snapshot, let path = eligibleOriginalPath else { return }
        let current = generation, revision = snapshot.revision
        let accepted = canonical(snapshot.saveExport)
        awaitingReplaceConfirmation = true; updateControls()
        confirmReplaceOriginal(window, path) { [weak self] confirmed in
            guard let self else { return }
            self.awaitingReplaceConfirmation = false
            guard confirmed else { self.updateControls(); return }
            guard self.generation == current, self.artifactID == snapshot.artifactID,
                  !self.busy, !self.pickerOpen, !self.requiresReopen,
                  self.playbackState == .idle, !self.stagedDiffers,
                  self.eligibleOriginalPath == path,
                  self.presentation?.snapshot.revision == revision,
                  self.canonical(self.presentation?.snapshot.saveExport) == accepted,
                  let cancel = NativeRecordingEditorCancel() else {
                self.showError("Recording changed before replacement was confirmed. Try again.")
                self.updateControls(); return
            }
            self.busy = true; self.activeCancel = cancel; self.replacing = true
            self.progress.doubleValue = 0; self.progress.isHidden = false
            self.status.textColor = self.tokens.color("text-muted")
            self.status.stringValue = "Preparing replacement…"
            self.updateControls()
            self.worker.replaceOriginal(cancel: cancel, progress: { [weak self] value in
                guard let self, self.generation == current,
                      self.activeCancel === cancel else { return }
                self.progress.doubleValue = Double(value.completedPerMille)
                self.status.stringValue = value.message
            }, completion: { [weak self] result in
                guard let self, self.generation == current,
                      self.activeCancel === cancel else { return }
                self.busy = false; self.activeCancel = nil; self.progress.isHidden = true
                self.replacing = false
                switch result {
                case .success(let replaced):
                    guard replaced.path == path,
                          replaced.presentation.snapshot.artifactID == snapshot.artifactID,
                          replaced.presentation.snapshot.revision > revision else {
                        self.markRequiresReopen("Replacement result did not match the original. Close and reopen this editor.")
                        return
                    }
                    self.invalidateComparison()
                    self.sourceFrameCache = nil; self.sourceFrameCancel = nil
                    self.cropAdjustmentPriorImage = nil; self.cropAdjustmentActive = false
                    self.cropOverlay.isHidden = true; self.cropOverlay.setEditingEnabled(false)
                    self.trimTimeline.clearThumbnails(); self.thumbnailRetryAvailable = false
                    self.estimate = nil
                    self.qualityPreference = "preserve"; self.preserveQuality = true
                    self.compressQuality = "highest"
                    self.gifFramesPerSecond = 15; self.gifMaximumWidth = 800
                    self.maximumSizeEnabled = false
                    self.resolutionPreset = .original; self.customOutput = false
                    self.stagedCrop = nil; self.cropAspectUnlocked = false
                    self.savedEdit = nil; self.savedExport = nil
                    self.publish(replaced.presentation, initialize: true)
                    self.status.stringValue = "Replaced original: \(path)"
                    self.didReplaceOriginal(snapshot.artifactID)
                    self.generateThumbnails()
                case .failure(let error):
                    if (error as? RecordingReplaceError)?.requiresReopen == true {
                        self.markRequiresReopen("Replacement state is uncertain: \(error.localizedDescription). Close and reopen this editor.")
                        return
                    }
                    self.showError("Couldn’t replace original: \(error.localizedDescription)")
                }
                self.updateControls()
            })
        }
    }

    private var eligibleOriginalPath: String? {
        guard let path = originalPath, !path.isEmpty,
              let format = presentation?.snapshot.saveExport["format"] as? String,
              ["mp4", "gif"].contains(format),
              URL(fileURLWithPath: path).pathExtension.lowercased() == format else { return nil }
        return path
    }

    private func markRequiresReopen(_ message: String) {
        requiresReopen = true
        invalidateComparison(); sourceFrameCache = nil; sourceFrameCancel = nil
        cropAdjustmentPriorImage = nil; trimTimeline.clearThumbnails()
        presentation = nil; setPreviewImage(nil)
        showError(message); updateControls()
    }

    private func chooseDestination() {
        guard !busy else { return }
        pickerOpen = true; updateControls()
        // Shipping's "Change…" picks the folder; the filename field names the file.
        let panel = NSOpenPanel(); panel.title = "Choose save location"
        panel.canChooseDirectories = true; panel.canChooseFiles = false
        panel.canCreateDirectories = true; panel.allowsMultipleSelection = false
        panel.prompt = "Choose"
        panel.directoryURL = URL(fileURLWithPath: destinationDirectory, isDirectory: true)
        panel.beginSheetModal(for: window) { [weak self] response in
            guard let self else { return }
            self.pickerOpen = false
            if response == .OK, let url = panel.url { self.setDestinationDirectory(url.path) }
            self.updateControls(); self.layout()
        }
    }

    private func revealSavedCopy() {
        guard let lastSavedPath else { return }
        revealFiles([URL(fileURLWithPath: lastSavedPath)])
    }

    private func setDestinationDirectory(_ path: String) {
        destinationDirectory = path
        destination.stringValue = path
        destination.toolTip = path
    }

    /// `<folder>/<filename>.<mp4|gif>`, as the save footer shows it.
    private var destinationPath: String {
        let ext = format.indexOfSelectedItem == 1 ? "gif" : "mp4"
        return URL(fileURLWithPath: destinationDirectory, isDirectory: true)
            .appendingPathComponent("\(filenameField.stringValue).\(ext)").path
    }

    /// Matches the wgpu host's default `Captures_<local time>_edited` stem.
    private static func defaultFilenameStem(now: Date = Date()) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd_HH-mm-ss"
        return "Captures_\(formatter.string(from: now))_edited"
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
        invalidateComparison()
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
        refreshCaption()
    }

    private func stageGraphicalCrop(_ crop: NativeRecordingCropRect) {
        invalidateComparison()
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
        // Shipping offers Preserve quality only for MP4 and moves a GIF to
        // Compress at the remembered preset; Maximum keeps its limit.
        if format.indexOfSelectedItem == 1, !maximumSizeEnabled, preserveQuality {
            preserveQuality = false
            selectQualityPreset(compressQuality)
            qualityPreference = compressQuality
        }
        estimate = nil; updateControls(); layout()
    }
    @objc private func gifFrameRateChanged() {
        let title = gifFrameRate.titleOfSelectedItem ?? "15 FPS"
        gifFramesPerSecond = UInt16(title.split(separator: " ").first.map(String.init) ?? "15") ?? 15
        estimate = nil; updateControls()
    }
    @objc private func gifMaximumWidthChanged() {
        let title = gifMaximumWidthControl.titleOfSelectedItem ?? "800 px"
        gifMaximumWidth = UInt32(title.split(separator: " ").first.map(String.init) ?? "800") ?? 800
        estimate = nil; updateControls()
    }
    @objc private func qualityModeChanged() {
        switch qualityMode.selectedItem?.representedObject as? String {
        case "preserve":
            maximumSizeEnabled = false; preserveQuality = true; qualityPreference = "preserve"
        case "compress":
            maximumSizeEnabled = false; preserveQuality = false
            selectQualityPreset(compressQuality); qualityPreference = compressQuality
        case "maximum":
            // Maximum saves at Preserve quality and keeps the palette preset.
            maximumSizeEnabled = true
        default: break
        }
        invalidateComparison()
        estimate = nil; updateControls(); layout()
    }
    @objc private func maximumSizeUnitChanged() {
        guard let title = maximumSizeUnits.titleOfSelectedItem,
              let unit = RecordingFileSizeUnit.allCases.first(where: { $0.label == title }) else {
            maximumSizeUnits.selectItem(withTitle: maximumSizeUnit.label); return
        }
        if let bytes = maximumSizeUnit.bytes(maximumSizeValue.stringValue) {
            maximumSizeValue.stringValue = unit.value(bytes)
        }
        maximumSizeUnit = unit
        estimate = nil; updateControls(); layout()
    }
    @objc private func qualityChanged() {
        compressQuality = selectedQualityPreset
        qualityPreference = compressQuality
        estimate = nil; updateControls()
    }
    @objc private func stageChanged() { invalidateComparison(); estimate = nil; updateControls() }

    @objc private func volumeSliderChanged(_ sender: NSSlider) {
        let field = sender === systemVolumeSlider ? systemVolume : microphoneVolume
        field.stringValue = "\(Int(sender.doubleValue.rounded()))%"
        stageChanged()
    }

    private func syncVolumeSlider(_ slider: NSSlider, from field: NSTextField) {
        if let gain = volume(field) { slider.doubleValue = Double(gain) * 100 }
    }

    private func resetTrim() {
        guard let duration = presentation?.snapshot.durationMilliseconds, duration > 0 else { return }
        trimStart.stringValue = "0"; trimEnd.stringValue = String(duration)
        invalidateComparison()
        estimate = nil; syncTimelineFromFields(); updateControls()
    }

    /// The selected Compress preset's shared value (`highest` … `tiny`).
    private var selectedQualityPreset: String {
        quality.selectedItem?.representedObject as? String ?? compressQuality
    }

    private func selectQualityPreset(_ value: String) {
        if let index = quality.itemArray.firstIndex(where: { $0.representedObject as? String == value }) {
            quality.selectItem(at: index)
        }
    }

    private var selectedQualityModeValue: String {
        maximumSizeEnabled ? "maximum" : preserveQuality ? "preserve" : "compress"
    }

    private var maximumSizeBytes: UInt64? {
        maximumSizeUnit.bytes(maximumSizeValue.stringValue).flatMap { $0 >= 100_000 ? $0 : nil }
    }

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

    private func dimensionsAtMaximumWidth(_ input: NativeRecordingDimensions,
                                          maximumWidth: UInt32) -> NativeRecordingDimensions {
        guard input.width > maximumWidth else { return input }
        let scale = Double(maximumWidth) / Double(input.width)
        let scaledHeight = max(2, Int((Double(input.height) * scale).rounded()))
        let evenHeight = scaledHeight.isMultiple(of: 2) ? scaledHeight : scaledHeight - 1
        return NativeRecordingDimensions(width: maximumWidth,
                                         height: UInt32(max(2, evenHeight)))
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
        // "Original — W × H" names the crop/output base, as shipping does.
        rebuildOutputModeMenu(base: NativeRecordingGeometry.constrain(
            cropInputDimensions(source: source), preset: .original) ?? cropInputDimensions(source: source))
        outputMode.selectItem(at: customOutput ? NativeRecordingResolutionPreset.allCases.count
            : Int(resolutionPreset.rawValue))
    }

    private func syncTimelineFromFields() {
        guard let duration = presentation?.snapshot.durationMilliseconds,
              let start = UInt64(trimStart.stringValue), let end = UInt64(trimEnd.stringValue),
              start < end, end <= duration else { return }
        trimTimeline.setValues(start: start, end: end, duration: duration)
    }

    private func updateControls() {
        if stagedDiffers || cropAdjustmentActive || playbackState != .idle {
            invalidateComparison()
        }
        let available = presentation != nil && !busy && !pickerOpen
            && !awaitingReplaceConfirmation && !requiresReopen && playbackState == .idle
        let validMaximum = !maximumSizeEnabled || maximumSizeBytes != nil
        let valid = pendingCropInputValid && stagedEdit != nil && stagedExport != nil
            && validMaximum
        let gif = format.indexOfSelectedItem == 1
        [trimStart, trimEnd, format, filenameField].forEach { $0.isEnabled = available }
        resetTrimButton?.isEnabled = available
        // Save quality: mode, Compress preset or Maximum limit, then Est. size.
        let mode = selectedQualityModeValue
        if let index = qualityMode.itemArray.firstIndex(where: {
            $0.representedObject as? String == mode }) {
            qualityMode.selectItem(at: index)
        }
        // Shipping offers Preserve quality only for MP4; an accepted Preserve
        // GIF keeps showing its mode until changed.
        if let preserveItem = qualityMode.itemArray.first(where: {
            $0.representedObject as? String == "preserve" }) {
            preserveItem.isHidden = gif && mode != "preserve"
        }
        qualityMode.isEnabled = available
        qualityModeHelp.stringValue = qualityMode.selectedItem?.toolTip ?? ""
        quality.isHidden = mode != "compress"; qualityLabel.isHidden = quality.isHidden
        quality.isEnabled = available && mode == "compress"
        for view in [maximumSizeLabel, maximumSizeWarning] as [NSView] {
            view.isHidden = !maximumSizeEnabled
        }
        maximumSizeValue.isHidden = !maximumSizeEnabled
        maximumSizeUnits.isHidden = !maximumSizeEnabled
        maximumSizeInvalid.isHidden = !maximumSizeEnabled || validMaximum
        maximumSizeValue.isEnabled = available && maximumSizeEnabled
        maximumSizeUnits.isEnabled = available && maximumSizeEnabled
        gifPanel.isHidden = !gif
        gifFrameRate.isEnabled = available && gif
        gifMaximumWidthControl.isEnabled = available && gif
        cropEnabled.isEnabled = available
        cropLock.isEnabled = available && stagedCrop != nil
        [cropX, cropY, cropWidth, cropHeight].forEach {
            $0.isEnabled = available && stagedCrop != nil
        }
        outputMode.isEnabled = available
        for view in [outputWidth, outputHeight, outputWidthLabel, outputHeightLabel] {
            view.isHidden = !customOutput
        }
        outputWidth.isEnabled = available && customOutput
        outputHeight.isEnabled = available && customOutput
        let hasSystem = presentation?.snapshot.hasSystemAudio == true
        let hasMicrophone = presentation?.snapshot.hasMicrophoneAudio == true
        let hasAudio = hasSystem || hasMicrophone
        audioPanel.isHidden = !hasAudio
        gifAudioNote.isHidden = !gif
        audioPanel.layer?.backgroundColor = tokens.color(gif ? "caution-surface" : "surface-raised").cgColor
        audioPanel.layer?.borderColor = tokens.color(gif ? "caution-surface" : "border-subtle").cgColor
        for view in [systemAudio, systemVolume, systemVolumeSlider] as [NSView] {
            view.isHidden = !hasSystem || gif
        }
        for view in [microphoneAudio, microphoneVolume, microphoneVolumeSlider] as [NSView] {
            view.isHidden = !hasMicrophone || gif
        }
        monoOutput.isHidden = !hasAudio || gif
        // The toolbar note beside "Preview" names the playback mode.
        if cropAdjustmentActive {
            audioNote.stringValue = "Source crop"
        } else if comparison != nil {
            audioNote.stringValue = "Encoded comparison"
        } else if !playbackSoundEnabled {
            audioNote.stringValue = "Silent playback"
        } else if playbackAudioEnabled == true {
            audioNote.stringValue = playbackState == .playing ? "Sound active" : "Audio used"
        } else if gif {
            audioNote.stringValue = "GIF · no audio"
        } else if !hasAudio {
            audioNote.stringValue = "No audio tracks"
        } else if playbackAudioEnabled == false {
            audioNote.stringValue = "Mix silent"
        } else {
            audioNote.stringValue = "Uses accepted mix"
        }
        systemVolume.isEnabled = available && !gif && systemAudio.state == .on
        microphoneVolume.isEnabled = available && !gif && microphoneAudio.state == .on
        systemVolumeSlider.isEnabled = systemVolume.isEnabled
        microphoneVolumeSlider.isEnabled = microphoneVolume.isEnabled
        systemAudio.isEnabled = available && !gif
        microphoneAudio.isEnabled = available && !gif
        monoOutput.isEnabled = available && !gif
        trimTimeline.setEditingEnabled(available && stagedEdit != nil)
        trimTimeline.seekEnabled = available && valid && !stagedDiffers && !cropAdjustmentActive
        thumbnailRetryButton?.isHidden = !thumbnailRetryAvailable
        thumbnailRetryButton?.isEnabled = available && thumbnailRetryAvailable
        thumbnailStatusLabel.isHidden = !thumbnailRetryAvailable
        thumbnailStatusLabel.stringValue = "\(trimTimeline.thumbnailStateDescription)."
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
        comparisonButton?.isEnabled = available && valid && !stagedDiffers
            && !cropAdjustmentActive
        comparisonHideButton?.isHidden = comparison == nil
        comparisonSlider.isHidden = comparison == nil
        cropOverlay.interceptsPendingInput = cropAdjustmentActive && available
            && hasPendingCropInput
        cropOverlay.setEditingEnabled(cropAdjustmentActive && available && !hasPendingCropInput)
        playbackLoop.isEnabled = presentation != nil && !busy && !pickerOpen
            && !awaitingReplaceConfirmation && !requiresReopen
            && playbackState != .pausing
        playbackSound.isEnabled = available
            && playbackState == .idle
        switch playbackState {
        case .idle:
            playbackButton?.title = "Play"
            playbackButton?.icon = .shipping("resume")
            playbackButton?.setAccessibilityLabel(playbackSoundEnabled
                ? "Play recording preview with sound" : "Play silent recording preview")
            playbackButton?.toolTip = stagedDiffers ? "Apply staged edits before playing."
                : "Play the accepted trim and mix; Sound is off by default."
            playbackButton?.isEnabled = available && valid && !stagedDiffers
                && !cropAdjustmentActive
        case .playing:
            playbackButton?.title = "Pause"
            playbackButton?.icon = .shipping("pause")
            playbackButton?.setAccessibilityLabel("Pause recording preview")
            playbackButton?.toolTip = "Pause the preview."
            playbackButton?.isEnabled = true
        case .pausing:
            playbackButton?.title = "Pausing…"
            playbackButton?.icon = .shipping("pause")
            playbackButton?.setAccessibilityLabel("Pausing recording preview")
            playbackButton?.isEnabled = false
        }
        playbackButton?.isHidden = cropAdjustmentActive || preview.image == nil
        applyButton?.isEnabled = available && valid && stagedDiffers
        seekSlider.isEnabled = available && valid && !stagedDiffers
        changeButton?.isEnabled = available
        estimateButton?.isHidden = maximumSizeEnabled
        estimateButton?.isEnabled = available && valid && !stagedDiffers && !maximumSizeEnabled
        saveButton?.isEnabled = available && valid && !stagedDiffers
            && !filenameField.stringValue.isEmpty
        let canReplace = eligibleOriginalPath != nil
        replaceButton?.isEnabled = available && valid && !stagedDiffers
            && !cropAdjustmentActive && canReplace
        replaceButton?.isHidden = activeCancel != nil
        replaceButton?.toolTip = originalPath == nil ? "No saved original file is available."
            : canReplace ? "Confirm replacement of \(eligibleOriginalPath ?? "") and its History item."
            : "Choose the saved original’s MP4 or GIF format."
        cancelButton?.isHidden = activeCancel == nil
        cancelButton?.isEnabled = activeCancel != nil && activeCancel?.isCancelled != true
        showInFolderButton?.isHidden = lastSavedPath == nil || activeCancel != nil
        showInFolderButton?.toolTip = lastSavedPath
        // Shipping's footer Cancel, named for the operation it stops.
        let cancelTitle: String
        if activeCancel == nil {
            cancelTitle = "Cancel"
        } else if thumbnailCancel === activeCancel {
            cancelTitle = "Cancel thumbnails"
        } else if sourceFrameCancel === activeCancel {
            cancelTitle = "Cancel source preview"
        } else if comparisonCancel === activeCancel {
            cancelTitle = "Cancel comparison"
        } else if estimating {
            cancelTitle = "Cancel estimate"
        } else if replacing {
            cancelTitle = "Cancel replacement"
        } else {
            cancelTitle = "Cancel export"
        }
        if cancelButton?.title != cancelTitle {
            cancelButton?.title = cancelTitle
            cancelButton?.setAccessibilityLabel(cancelTitle)
        }
        // Timeline summary follows the staged trim, like shipping.
        if let duration = presentation?.snapshot.durationMilliseconds {
            let start = UInt64(trimStart.stringValue) ?? trimTimeline.startMilliseconds
            let end = UInt64(trimEnd.stringValue) ?? trimTimeline.endMilliseconds
            let summary = RecordingEditorCopy.trimSummary(start: min(start, end), end: max(start, end),
                                                          duration: duration)
            trimRangeLabel.stringValue = summary.range
            trimSelectedLabel.stringValue = summary.selected
        } else {
            trimRangeLabel.stringValue = ""; trimSelectedLabel.stringValue = ""
        }
        // Est. size uses the shared presentation (pending, staged, cap, estimate).
        var input: [String: Any] = [
            "estimating": estimating,
            "unapplied": stagedDiffers,
            "invalid_maximum": maximumSizeEnabled && !validMaximum,
            "estimate_exact": estimate?.exact ?? false,
            "original_bytes": (presentation?.snapshot.source["size_bytes"] as? NSNumber)?.uint64Value ?? 0,
        ]
        if let cap = (presentation?.snapshot.saveExport["max_size_bytes"] as? NSNumber)?.uint64Value {
            input["maximum_bytes"] = cap
        }
        if let estimate { input["estimate_bytes"] = estimate.sizeBytes }
        let shown = RecordingEditorCopy.estimate(input)
        estimateLabel.stringValue = shown.label
        estimateLabel.textColor = tokens.color(shown.muted ? "text-subtle" : "text")
        estimateDelta.isHidden = shown.deltaLabel == nil
        estimateDelta.stringValue = shown.deltaLabel ?? ""
        estimateDelta.textColor = tokens.color(shown.deltaSmaller ? "positive-text" : "danger-text")
        estimateDelta.layer?.backgroundColor = tokens.color(
            shown.deltaSmaller ? "positive-surface" : "danger-surface").cgColor
        previewNote.isHidden = cropAdjustmentActive || comparison != nil
            || !(presentation?.snapshot.export["max_size_bytes"] is NSNumber)
        refreshCaption()
        relayoutIfNeeded()
    }

    /// Relayout when card visibility or size changes; otherwise only the
    /// estimate row moves, so in-progress drags keep their geometry.
    private func relayoutIfNeeded() {
        let flags: [Bool] = [
            gifPanel.isHidden, audioPanel.isHidden, gifAudioNote.isHidden, customOutput,
            quality.isHidden, maximumSizeValue.isHidden, maximumSizeInvalid.isHidden,
            comparison != nil, previewNote.isHidden, thumbnailRetryButton?.isHidden ?? true,
            cancelButton?.isHidden ?? true, replaceButton?.isHidden ?? true,
            showInFolderButton?.isHidden ?? true, droppedFramesBand.isHidden,
            systemAudio.isHidden, microphoneAudio.isHidden,
        ]
        let signature = flags.map { $0 ? "1" : "0" }.joined()
        if signature != layoutSignature {
            layoutSignature = signature
            layout()
        } else if qualityPanel.frame.width > 0 {
            let origin = qualityPanel.frame.origin
            layoutQualityCard(width: qualityPanel.frame.width)
            qualityPanel.frame.origin = origin
        }
    }

    private func refreshCaption() {
        guard let snapshot = presentation?.snapshot else { return }
        if cropAdjustmentActive {
            let position = sourceFrameCache?.positionMilliseconds ?? snapshot.positionMilliseconds
            sourceLabel.stringValue = "Uncropped source · \(time(position)) · \(snapshot.width) × \(snapshot.height) · Drag to stage the crop, then Apply edits."
        } else if let comparison {
            sourceLabel.stringValue = "Encoded · accepted \(time(comparison.positionMilliseconds)) · \(comparison.after.width) × \(comparison.after.height)"
        } else {
            let position = playbackPositionMilliseconds ?? snapshot.positionMilliseconds
            let gif = snapshot.export["format"] as? String == "gif"
            let acceptedQuality = snapshot.export["quality"] as? String ?? "preserve"
            let qualityName = quality.itemArray
                .first { $0.representedObject as? String == acceptedQuality }?.title
                ?? "Preserve quality"
            let size = preview.image?.size ?? .zero
            sourceLabel.stringValue = "\(time(position)) / \(time(snapshot.durationMilliseconds)) · Source \(snapshot.width) × \(snapshot.height) · \(gif ? "GIF" : "MP4") \(qualityName) preview \(Int(size.width)) × \(Int(size.height))"
        }
    }

    private func closeSession() {
        invalidateComparison()
        generation += 1; artifactID = nil; presentation = nil; activeCancel = nil
        originalPath = nil; requiresReopen = false; awaitingReplaceConfirmation = false
        thumbnailCancel = nil; thumbnailRetryAvailable = false
        playbackCancel = nil; playbackState = .idle
        playbackPositionMilliseconds = nil; playbackReachedEOF = false; playbackFramePresented = false
        playbackLoopEnabled = false; playbackLoopControl = nil; playbackLoop.state = .off
        playbackSoundEnabled = false; playbackAudioEnabled = nil; playbackSound.state = .off
        gifFramesPerSecond = 15; gifFrameRate.selectItem(withTitle: "15 FPS")
        gifMaximumWidth = 800; gifMaximumWidthControl.selectItem(withTitle: "800 px")
        if gifMaximumWidthControl.item(withTitle: "Original") != nil {
            gifMaximumWidthControl.removeItem(withTitle: "Original")
        }
        maximumSizeEnabled = false; preserveQuality = true; estimating = false; replacing = false
        lastSavedPath = nil; droppedFramesBand.isHidden = true
        maximumSizeUnit = .megabytes; maximumSizeUnits.selectItem(withTitle: "MB")
        maximumSizeValue.stringValue = "10"
        qualityPreference = "preserve"; compressQuality = "highest"
        sourceFrameCache = nil; sourceFrameCancel = nil
        cropAdjustmentActive = false; cropAdjustmentPriorImage = nil
        previewActualSize = false
        cropOverlay.setEditingEnabled(false); cropOverlay.isHidden = true
        playbackStopActions.removeAll(); closeAfterPlayback = false
        terminateAfterPlayback = false; switchAfterPlayback = nil
        trimTimeline.setPlaybackPosition(nil); trimTimeline.setAcceptedPosition(nil)
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

    /// Shipping `formatEditorTime`: milliseconds under a minute, else `m:ss`.
    private func time(_ milliseconds: UInt64) -> String {
        RecordingEditorCopy.time(milliseconds,
                                 duration: presentation?.snapshot.durationMilliseconds ?? 0)
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

    @discardableResult private func button(_ title: String, parent: NSView,
                                            action: @escaping () -> Void) -> CaptureButton {
        let value = CaptureButton(title, frame: .zero, tokens: tokens, action: action)
        parent.addSubview(value); return value
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
