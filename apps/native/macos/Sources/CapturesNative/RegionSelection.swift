import AppKit
import CCapturesSettings

/// Event state only; all drag/aspect geometry executes the shared Rust algorithm.
struct RegionSelection {
    static let presets: [(String, Double)] = [("Free", 0), ("1:1", 1), ("4:3", 4.0/3.0),
        ("3:2", 1.5), ("16:9", 16.0/9.0), ("9:16", 9.0/16.0)]
    let bounds: CapturesSelectionBounds
    var rect = CapturesSelectionRect()
    private(set) var aspect = 0.0
    private var origin = CapturesSelectionPoint()
    private var current = CapturesSelectionPoint()
    private var initial = CapturesSelectionRect()
    private(set) var mode: UInt32?
    init(bounds: CapturesSelectionBounds) { self.bounds = bounds }
    var capturable: Bool { rect.width >= 2 && rect.height >= 2 }
    var nsRect: NSRect { NSRect(x: rect.x, y: rect.y, width: rect.width, height: rect.height) }
    var corners: [NSPoint] { [NSPoint(x: rect.x, y: rect.y), NSPoint(x: rect.x + rect.width, y: rect.y),
        NSPoint(x: rect.x, y: rect.y + rect.height), NSPoint(x: rect.x + rect.width, y: rect.y + rect.height)] }

    mutating func begin(_ point: NSPoint, handleRadius: CGFloat, shift: Bool) {
        origin = CapturesSelectionPoint(x: point.x, y: point.y); current = origin
        initial = rect
        let corner = capturable ? corners.firstIndex(where: { abs($0.x - point.x) <= handleRadius && abs($0.y - point.y) <= handleRadius }) : nil
        mode = corner.map { UInt32($0 + 2) } ?? (capturable && nsRect.contains(point) ? 1 : 0)
        update(point, shift: shift)
    }
    mutating func update(_ point: NSPoint, shift: Bool) {
        current = CapturesSelectionPoint(x: point.x, y: point.y)
        recompute(shift: shift)
    }
    mutating func recompute(shift: Bool) {
        guard let mode else { return }
        var result = rect
        if captures_selection_drag_v1(mode, origin, current, initial, bounds, aspect, shift, &result) { rect = result }
    }
    mutating func end() { mode = nil }
    mutating func setAspect(_ value: Double) {
        aspect = value
        var result = rect
        if capturable && captures_selection_constrain_v1(rect, bounds, value, &result) { rect = result }
    }
}

private final class RegionCanvas: NSView {
    weak var selector: RegionSelectionView?
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) { selector?.drawSelection() }
    override func mouseDown(with event: NSEvent) { selector?.mouseDown(with: event) }
    override func mouseDragged(with event: NSEvent) { selector?.mouseDragged(with: event) }
    override func mouseUp(with event: NSEvent) { selector?.mouseUp(with: event) }
}

