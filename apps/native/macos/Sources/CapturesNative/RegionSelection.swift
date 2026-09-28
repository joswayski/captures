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

/// Shipping `CaptureDim` under the selection chrome, in its own layer so the
/// dim can fade in on reveal (`.capture-region .capture-shade` over `--dur-4`
/// `ease-in-out`, and New Capture's region and window shades) while the frozen
/// snapshot beneath is opaque from the first frame. The fade is
/// presentation-only; the model opacity stays 1.
final class CaptureShadeView: NSView {
    private let tokens: Tokens
    private var pendingFade = false
    var drawShade: () -> Void = {}
    /// Whether the dim on screen at reveal fades (New Capture's Full screen
    /// shade does not).
    var fadesOnReveal: () -> Bool = { true }
    override var isFlipped: Bool { true }

    init(frame: NSRect, tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: frame)
        wantsLayer = true
        setAccessibilityElement(false)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func draw(_ dirtyRect: NSRect) { drawShade() }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    /// Fade the dim in once this view is on screen (skipped under Reduce Motion).
    func fadeInOnReveal() {
        pendingFade = true
        if window != nil { playFade() }
    }

    /// Whether the reveal fade is playing, for tests.
    var isFading: Bool { layer?.animation(forKey: "capture-shade-fade") != nil }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window != nil && pendingFade { playFade() }
    }

    private func playFade() {
        pendingFade = false
        let tween = NativeMotion.transition("capture_shade_fade", tokens: tokens)
        guard tween.duration > 0, fadesOnReveal(), let layer else { return }
        let fade = CABasicAnimation(keyPath: "opacity")
        fade.fromValue = 0; fade.toValue = 1
        fade.duration = tween.duration; fade.timingFunction = tween.timing
        fade.fillMode = .backwards
        layer.add(fade, forKey: "capture-shade-fade")
    }
}

private final class RegionCanvas: NSView {
    weak var selector: RegionSelectionView?
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) { selector?.drawSelection() }
    override func mouseDown(with event: NSEvent) { selector?.mouseDown(with: event) }
    override func mouseDragged(with event: NSEvent) { selector?.mouseDragged(with: event) }
    override func mouseUp(with event: NSEvent) { selector?.mouseUp(with: event) }
    override func mouseMoved(with event: NSEvent) { selector?.mouseMoved(with: event) }
}

