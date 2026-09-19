import AppKit
import CCapturesSettings

enum UnifiedCaptureTarget: String, CaseIterable {
    case region
    case window
    case display

    var title: String {
        switch self {
        case .region: return "Region"
        case .window: return "Window"
        case .display: return "Full screen"
        }
    }
}

struct UnifiedCaptureControlsState: Equatable {
    var target: UnifiedCaptureTarget
    var aspectIndex: Int

    static let initial = UnifiedCaptureControlsState(target: .region, aspectIndex: 0)
}

private final class GlassPopUpButton: NSPopUpButton {
    var tokens: Tokens!
    var change: ((Int) -> Void)?

    @objc private func selectedValue() { change?(indexOfSelectedItem) }

    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1),
            xRadius: tokens.number("r-md"), yRadius: tokens.number("r-md"))
        tokens.color("glass-raised").setFill(); path.fill()
        tokens.color(window?.firstResponder === self ? "theme-accent" : "glass-border").setStroke()
        path.stroke()
        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: tokens.number("text-sm"), weight: .medium),
            .foregroundColor: tokens.color("glass-text"),
        ]
        (title as NSString).draw(at: NSPoint(x: tokens.number("s-4"), y: 10),
            withAttributes: attributes)
        ("⌄" as NSString).draw(at: NSPoint(x: bounds.width - tokens.number("s-7"), y: 10),
            withAttributes: attributes)
    }

    func bindChange(_ callback: @escaping (Int) -> Void) {
        change = callback; target = self; action = #selector(selectedValue)
    }
}

final class CaptureControlsView: NSView {
    private let tokens: Tokens
    private let autoStart: Bool
    private var targetButtons: [UnifiedCaptureTarget: CaptureButton] = [:]
    private let aspectLabel = NSTextField(labelWithString: "Aspect")
    private let aspectMenu: GlassPopUpButton
    private let displayMenu: GlassPopUpButton
    private let captureButton: CaptureButton
    private var panelDragOffset: NSPoint?
    var switchTarget: (UnifiedCaptureTarget) -> Void = { _ in }
    var changeAspect: (Int) -> Void = { _ in }
    var changeDisplay: (Int) -> Void = { _ in }
    var confirm: () -> Void = {}
    var cancel: () -> Void = {}
    private(set) var target: UnifiedCaptureTarget = .region

    override var isFlipped: Bool { true }

