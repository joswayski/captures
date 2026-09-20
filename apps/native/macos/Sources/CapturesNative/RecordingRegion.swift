import AppKit

/// Passive guide with a genuinely transparent recording hole. Paint only
/// outside it, even when the platform cannot exclude overlay windows.
final class RecordingRegionView: NSView {
    let region: NSRect
    private let tokens: Tokens
    override var isFlipped: Bool { true }

    init(frame: NSRect, region: NSRect, tokens: Tokens) {
        self.region = region; self.tokens = tokens
        super.init(frame: frame)
        setAccessibilityElement(false)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func draw(_ dirtyRect: NSRect) {
        // Outward backing-pixel rounding protects the hole at fractional scale.
        let hole = convertFromBacking(convertToBacking(region).integral).intersection(bounds)
        NSGraphicsContext.saveGraphicsState()
        defer { NSGraphicsContext.restoreGraphicsState() }
        NSGraphicsContext.current?.shouldAntialias = false
        tokens.color("glass-veil").setFill()
        for rect in Self.outside(bounds, hole: hole) { rect.fill() }
        tokens.color("theme-accent").setFill()
        let border = hole.insetBy(dx: -tokens.number("s-1"), dy: -tokens.number("s-1"))
            .intersection(bounds)
        for rect in Self.outside(border, hole: hole) { rect.fill() }
    }

    private static func outside(_ bounds: NSRect, hole: NSRect) -> [NSRect] {
        [
            NSRect(x: bounds.minX, y: bounds.minY, width: bounds.width, height: hole.minY - bounds.minY),
            NSRect(x: bounds.minX, y: hole.maxY, width: bounds.width, height: bounds.maxY - hole.maxY),
            NSRect(x: bounds.minX, y: hole.minY, width: hole.minX - bounds.minX, height: hole.height),
            NSRect(x: hole.maxX, y: hole.minY, width: bounds.maxX - hole.maxX, height: hole.height),
        ]
    }
}

final class RecordingRegionPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    init(screen: NSScreen, region: NSRect, tokens: Tokens) {
        super.init(contentRect: screen.frame, styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered, defer: false)
        title = "Captures Recording Region"
        isReleasedWhenClosed = false; isOpaque = false; backgroundColor = .clear
        hasShadow = false; ignoresMouseEvents = true
        level = .floating; collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        sharingType = .none
        contentView = RecordingRegionView(frame: NSRect(origin: .zero, size: screen.frame.size),
            region: region, tokens: tokens)
    }
}
