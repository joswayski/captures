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

    /// Shipping `.window-target span`: the window title, else the app, else "Window".
    var chipTitle: String {
        let window = title.trimmingCharacters(in: .whitespacesAndNewlines)
        if !window.isEmpty { return window }
        let app = appName?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return app.isEmpty ? "Window" : app
    }
}

/// Visible display corner radius, mirroring `captures-macos-window`: the
/// screen's bezel outline first, then the legacy private radius. Each private
/// selector is used only when NSScreen responds to it.
enum DisplayCornerRadius {
    static func points(for screen: NSScreen) -> CGFloat {
        let bezel = NSSelectorFromString("bezelPath")
        if screen.responds(to: bezel),
           let path = screen.perform(bezel)?.takeUnretainedValue() as? NSBezierPath {
            let bezelRadius = halfPoints(radius(outline: outlinePoints(path), frame: screen.frame))
            if bezelRadius > 0 { return bezelRadius }
        }
        if let name = ["_displayCornerRadius", "_cornerRadius"].first(where: {
               screen.responds(to: NSSelectorFromString($0)) }),
           let value = screen.value(forKey: name) as? NSNumber {
            return halfPoints(CGFloat(value.doubleValue))
        }
        return 0
    }

    static func halfPoints(_ value: CGFloat) -> CGFloat {
        value.isFinite && value > 0 ? (value * 2).rounded() / 2 : 0
    }

    /// On-path points only; control points are ignored so squircles are not
    /// mistaken for a smaller radius.
    static func outlinePoints(_ path: NSBezierPath) -> [NSPoint] {
        var result: [NSPoint] = []
        var associated = [NSPoint](repeating: .zero, count: 3)
        for index in 0..<path.elementCount {
            let kind = associated.withUnsafeMutableBufferPointer {
                path.element(at: index, associatedPoints: $0.baseAddress)
            }
            switch kind.rawValue {
            case 0, 1: result.append(associated[0]) // move, line
            case 2: result.append(associated[2]) // cubic curve
            case 4: result.append(associated[1]) // quadratic curve
            default: break
            }
        }
        return result
    }

    /// How far the outline's axis-aligned spines stop short of its bounds.
    static func radius(outline points: [NSPoint], frame: NSRect) -> CGFloat {
        guard let minX = points.map(\.x).min(), let maxX = points.map(\.x).max(),
              let minY = points.map(\.y).min(), let maxY = points.map(\.y).max() else { return 0 }
        let spine: CGFloat = 0.5
        let left = points.filter { $0.x <= minX + spine }.map(\.y)
        let right = points.filter { $0.x >= maxX - spine }.map(\.y)
        let top = points.filter { $0.y >= maxY - spine }.map(\.x)
        let bottom = points.filter { $0.y <= minY + spine }.map(\.x)
        let insets: [CGFloat] = [
            maxY - (left.max() ?? maxY), (left.min() ?? minY) - minY,
            maxY - (right.max() ?? maxY), (right.min() ?? minY) - minY,
            (top.min() ?? minX) - minX, maxX - (top.max() ?? maxX),
            (bottom.min() ?? minX) - minX, maxX - (bottom.max() ?? maxX),
        ]
        let longest = insets.filter(\.isFinite).max() ?? 0
        let allowed = min(frame.width, frame.height) / 2
        return longest > 0 && allowed.isFinite ? min(longest, allowed) : 0
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
    private let displayCornerRadius: CGFloat
    private let hitTest: HitTest
    private let canvas = WindowSelectionCanvas()
    private let toolbar = NSView()
    private let targetName = NSTextField(labelWithString: "")
    private let hint: NSTextField
    private var captureButton: CaptureButton!
    private(set) var hoveredIndex: Int64 = -1
    /// Shipping leaves the screen clear until the pointer resolves a target.
    private(set) var hasHoverTarget = false
    private(set) var selectedIndex: Int64?
    var confirm: (WindowSelectionChoice) -> Void
    var cancel: () -> Void
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }

    init(frame: NSRect, image: CGImage?, targets: [WindowSelectionTarget], tokens: Tokens,
         autoStart: Bool, displayCornerRadius: CGFloat = 0, hitTest: @escaping HitTest,
         confirm: @escaping (WindowSelectionChoice) -> Void, cancel: @escaping () -> Void) {
        self.tokens = tokens; self.autoStart = autoStart; self.targets = targets
        self.displayCornerRadius = max(0, displayCornerRadius)
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
        // The shipping direct overlay has no toolbar: a click commits.
        toolbar.isHidden = autoStart

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
        hoveredIndex = index; hasHoverTarget = true; update()
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

    /// The glass chip shipping shows for the hovered target, if any.
    var hoverChipText: String? {
        guard hasHoverTarget else { return nil }
        guard case .window(let index, _) = activeChoice else { return "Entire display" }
        return targets[index].chipTitle
    }

    fileprivate func drawSelection() {
        guard hasHoverTarget else { return }
        let accent = tokens.color("theme-accent")
        if case .window(let index, _) = activeChoice {
            let target = targets[index]
            let radius = min(target.cornerRadius, target.rect.width / 2, target.rect.height / 2)
            let hole = NSBezierPath(roundedRect: target.rect, xRadius: radius, yRadius: radius)
            let veil = NSBezierPath(rect: bounds)
            veil.append(hole)
            veil.windingRule = .evenOdd; tokens.color("capture-shade-window").setFill(); veil.fill()
            accent.withAlphaComponent(0.14).setFill(); hole.fill()
            // An outer 2 pt ring, so it never covers the window's own edge.
            let ring = NSBezierPath(roundedRect: target.rect.insetBy(dx: -1, dy: -1),
                xRadius: radius + 1, yRadius: radius + 1)
            ring.lineWidth = 2; accent.setStroke(); ring.stroke()
            NSGraphicsContext.saveGraphicsState()
            hole.addClip()
            let margin = tokens.number("s-2")
            drawChip(target.chipTitle, at: NSPoint(x: target.rect.minX + margin, y: target.rect.minY + margin),
                maxWidth: target.rect.width - 8)
            NSGraphicsContext.restoreGraphicsState()
        } else {
            // `.capture-display-outline`: inset 2 pt ring following the display corners.
            let radius = max(0, displayCornerRadius - 1)
            let outline = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1), xRadius: radius, yRadius: radius)
            outline.lineWidth = 2; accent.setStroke(); outline.stroke()
            let inset = tokens.number("s-4")
            drawChip("Entire display", at: NSPoint(x: inset, y: inset), maxWidth: min(360, bounds.width - 8))
        }
    }

    /// Shipping glass chip: one ellipsized `--text-xs` medium line.
    private func drawChip(_ text: String, at origin: NSPoint, maxWidth: CGFloat) {
        let style = NSMutableParagraphStyle(); style.lineBreakMode = .byTruncatingTail
        let string = NSAttributedString(string: text, attributes: [
            .font: NSFont.systemFont(ofSize: tokens.number("text-xs"), weight: .medium),
            .foregroundColor: tokens.color("glass-text"), .paragraphStyle: style,
        ])
        let padX = tokens.number("s-3"), padY = tokens.number("s-2")
        let size = string.size()
        let textWidth = max(0, min(ceil(size.width), maxWidth - padX * 2))
        let chip = NSRect(x: origin.x, y: origin.y, width: textWidth + padX * 2, height: ceil(size.height) + padY * 2)
        tokens.color("glass-strong").setFill()
        NSBezierPath(roundedRect: chip, xRadius: tokens.number("r-sm"), yRadius: tokens.number("r-sm")).fill()
        string.draw(with: NSRect(x: chip.minX + padX, y: chip.minY + padY, width: textWidth, height: ceil(size.height)),
            options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
    }
}

final class WindowSelectionPanel: NSPanel {
    let selector: WindowSelectionView
    override var canBecomeKey: Bool { true }

    init(screen: NSScreen, image: CGImage?, targets: [WindowSelectionTarget], tokens: Tokens,
         autoStart: Bool, hitTest: @escaping WindowSelectionView.HitTest,
         confirm: @escaping (WindowSelectionChoice) -> Void, cancel: @escaping () -> Void) {
        selector = WindowSelectionView(frame: NSRect(origin: .zero, size: screen.frame.size), image: image,
            targets: targets, tokens: tokens, autoStart: autoStart,
            displayCornerRadius: DisplayCornerRadius.points(for: screen), hitTest: hitTest,
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