    init(frame: NSRect, tokens: Tokens, autoStart: Bool, displayTitles: [String],
         selectedDisplay: Int) {
        self.tokens = tokens; self.autoStart = autoStart
        aspectMenu = GlassPopUpButton(frame: .zero, pullsDown: false)
        displayMenu = GlassPopUpButton(frame: .zero, pullsDown: false)
        captureButton = CaptureButton("Capture", frame: .zero, tokens: tokens, glass: true) {}
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = tokens.color("glass-strong").cgColor
        layer?.cornerRadius = tokens.number("r-2xl")
        layer?.borderWidth = 1; layer?.borderColor = tokens.color("glass-border").cgColor
        layer?.shadowColor = NSColor.black.cgColor; layer?.shadowOpacity = 0.44
        layer?.shadowRadius = 22; layer?.shadowOffset = NSSize(width: 0, height: -8)
        setAccessibilityRole(.group); setAccessibilityLabel("Screenshot controls")

        let note = NSTextField(labelWithString: autoStart
            ? "Controls are hidden from screenshots  ·  Auto-capture is on. Selecting a target starts immediately."
            : "Controls are hidden from screenshots  ·  Press Enter to confirm")
        note.frame = NSRect(x: 16, y: 60, width: frame.width - 32, height: 22)
        note.alignment = .center
        note.font = .systemFont(ofSize: tokens.number("text-xs"), weight: .medium)
        note.textColor = tokens.color("glass-text-subtle")
        note.setAccessibilityLabel(note.stringValue)
        addSubview(note)

        let narrow = frame.width < 820
        let close = control("×", x: 8, width: 32) { [weak self] in self?.cancel() }
        close.setAccessibilityLabel("Close capture controls")
        let screenshot = control("Screenshot", x: 48, width: narrow ? 88 : 92) {}
        screenshot.icon = .capture
        screenshot.selected = true; screenshot.setAccessibilityValue(1)
        screenshot.enterActionBlock = { [weak self] in self?.confirm() }
        let recordX = screenshot.frame.maxX + 4
        let record = control("Record", x: recordX, width: narrow ? 64 : 76) {}
        record.icon = .record
        record.isEnabled = false
        record.toolTip = "Recording is not available in this native build"
        record.setAccessibilityHelp("Recording is not available yet")

        let dividerX = record.frame.maxX + 8
        let divider = NSView(frame: NSRect(x: dividerX, y: 18, width: 1, height: 24))
        divider.wantsLayer = true; divider.layer?.backgroundColor = tokens.color("glass-border").cgColor
        addSubview(divider)

        var x = dividerX + 9
        for mode in UnifiedCaptureTarget.allCases {
            let width: CGFloat
            if narrow {
                width = mode == .region ? 62 : mode == .window ? 68 : 94
            } else {
                width = mode == .display ? 106 : 78
            }
            let button = control(mode.title, x: x, width: width) { [weak self] in
                self?.selectTarget(mode, notify: true)
            }
            button.setAccessibilityRole(.radioButton)
            button.setAccessibilityLabel(mode.title)
            button.icon = mode == .window ? .window : mode == .display ? .display : .capture
            button.enterActionBlock = { [weak self] in self?.confirm() }
            targetButtons[mode] = button
            x += width + 4
        }

        let pickerX = x + 4
        let pickerWidth = frame.width - 122 - 8 - pickerX
        aspectLabel.frame = NSRect(x: pickerX, y: 21, width: 42, height: 18)
        aspectLabel.font = .systemFont(ofSize: tokens.number("text-xs"), weight: .medium)
        aspectLabel.textColor = tokens.color("glass-text-subtle")
        addSubview(aspectLabel)
        aspectMenu.frame = NSRect(x: pickerX + 44, y: 12,
            width: pickerWidth - 44, height: 36)
        aspectMenu.tokens = tokens
        aspectMenu.addItems(withTitles: RegionSelection.presets.map { $0.0 })
        aspectMenu.setAccessibilityLabel("Region aspect ratio")
        aspectMenu.bindChange { [weak self] index in self?.changeAspect(index) }
        addSubview(aspectMenu)

        displayMenu.frame = NSRect(x: pickerX, y: 12, width: pickerWidth, height: 36)
        displayMenu.tokens = tokens; displayMenu.addItems(withTitles: displayTitles)
        displayMenu.selectItem(at: selectedDisplay)
        displayMenu.setAccessibilityLabel("Display")
        displayMenu.bindChange { [weak self] index in self?.changeDisplay(index) }
        addSubview(displayMenu)

        captureButton.frame = NSRect(x: frame.width - 122, y: 10, width: 112, height: 40)
        captureButton.icon = .capture
        captureButton.primary = true; captureButton.actionBlock = { [weak self] in self?.confirm() }
        captureButton.enterActionBlock = { [weak self] in self?.confirm() }
        captureButton.setAccessibilityLabel("Take screenshot")
        captureButton.keyEquivalent = "\r"; captureButton.keyEquivalentModifierMask = []
        addSubview(captureButton)

        for button in subviews.compactMap({ $0 as? CaptureButton }) {
            button.escapeActionBlock = { [weak self] in self?.cancel() }
        }
        selectTarget(.region, notify: false)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func hitTest(_ point: NSPoint) -> NSView? {
        guard let hit = super.hitTest(point) else { return nil }
        return hit is CaptureButton || hit is GlassPopUpButton ? hit : self
    }

    override func mouseDown(with event: NSEvent) {
        guard let superview else { return }
        beginPanelDrag(at: superview.convert(event.locationInWindow, from: nil))
    }

    override func mouseDragged(with event: NSEvent) {
        guard let superview else { return }
        dragPanel(to: superview.convert(event.locationInWindow, from: nil))
    }

    override func mouseUp(with event: NSEvent) { endPanelDrag() }

    func beginPanelDrag(at point: NSPoint) {
        panelDragOffset = NSPoint(x: point.x - frame.minX, y: point.y - frame.minY)
    }

    func dragPanel(to point: NSPoint) {
        guard let superview, let offset = panelDragOffset else { return }
        let inset: CGFloat = 16
        let maxX = max(inset, superview.bounds.width - frame.width - inset)
        let maxY = max(inset, superview.bounds.height - frame.height - inset)
        frame.origin = NSPoint(x: min(max(inset, point.x - offset.x), maxX),
            y: min(max(inset, point.y - offset.y), maxY))
    }

    func endPanelDrag() { panelDragOffset = nil }

    private func control(_ title: String, x: CGFloat, width: CGFloat,
                         action: @escaping () -> Void) -> CaptureButton {
        let button = CaptureButton(title, frame: NSRect(x: x, y: 12, width: width, height: 36),
            tokens: tokens, glass: true, action: action)
        addSubview(button); return button
    }

    func selectTarget(_ target: UnifiedCaptureTarget, notify: Bool) {
        self.target = target
        for (mode, button) in targetButtons {
            button.selected = mode == target
            button.setAccessibilityValue(button.selected ? 1 : 0)
            button.needsDisplay = true
        }
        aspectLabel.isHidden = target != .region
        aspectMenu.isHidden = target != .region
        displayMenu.isHidden = target != .display
        if notify { switchTarget(target) }
    }

    func selectAspect(_ index: Int) { aspectMenu.selectItem(at: index) }
    func selectDisplay(_ index: Int) { displayMenu.selectItem(at: index) }
    func setCaptureEnabled(_ enabled: Bool) {
        captureButton.isEnabled = enabled
        captureButton.isHidden = autoStart
        captureButton.needsDisplay = true
    }
}

private final class UnifiedCaptureCanvas: NSView {
    weak var selector: UnifiedCaptureSelectionView?
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) { selector?.drawSelection() }
    override func mouseDown(with event: NSEvent) { selector?.mouseDown(with: event) }
    override func mouseDragged(with event: NSEvent) { selector?.mouseDragged(with: event) }
    override func mouseUp(with event: NSEvent) { selector?.mouseUp(with: event) }
    override func mouseMoved(with event: NSEvent) { selector?.mouseMoved(with: event) }
    override func resetCursorRects() {
        addCursorRect(bounds, cursor: selector?.target == .region ? .crosshair : .pointingHand)
    }
}