/// Same retained image/canvas/native-button tree in live selection and CI fixtures.
/// Only the canvas and changed labels redraw on input; no animation/display timer.
final class RegionSelectionView: NSView {
    private let tokens: Tokens
    private let autoStart: Bool
    private(set) var selection: RegionSelection
    private let canvas = RegionCanvas()
    private let toolbar = NSView()
    private let hint: NSTextField
    private let dimensions = NSTextField(labelWithString: "")
    private var aspectButtons: [CaptureButton] = []
    private var captureButton: CaptureButton!
    var confirm: (CapturesSelectionRect) -> Void
    var cancel: () -> Void
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }

    init(frame: NSRect, image: CGImage?, tokens: Tokens, autoStart: Bool,
         confirm: @escaping (CapturesSelectionRect) -> Void, cancel: @escaping () -> Void) {
        self.tokens = tokens; self.autoStart = autoStart; self.confirm = confirm; self.cancel = cancel
        hint = NSTextField(labelWithString: CaptureGuidanceCopy.directHint(
            CaptureGuidanceCopy.regionTitle, CaptureGuidanceCopy.regionHint, confirm: !autoStart))
        selection = RegionSelection(bounds: CapturesSelectionBounds(width: frame.width, height: frame.height))
        super.init(frame: frame)
        wantsLayer = true
        if let image {
            let background = NSImageView(frame: bounds)
            background.wantsLayer = true
            background.image = NSImage(cgImage: image, size: bounds.size)
            background.imageScaling = .scaleAxesIndependently
            background.setAccessibilityElement(false)
            addSubview(background)
        }
        canvas.frame = bounds; canvas.wantsLayer = true; canvas.selector = self
        canvas.setAccessibilityElement(false); addSubview(canvas)
        let gap = tokens.number("s-4"), height = tokens.number("h-lg")
        let buttonWidth = tokens.number("s-12")
        toolbar.wantsLayer = true; toolbar.layer?.backgroundColor = tokens.color("glass-strong").cgColor
        toolbar.layer?.cornerRadius = tokens.number("r-xl")
        toolbar.layer?.borderWidth = 1; toolbar.layer?.borderColor = tokens.color("glass-border").cgColor
        let width = buttonWidth * 8.5 + gap * 9
        toolbar.frame = NSRect(x: (frame.width - width) / 2, y: frame.height - height - gap * 4, width: width, height: height + gap * 2)
        addSubview(toolbar)
        for (index, preset) in RegionSelection.presets.enumerated() {
            let button = CaptureButton(preset.0, frame: NSRect(x: gap + CGFloat(index) * (buttonWidth + gap), y: gap,
                width: buttonWidth, height: height), tokens: tokens, glass: true) { [weak self] in
                self?.setAspect(preset.1)
            }
            button.setAccessibilityLabel("Region aspect \(preset.0)")
            aspectButtons.append(button); toolbar.addSubview(button)
        }
        captureButton = CaptureButton("Capture", frame: NSRect(x: gap + 6 * (buttonWidth + gap), y: gap,
            width: buttonWidth * 1.5, height: height), tokens: tokens, glass: true) { [weak self] in self?.confirmSelection() }
        captureButton.selected = true
        captureButton.keyEquivalent = "\r"; captureButton.keyEquivalentModifierMask = []
        toolbar.addSubview(captureButton)
        let close = CaptureButton("Cancel", frame: NSRect(x: captureButton.frame.maxX + gap, y: gap,
            width: buttonWidth, height: height), tokens: tokens, glass: true) { [weak self] in self?.cancel() }
        close.keyEquivalent = "\u{1b}"; close.keyEquivalentModifierMask = []
        toolbar.addSubview(close)
        hint.font = .systemFont(ofSize: tokens.number("text-md")); hint.textColor = tokens.color("glass-text")
        hint.alignment = .center; hint.wantsLayer = true
        hint.layer?.backgroundColor = tokens.color("glass-strong").cgColor; hint.layer?.cornerRadius = tokens.number("r-sm")
        hint.frame = NSRect(x: toolbar.frame.minX, y: toolbar.frame.minY - height - gap, width: width, height: height)
        addSubview(hint)
        dimensions.font = .monospacedDigitSystemFont(ofSize: tokens.number("text-xs"), weight: .semibold)
        dimensions.textColor = tokens.color("theme-accent-ink"); dimensions.alignment = .center
        dimensions.wantsLayer = true; dimensions.layer?.backgroundColor = tokens.color("theme-accent").cgColor
        dimensions.layer?.cornerRadius = tokens.number("r-sm"); addSubview(dimensions)
        setAccessibilityRole(.group); setAccessibilityLabel("Capture region selector")
        update()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func setAspect(_ value: Double) { selection.setAspect(value); update() }
    func begin(_ point: NSPoint, shift: Bool = false) {
        selection.begin(point, handleRadius: tokens.number("s-4"), shift: shift); update()
    }
    func drag(_ point: NSPoint, shift: Bool = false) { selection.update(point, shift: shift); update() }
    func end() {
        guard selection.mode != nil else { return }
        let created = selection.mode == 0
        selection.end(); update()
        if autoStart && created { confirmSelection() }
    }
    func confirmSelection() { if selection.capturable && selection.mode == nil { confirm(selection.rect) } }
    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self); begin(convert(event.locationInWindow, from: nil), shift: event.modifierFlags.contains(.shift))
    }
    override func mouseDragged(with event: NSEvent) { drag(convert(event.locationInWindow, from: nil), shift: event.modifierFlags.contains(.shift)) }
    override func mouseUp(with event: NSEvent) { drag(convert(event.locationInWindow, from: nil), shift: event.modifierFlags.contains(.shift)); end() }
    override func flagsChanged(with event: NSEvent) { selection.recompute(shift: event.modifierFlags.contains(.shift)); update() }
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { cancel() }
        else if event.keyCode == 36 || event.keyCode == 76 { confirmSelection() }
        else { super.keyDown(with: event) }
    }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .crosshair) }

    private func update() {
        canvas.needsDisplay = true
        captureButton.isEnabled = selection.capturable && selection.mode == nil
        for (index, button) in aspectButtons.enumerated() {
            button.selected = selection.aspect == RegionSelection.presets[index].1
            button.setAccessibilityValue(button.selected ? 1 : 0)
            button.needsDisplay = true
        }
        dimensions.isHidden = !selection.capturable
        let text = "\(Int(selection.rect.width.rounded())) × \(Int(selection.rect.height.rounded()))"
        dimensions.stringValue = text
        dimensions.setAccessibilityLabel("Selected region \(text) logical pixels")
        let labelWidth = dimensions.intrinsicContentSize.width + tokens.number("s-6")
        let height = tokens.number("h-xs")
        dimensions.frame = NSRect(x: min(max(0, selection.nsRect.midX - labelWidth / 2), bounds.width - labelWidth),
            y: selection.rect.y >= height + tokens.number("s-3") ? selection.rect.y - height - tokens.number("s-3") : selection.rect.y + tokens.number("s-3"),
            width: labelWidth, height: height)
        captureButton.needsDisplay = true
    }

    fileprivate func drawSelection() {
        let path = NSBezierPath(rect: bounds)
        if selection.capturable { path.appendRect(selection.nsRect) }
        path.windingRule = .evenOdd; tokens.color("glass-veil").setFill(); path.fill()
        guard selection.capturable else { return }
        tokens.color("theme-accent").setStroke()
        let border = NSBezierPath(rect: selection.nsRect); border.lineWidth = tokens.number("s-1") * 0.75; border.stroke()
        for corner in selection.corners {
            let radius = tokens.number("s-3") - tokens.number("s-1") / 2
            let handle = NSBezierPath(ovalIn: NSRect(x: corner.x - radius, y: corner.y - radius, width: radius * 2, height: radius * 2))
            tokens.color("theme-accent").setFill(); handle.fill()
            tokens.color("theme-accent-ink").setStroke(); handle.lineWidth = tokens.number("s-1"); handle.stroke()
        }
    }
}

final class RegionSelectionPanel: NSPanel {
    let selector: RegionSelectionView
    override var canBecomeKey: Bool { true }
    init(screen: NSScreen, image: CGImage?, tokens: Tokens, autoStart: Bool,
         confirm: @escaping (CapturesSelectionRect) -> Void, cancel: @escaping () -> Void) {
        selector = RegionSelectionView(frame: NSRect(origin: .zero, size: screen.frame.size), image: image,
            tokens: tokens, autoStart: autoStart, confirm: confirm, cancel: cancel)
        super.init(contentRect: screen.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        isReleasedWhenClosed = false; isOpaque = false; backgroundColor = .clear; hasShadow = false
        level = .screenSaver; collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        sharingType = .none; contentView = selector
        makeFirstResponder(selector)
    }
}
