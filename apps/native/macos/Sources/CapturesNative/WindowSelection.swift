import AppKit
import CCapturesSettings

struct WindowSelectionDisplay {
    let id: String
    let name: String
    let x: Int
    let y: Int
    let width: Int
    let height: Int

    init?(_ value: [String: Any]) {
        guard let id = value["id"] as? String,
              let name = value["name"] as? String,
              let x = value["x"] as? NSNumber,
              let y = value["y"] as? NSNumber,
              let width = value["width"] as? NSNumber,
              let height = value["height"] as? NSNumber,
              width.intValue > 0, height.intValue > 0
        else { return nil }
        self.id = id; self.name = name
        self.x = x.intValue; self.y = y.intValue
        self.width = width.intValue; self.height = height.intValue
    }

    var size: CGSize { CGSize(width: CGFloat(width), height: CGFloat(height)) }
}

struct WindowSelectionTarget: Equatable {
    let id: String
    let title: String
    let appName: String?
    let rect: NSRect
    let cornerRadius: CGFloat

    init?(_ value: [String: Any], display: WindowSelectionDisplay,
          fallbackCornerRadius: CGFloat = 0) {
        guard let id = value["id"] as? String,
              let title = value["title"] as? String,
              let x = value["x"] as? NSNumber,
              let y = value["y"] as? NSNumber,
              let width = value["width"] as? NSNumber,
              let height = value["height"] as? NSNumber,
              width.intValue > 0, height.intValue > 0
        else { return nil }
        self.id = id; self.title = title
        appName = value["app_name"] as? String
        rect = NSRect(x: CGFloat(x.intValue - display.x), y: CGFloat(y.intValue - display.y),
            width: CGFloat(width.intValue), height: CGFloat(height.intValue))
        cornerRadius = CGFloat(max(0,
            (value["corner_radius"] as? NSNumber)?.doubleValue ?? Double(fallbackCornerRadius)))
    }

    init(id: String, title: String, appName: String?, rect: NSRect, cornerRadius: CGFloat) {
        self.id = id; self.title = title; self.appName = appName
        self.rect = rect; self.cornerRadius = cornerRadius
    }

    var name: String {
        let app = appName?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let window = title.trimmingCharacters(in: .whitespacesAndNewlines)
        if !app.isEmpty && !window.isEmpty { return "\(app) — \(window)" }
        if !window.isEmpty { return window }
        if !app.isEmpty { return app }
        return "Window"
    }
}

enum WindowSelectionChoice: Equatable {
    case display
    case region(CapturesSelectionRect)
    case window(index: Int, id: String)

    static func == (lhs: WindowSelectionChoice, rhs: WindowSelectionChoice) -> Bool {
        switch (lhs, rhs) {
        case (.display, .display): return true
        case (.region(let lhs), .region(let rhs)):
            return lhs.x == rhs.x && lhs.y == rhs.y
                && lhs.width == rhs.width && lhs.height == rhs.height
        case (.window(let lhsIndex, let lhsID), .window(let rhsIndex, let rhsID)):
            return lhsIndex == rhsIndex && lhsID == rhsID
        default: return false
        }
    }
}

private final class WindowSelectionCanvas: NSView {
    weak var selector: WindowSelectionView?
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) { selector?.drawSelection() }
    override func mouseMoved(with event: NSEvent) { selector?.mouseMoved(with: event) }
    override func mouseDown(with event: NSEvent) { selector?.mouseDown(with: event) }
}

/// AppKit owns presentation and input only. Rust resolves every point against
/// the prepared z-ordered targets and returns either a window index or display.
final class WindowSelectionView: NSView {
    typealias HitTest = (CapturesSelectionPoint) -> Int64?