final class UnifiedCaptureSelectionView: NSView {
    typealias HitTest = (CapturesSelectionPoint) -> Int64?

    private let tokens: Tokens
    private let autoStart: Bool
    private let targets: [WindowSelectionTarget]
    private let hitTest: HitTest
    private let canvas = UnifiedCaptureCanvas()
    private(set) var region: RegionSelection
    private(set) var selectedWindowIndex: Int64?
    private(set) var hoveredWindowIndex: Int64 = -1
    private(set) var target: UnifiedCaptureTarget = .region
    private(set) var aspectIndex = 0
    let controls: CaptureControlsView
    private let selectionLabel = NSTextField(labelWithString: "")
    private let guidance = Surface()
    private let guidanceTitle = NSTextField(labelWithString: "")
    private let guidanceDetail = NSTextField(labelWithString: "")
    private let currentDisplayTitle: String
    private var regionGestureActive = false
    var confirm: (WindowSelectionChoice) -> Void
    var cancel: () -> Void
    var changeDisplay: (Int) -> Void

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }

    init(frame: NSRect, image: CGImage?, targets: [WindowSelectionTarget], tokens: Tokens,
         autoStart: Bool, hitTest: @escaping HitTest, displayTitles: [String],
         selectedDisplay: Int, confirm: @escaping (WindowSelectionChoice) -> Void,
         cancel: @escaping () -> Void, changeDisplay: @escaping (Int) -> Void) {
        self.tokens = tokens; self.autoStart = autoStart; self.targets = targets
        self.hitTest = hitTest; self.confirm = confirm; self.cancel = cancel
        self.changeDisplay = changeDisplay
        currentDisplayTitle = displayTitles.indices.contains(selectedDisplay)
            ? displayTitles[selectedDisplay] : "Full screen"
        region = RegionSelection(bounds: CapturesSelectionBounds(width: frame.width, height: frame.height))
        let controlsWidth = min(frame.width - 32, 854)
        controls = CaptureControlsView(frame: NSRect(x: (frame.width - controlsWidth) / 2,
            y: frame.height - 112, width: controlsWidth, height: 86), tokens: tokens,
            autoStart: autoStart, displayTitles: displayTitles, selectedDisplay: selectedDisplay)
        super.init(frame: frame)
        wantsLayer = true; layer?.backgroundColor = NSColor.clear.cgColor
        if let image {
            let background = NSImageView(frame: bounds)
            background.image = NSImage(cgImage: image, size: bounds.size)
            background.imageScaling = .scaleAxesIndependently
            background.setAccessibilityElement(false); addSubview(background)
        }
        canvas.frame = bounds; canvas.selector = self; canvas.setAccessibilityElement(false)
        addSubview(canvas)
        guidance.wantsLayer = true
        guidance.layer?.backgroundColor = tokens.color("glass-strong").cgColor
        guidance.layer?.cornerRadius = tokens.number("r-lg")
        guidance.layer?.borderWidth = 1
        guidance.layer?.borderColor = tokens.color("glass-border").cgColor
        guidanceTitle.alignment = .center
        guidanceTitle.font = .systemFont(ofSize: tokens.number("text-md"), weight: .semibold)
        guidanceTitle.textColor = tokens.color("glass-text")
        guidanceDetail.alignment = .center
        guidanceDetail.font = .systemFont(ofSize: tokens.number("text-xs"), weight: .medium)
        guidanceDetail.textColor = tokens.color("glass-text-subtle")
        guidance.addSubview(guidanceTitle); guidance.addSubview(guidanceDetail)
        addSubview(guidance)
        selectionLabel.font = .systemFont(ofSize: tokens.number("text-xs"), weight: .semibold)
        selectionLabel.textColor = tokens.color("glass-text")
        selectionLabel.alignment = .center; selectionLabel.wantsLayer = true
        selectionLabel.layer?.backgroundColor = tokens.color("glass-strong").cgColor
        selectionLabel.layer?.cornerRadius = tokens.number("r-sm")
        addSubview(selectionLabel)
        controls.switchTarget = { [weak self] target in self?.setTarget(target) }
        controls.changeAspect = { [weak self] index in self?.setAspect(index) }
        controls.changeDisplay = { [weak self] index in self?.changeDisplay(index) }
        controls.confirm = { [weak self] in self?.confirmSelection() }
        controls.cancel = { [weak self] in self?.cancel() }
        addSubview(controls)
        setAccessibilityRole(.group); setAccessibilityLabel("New Capture screenshot selector")
        update()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    var isGuidanceVisible: Bool { !guidance.isHidden }
    var controlsState: UnifiedCaptureControlsState {
        UnifiedCaptureControlsState(target: target, aspectIndex: aspectIndex)
    }

    var choice: WindowSelectionChoice? {
        switch target {
        case .region: return region.capturable && region.mode == nil ? .region(region.rect) : nil
        case .window:
            guard let index = selectedWindowIndex, index >= 0, index < Int64(targets.count) else { return nil }
            return .window(index: Int(index), id: targets[Int(index)].id)
        case .display: return .display
        }
    }

    func setTarget(_ target: UnifiedCaptureTarget) {
        self.target = target; controls.selectTarget(target, notify: false)
        if target == .display && autoStart { confirmSelection(); return }
        update()
    }

    func setAspect(_ index: Int) {
        guard RegionSelection.presets.indices.contains(index) else { return }
        aspectIndex = index
        region.setAspect(RegionSelection.presets[index].1)
        controls.selectAspect(index); update()
    }

    func restoreControls(_ state: UnifiedCaptureControlsState) {
        setAspect(state.aspectIndex)
        setTarget(state.target)
    }

    func beginRegion(_ point: NSPoint, shift: Bool = false) {
        guard target == .region else { return }
        region.begin(point, handleRadius: tokens.number("s-4"), shift: shift); update()
    }
    func dragRegion(_ point: NSPoint, shift: Bool = false) {
        guard target == .region else { return }
        region.update(point, shift: shift); update()
    }
    func endRegion() {
        guard target == .region, region.mode != nil else { return }
        let created = region.mode == 0
        region.end(); update()
        if autoStart && created { confirmSelection() }
    }

    @discardableResult func hoverWindow(_ point: NSPoint) -> Bool {
        guard target == .window, point.x.isFinite, point.y.isFinite,
              let index = hitTest(CapturesSelectionPoint(x: point.x, y: point.y)),
              index == -1 || (index >= 0 && index < Int64(targets.count)) else { return false }
        hoveredWindowIndex = index; update(); return true
    }

    func selectWindow(_ point: NSPoint) {
        guard hoverWindow(point) else { return }
        if hoveredWindowIndex < 0 {
            selectedWindowIndex = nil; setTarget(.display)
        } else {
            selectedWindowIndex = hoveredWindowIndex; update()
            if autoStart { confirmSelection() }
        }
    }

    func confirmSelection() { if let choice { confirm(choice) } }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let point = convert(event.locationInWindow, from: nil)
        guard !controls.frame.contains(point) else {
            regionGestureActive = false
            return
        }
        switch target {
        case .region:
            regionGestureActive = true
            beginRegion(point, shift: event.modifierFlags.contains(.shift))
        case .window: selectWindow(point)
        case .display: if autoStart { confirmSelection() }
        }
    }
    override func mouseDragged(with event: NSEvent) {
        if target == .region && regionGestureActive {
            dragRegion(convert(event.locationInWindow, from: nil),
                shift: event.modifierFlags.contains(.shift))
        }
    }
    override func mouseUp(with event: NSEvent) {
        if target == .region && regionGestureActive {
            dragRegion(convert(event.locationInWindow, from: nil),
                shift: event.modifierFlags.contains(.shift)); endRegion()
        }
        regionGestureActive = false
    }
    override func mouseMoved(with event: NSEvent) {
        if target == .window { _ = hoverWindow(convert(event.locationInWindow, from: nil)) }
    }
    override func flagsChanged(with event: NSEvent) {
        if target == .region {
            region.recompute(shift: event.modifierFlags.contains(.shift)); update()
        }
    }
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { cancel() }
        else if event.keyCode == 36 || event.keyCode == 76 { confirmSelection() }
        else { super.keyDown(with: event) }
    }
    func updatePointerLocation() {
        guard target == .window, let window else { return }
        let point = window.convertPoint(fromScreen: NSEvent.mouseLocation)
        _ = hoverWindow(convert(point, from: nil))
    }

    private func update() {
        controls.setCaptureEnabled(choice != nil)
        let label: String
        let rect: NSRect?
        switch target {
        case .region:
            guidance.isHidden = region.mode != nil
            guidanceTitle.stringValue = "Drag to select a region"
            guidanceDetail.stringValue = "Shift for square  ·  Esc to cancel"
            label = region.capturable
                ? "\(Int(region.rect.width.rounded())) × \(Int(region.rect.height.rounded()))" : ""
            rect = region.capturable ? region.nsRect : nil
        case .window:
            let active = hoveredWindowIndex >= 0 ? hoveredWindowIndex : selectedWindowIndex ?? -1
            if active >= 0, active < Int64(targets.count) {
                guidance.isHidden = true
                label = targets[Int(active)].name; rect = targets[Int(active)].rect
            } else {
                guidance.isHidden = false
                guidanceTitle.stringValue = "Select a window"
                guidanceDetail.stringValue = "Click a window  ·  Esc to cancel"
                label = ""; rect = nil
            }
        case .display:
            guidance.isHidden = false
            let parts = currentDisplayTitle.components(separatedBy: " · ")
            guidanceTitle.stringValue = parts.first ?? currentDisplayTitle
            guidanceDetail.stringValue = parts.dropFirst().joined(separator: "  ·  ")
            label = ""; rect = nil
        }
        let displayGuidance = target == .display
        let guidanceWidth: CGFloat = displayGuidance ? 280 : 300
        let guidanceHeight: CGFloat = displayGuidance ? 82 : 62
        guidance.frame = NSRect(x: (bounds.width - guidanceWidth) / 2,
            y: displayGuidance ? (bounds.height - guidanceHeight) / 2 : 26,
            width: guidanceWidth, height: guidanceHeight)
        guidanceTitle.frame = NSRect(x: 12, y: displayGuidance ? 18 : 10,
            width: guidanceWidth - 24, height: 22)
        guidanceDetail.frame = NSRect(x: 12, y: displayGuidance ? 43 : 34,
            width: guidanceWidth - 24, height: 18)
        guidance.setAccessibilityLabel([guidanceTitle.stringValue, guidanceDetail.stringValue]
            .filter { !$0.isEmpty }.joined(separator: ". "))
        selectionLabel.stringValue = label
        selectionLabel.isHidden = label.isEmpty
        if let rect {
            let width = min(max(80, selectionLabel.intrinsicContentSize.width + tokens.number("s-6")),
                bounds.width - tokens.number("s-8"))
            let height = tokens.number("h-xs")
            selectionLabel.frame = NSRect(x: min(max(tokens.number("s-4"), rect.midX - width / 2),
                bounds.width - width - tokens.number("s-4")),
                y: rect.minY >= height + tokens.number("s-3")
                    ? rect.minY - height - tokens.number("s-3") : rect.minY + tokens.number("s-3"),
                width: width, height: height)
        }
        selectionLabel.setAccessibilityLabel(label)
        canvas.needsDisplay = true; canvas.discardCursorRects(); canvas.resetCursorRects()
    }

    fileprivate func drawSelection() {
        let selectedRect: NSRect?
        let radius: CGFloat
        switch target {
        case .region: selectedRect = region.capturable ? region.nsRect : nil; radius = 0
        case .window:
            let active = hoveredWindowIndex >= 0 ? hoveredWindowIndex : selectedWindowIndex ?? -1
            if active >= 0, active < Int64(targets.count) {
                selectedRect = targets[Int(active)].rect; radius = targets[Int(active)].cornerRadius
            } else { selectedRect = nil; radius = 0 }
        case .display: selectedRect = bounds.insetBy(dx: 2, dy: 2); radius = 0
        }
        let veil = NSBezierPath(rect: bounds)
        if let selectedRect {
            veil.append(NSBezierPath(roundedRect: selectedRect, xRadius: radius, yRadius: radius))
        }
        veil.windingRule = .evenOdd; tokens.color("glass-veil").setFill(); veil.fill()
        guard let selectedRect else { return }
        tokens.color("theme-accent").setStroke()
        let border = NSBezierPath(roundedRect: selectedRect, xRadius: radius, yRadius: radius)
        border.lineWidth = 1.5; border.stroke()
        if target == .region {
            for corner in region.corners {
                let handle = NSBezierPath(ovalIn: NSRect(x: corner.x - 5, y: corner.y - 5,
                    width: 10, height: 10))
                tokens.color("theme-accent").setFill(); handle.fill()
                tokens.color("theme-accent-ink").setStroke(); handle.stroke()
            }
        }
    }
}