/// Same retained image/canvas/native-button tree in live selection and CI fixtures.
/// Only the shade, canvas and changed labels redraw on input; no display timer
/// (the shade's reveal fade is a Core Animation opacity animation).
final class RegionSelectionView: NSView {
    private let tokens: Tokens
    private let autoStart: Bool
    private(set) var selection: RegionSelection
    private let canvas = RegionCanvas()
    private let shade: CaptureShadeView
    private let toolbar = NSView()
    private let guidance: CaptureGuidanceChip
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
        guidance = CaptureGuidanceChip(tokens: tokens)
        shade = CaptureShadeView(frame: NSRect(origin: .zero, size: frame.size), tokens: tokens)
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
        shade.drawShade = { [weak self] in self?.drawShade() }
        addSubview(shade)
        shade.fadeInOnReveal()
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
        // The shipping direct overlay has no toolbar: a drag commits on release.
        toolbar.isHidden = autoStart
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
        // Shipping `CaptureGuidance`, 16% from the top; hidden while dragging.
        addSubview(guidance)
        setGuidanceCopy(feedback: false)
        guidance.setPresent(true)
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
        guard autoStart && created else { return }
        if selection.capturable { confirmSelection() } else { showSelectionFeedback() }
    }

    private var feedbackToken = 0
    /// Shipping `showSelectionFeedback`: a click without a region re-keys the
    /// chip with "Click and drag to select a region", the accent border and
    /// the nudge for 1.8 seconds, then re-keys it back.
    private func showSelectionFeedback() {
        feedbackToken &+= 1
        let token = feedbackToken
        setGuidanceCopy(feedback: true)
        guidance.mount(feedback: true)
        let seconds = CaptureMenuPolicy.copy.chip.feedbackSeconds
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { [weak self] in
            guard let self, self.feedbackToken == token else { return }
            self.setGuidanceCopy(feedback: false)
            self.guidance.mount(feedback: false)
        }
    }

    private func setGuidanceCopy(feedback: Bool) {
        var hint = CaptureGuidanceCopy.regionHint
        if !autoStart { hint += " · " + CaptureGuidanceCopy.confirm }
        guidance.setCopy(title: feedback ? CaptureGuidanceCopy.regionFeedbackTitle : CaptureGuidanceCopy.regionTitle,
            hint: hint, in: bounds)
    }

    var guidanceText: String { guidance.guidanceText }
    var guidanceChip: CaptureGuidanceChip { guidance }
    var isGuidanceVisible: Bool { guidance.isShowing }
    func confirmSelection() { if selection.capturable && selection.mode == nil { confirm(selection.rect) } }
    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self); begin(convert(event.locationInWindow, from: nil), shift: event.modifierFlags.contains(.shift))
    }
    override func mouseDragged(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        guidance.duck(at: point)
        drag(point, shift: event.modifierFlags.contains(.shift))
    }
    override func mouseMoved(with event: NSEvent) { guidance.duck(at: convert(event.locationInWindow, from: nil)) }
    override func mouseUp(with event: NSEvent) { drag(convert(event.locationInWindow, from: nil), shift: event.modifierFlags.contains(.shift)); end() }
    override func flagsChanged(with event: NSEvent) { selection.recompute(shift: event.modifierFlags.contains(.shift)); update() }
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { cancel() }
        else if event.keyCode == 36 || event.keyCode == 76 { confirmSelection() }
        else { super.keyDown(with: event) }
    }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .crosshair) }

    private func update() {
        canvas.needsDisplay = true; shade.needsDisplay = true
        guidance.setSuppressed(selection.mode != nil)
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
        let labelWidth = dimensions.intrinsicContentSize.width + tokens.number("s-4") * 2
        // Shipping `.selection-dimensions`: left-aligned inside the 1.5 pt border,
        // 30 pt above the box, or `--s-3` inside it near the top of the screen.
        let border: CGFloat = 1.5
        dimensions.frame = NSRect(x: min(max(0, selection.rect.x + border), bounds.width - labelWidth),
            y: selection.rect.y + border + (selection.rect.y < 30 ? tokens.number("s-3") : -30),
            width: labelWidth, height: tokens.number("h-xs"))
        captureButton.needsDisplay = true
    }

    fileprivate func drawShade() {
        let path = NSBezierPath(rect: bounds)
        if selection.capturable { path.appendRect(selection.nsRect) }
        path.windingRule = .evenOdd; tokens.color("capture-shade").setFill(); path.fill()
    }

    /// The reveal fade of the dim, for tests.
    var isShadeFading: Bool { shade.isFading }

    fileprivate func drawSelection() {
        guard selection.capturable else { return }
        RegionSelectionView.drawMarquee(selection.nsRect, tokens: tokens, radius: 0)
    }

    /// Shipping marquee: a 1.5 pt accent border between a 1 pt dark outer
    /// hairline and a 1 pt light inner hairline. The direct `.selection-box`
    /// is square and has no handles; New Capture rounds it by 2 pt.
    static func drawMarquee(_ rect: NSRect, tokens: Tokens, radius: CGFloat) {
        for (inset, width, color, extra) in [(-0.5, 1.0, "selection-hairline-outer", 0.5),
                                             (0.75, 1.5, "theme-accent", -0.75),
                                             (2.0, 1.0, "selection-hairline-inner", -2.0)] as [(CGFloat, CGFloat, String, CGFloat)] {
            let corner = max(0, radius + extra)
            let line = NSBezierPath(roundedRect: rect.insetBy(dx: inset, dy: inset), xRadius: corner, yRadius: corner)
            line.lineWidth = width; tokens.color(color).setStroke(); line.stroke()
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
        sharingType = .none; acceptsMouseMovedEvents = true; contentView = selector
        makeFirstResponder(selector)
    }
}