    private let tokens: Tokens
    private let autoStart: Bool
    private let targets: [WindowSelectionTarget]
    private let hitTest: HitTest
    private let canvas = WindowSelectionCanvas()
    private let toolbar = NSView()
    private let targetName = NSTextField(labelWithString: "")
    private let hint: NSTextField
    private var captureButton: CaptureButton!
    private(set) var hoveredIndex: Int64 = -1
    private(set) var selectedIndex: Int64?
    var confirm: (WindowSelectionChoice) -> Void
    var cancel: () -> Void
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }

    init(frame: NSRect, image: CGImage?, targets: [WindowSelectionTarget], tokens: Tokens,
         autoStart: Bool, hitTest: @escaping HitTest,
         confirm: @escaping (WindowSelectionChoice) -> Void, cancel: @escaping () -> Void) {
        self.tokens = tokens; self.autoStart = autoStart; self.targets = targets
        self.hitTest = hitTest; self.confirm = confirm; self.cancel = cancel
        hint = NSTextField(labelWithString: CaptureGuidanceCopy.directHint(
            CaptureGuidanceCopy.windowTitle, CaptureGuidanceCopy.hint, confirm: !autoStart))
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = NSColor.clear.cgColor
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

        let gap = tokens.number("s-4"), controlHeight = tokens.number("h-lg")
        let toolbarWidth = min(frame.width - gap * 4, 620)
        toolbar.frame = NSRect(x: (frame.width - toolbarWidth) / 2,
            y: frame.height - controlHeight - gap * 4,
            width: toolbarWidth, height: controlHeight + gap * 2)
        toolbar.wantsLayer = true; toolbar.layer?.backgroundColor = tokens.color("glass-strong").cgColor
        toolbar.layer?.cornerRadius = tokens.number("r-xl")
        toolbar.layer?.borderWidth = 1; toolbar.layer?.borderColor = tokens.color("glass-border").cgColor
        addSubview(toolbar)

        targetName.frame = NSRect(x: gap, y: gap, width: toolbarWidth - 232, height: controlHeight)
        targetName.font = .systemFont(ofSize: tokens.number("text-md"), weight: .medium)
        targetName.textColor = tokens.color("glass-text"); targetName.lineBreakMode = .byTruncatingMiddle
        toolbar.addSubview(targetName)
        captureButton = CaptureButton("Capture", frame: NSRect(x: toolbarWidth - 216, y: gap,
            width: 116, height: controlHeight), tokens: tokens, glass: true) { [weak self] in self?.confirmSelection() }
        captureButton.selected = true; captureButton.keyEquivalent = "\r"; captureButton.keyEquivalentModifierMask = []
        toolbar.addSubview(captureButton)
        let close = CaptureButton("Cancel", frame: NSRect(x: toolbarWidth - 92, y: gap,
            width: 80, height: controlHeight), tokens: tokens, glass: true) { [weak self] in self?.cancel() }
        close.keyEquivalent = "\u{1b}"; close.keyEquivalentModifierMask = []
        toolbar.addSubview(close)

        hint.font = .systemFont(ofSize: tokens.number("text-md")); hint.textColor = tokens.color("glass-text")
        hint.alignment = .center; hint.wantsLayer = true
        hint.layer?.backgroundColor = tokens.color("glass-strong").cgColor
        hint.layer?.cornerRadius = tokens.number("r-sm")
        hint.frame = NSRect(x: toolbar.frame.minX, y: toolbar.frame.minY - controlHeight - gap,
            width: toolbarWidth, height: controlHeight)
        addSubview(hint)
        setAccessibilityRole(.group); setAccessibilityLabel("Capture window selector")
        update()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    var choice: WindowSelectionChoice? {
        guard let selectedIndex else { return nil }
        return choice(for: selectedIndex)
    }

    var activeChoice: WindowSelectionChoice {
        choice(for: hoveredIndex)
    }

    private func choice(for index: Int64) -> WindowSelectionChoice {
        guard index >= 0, index < Int64(targets.count) else { return .display }
        let index = Int(index)
        return .window(index: index, id: targets[index].id)
    }

    @discardableResult func hover(_ point: NSPoint) -> Bool {
        guard point.x.isFinite, point.y.isFinite,
              let index = hitTest(CapturesSelectionPoint(x: point.x, y: point.y)),
              index == -1 || (index >= 0 && index < Int64(targets.count))
        else { return false }
        hoveredIndex = index; update()
        return true
    }

    func select(_ point: NSPoint) {
        guard hover(point) else { return }
        selectedIndex = hoveredIndex; update()
        if autoStart { confirmSelection() }
    }

    func confirmSelection() {
        guard let choice else { return }
        confirm(choice)
    }

    override func mouseMoved(with event: NSEvent) {
        hover(convert(event.locationInWindow, from: nil))
    }
    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        select(convert(event.locationInWindow, from: nil))
    }
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { cancel() }
        else if event.keyCode == 36 || event.keyCode == 76 { confirmSelection() }
        else { super.keyDown(with: event) }
    }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .crosshair) }

    private func update() {
        targetName.stringValue = choiceName
        targetName.setAccessibilityLabel("Target: \(choiceName)")
        setAccessibilityValue(choiceName)
        canvas.needsDisplay = true
        captureButton.isEnabled = choice != nil; captureButton.needsDisplay = true
    }

    private var choiceName: String {
        guard case .window(let index, _) = activeChoice else { return "Entire display" }
        return targets[index].name
    }

    fileprivate func drawSelection() {
        let veil = NSBezierPath(rect: bounds)
        if case .window(let index, _) = activeChoice {
            let target = targets[index]
            veil.append(NSBezierPath(roundedRect: target.rect,
                xRadius: target.cornerRadius, yRadius: target.cornerRadius))
        }
        veil.windingRule = .evenOdd; tokens.color("glass-veil").setFill(); veil.fill()

        let border: NSBezierPath
        if case .window(let index, _) = activeChoice {
            let target = targets[index]
            border = NSBezierPath(roundedRect: target.rect,
                xRadius: target.cornerRadius, yRadius: target.cornerRadius)
        } else {
            border = NSBezierPath(rect: bounds.insetBy(dx: 2, dy: 2))
        }
        tokens.color("theme-accent").setStroke()
        border.lineWidth = tokens.number("s-1") * 0.75; border.stroke()
    }
}

final class WindowSelectionPanel: NSPanel {
    let selector: WindowSelectionView
    override var canBecomeKey: Bool { true }

    init(screen: NSScreen, image: CGImage?, targets: [WindowSelectionTarget], tokens: Tokens,
         autoStart: Bool, hitTest: @escaping WindowSelectionView.HitTest,
         confirm: @escaping (WindowSelectionChoice) -> Void, cancel: @escaping () -> Void) {
        selector = WindowSelectionView(frame: NSRect(origin: .zero, size: screen.frame.size), image: image,
            targets: targets, tokens: tokens, autoStart: autoStart, hitTest: hitTest,
            confirm: confirm, cancel: cancel)
        super.init(contentRect: screen.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        isReleasedWhenClosed = false; isOpaque = false; backgroundColor = .clear; hasShadow = false
        level = .screenSaver; collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        sharingType = .none; acceptsMouseMovedEvents = true; contentView = selector
        makeFirstResponder(selector)
    }

    func updatePointerLocation() {
        let windowPoint = convertPoint(fromScreen: NSEvent.mouseLocation)
        selector.hover(selector.convert(windowPoint, from: nil))
    }
}