final class UnifiedCapturePanel: NSPanel {
    let selector: UnifiedCaptureSelectionView
    override var canBecomeKey: Bool { true }
    override func cancelOperation(_ sender: Any?) { selector.cancel() }

    init(screen: NSScreen, image: CGImage?, targets: [WindowSelectionTarget], tokens: Tokens,
         autoStart: Bool, hitTest: @escaping UnifiedCaptureSelectionView.HitTest,
         displayTitles: [String], selectedDisplay: Int,
         confirm: @escaping (WindowSelectionChoice) -> Void, cancel: @escaping () -> Void,
         changeDisplay: @escaping (Int) -> Void) {
        selector = UnifiedCaptureSelectionView(frame: NSRect(origin: .zero, size: screen.frame.size),
            image: image, targets: targets, tokens: tokens, autoStart: autoStart,
            hitTest: hitTest, displayTitles: displayTitles, selectedDisplay: selectedDisplay,
            confirm: confirm, cancel: cancel, changeDisplay: changeDisplay)
        super.init(contentRect: screen.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        title = "Captures Capture Controls"
        isReleasedWhenClosed = false; isOpaque = false; backgroundColor = .clear; hasShadow = false
        level = .screenSaver; collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        sharingType = .none; acceptsMouseMovedEvents = true; contentView = selector
        makeFirstResponder(selector)
    }
}
