import AppKit
import CoreImage
import ImageIO
import QuartzCore
import CCapturesSettings

struct MiniPreviewSettings: Equatable {
    let enabled: Bool
    let placement: String
    let includeInCaptures: Bool
}

private final class MiniPreviewImageView: NSView {
    let image: NSImage
    init(frame: NSRect, image: NSImage) { self.image = image; super.init(frame: frame) }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func draw(_ dirtyRect: NSRect) {
        let scale = max(bounds.width / image.size.width, bounds.height / image.size.height)
        let size = NSSize(width: image.size.width * scale, height: image.size.height * scale)
        let destination = NSRect(x: (bounds.width - size.width) / 2,
                                 y: (bounds.height - size.height) / 2,
                                 width: size.width, height: size.height)
        image.draw(in: destination, from: .zero, operation: .sourceOver, fraction: 1,
                   respectFlipped: true, hints: [.interpolation: NSImageInterpolation.high])
    }
}

enum MiniPreviewButtonKind: CaseIterable {
    case close, trash, edit, copy, save, folder, collapse, clear, check

    var iconName: String {
        switch self {
        case .close, .clear: return "close"
        case .trash: return "trash"
        case .edit: return "edit"
        case .copy: return "copy"
        case .save: return "save"
        case .folder: return "folder"
        case .collapse: return "preview-stack"
        case .check: return "check"
        }
    }
}

/// Preview-only control so this floating chrome does not inherit Workbench button styling.
final class MiniPreviewButton: NSButton {
    /// A main action's glyph pops in when it changes (Save → Saved → Show in
    /// Folder), as shipping remounts its SVG.
    var kind: MiniPreviewButtonKind {
        didSet { if popsIcon && kind != oldValue { popIcon() } }
    }
    /// Shipping `.thumbnail-main-actions svg`: Copy and Save pop new glyphs.
    var popsIcon = false
    private let tokens: Tokens
    private let primary: Bool
    private var tracking: NSTrackingArea?
    private var hovered = false
    private var focused = false
    private let actionBlock: () -> Void
    /// Shipping Minimize: on hover/focus the icon gives way to this label and
    /// the control widens inward from the pile's screen edge.
    var hoverLabel: String? { didSet { applyHoverShape() } }
    /// `STACK_MINIMIZE_HOVER_WIDTH` (`.thumbnail-stack-minimize:hover`).
    var hoverWidth: CGFloat = 92
    /// Keep the trailing edge fixed (right-anchored piles grow leftward).
    var growsFromTrailingEdge = false
    private var restFrame: NSRect?
    var showsHoverLabel: Bool { hoverLabel != nil && (hovered || focused) }
    /// Show less morph: 0 is the 28 pt stack icon, 1 the hover pill. The width
    /// follows the 240 ms morph, the icon/label crossfade the 180 ms swap.
    private(set) var morphWidth: Double = 0
    private(set) var morphSwap: Double = 0
    private var morphAnimation: (start: CFTimeInterval, width: Double, swap: Double, target: Double)?
    private var iconPopStart: CFTimeInterval?
    private(set) var iconHover: Double = 0
    private var iconHoverAnimation: (start: CFTimeInterval, from: Double, target: Double)?
    /// Drives self-drawn morph, icon pop and hover colors while they run.
    private var ticker: Timer?
    /// Shipping instant glass tip (`data-tooltip`). Labelled actions have none;
    /// the system tooltip is never used.
    var tooltipText: String?
    /// Reports hover/focus changes so the preview can show the glass tip.
    var tooltipChanged: ((MiniPreviewButton, Bool) -> Void)?
    var showsTooltip: Bool { tooltipText != nil && !isHidden && (hovered || focused) }
    /// Shared editor presence (`CAPTURES_EDITOR_PHASE_*`) of an Edit control.
    private(set) var editorPhase = UInt32(CAPTURES_EDITOR_PHASE_IDLE)
    /// `data-editor-just-opened`: the click that opened the editor keeps the
    /// pill passive ("In editor") until the pointer leaves it.
    private(set) var editorJustOpened = false
    private var editorRestFrame: NSRect?
    var editorPresent: Bool { editorPhase == UInt32(CAPTURES_EDITOR_PHASE_PRESENT) }
    /// The pill's current label, for tests and accessibility checks.
    var editorLabel: String {
        String(cString: captures_preview_editor_label_v1(editorPhase, hovered || focused, editorJustOpened))
    }

    init(_ title: String, kind: MiniPreviewButtonKind, frame: NSRect, tokens: Tokens,
         primary: Bool = false, action: @escaping () -> Void) {
        self.kind = kind; self.tokens = tokens; self.primary = primary; self.actionBlock = action
        super.init(frame: frame)
        self.title = title; isBordered = false; setButtonType(.momentaryPushIn)
        target = self; self.action = #selector(activate)
        setAccessibilityLabel(title); toolTip = nil
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    deinit { ticker?.invalidate() }

    /// The CSS `box-shadow` token under this control: `shadow-sm` on
    /// `.icon-button`s, `shadow-md` on the main actions and
    /// `thumbnail-card-shadow` on the stack toolbar.
    var boxShadowToken: String? { didSet { refreshBoxShadow() } }
    var boxShadowLayerCount: Int { layer?.sublayers?.filter { $0.name == BoxShadowLayers.layerName }.count ?? 0 }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        refreshBoxShadow()
    }

    /// Rebuild the shadow for the current size. The present editor pill keeps
    /// its own accent glow (the button layer's shadow) instead.
    private func refreshBoxShadow() {
        guard let token = boxShadowToken, !(kind == .edit && editorPresent) else {
            BoxShadowLayers.remove(from: layer)
            return
        }
        wantsLayer = true
        guard let layer else { return }
        BoxShadowLayers.install(tokens.shadow(token), on: layer, bounds: bounds, radius: tokens.number("r-md"))
    }

    @objc private func activate() {
        if kind == .edit { editorJustOpened = true; needsDisplay = true }
        actionBlock()
    }

    /// Shipping `thumbnail-action-pop`: the glyph scales and fades in.
    func popIcon() {
        guard window?.isVisible == true, !NativeMotion.reduceMotion else { return }
        iconPopStart = CACurrentMediaTime()
        startTicker(); needsDisplay = true
    }

    private func startTicker() {
        guard ticker == nil else { return }
        let timer = Timer(timeInterval: 1.0 / 60, repeats: true) { [weak self] _ in self?.tick() }
        RunLoop.main.add(timer, forMode: .common); ticker = timer
    }

    /// Advance self-drawn motion; the ticker stops once all transitions rest.
    private func tick() {
        let now = CACurrentMediaTime()
        var running = false
        if let hover = iconHoverAnimation {
            let spec = NativeMotion.transition("preview_icon_hover", tokens: tokens)
            let elapsed = now - hover.start
            if spec.duration > 0, elapsed < spec.duration {
                iconHover = hover.from + (hover.target - hover.from) * NativeMotion.ease(spec.timing, elapsed / spec.duration)
                running = true
            } else {
                iconHover = hover.target; iconHoverAnimation = nil
            }
            needsDisplay = true
        }
        if let morph = morphAnimation {
            let elapsed = now - morph.start
            func value(_ name: String, from: Double) -> Double {
                let spec = NativeMotion.transition(name, tokens: tokens)
                guard spec.duration > 0, elapsed < spec.duration else { return morph.target }
                return from + (morph.target - from) * NativeMotion.ease(spec.timing, elapsed / spec.duration)
            }
            morphWidth = value("preview_minimize_morph", from: morph.width)
            morphSwap = value("preview_minimize_swap", from: morph.swap)
            if morphWidth == morph.target && morphSwap == morph.target {
                morphAnimation = nil
            } else {
                running = true
            }
            layoutMorph()
        }
        if let start = iconPopStart {
            if now - start < NativeMotion.duration("preview_action_icon_pop", tokens: tokens) {
                running = true
            } else {
                iconPopStart = nil
            }
            needsDisplay = true
        }
        if !running { ticker?.invalidate(); ticker = nil }
    }
    override var isHidden: Bool {
        didSet {
            guard isHidden, !oldValue else { return }
            // Hidden chrome cannot stay hovered or keep its tip on screen.
            hovered = false; editorJustOpened = false
            iconHover = 0; iconHoverAnimation = nil
            tooltipChanged?(self, false)
        }
    }

    /// Shipping `.thumbnail-editor-control`: the compact Edit icon, or while an
    /// editor shows this capture the "In editor" pill, widening away from its
    /// corner over the shipping morph. Leaving ignores clicks.
    func setEditorPhase(_ phase: UInt32, animated: Bool) {
        guard kind == .edit else { return }
        editorPhase = phase
        let rest = editorRestFrame ?? frame
        editorRestFrame = rest
        let width = editorPresent ? pillWidth() : rest.width
        let next = NSRect(x: growsFromTrailingEdge ? rest.maxX - width : rest.minX,
                          y: rest.minY, width: width, height: rest.height)
        let idle = phase == UInt32(CAPTURES_EDITOR_PHASE_IDLE)
        let lingering = phase == UInt32(CAPTURES_EDITOR_PHASE_LINGERING)
        tooltipText = idle || lingering ? "Edit" : nil
        isEnabled = phase != UInt32(CAPTURES_EDITOR_PHASE_LEAVING)
        setAccessibilityLabel(String(cString: captures_preview_editor_label_v1(phase, true, false)))
        // `0 0 14px rgba(accent, .2)` glow around the present pill.
        wantsLayer = true
        layer?.shadowColor = tokens.color("theme-accent").cgColor
        layer?.shadowRadius = 7
        layer?.shadowOffset = .zero
        layer?.shadowOpacity = editorPresent ? 0.2 : 0
        refreshBoxShadow()
        let morph = NativeMotion.transition("preview_editor_morph", tokens: tokens)
        if animated, window?.isVisible == true, morph.duration > 0, frame != next {
            NSAnimationContext.runAnimationGroup { context in
                context.duration = morph.duration
                context.timingFunction = morph.timing
                animator().frame = next
            }
        } else if frame != next {
            frame = next
        }
        if showsTooltip { tooltipChanged?(self, true) } else { tooltipChanged?(self, false) }
        needsDisplay = true
    }

    private var pillFont: NSFont { .systemFont(ofSize: tokens.number("text-2xs"), weight: .semibold) }

    private func pillWidth() -> CGFloat {
        func measure(_ text: String) -> Double {
            Double(NSAttributedString(string: text, attributes: [.font: pillFont]).size().width)
        }
        return CGFloat(captures_preview_editor_pill_width_v1(measure("In editor"),
                                                            measure("Show in editor")))
    }
    override func updateTrackingAreas() {
        if let tracking { removeTrackingArea(tracking) }
        tracking = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                                  owner: self, userInfo: nil)
        addTrackingArea(tracking!); super.updateTrackingAreas()
    }
    override func mouseEntered(with event: NSEvent) {
        hovered = true; applyIconHover(); applyHoverShape(); needsDisplay = true
        tooltipChanged?(self, showsTooltip)
    }
    override func mouseExited(with event: NSEvent) {
        hovered = false; editorJustOpened = false; applyIconHover(); applyHoverShape(); needsDisplay = true
        tooltipChanged?(self, showsTooltip)
    }
    private func applyIconHover() {
        guard boxShadowToken == "shadow-sm" else { return }
        let target: Double = hovered && isEnabled && !isHidden ? 1 : 0
        let spec = NativeMotion.transition("preview_icon_hover", tokens: tokens)
        if window?.isVisible == true, spec.duration > 0, iconHover != target {
            iconHoverAnimation = (CACurrentMediaTime(), iconHover, target)
            startTicker()
        } else {
            iconHover = target; iconHoverAnimation = nil
        }
    }

    /// Shipping `.icon-button.delete:hover` uses white on the signal fill,
    /// not the ordinary raised-glass hover used by Close and Edit.
    var chromeBackground: NSColor {
        if primary { return tokens.color("theme-accent") }
        if boxShadowToken == "shadow-sm" {
            let rest = tokens.color("glass-strong")
            return rest.blended(withFraction: isEnabled ? iconHover : 0,
                                of: tokens.color(kind == .trash ? "theme-signal" : "glass-raised")) ?? rest
        }
        return tokens.color(hovered || cell?.isHighlighted == true ? "glass-raised" : "glass-strong")
    }
    var chromeForeground: NSColor {
        if primary { return tokens.color("theme-accent-ink") }
        if kind == .trash {
            let rest = tokens.color("theme-signal-text")
            return rest.blended(withFraction: isEnabled ? iconHover : 0, of: .white) ?? rest
        }
        return tokens.color("glass-text")
    }
    override func becomeFirstResponder() -> Bool {
        let result = super.becomeFirstResponder()
        if result { focused = true; applyHoverShape(); (superview as? MiniPreviewCardView)?.focusWithinChanged(); tooltipChanged?(self, showsTooltip) }
        needsDisplay = true; return result
    }
    override func resignFirstResponder() -> Bool {
        let result = super.resignFirstResponder()
        if result { focused = false; applyHoverShape(); (superview as? MiniPreviewCardView)?.focusWithinChanged(); tooltipChanged?(self, showsTooltip) }
        needsDisplay = true; return result
    }
    private func applyHoverShape() {
        guard hoverLabel != nil else { return }
        let rest = restFrame ?? frame
        restFrame = rest
        let target: Double = showsHoverLabel ? 1 : 0
        let morph = NativeMotion.transition("preview_minimize_morph", tokens: tokens)
        if window?.isVisible == true, morph.duration > 0, morphWidth != target || morphSwap != target {
            if morphAnimation?.target != target {
                morphAnimation = (CACurrentMediaTime(), morphWidth, morphSwap, target)
            }
            startTicker()
        } else {
            morphAnimation = nil; morphWidth = target; morphSwap = target
        }
        layoutMorph()
    }

    /// Width between the resting icon and the hover pill, growing inward from
    /// the pile's screen edge.
    private func layoutMorph() {
        guard let rest = restFrame else { return }
        let width = rest.width + (max(hoverWidth, rest.width) - rest.width) * CGFloat(morphWidth)
        let next = NSRect(x: growsFromTrailingEdge ? rest.maxX - width : rest.minX,
                          y: rest.minY, width: width, height: rest.height)
        if frame != next { frame = next }
        needsDisplay = true
    }

    /// The morphing Show less control: the stack icon slides toward the pile
    /// edge and fades as the label arrives from it (`--thumbnail-minimize-slide`).
    private func drawMorph(_ label: String) {
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5),
                                xRadius: tokens.number("r-md"), yRadius: tokens.number("r-md"))
        tokens.color("glass-strong").setFill(); path.fill()
        tokens.color("glass-border").setStroke(); path.stroke()
        NSGraphicsContext.saveGraphicsState()
        NSBezierPath(rect: bounds).addClip()
        let slide: CGFloat = growsFromTrailingEdge ? -1 : 1
        let swap = CGFloat(morphSwap)
        let restWidth = restFrame?.width ?? bounds.height
        let restMidX = growsFromTrailingEdge ? bounds.width - restWidth / 2 : restWidth / 2
        if swap < 1 {
            let side = 16 * (1 - 0.2 * swap)
            tokens.color("glass-text").withAlphaComponent(1 - swap).setStroke()
            drawIcon(in: NSRect(x: restMidX - side / 2 - 8 * slide * swap,
                                y: (bounds.height - side) / 2, width: side, height: side))
        }
        if swap > 0 {
            let text = NSAttributedString(string: label, attributes: [
                .font: NSFont.systemFont(ofSize: tokens.number("text-2xs"), weight: .semibold),
                .foregroundColor: tokens.color("glass-text").withAlphaComponent(swap)])
            let size = text.size()
            text.draw(at: NSPoint(x: (bounds.width - size.width) / 2 + 4 * slide * (1 - swap),
                                  y: (bounds.height - size.height) / 2))
        }
        NSGraphicsContext.restoreGraphicsState()
        if focused {
            tokens.color("theme-accent").setStroke()
            let focus = NSBezierPath(roundedRect: bounds.insetBy(dx: 2, dy: 2), xRadius: 5, yRadius: 5)
            focus.lineWidth = 2; focus.stroke()
        }
    }
    override func draw(_ dirtyRect: NSRect) {
        if kind == .edit && editorPresent { drawEditorPill(); return }
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5),
                                xRadius: tokens.number("r-md"), yRadius: tokens.number("r-md"))
        if let hoverLabel, morphWidth > 0 || morphSwap > 0 {
            drawMorph(hoverLabel)
            return
        }
        chromeBackground.setFill()
        path.fill(); (primary ? NSColor.clear : tokens.color("glass-border")).setStroke(); path.stroke()
        let color = chromeForeground
        color.setStroke(); color.setFill()
        var iconX: CGFloat = 6
        if !title.isEmpty && bounds.width > 40 {
            let text = NSAttributedString(string: title, attributes: [.font: NSFont.systemFont(ofSize: tokens.number("text-xs"), weight: .semibold), .foregroundColor: color])
            let size = text.size()
            let gap = tokens.number("s-3")
            iconX = (bounds.width - size.width - gap - 16) / 2
            text.draw(at: NSPoint(x: iconX + 16 + gap, y: (bounds.height - size.height) / 2))
        }
        // A new main-action glyph pops in from 65 % at a quarter opacity.
        let pop = iconPopStart.flatMap {
            NativeMotion.pose("preview_action_icon_pop", at: CACurrentMediaTime() - $0, tokens: tokens)
        }
        let side = 16 * CGFloat(pop?.scale ?? 1)
        color.withAlphaComponent(CGFloat(pop?.opacity ?? 1)).setStroke()
        drawIcon(in: NSRect(x: iconX + (16 - side) / 2, y: (bounds.height - side) / 2,
                            width: side, height: side))
        if window?.firstResponder === self {
            tokens.color("theme-accent").setStroke(); let focus = NSBezierPath(roundedRect: bounds.insetBy(dx: 2, dy: 2), xRadius: 5, yRadius: 5); focus.lineWidth = 2; focus.stroke()
        }
    }
    /// `.thumbnail-editor-control.is-present`: an accent-outlined glass pill
    /// that fills with the accent and offers "Show in editor" on hover/focus.
    private func drawEditorPill() {
        let offersAction = (hovered || focused) && !editorJustOpened
        let radius = bounds.height / 2
        let pill = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5),
                                xRadius: radius - 0.5, yRadius: radius - 0.5)
        let accent = tokens.color("theme-accent")
        (offersAction ? accent : tokens.color("glass-strong")).setFill(); pill.fill()
        if !offersAction {
            accent.withAlphaComponent(0.72).setStroke(); pill.lineWidth = 1; pill.stroke()
        }
        let color = tokens.color(offersAction ? "theme-accent-ink" : "theme-accent-text")
        color.setStroke(); color.setFill()
        // Padding `3px 9px 3px 7px` inside a 1 px border; an 11 pt icon at 2.2 units.
        let icon = NSRect(x: 8, y: (bounds.height - 11) / 2, width: 11, height: 11)
        drawIcon(in: icon, lineWidth: 2.2)
        let text = NSAttributedString(string: editorLabel, attributes: [
            .font: pillFont, .foregroundColor: color])
        NSGraphicsContext.saveGraphicsState()
        NSBezierPath(rect: bounds.insetBy(dx: 1, dy: 1)).addClip()
        text.draw(at: NSPoint(x: icon.maxX + 5, y: (bounds.height - text.size().height) / 2))
        NSGraphicsContext.restoreGraphicsState()
        if window?.firstResponder === self {
            tokens.color("theme-accent").setStroke()
            let focus = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1),
                                     xRadius: radius - 1, yRadius: radius - 1)
            focus.lineWidth = 2; focus.stroke()
        }
    }

    private func drawIcon(in r: NSRect, lineWidth: CGFloat = 1.8) {
        ShippingIcons.stroke(kind.iconName, in: r,
                             width: kind == .collapse ? lineWidth * 1.5 : lineWidth,
                             flipped: isFlipped)
    }
}

/// Shipping `.clipboard-confirmation`: shown while the clipboard still holds
/// this capture, when the Copy action is hidden.
final class MiniPreviewClipboardChip: NSView {
    static let title = "Copied to clipboard"
    private let tokens: Tokens

    init(tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: .zero)
        setAccessibilityElement(true); setAccessibilityRole(.staticText)
        setAccessibilityLabel(Self.title)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    private var text: NSAttributedString {
        NSAttributedString(string: Self.title, attributes: [
            .font: NSFont.systemFont(ofSize: tokens.number("text-2xs"), weight: .semibold),
            .foregroundColor: NSColor(srgbRed: 0xea / 255, green: 1, blue: 0xf0 / 255, alpha: 1)])
    }

    override var intrinsicContentSize: NSSize {
        let size = text.size()
        return NSSize(width: ceil(tokens.number("s-4") * 2 + 12 + tokens.number("s-2") + size.width),
                      height: ceil(max(size.height, 12) + 6))
    }

    override func draw(_ dirtyRect: NSRect) {
        let pill = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5),
                                xRadius: bounds.height / 2, yRadius: bounds.height / 2)
        NSColor(srgbRed: 10 / 255, green: 22 / 255, blue: 15 / 255, alpha: 0.9).setFill(); pill.fill()
        NSColor(srgbRed: 53 / 255, green: 163 / 255, blue: 93 / 255, alpha: 0.55).setStroke()
        pill.lineWidth = 1; pill.stroke()
        let icon = NSRect(x: tokens.number("s-4"), y: (bounds.height - 12) / 2, width: 12, height: 12)
        NSColor(srgbRed: 0x7f / 255, green: 0xd7 / 255, blue: 0x9c / 255, alpha: 1).setStroke()
        ShippingIcons.stroke("check", in: icon, width: 2.4, flipped: isFlipped)
        let label = text
        label.draw(at: NSPoint(x: icon.maxX + tokens.number("s-2"),
                               y: (bounds.height - label.size().height) / 2))
    }
}

final class MiniPreviewCardView: NSView, NSDraggingSource {
    static let savedFeedbackDuration: TimeInterval = 1
    private let tokens: Tokens
    private let mirrored: Bool
    private let imageView: MiniPreviewImageView
    /// `.thumbnail-media`: clips the hover blur and scale to the rounded card.
    private let mediaClip = NSView()
    /// `brightness(.5)` over opaque media, as a black layer at 50%.
    private let mediaDim = NSView()
    /// `.thumbnail-editor-active` ring outside the card edge.
    private let editorRing = CALayer()
    private let depthShade = NSView()
    /// The hovered pile's `0 0 0 1px rgba(accent, .55), 0 0 22px rgba(accent, .28)`.
    private let pileGlow = CALayer()
    /// Rear pile media blur radius (`pose × 1.15px`, `× 0.75px` fanned).
    private(set) var depthBlurRadius: Double = 0
    var pileGlowOpacity: Float { pileGlow.opacity }
    /// `thumbnail-capture-highlight`: the accent outline a new card fades out.
    private let highlightRing = CALayer()
    private let dimensions: NSTextField
    /// `.thumbnail-meta .warning` beside the metadata.
    private let warningLabel = NSTextField(labelWithString: "")
    private let status = NSTextField(labelWithString: "")
    private var actionButtons: [MiniPreviewButton] = []
    private var saveButton: MiniPreviewButton?
    private var copyButton: MiniPreviewButton?
    private var closeButton: MiniPreviewButton?
    private var editButton: MiniPreviewButton?
    private let clipboardChip: MiniPreviewClipboardChip
    /// The clipboard still holds this capture: Copy hides and the chip shows.
    private(set) var clipboardCurrent = false
    /// The last copy of this capture failed ("Clipboard unavailable").
    var copyFailed = false { didSet { updateWarning() } }
    /// The shown warning chip, for tests and accessibility checks.
    /// The warning shipping picks for this card, whether or not hover chrome
    /// currently covers the metadata row it sits in.
    var warningText: String? { warningLabel.stringValue.isEmpty ? nil : warningLabel.stringValue }
    /// Locked while an exit plays: no hover, focus or clicks.
    private(set) var isExiting = false
    /// The capture highlight is still fading.
    var isHighlighting: Bool { highlightRing.animation(forKey: "preview-capture-highlight") != nil }
    private var savedFeedbackActive = false
    private var savedFeedbackToken = 0
    private var highlightToken = 0
    private var saved = false
    private var tracking: NSTrackingArea?
    private var compact = false
    private var chromeVisible = false
    private var pointerInside = false
    /// Shared editor presence phase (`CAPTURES_EDITOR_PHASE_*`).
    private(set) var editorPhase = UInt32(CAPTURES_EDITOR_PHASE_IDLE)
    private var editorPinned: Bool { editorPhase != UInt32(CAPTURES_EDITOR_PHASE_IDLE) }
    /// The stack's stale-pointer lock: hover waits for real pointer movement.
    var isHoverLocked: () -> Bool = { false }
    /// Shipping hover media treatment is applied (blur, brightness, scale).
    private(set) var mediaHovered = false
    /// Target Gaussian radius of the media's Core Image blur filter.
    private(set) var mediaBlurRadius: Double = 0
    var mediaDimOpacity: CGFloat { mediaDim.alphaValue }
    var mediaScale: CGFloat { mediaClip.bounds.width > 0 ? imageView.frame.width / mediaClip.bounds.width : 1 }
    var editorRingOpacity: Float { editorRing.opacity }
    var iconButtons: [MiniPreviewButton] { actionButtons.filter { $0.tooltipText != nil || $0.kind == .edit } }
    var editControl: MiniPreviewButton? { editButton }
    private var press: NSPoint?
    private var fileDragStarted = false
    var preparedDragPath: String?
    var dragEnded: ((NSPoint, NSDragOperation) -> Void)?
    var isOutboundFileDragEnabled: Bool { !compact && preparedDragPath != nil }
    private(set) var artifactID: String
    var hasVisibleLabels: Bool { !dimensions.isHidden || !status.isHidden }
    override var isFlipped: Bool { true }

    init(frame: NSRect, artifactID: String, image: NSImage, tokens: Tokens,
         width: Int, height: Int, sizeBytes: UInt64 = 0, saved: Bool, rightAnchor: Bool,
         copy: @escaping () -> Void, save: @escaping () -> Void,
         open: @escaping () -> Void, trash: @escaping () -> Void,
         dismiss: @escaping () -> Void, discard: @escaping () -> Void) {
        self.artifactID = artifactID; self.tokens = tokens
        self.mirrored = rightAnchor
        self.saved = saved
        imageView = MiniPreviewImageView(frame: frame, image: image)
        dimensions = NSTextField(labelWithString: Self.metadata(width: width, height: height,
                                                                sizeBytes: sizeBytes))
        clipboardChip = MiniPreviewClipboardChip(tokens: tokens)
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = tokens.color("glass-strong").cgColor
        layer?.cornerRadius = tokens.number("thumbnail-card-radius")
        layer?.borderWidth = 1; layer?.borderColor = tokens.color("glass-border").cgColor

        mediaClip.frame = bounds; mediaClip.wantsLayer = true
        mediaClip.layer?.cornerRadius = tokens.number("thumbnail-card-radius")
        mediaClip.layer?.masksToBounds = true
        mediaClip.setAccessibilityElement(false)
        addSubview(mediaClip)
        imageView.frame = mediaClip.bounds
        imageView.wantsLayer = true
        imageView.layerUsesCoreImageFilters = true
        if let blur = CIFilter(name: "CIGaussianBlur") {
            blur.setDefaults(); blur.setValue(0, forKey: kCIInputRadiusKey); blur.name = "blur"
            imageView.layer?.filters = [blur]
        }
        imageView.setAccessibilityLabel("Screenshot thumbnail")
        mediaClip.addSubview(imageView)
        mediaDim.frame = mediaClip.bounds; mediaDim.wantsLayer = true
        mediaDim.layer?.backgroundColor = NSColor.black.cgColor
        mediaDim.alphaValue = 0; mediaDim.setAccessibilityElement(false)
        mediaClip.addSubview(mediaDim)

        depthShade.frame = bounds; depthShade.wantsLayer = true
        depthShade.layer?.cornerRadius = tokens.number("thumbnail-card-radius")
        depthShade.isHidden = true; depthShade.setAccessibilityElement(false)
        addSubview(depthShade)

        let inset: CGFloat = 8
        dimensions.font = .systemFont(ofSize: tokens.number("text-2xs")); dimensions.textColor = tokens.color("glass-text")
        let metadataWidth = ceil(dimensions.attributedStringValue.size().width) + 8
        dimensions.frame = NSRect(x: inset, y: bounds.height - 25,
                                  width: min(metadataWidth, bounds.width - inset * 2), height: 17)
        styleLabelBacking(dimensions); addSubview(dimensions)
        warningLabel.font = .systemFont(ofSize: tokens.number("text-2xs"))
        warningLabel.textColor = tokens.color("theme-accent-text")
        warningLabel.alignment = .center
        styleLabelBacking(warningLabel); warningLabel.isHidden = true; addSubview(warningLabel)
        let chipSize = clipboardChip.intrinsicContentSize
        clipboardChip.frame = NSRect(x: bounds.width - inset - chipSize.width,
                                     y: bounds.height - inset - chipSize.height,
                                     width: chipSize.width, height: chipSize.height)
        clipboardChip.isHidden = true; addSubview(clipboardChip)
        status.frame = NSRect(x: bounds.width - 126, y: bounds.height - 25, width: 118, height: 17)
        status.alignment = .right; status.lineBreakMode = .byTruncatingTail
        status.font = .systemFont(ofSize: tokens.number("text-sm"))
        status.textColor = tokens.color("glass-text-muted"); styleLabelBacking(status)
        status.isHidden = true; addSubview(status)

        let gap = tokens.number("s-3")
        let outerX = mirrored ? bounds.width - inset - 28 : inset
        let groupStart = mirrored && saved ? outerX - 28 - gap : outerX
        let close = addButton("Close", .close, x: groupStart, y: inset, action: dismiss)
        close.tooltipText = "Close"
        closeButton = close
        // Before a folder save Delete dissolves only the preview; after it,
        // Delete moves the export to the Trash.
        let delete = addButton("Delete", .trash, x: groupStart + (saved ? 28 + gap : 0), y: inset) { [weak self] in
            self?.saved == true ? trash() : discard()
        }
        delete.tooltipText = "Delete"
        let edit = addButton("Edit", .edit, x: mirrored ? inset : bounds.width - 36, y: inset, action: open)
        edit.tooltipText = "Edit"
        // The present pill widens away from the inner corner.
        edit.growsFromTrailingEdge = !mirrored
        editButton = edit
        let centerX = (bounds.width - 140) / 2
        let centerTop = (bounds.height - 64 - gap) / 2
        copyButton = addButton("Copy", .copy, x: centerX, y: centerTop, width: 140, action: copy)
        copyButton?.popsIcon = true
        let saveControl = addButton(saved ? "Show in Folder" : "Save file", saved ? .folder : .save,
                                    x: centerX, y: centerTop + 32 + gap, width: 140, primary: true, action: save)
        saveControl.popsIcon = true
        saveButton = saveControl
        setChromeVisible(false)
        // `0 0 0 2px rgba(accent, .9), 0 0 14px rgba(accent, .28)`, drawn under
        // the media so the glow shows only outside the card.
        let radius = tokens.number("thumbnail-card-radius")
        let accent = tokens.color("theme-accent")
        editorRing.frame = bounds.insetBy(dx: -2, dy: -2)
        editorRing.cornerRadius = radius + 2
        editorRing.borderWidth = 2
        editorRing.borderColor = accent.withAlphaComponent(0.9).cgColor
        editorRing.shadowColor = accent.cgColor
        editorRing.shadowOpacity = 0.28
        editorRing.shadowRadius = 7
        editorRing.shadowOffset = .zero
        editorRing.opacity = 0
        layer?.insertSublayer(editorRing, at: 0)
        // `0 0 0 1px rgba(accent, .85), 0 0 18px rgba(accent, .24)` over the card.
        highlightRing.frame = bounds
        highlightRing.cornerRadius = radius
        highlightRing.borderWidth = 1
        highlightRing.borderColor = accent.withAlphaComponent(0.85).cgColor
        highlightRing.shadowColor = accent.cgColor
        highlightRing.shadowOpacity = 0.24
        highlightRing.shadowRadius = 9
        highlightRing.shadowOffset = .zero
        highlightRing.opacity = 0
        layer?.addSublayer(highlightRing)
        if let layer {
            // `--thumbnail-card-shadow`, under everything the card draws.
            BoxShadowLayers.install(tokens.shadow("thumbnail-card-shadow"), on: layer, bounds: bounds, radius: radius)
            pileGlow.frame = bounds.insetBy(dx: -1, dy: -1)
            pileGlow.cornerRadius = radius + 1
            pileGlow.borderWidth = 1
            pileGlow.borderColor = accent.withAlphaComponent(0.55).cgColor
            let glowBounds = CGRect(x: 1, y: 1, width: bounds.width, height: bounds.height)
            pileGlow.shadowPath = BoxShadowLayers.roundedPath(glowBounds, radius: radius)
            pileGlow.shadowColor = accent.cgColor
            pileGlow.shadowOpacity = 0.28
            pileGlow.shadowRadius = 11
            pileGlow.shadowOffset = .zero
            let outside = CAShapeLayer()
            let cutout = CGMutablePath()
            cutout.addRect(pileGlow.bounds.insetBy(dx: -48, dy: -48))
            cutout.addPath(BoxShadowLayers.roundedPath(glowBounds, radius: radius))
            outside.frame = pileGlow.bounds
            outside.path = cutout
            outside.fillRule = .evenOdd
            pileGlow.mask = outside
            pileGlow.opacity = 0
            layer.insertSublayer(pileGlow, at: 0)
        }
        setAccessibilityRole(.group); setAccessibilityLabel("Screenshot mini preview")
    }

    /// Shipping `thumbnail-capture-highlight`: hold the outline for a second,
    /// then fade it. `startedAgo` resumes it on a rebuilt stack.
    /// The highlight ends on the main-queue clock, like the stack's exits and
    /// rebuilds. Core Animation's own removal of a finished animation is not
    /// guaranteed while the window server is not compositing the panel (an
    /// occluded panel, as on the self-hosted CI VM), and the card would then
    /// report a highlight forever.
    func playCaptureHighlight(startedAgo: Double = 0) {
        highlightToken &+= 1
        let token = highlightToken
        let seconds = NativeMotion.play("preview_capture_highlight", onLayer: highlightRing, down: 1,
                                        tokens: tokens, startedAgo: startedAgo,
                                        key: "preview-capture-highlight")
        guard seconds > 0 else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { [weak self] in
            guard let self, self.highlightToken == token else { return }
            self.highlightRing.removeAnimation(forKey: "preview-capture-highlight")
        }
    }

    /// Shipping `.thumbnail-meta .warning`: "Not in History" or "Clipboard
    /// unavailable" beside the metadata, which native captures reach only
    /// through a failed copy (History is written before a card appears).
    private func updateWarning() {
        let pointer: UnsafePointer<CChar>? = captures_preview_card_warning_v1(clipboardCurrent, true, copyFailed)
        let warning = pointer.map { String(cString: $0) }
        warningLabel.stringValue = warning ?? ""
        warningLabel.setAccessibilityLabel(warning)
        let width = ceil(warningLabel.attributedStringValue.size().width) + 8
        warningLabel.frame = NSRect(x: dimensions.frame.maxX + tokens.number("s-3"), y: dimensions.frame.minY,
                                    width: min(width, max(0, bounds.width - dimensions.frame.maxX - 16)),
                                    height: dimensions.frame.height)
        warningLabel.isHidden = warning == nil || dimensions.isHidden || isExiting
    }

    /// Freeze the card for its exit: shipping locks the hover look, hides the
    /// metadata and ignores input while the card leaves.
    func beginExit() {
        isExiting = true
        setMediaHovered(true, animated: false)
        dimensions.isHidden = true; warningLabel.isHidden = true; status.isHidden = true
        actionButtons.forEach { $0.tooltipChanged?($0, false) }
    }

    /// `thumbnail-dismiss-streak`: the media stretches and smears into a
    /// horizontal motion blur inside its clip.
    func playDismissStreak(delay: Double) {
        guard let layer = imageView.layer,
              let spec = NativeMotion.catalog.keyframes["preview_dismiss_streak"] else { return }
        NativeMotion.play("preview_dismiss_streak", onLayer: layer, down: 1, tokens: tokens,
                          holdEnd: true, delay: delay, key: "preview-dismiss-streak")
        guard let streak = CIFilter(name: "CIMotionBlur") else { return }
        streak.setDefaults()
        streak.setValue(0, forKey: kCIInputRadiusKey)
        streak.setValue(0, forKey: kCIInputAngleKey)
        streak.name = "streak"
        layer.filters = (layer.filters ?? []) + [streak]
        let blur = CAKeyframeAnimation(keyPath: "filters.streak.inputRadius")
        // The first key is the locked 2 pt hover blur, already on the media.
        blur.values = spec.frames.map { NSNumber(value: $0.offset == 0 ? 0 : $0.blur) }
        blur.keyTimes = spec.frames.map { NSNumber(value: $0.offset) }
        let timing = NativeMotion.timingFunction(spec.easing, tokens: tokens)
        blur.timingFunctions = Array(repeating: timing, count: spec.frames.count - 1)
        blur.duration = NativeMotion.seconds(spec.duration, tokens: tokens)
        blur.beginTime = layer.convertTime(CACurrentMediaTime(), from: nil) + delay
        blur.fillMode = .both
        blur.isRemovedOnCompletion = false
        layer.add(blur, forKey: "preview-dismiss-streak-blur")
    }

    /// `thumbnail-arrive`'s `filter: blur(3px → 0)` on the media, through a
    /// second Core Image Gaussian so the hover and depth blur keep their own
    /// radius. The filter is added for the arrival and removed once it lands.
    @discardableResult func playArrivalBlur(reduced: Bool = NativeMotion.reduceMotion) -> Bool {
        guard !reduced, let layer = imageView.layer,
              let arrive = CIFilter(name: "CIGaussianBlur") else { return false }
        arrive.setDefaults(); arrive.setValue(0, forKey: kCIInputRadiusKey); arrive.name = "arrive"
        let others = (layer.filters ?? []).filter { ($0 as? CIFilter)?.name != "arrive" }
        layer.filters = others + [arrive]
        CATransaction.begin()
        CATransaction.setCompletionBlock { [weak layer] in
            guard let layer, layer.animation(forKey: "preview-arrive-blur") == nil else { return }
            layer.filters = (layer.filters ?? []).filter { ($0 as? CIFilter)?.name != "arrive" }
        }
        let seconds = NativeMotion.playBlur("preview_card_arrive", onLayer: layer, filter: "arrive",
                                            tokens: tokens, key: "preview-arrive-blur", reduced: false)
        CATransaction.commit()
        if seconds == 0 { layer.filters = others }
        return seconds > 0
    }

    /// The media as a dust source: cover-cropped to the card in its rounded
    /// rect, unfiltered (the chips carry the hover blur and brightness).
    func dustSource(scale: CGFloat) -> CGImage? {
        let image = imageView.image
        let width = Int((bounds.width * scale).rounded()), height = Int((bounds.height * scale).rounded())
        guard width > 0, height > 0, image.size.width > 0, image.size.height > 0,
              let space = CGColorSpace(name: CGColorSpace.sRGB),
              let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8,
                                      bytesPerRow: width * 4, space: space,
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        context.scaleBy(x: scale, y: scale)
        let radius = tokens.number("thumbnail-card-radius")
        let rect = CGRect(origin: .zero, size: bounds.size)
        context.addPath(CGPath(roundedRect: rect, cornerWidth: radius, cornerHeight: radius, transform: nil))
        context.clip()
        let cover = max(rect.width / image.size.width, rect.height / image.size.height)
        let size = NSSize(width: image.size.width * cover, height: image.size.height * cover)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: false)
        image.draw(in: NSRect(x: (rect.width - size.width) / 2, y: (rect.height - size.height) / 2,
                              width: size.width, height: size.height),
                   from: .zero, operation: .copy, fraction: 1)
        NSGraphicsContext.restoreGraphicsState()
        return context.makeImage()
    }

    /// Centre of Delete, where the ash front starts.
    var deleteOrigin: CGPoint {
        let tables = PreviewMotionTables.shared
        let left = saved ? tables.deleteOriginAfterCloseX : tables.deleteOriginFirstX
        return CGPoint(x: mirrored ? bounds.width - left : left, y: tables.deleteOriginY)
    }

    /// `thumbnail-delete-img-fade`: the frozen card fades over its dust chips
    /// (hold 20 % of 0.55 s, then `cubic-bezier(0.22, 0.1, 0.25, 1)`).
    func fadeForDust() {
        guard let layer else { return }
        let duration = 0.55
        let fade = CAKeyframeAnimation(keyPath: "opacity")
        fade.values = [1, 1, 0]
        fade.keyTimes = [0, 0.2, 1]
        fade.timingFunctions = [CAMediaTimingFunction(name: .linear),
                                CAMediaTimingFunction(controlPoints: 0.22, 0.1, 0.25, 1)]
        fade.duration = duration
        fade.fillMode = .forwards
        fade.isRemovedOnCompletion = false
        layer.add(fade, forKey: "preview-dust-source")
    }

    /// Shipping `.thumbnail-stack-minimized .thumbnail-media { filter: blur() }`
    /// on a compact card, through the media's Core Image Gaussian.
    func setDepthBlur(_ radius: Double, from start: Double? = nil, duration: Double = 0,
                      timing: CAMediaTimingFunction? = nil, delay: Double = 0) {
        guard let layer = imageView.layer else { return }
        let from = start ?? currentDepthBlur
        depthBlurRadius = radius
        layer.setValue(radius, forKeyPath: "filters.blur.inputRadius")
        guard duration > 0, window?.isVisible == true, from != radius else {
            layer.removeAnimation(forKey: "preview-depth-blur")
            return
        }
        let blur = CABasicAnimation(keyPath: "filters.blur.inputRadius")
        blur.fromValue = from; blur.toValue = radius
        blur.duration = duration
        blur.timingFunction = timing ?? CAMediaTimingFunction(name: .easeInEaseOut)
        // A staggered fan layer holds its old radius until its turn.
        blur.beginTime = layer.convertTime(CACurrentMediaTime(), from: nil) + delay
        blur.fillMode = .backwards
        layer.add(blur, forKey: "preview-depth-blur")
    }

    /// The media blur on screen now, mid-transition included.
    var currentDepthBlur: Double {
        (imageView.layer?.presentation()?.value(forKeyPath: "filters.blur.inputRadius") as? Double)
            ?? depthBlurRadius
    }

    /// The hovered pile's accent ring and glow, easing with the fan.
    func setPileGlow(_ visible: Bool, duration: Double, timing: CAMediaTimingFunction? = nil,
                     delay: Double = 0) {
        let target: Float = visible ? 1 : 0
        if duration > 0, window?.isVisible == true, pileGlow.opacity != target {
            let fade = CABasicAnimation(keyPath: "opacity")
            fade.fromValue = (pileGlow.presentation() ?? pileGlow).opacity
            fade.toValue = target
            fade.duration = duration
            fade.timingFunction = timing ?? CAMediaTimingFunction(name: .easeInEaseOut)
            fade.beginTime = pileGlow.convertTime(CACurrentMediaTime(), from: nil) + delay
            fade.fillMode = .backwards
            pileGlow.add(fade, forKey: "preview-pile-glow")
        }
        CATransaction.begin(); CATransaction.setDisableActions(true)
        pileGlow.opacity = target
        CATransaction.commit()
    }

    /// The compact depth shade eases with the stack's flight.
    func setDepthShade(visible: Bool, animated: Bool) {
        if animated { depthShade.animator().alphaValue = visible ? 1 : 0 } else { depthShade.alphaValue = visible ? 1 : 0 }
    }

    /// Apply the shared editor presence: the Edit control morphs into the
    /// "In editor" pill, stays pinned while leaving and lingering, and the card
    /// ring arrives over `0.22s ease` and eases out over the 550 ms leave.
    func setEditorPhase(_ phase: UInt32, animated: Bool) {
        editorPhase = phase
        editButton?.setEditorPhase(phase, animated: animated)
        let present = phase == UInt32(CAPTURES_EDITOR_PHASE_PRESENT)
        let target: Float = present && !compact ? 1 : 0
        let spec = NativeMotion.transition(present ? "preview_editor_ring" : "preview_editor_ring_leave",
                                           tokens: tokens)
        if animated, window?.isVisible == true, spec.duration > 0, editorRing.opacity != target {
            let fade = CABasicAnimation(keyPath: "opacity")
            fade.fromValue = (editorRing.presentation() ?? editorRing).opacity
            fade.toValue = target
            fade.duration = spec.duration
            fade.timingFunction = spec.timing
            editorRing.add(fade, forKey: "preview-editor-ring")
        }
        CATransaction.begin(); CATransaction.setDisableActions(true)
        editorRing.opacity = target
        CATransaction.commit()
        if compact { editButton?.isHidden = true } else { setChromeVisible(chromeVisible) }
    }

    /// `html:not(.thumbnail-native-tracking) .thumbnail-card:hover img`: blur,
    /// darken and scale the media over its shipping transitions.
    private func setMediaHovered(_ hovered: Bool, animated: Bool = true) {
        guard hovered != mediaHovered else { return }
        mediaHovered = hovered
        let media = captures_preview_hover_media_v1()
        let live = animated && window?.isVisible == true
        let filter = NativeMotion.transition("preview_media_filter", tokens: tokens)
        let scaling = NativeMotion.transition("preview_media_scale", tokens: tokens)
        let radius = hovered ? media.blur : 0
        let from = mediaBlurRadius
        mediaBlurRadius = radius
        if let layer = imageView.layer {
            layer.setValue(radius, forKeyPath: "filters.blur.inputRadius")
            if live, filter.duration > 0 {
                let blur = CABasicAnimation(keyPath: "filters.blur.inputRadius")
                blur.fromValue = from; blur.toValue = radius
                blur.duration = filter.duration; blur.timingFunction = filter.timing
                layer.add(blur, forKey: "preview-media-blur")
            }
        }
        let dim = hovered ? CGFloat(1 - media.brightness) : 0
        let scale = hovered ? CGFloat(media.scale) : 1
        let size = NSSize(width: mediaClip.bounds.width * scale, height: mediaClip.bounds.height * scale)
        let scaled = NSRect(x: (mediaClip.bounds.width - size.width) / 2,
                            y: (mediaClip.bounds.height - size.height) / 2,
                            width: size.width, height: size.height)
        guard live else {
            mediaDim.alphaValue = dim; imageView.frame = scaled
            return
        }
        NSAnimationContext.runAnimationGroup { context in
            context.duration = filter.duration; context.timingFunction = filter.timing
            mediaDim.animator().alphaValue = dim
        }
        NSAnimationContext.runAnimationGroup { context in
            context.duration = scaling.duration; context.timingFunction = scaling.timing
            imageView.animator().frame = scaled
        }
    }

    /// Re-evaluate pointer hover after the stack's stale-pointer lock changes.
    func hoverLockChanged() { refreshChrome() }

    /// Shipping `:focus-within` matches only while the document has focus,
    /// so a card's (or its controls') keyboard focus counts only while the
    /// panel is key. Merely ordering the panel in makes its initial card
    /// first responder; that alone must not raise chrome.
    private var hasKeyFocusWithin: Bool {
        guard let window, window.isKeyWindow, let responder = window.firstResponder as? NSView else { return false }
        return responder.isDescendant(of: self)
    }

    /// Pointer hover (once unlocked) or key-window focus within keeps chrome up.
    fileprivate func refreshChrome() {
        guard !isExiting else { return }
        setChromeVisible((pointerInside && !isHoverLocked()) || hasKeyFocusWithin)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func setStatus(_ value: String, detail: String? = nil) {
        status.stringValue = value
        status.isHidden = compact || value.isEmpty
        status.setAccessibilityLabel(value.isEmpty ? nil : value)
        status.toolTip = detail
    }

    static func metadata(width: Int, height: Int, sizeBytes: UInt64) -> String {
        guard let value = captures_preview_card_metadata_v1(UInt32(clamping: width),
                                                            UInt32(clamping: height), sizeBytes)
        else { return "\(width) × \(height)" }
        defer { captures_settings_free_v1(value) }
        return String(cString: value)
    }

    var metadataText: String { dimensions.stringValue }

    func setClipboardCurrent(_ current: Bool) {
        guard clipboardCurrent != current else { return }
        clipboardCurrent = current
        // With Copy hidden, the remaining centered action sits at the card center.
        let gap = tokens.number("s-3")
        saveButton?.frame.origin.y = current ? (bounds.height - 32) / 2
            : (bounds.height - 64 - gap) / 2 + 32 + gap
        copyButton?.isHidden = compact || !chromeVisible || current
        clipboardChip.isHidden = compact || !current
        // `clipboard-confirmation-arrive`; a returning Copy remounts its glyph.
        if current && !compact {
            NativeMotion.play("preview_clipboard_chip_arrive", on: clipboardChip, tokens: tokens,
                              key: "preview-clipboard-arrive")
        } else if !current {
            copyButton?.popIcon()
        }
        updateWarning()
    }

    /// Brief shipping "Saved" confirmation on the Save/Show in Folder action.
    func showSavedFeedback() {
        savedFeedbackToken &+= 1
        let token = savedFeedbackToken
        savedFeedbackActive = true
        updateSaveButton(saved: saved)
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.savedFeedbackDuration) { [weak self] in
            guard let self, self.savedFeedbackToken == token else { return }
            self.savedFeedbackActive = false
            self.updateSaveButton(saved: self.saved)
        }
    }

    func updateSaveButton(saved: Bool) {
        let title = savedFeedbackActive ? "Saved" : saved ? "Show in Folder" : "Save file"
        saveButton?.title = title
        saveButton?.kind = savedFeedbackActive ? .check : saved ? .folder : .save
        saveButton?.setAccessibilityLabel(title)
        self.saved = saved
        let step = 28 + tokens.number("s-3")
        let start = mirrored ? bounds.width - 36 - (saved ? step : 0) : 8
        closeButton?.frame.origin.x = start
        let delete = actionButtons.first(where: { $0.kind == .trash })
        delete?.frame.origin.x = start + (saved ? step : 0)
        setChromeVisible(chromeVisible)
        saveButton?.needsDisplay = true
    }

    var statusText: String { status.stringValue }

    func setCompact(_ compact: Bool, depth: Int) {
        self.compact = compact
        depthShade.isHidden = !compact || depth == 0
        depthShade.layer?.backgroundColor = tokens.color("glass-strong-solid")
            .withAlphaComponent(CGFloat(captures_preview_dim_opacity_v1(depth))).cgColor
        dimensions.isHidden = compact || chromeVisible
        status.isHidden = compact || status.stringValue.isEmpty
        actionButtons.forEach { if $0 !== editButton { $0.isHidden = compact || !chromeVisible } }
        if clipboardCurrent { copyButton?.isHidden = true }
        clipboardChip.isHidden = compact || !clipboardCurrent
        closeButton?.isHidden = compact || !chromeVisible || !saved
        editButton?.isHidden = compact || !(chromeVisible || editorPinned)
        CATransaction.begin(); CATransaction.setDisableActions(true)
        editorRing.opacity = !compact && editorPhase == UInt32(CAPTURES_EDITOR_PHASE_PRESENT) ? 1 : 0
        CATransaction.commit()
        // Clear the pile blur first: the hover blur shares the media filter.
        if !compact {
            if depthBlurRadius != 0 { setDepthBlur(0) }
            setPileGlow(false, duration: 0)
        }
        setMediaHovered(!compact && chromeVisible, animated: false)
        updateWarning()
    }

    @discardableResult private func addButton(_ title: String, _ kind: MiniPreviewButtonKind, x: CGFloat,
        y: CGFloat, width: CGFloat = 28, primary: Bool = false, action: @escaping () -> Void) -> MiniPreviewButton {
        let button = MiniPreviewButton(title, kind: kind, frame: NSRect(x: x, y: y, width: width, height: kind == .copy || kind == .save || kind == .folder ? 32 : 28), tokens: tokens, primary: primary, action: action)
        button.boxShadowToken = kind == .copy || kind == .save || kind == .folder ? "shadow-md" : "shadow-sm"
        addSubview(button); actionButtons.append(button); return button
    }

    private func setChromeVisible(_ visible: Bool) {
        guard !compact, !isExiting else { return }
        chromeVisible = visible
        actionButtons.forEach { if $0 !== editButton { $0.isHidden = !visible } }
        if clipboardCurrent { copyButton?.isHidden = true }
        closeButton?.isHidden = !visible || !saved
        // An open, leaving or lingering editor keeps the control pinned.
        editButton?.isHidden = !visible && !editorPinned
        dimensions.isHidden = visible
        setMediaHovered(visible)
        updateWarning()
    }
    override var acceptsFirstResponder: Bool { !compact && !isExiting }
    override func becomeFirstResponder() -> Bool {
        let result = super.becomeFirstResponder()
        if result, window?.isKeyWindow == true { setChromeVisible(true) }
        return result
    }
    override func resignFirstResponder() -> Bool {
        let result = super.resignFirstResponder()
        if result { focusWithinChanged() }
        return result
    }
    /// Shipping `:focus-within`: keyboard focus on the card or one of its
    /// controls keeps the chrome up while the panel is key. Once focus
    /// leaves, the chrome hides unless the pointer is still over the card.
    fileprivate func focusWithinChanged() {
        // The window's first responder updates after this callback returns.
        DispatchQueue.main.async { [weak self] in self?.refreshChrome() }
    }
    override func updateTrackingAreas() {
        if let tracking { removeTrackingArea(tracking) }
        tracking = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self, userInfo: nil)
        addTrackingArea(tracking!); super.updateTrackingAreas()
    }
    override func mouseEntered(with event: NSEvent) {
        pointerInside = true
        refreshChrome()
    }
    override func mouseExited(with event: NSEvent) {
        pointerInside = false
        let focusedControl = window?.isKeyWindow == true
            && actionButtons.contains { $0.window?.firstResponder === $0 }
        if !focusedControl { setChromeVisible(false) }
    }

    private func styleLabelBacking(_ label: NSTextField) {
        label.wantsLayer = true
        label.layer?.backgroundColor = tokens.color("glass-strong").cgColor
        label.layer?.cornerRadius = tokens.number("r-xs")
        label.layer?.masksToBounds = true
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        guard !isExiting, let hit = super.hitTest(point) else { return nil }
        // The image and labels are decorative; receive their gestures on the
        // card. Buttons retain their own click handling.
        return actionButtons.contains(where: { hit === $0 && !$0.isHidden }) ? hit : self
    }

    override func mouseDown(with event: NSEvent) {
        press = event.locationInWindow
        fileDragStarted = false
    }

    override func mouseDragged(with event: NSEvent) {
        guard isOutboundFileDragEnabled, !fileDragStarted, let press, let path = preparedDragPath,
              hypot(event.locationInWindow.x - press.x, event.locationInWindow.y - press.y) >= 4,
              imageView.image.size.width > 0, imageView.image.size.height > 0 else { return }
        fileDragStarted = true
        let item = NSDraggingItem(pasteboardWriter: NSURL(fileURLWithPath: path))
        item.setDraggingFrame(mediaClip.frame, contents: imageView.image)
        beginDraggingSession(with: [item], event: event, source: self)
    }

    override func mouseUp(with event: NSEvent) {
        press = nil
        fileDragStarted = false
    }

    func draggingSession(_ session: NSDraggingSession,
                         sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        sourceOperationMask(for: context)
    }

    func sourceOperationMask(for context: NSDraggingContext) -> NSDragOperation { .copy }

    func draggingSession(_ session: NSDraggingSession, endedAt screenPoint: NSPoint,
                         operation: NSDragOperation) {
        finishDrag(at: screenPoint, operation: operation)
    }

    func finishDrag(at screenPoint: NSPoint, operation: NSDragOperation) {
        press = nil
        fileDragStarted = false
        dragEnded?(screenPoint, operation)
    }

    func rejectDrop() {
        guard !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion else { return }
        let shake = CAKeyframeAnimation(keyPath: "transform.translation.x")
        shake.values = [0, -8, 7, -5, 3, 0]
        shake.duration = 0.42
        layer?.add(shake, forKey: "preview-drop-reject")
    }
}

struct MiniPreviewResource {
    var artifact: CaptureArtifact
    let image: NSImage
}

private final class MiniPreviewDocumentView: NSView {
    override var isFlipped: Bool { true }
}

private final class MiniPreviewExpandButton: NSButton {
    private let actionBlock: () -> Void
    private let move: (NSPoint) -> Void
    private var press: NSPoint?
    private var frameOrigin = NSPoint.zero
    private var dragging = false
    private let fan: (Bool) -> Void
    /// The carried pile's desktop pointer, nil when the carry ends.
    private let carry: (NSPoint?) -> Void
    private var tracking: NSTrackingArea?

    init(frame: NSRect, count: Int, move: @escaping (NSPoint) -> Void,
         fan: @escaping (Bool) -> Void, carry: @escaping (NSPoint?) -> Void = { _ in },
         action: @escaping () -> Void) {
        actionBlock = action; self.move = move; self.fan = fan; self.carry = carry
        super.init(frame: frame)
        title = ""; isBordered = false; setButtonType(.momentaryPushIn)
        target = self; self.action = #selector(activate)
        // Shipping's collapsed hit target is named but carries no tooltip.
        setAccessibilityLabel(count == 1 ? "Expand preview" : "Expand \(count) previews")
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func activate() { actionBlock() }
    override func draw(_ dirtyRect: NSRect) {}
    override func resetCursorRects() { addCursorRect(bounds, cursor: .openHand) }
    override func updateTrackingAreas() {
        if let tracking { removeTrackingArea(tracking) }
        let next = NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeAlways],
                                  owner: self, userInfo: nil)
        addTrackingArea(next); tracking = next
        super.updateTrackingAreas()
    }
    override func mouseEntered(with event: NSEvent) { fan(true) }
    override func mouseExited(with event: NSEvent) { if press == nil { fan(false) } }
    override func mouseDown(with event: NSEvent) {
        guard let window else { return }
        press = window.convertPoint(toScreen: event.locationInWindow); fan(true)
        frameOrigin = window.frame.origin; dragging = false
    }
    override func mouseDragged(with event: NSEvent) {
        guard let window, let press else { return }
        let current = window.convertPoint(toScreen: event.locationInWindow)
        let delta = NSSize(width: current.x - press.x, height: current.y - press.y)
        guard dragging || max(abs(delta.width), abs(delta.height)) >= 4 else { return }
        dragging = true
        move(NSPoint(x: frameOrigin.x + delta.width, y: frameOrigin.y + delta.height))
        carry(current)
    }
    override func mouseUp(with event: NSEvent) {
        let clicked = press != nil && !dragging && bounds.contains(convert(event.locationInWindow, from: nil))
        if dragging { carry(nil) }
        press = nil; dragging = false
        fan(bounds.contains(convert(event.locationInWindow, from: nil)))
        if clicked { actionBlock() }
    }
}

/// Shipping `.thumbnail-overflow-cue`: a centered chevron tab on the stack
/// edge that scrolls one card slot toward hidden captures.
final class MiniPreviewOverflowCue: NSButton {
    static let size = NSSize(width: 46, height: 22)
    let above: Bool
    private let tokens: Tokens
    private let actionBlock: () -> Void
    private var tracking: NSTrackingArea?
    private var hovered = false
    override var isFlipped: Bool { true }

    init(above: Bool, label: String, frame: NSRect, tokens: Tokens, action: @escaping () -> Void) {
        self.above = above; self.tokens = tokens; actionBlock = action
        super.init(frame: frame)
        title = ""; isBordered = false; setButtonType(.momentaryPushIn)
        target = self; self.action = #selector(activate)
        // Named for accessibility; shipping cues carry no tooltip.
        setAccessibilityLabel(label)
        // `box-shadow: var(--glass-shadow)`; the square corners sit on the
        // window edge, so a uniformly rounded shadow differs only off-screen.
        wantsLayer = true
        if let layer {
            BoxShadowLayers.install(tokens.shadow("glass-shadow"), on: layer, bounds: bounds,
                                    radius: tokens.number("r-lg"))
        }
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func activate() { actionBlock() }
    override func updateTrackingAreas() {
        if let tracking { removeTrackingArea(tracking) }
        let next = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                                  owner: self, userInfo: nil)
        addTrackingArea(next); tracking = next
        super.updateTrackingAreas()
    }
    override func mouseEntered(with event: NSEvent) { hovered = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hovered = false; needsDisplay = true }

    override func draw(_ dirtyRect: NSRect) {
        // Only the edge facing the cards is rounded; the other sits on the window edge.
        let b = bounds.insetBy(dx: 0.5, dy: 0.5)
        let r = min(tokens.number("r-lg"), b.height / 2)
        let path = NSBezierPath()
        if above {
            path.move(to: NSPoint(x: b.minX, y: b.minY)); path.line(to: NSPoint(x: b.maxX, y: b.minY))
            path.line(to: NSPoint(x: b.maxX, y: b.maxY - r))
            path.appendArc(from: NSPoint(x: b.maxX, y: b.maxY), to: NSPoint(x: b.maxX - r, y: b.maxY), radius: r)
            path.line(to: NSPoint(x: b.minX + r, y: b.maxY))
            path.appendArc(from: NSPoint(x: b.minX, y: b.maxY), to: NSPoint(x: b.minX, y: b.maxY - r), radius: r)
        } else {
            path.move(to: NSPoint(x: b.minX, y: b.maxY)); path.line(to: NSPoint(x: b.minX, y: b.minY + r))
            path.appendArc(from: NSPoint(x: b.minX, y: b.minY), to: NSPoint(x: b.minX + r, y: b.minY), radius: r)
            path.line(to: NSPoint(x: b.maxX - r, y: b.minY))
            path.appendArc(from: NSPoint(x: b.maxX, y: b.minY), to: NSPoint(x: b.maxX, y: b.minY + r), radius: r)
            path.line(to: NSPoint(x: b.maxX, y: b.maxY))
        }
        path.close()
        tokens.color(hovered || cell?.isHighlighted == true ? "glass-raised" : "glass-strong").setFill(); path.fill()
        tokens.color("glass-border").setStroke(); path.lineWidth = 1; path.stroke()
        // Shipping 16-unit chevron (`M3.5 10 8 5.5 12.5 10` / `M3.5 6 8 10.5 12.5 6`).
        let box = NSRect(x: (bounds.width - 16) / 2, y: (bounds.height - 16) / 2, width: 16, height: 16)
        tokens.color("glass-text").setStroke()
        ShippingIcons.stroke(above ? "preview-overflow-up" : "preview-overflow-down",
                             in: box, width: 3, flipped: isFlipped)
        if window?.firstResponder === self {
            tokens.color("theme-accent").setStroke()
            let focus = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1), xRadius: r, yRadius: r)
            focus.lineWidth = 2; focus.stroke()
        }
    }
}

final class MiniPreviewView: NSView {
    private let geometry: CapturesPreviewGeometry
    private let tokens: Tokens
    private let scroll = NSScrollView()
    private let document = MiniPreviewDocumentView()
    private var cards: [String: MiniPreviewCardView] = [:]
    private let restLayouts: [String: CapturesPreviewCardLayout]
    private let hoverLayouts: [String: CapturesPreviewCardLayout]
    private(set) var pileHovered = false
    private var pileExpandButton: MiniPreviewExpandButton?
    private var collapseButton: MiniPreviewButton?
    private var clearButton: MiniPreviewButton?
    private var overflowCues: [MiniPreviewOverflowCue] = []
    /// Top-anchored stacks open icon tips below (not `topAnchor`, an NSView member).
    private let anchoredAtTop: Bool
    /// Right-anchored stacks mirror the Close streak.
    private let anchoredRight: Bool
    /// One card slot (card plus gap), for survivors settling into a hole.
    private let cardSlot: CGFloat
    private let stackCollapsed: Bool
    /// Shipping pile gravity (-1 top … 1 bottom); spin fades in toward 0.
    private(set) var pileGravity: Double
    /// Cards playing an exit in place, and their dust overlays.
    private(set) var exitingArtifactIDs: Set<String> = []
    private var dustOverlays: [PreviewMotionOverlay] = []
    /// The latest list <-> pile flight; an older flight's end never lands.
    private var flyToken = 0
    /// Dust chips on screen, for tests.
    var dustChipCount: Int { dustOverlays.reduce(0) { $0 + ($1.content.sublayers?.count ?? 0) } }
    /// `.thumbnail-collapsed-hit-target::before/::after` sparkles.
    private var sparkles: PreviewMotionOverlay?
    private(set) var sparkling = false
    /// One shared glass tip for card icons and the stack toolbar.
    private let tooltipView: GlassTooltipView
    private weak var tooltipOwner: MiniPreviewButton?
    /// The styled tooltip currently shown, for tests and accessibility checks.
    var visibleTooltip: String? { tooltipView.isHidden ? nil : tooltipView.label.stringValue }
    var visibleTooltipFrame: NSRect? { tooltipView.isHidden ? nil : tooltipView.frame }
    /// Shipping stale-pointer suppression (`data-thumbnail-suppress-card-hover`).
    private var hoverLock = CapturesCardHoverLock()
    private var pointerTracking: NSTrackingArea?
    var isCardHoverLocked: Bool { hoverLock.locked }
    var visibleOverflowCueLabels: [String] {
        overflowCues.filter { !$0.isHidden }.compactMap { $0.accessibilityLabel() }
    }
    var stackToolbarButtons: [MiniPreviewButton] { [clearButton, collapseButton].compactMap { $0 } }
    private(set) var artifactIDs: [String] = []
    var renderedArtifactIDs: [String] { artifactIDs.filter { cards[$0] != nil } }
    var cardPaintOrder: [String] {
        document.subviews.compactMap { ($0 as? MiniPreviewCardView)?.artifactID }
    }
    var visibleCardActionTitles: [String] {
        cards.values.flatMap { card in
            card.subviews.compactMap { $0 as? MiniPreviewButton }
                .filter { !$0.isHidden }.map(\.title)
        }
    }
    var visibleCardLabelCount: Int { cards.values.filter(\.hasVisibleLabels).count }
    var pileExpandAccessibilityLabel: String? { pileExpandButton?.accessibilityLabel() }
    var documentHeight: CGFloat { document.frame.height }
    var viewportHeight: CGFloat { scroll.contentView.bounds.height }
    var scrollOffsetY: CGFloat { scroll.contentView.bounds.minY }
    override var isFlipped: Bool { true }

    init(geometry: CapturesPreviewGeometry, contentHeight: Double,
         resources: [String: MiniPreviewResource],
         ids: [String], layouts: [String: CapturesPreviewCardLayout],
         hoverLayouts: [String: CapturesPreviewCardLayout] = [:], collapsed: Bool,
         topAnchor: Bool, rightAnchor: Bool, tokens: Tokens, pileGravity: Double? = nil,
         copy: @escaping (String) -> Void,
         save: @escaping (String) -> Void, open: @escaping (String) -> Void,
         trash: @escaping (String) -> Void, dismiss: @escaping (String) -> Void,
         discard: @escaping (String) -> Void = { _ in },
         setCollapsed: @escaping (Bool) -> Void,
         clearAll: @escaping () -> Void, move: @escaping (NSPoint) -> Void = { _ in }) {
        self.geometry = geometry; self.tokens = tokens; self.restLayouts = layouts
        self.hoverLayouts = hoverLayouts; anchoredAtTop = topAnchor
        anchoredRight = rightAnchor; stackCollapsed = collapsed
        self.pileGravity = pileGravity ?? (topAnchor ? -1 : 1)
        let slots = ids.compactMap { id in layouts[id].map { CGFloat($0.y) } }.sorted()
        cardSlot = slots.count >= 2 ? slots[1] - slots[0] : CGFloat(geometry.card_height)
        tooltipView = GlassTooltipView(tokens: tokens, style: .previewIcon)
        artifactIDs = ids
        super.init(frame: NSRect(x: 0, y: 0, width: geometry.width, height: geometry.height))
        wantsLayer = true

        scroll.frame = bounds; scroll.autoresizingMask = [.width, .height]
        // Shipping `.thumbnail-stack` hides its scroll bar; edge chevrons cue overflow.
        scroll.drawsBackground = false; scroll.hasVerticalScroller = false
        scroll.scrollerStyle = .overlay; scroll.borderType = .noBorder
        scroll.contentView.drawsBackground = false
        addSubview(scroll)

        let padding = CGFloat(geometry.padding), cardHeight = CGFloat(geometry.card_height)
        let cardWidth = bounds.width - padding * 2
        let documentHeight = collapsed ? bounds.height : max(bounds.height, CGFloat(contentHeight))
        document.frame = NSRect(x: 0, y: 0, width: bounds.width,
            height: documentHeight)
        scroll.documentView = document

        for id in ids {
            guard let resource = resources[id], let layout = layouts[id] else { continue }
            let card = MiniPreviewCardView(frame: NSRect(x: padding, y: CGFloat(layout.y),
                width: cardWidth, height: cardHeight), artifactID: id,
                image: resource.image, tokens: tokens,
                width: resource.artifact.width, height: resource.artifact.height,
                sizeBytes: resource.artifact.sizeBytes,
                saved: resource.artifact.savedPath != nil, rightAnchor: rightAnchor,
                copy: { copy(id) }, save: { save(id) }, open: { open(id) },
                trash: { trash(id) }, dismiss: { dismiss(id) }, discard: { discard(id) })
            card.isHidden = false
            card.setCompact(collapsed, depth: layout.depth)
            card.setAccessibilityElement(layout.interactive)
            card.isHoverLocked = { [weak self] in self?.hoverLock.locked == true }
            for button in card.iconButtons {
                button.tooltipChanged = { [weak self] owner, visible in
                    self?.setTooltip(for: owner, visible: visible)
                }
            }
            document.addSubview(card); cards[id] = card
        }
        if collapsed {
            for (id, card) in cards {
                guard let layout = layouts[id] else { continue }
                card.setFrameOrigin(pileOrigin(id, layout: layout, hovered: false))
            }
            applyPilePoses(hovered: false, duration: 0)
        }

        if collapsed, let front = ids.compactMap({ id in
            layouts[id].map { (id, $0) }
        }).first(where: { $0.1.interactive }), let card = cards[front.0] {
            let expand = MiniPreviewExpandButton(frame: card.frame, count: ids.count, move: move,
                fan: { [weak self] hovered in self?.setPileHovered(hovered) },
                carry: { [weak self] point in self?.carryPile(at: point) }) {
                setCollapsed(false)
            }
            document.addSubview(expand); pileExpandButton = expand
            // The sparkle layers reach over the fanned pile (`inset: -96px
            // -10px -6px`, flipped for top-anchored stacks).
            let tables = PreviewMotionTables.shared
            let above = topAnchor ? tables.near : tables.reach
            let below = topAnchor ? tables.reach : tables.near
            let area = NSRect(x: card.frame.minX - tables.side, y: card.frame.minY - above,
                              width: card.frame.width + tables.side * 2,
                              height: card.frame.height + above + below)
            let overlay = PreviewMotionOverlay(frame: area)
            for (dots, index) in [(tables.early, 0), (tables.late, 1)] {
                let group = CALayer()
                group.frame = overlay.bounds
                group.opacity = 0
                group.name = index == 0 ? "early" : "late"
                for dot in dots {
                    let center = CGPoint(x: (area.width - 6) * dot.x + 3, y: (area.height - 6) * dot.y + 3)
                    let color = dot.accent ? tokens.color("theme-accent") : NSColor.white
                    for (radius, alpha) in [(dot.fade, dot.alpha * 0.35), (dot.core, dot.alpha)] {
                        let circle = CALayer()
                        circle.frame = CGRect(x: center.x - radius, y: center.y - radius,
                                              width: radius * 2, height: radius * 2)
                        circle.cornerRadius = radius
                        circle.backgroundColor = color.withAlphaComponent(alpha).cgColor
                        group.addSublayer(circle)
                    }
                }
                overlay.content.addSublayer(group)
            }
            document.addSubview(overlay); sparkles = overlay
        }

        if !collapsed {
            // Cues sit over the cards and under the stack toolbar, as shipped.
            for above in [true, false] {
                let size = MiniPreviewOverflowCue.size
                let label = String(cString: captures_preview_overflow_label_v1(above, topAnchor))
                let cue = MiniPreviewOverflowCue(above: above, label: label,
                    frame: NSRect(x: (bounds.width - size.width) / 2,
                                  y: above ? 6 : bounds.height - 6 - size.height,
                                  width: size.width, height: size.height),
                    tokens: tokens) { [weak self] in self?.scrollStack(bySlots: above ? -1 : 1) }
                cue.isHidden = true
                addSubview(cue); overflowCues.append(cue)
            }
            scroll.contentView.postsBoundsChangedNotifications = true
            NotificationCenter.default.addObserver(self, selector: #selector(scrollBoundsChanged(_:)),
                name: NSView.boundsDidChangeNotification, object: scroll.contentView)
        }

        let controlY = topAnchor ? 16 : bounds.height - 44
        if !collapsed && ids.count >= 2 {
            let outerX = rightAnchor ? bounds.width - padding - 28 : padding
            let step = 28 + tokens.number("s-1")
            let adjacentX = rightAnchor ? outerX - step : outerX + step
            // Clear all stays on the outside edge so the Show less pill never covers it.
            let clear = MiniPreviewButton("Clear all previews", kind: .clear,
                frame: NSRect(x: outerX, y: controlY, width: 28, height: 28), tokens: tokens,
                action: clearAll)
            clear.tooltipText = "Clear all"
            clear.tooltipChanged = { [weak self] owner, visible in
                self?.setTooltip(for: owner, visible: visible)
            }
            // Show less swaps its icon for a label instead of carrying a tip.
            let collapse = MiniPreviewButton("Minimize previews", kind: .collapse,
                frame: NSRect(x: adjacentX, y: controlY, width: 28, height: 28), tokens: tokens) {
                setCollapsed(true)
            }
            collapse.growsFromTrailingEdge = rightAnchor
            collapse.hoverLabel = "Show less"
            clear.boxShadowToken = "thumbnail-card-shadow"; collapse.boxShadowToken = "thumbnail-card-shadow"
            addSubview(collapse); addSubview(clear)
            collapseButton = collapse; clearButton = clear
        }

        if !collapsed {
            let newestAtTop = topAnchor
            let destinationY = newestAtTop ? 0 : max(0, document.bounds.height - scroll.contentView.bounds.height)
            scroll.contentView.scroll(to: NSPoint(x: 0, y: destinationY))
            scroll.reflectScrolledClipView(scroll.contentView)
            updateOverflowCues()
        }
        addSubview(tooltipView)
        setAccessibilityRole(.group)
        setAccessibilityLabel(ids.count == 1 ? "Screenshot mini preview" : "\(ids.count) screenshot mini previews")
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    deinit { carryTimer?.invalidate() }

    func setStatus(_ value: String, detail: String? = nil, for artifactID: String) {
        cards[artifactID]?.setStatus(value, detail: detail)
    }

    /// Play `id`'s exit in its slot: the Close streak, or dust from the trash
    /// control (the scale-and-fade fallback when chips cannot be built).
    /// Returns how long the slot is held and when survivors may start to
    /// settle, or nil when nothing plays (reduced motion, a hidden panel, a
    /// compact pile, or an unknown card).
    func playExit(for id: String, kind: MiniPreviewExitKind, delay: Double = 0,
                  textures: DustTextures? = nil) -> PreviewMotionTables.Exit? {
        guard let card = cards[id], !card.isExiting, !stackCollapsed, window?.isVisible == true,
              !NativeMotion.reduceMotion else { return nil }
        let tables = PreviewMotionTables.shared
        if tooltipOwner?.isDescendant(of: card) == true { tooltipOwner = nil; tooltipView.isHidden = true }
        // Paint above the survivors that slide into the slot.
        if let superview = card.superview {
            card.removeFromSuperview(); superview.addSubview(card)
        }
        card.beginExit()
        exitingArtifactIDs.insert(id)
        switch kind {
        case .dismiss:
            NativeMotion.play("preview_dismiss", on: card, tokens: tokens, holdEnd: true, delay: delay,
                              mirrorX: anchoredRight, key: "preview-exit")
            card.playDismissStreak(delay: delay)
            return PreviewMotionTables.Exit(hold: delay + tables.dismiss.hold,
                                            settleDelay: delay + tables.dismiss.settleDelay)
        case .dust:
            if let textures, playDust(on: card, textures: textures) { return tables.dust }
            NativeMotion.play("preview_delete_fallback", on: card, tokens: tokens, holdEnd: true,
                              key: "preview-exit")
            return tables.fallback
        }
    }

    private func playDust(on card: MiniPreviewCardView, textures: DustTextures) -> Bool {
        let scale = window?.backingScaleFactor ?? 2
        let pad = PreviewMotionTables.shared.dustPad
        guard pad > 0, let source = card.dustSource(scale: scale) else { return false }
        let seed = UInt32(truncatingIfNeeded: card.artifactID.unicodeScalars.reduce(5381) { ($0 &* 33) &+ Int($1.value) })
        let particles = PreviewMotionTables.dustParticles(card: card.bounds.size,
            image: NSSize(width: source.width, height: source.height), origin: card.deleteOrigin, seed: seed)
        guard !particles.isEmpty,
              let chips = try? textures.prepare(source: source, particles: particles, scale: scale, atlas: true),
              chips.count == particles.count else { return false }
        let overlay = PreviewMotionOverlay(frame: card.frame.insetBy(dx: -pad, dy: -pad))
        card.superview?.addSubview(overlay, positioned: .above, relativeTo: card)
        DustDissolve.play(in: overlay.content, chips: chips, particles: particles, pad: pad,
                          radius: tokens.number("thumbnail-card-radius"), scale: scale)
        card.fadeForDust()
        dustOverlays.append(overlay)
        return true
    }

    /// Older cards slide one slot toward the stack anchor into `id`'s slot
    /// after `delay`: bottom-anchored stacks move them down, top-anchored up.
    /// Cards already leaving keep their place.
    func settleSurvivors(into id: String, after delay: Double) {
        guard let index = artifactIDs.firstIndex(of: id) else { return }
        let older = Array(artifactIDs[..<index])
        let shift = anchoredAtTop ? -cardSlot : cardSlot
        DispatchQueue.main.asyncAfter(deadline: .now() + max(0, delay)) { [weak self] in
            guard let self, self.window?.isVisible == true else { return }
            let settle = NativeMotion.transition("preview_stack_settle", tokens: self.tokens)
            NSAnimationContext.runAnimationGroup { context in
                context.duration = settle.duration
                context.timingFunction = settle.timing
                for id in older where !self.exitingArtifactIDs.contains(id) {
                    guard let card = self.cards[id] else { continue }
                    card.animator().setFrameOrigin(NSPoint(x: card.frame.minX, y: card.frame.minY + shift))
                }
            }
        }
    }

    /// Play a stack toolbar motion on Clear all and Show less. The controls
    /// ignore clicks while it plays, as shipping does; `holdEnd` keeps a
    /// leaving toolbar on its last keyframe until the stack rebuilds, and an
    /// entering one unlocks afterwards.
    func playToolbar(_ name: String, holdEnd: Bool) {
        let seconds = stackToolbarButtons.map {
            NativeMotion.play(name, on: $0, tokens: tokens, holdEnd: holdEnd, key: "preview-toolbar")
        }.max() ?? 0
        guard seconds > 0 else { return }
        stackToolbarButtons.forEach { $0.isEnabled = false }
        guard !holdEnd else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { [weak self] in
            self?.stackToolbarButtons.forEach { $0.isEnabled = true }
        }
    }

    /// Screen rects of every card, carried across a rebuild for the list ↔
    /// pile flight.
    func cardScreenFrames() -> [String: NSRect] {
        guard let window else { return [:] }
        return cards.mapValues { window.convertToScreen($0.convert($0.bounds, to: nil)) }
    }

    /// Each card's media blur on screen, carried across a rebuild so expand
    /// starts from the pile's live blur.
    func cardMediaBlurs() -> [String: Double] { cards.mapValues(\.currentDepthBlur) }

    /// Shipping `thumbnail-card-expand` / minimize run: cards fly between the
    /// list and the pile with the compact look (chrome hidden, depth shade
    /// easing) over the shipping 0.52 s. Collapsing flies from the laid-out
    /// frames to `screenFrames`; expanding flies from them. Returns seconds.
    @discardableResult
    func playFly(_ screenFrames: [String: NSRect], collapsing: Bool, depths: [String: Int],
                 blurs: [String: Double] = [:]) -> Double {
        let fly = NativeMotion.transition("preview_stack_fly", tokens: tokens)
        guard let window, window.isVisible, fly.duration > 0 else { return 0 }
        overflowCues.forEach { $0.isHidden = true }
        var moves: [(MiniPreviewCardView, NSRect)] = []
        for (id, card) in cards {
            guard let screen = screenFrames[id], let superview = card.superview else { continue }
            let local = superview.convert(window.convertFromScreen(screen), from: nil)
            let laid = card.frame
            let pile = NSRect(origin: local.origin, size: laid.size)
            card.setCompact(true, depth: depths[id] ?? 0)
            card.setDepthShade(visible: !collapsing, animated: false)
            // The rest blur rides the flight in while collapsing. Expanding
            // clears the blur each card had on screen (shipping
            // `thumbnail-card-expand-blur` from `--thumbnail-stack-expand-blur-from`,
            // usually the fanned pile's), else the rest blur.
            let rest = captures_preview_pile_media_blur_v1(depths[id] ?? 0, false)
            card.setDepthBlur(collapsing ? rest : 0, from: collapsing ? 0 : blurs[id] ?? rest,
                              duration: fly.duration, timing: fly.timing)
            // Expanding drops the hovered pile's glow over `--dur-4`.
            if !collapsing { card.setPileGlow(false, duration: Double(tokens.number("dur-4")) / 1000) }
            if !collapsing { card.frame = pile }
            moves.append((card, collapsing ? pile : laid))
        }
        flyToken &+= 1
        let token = flyToken
        NSAnimationContext.runAnimationGroup { context in
            context.duration = fly.duration
            context.timingFunction = fly.timing
            for (card, target) in moves {
                card.animator().frame = target
                card.setDepthShade(visible: collapsing, animated: true)
            }
        }
        // Expanded cards get their labels back when the flight ends on the
        // main-queue clock (the same clock as the collapse rebuild), not on
        // Core Animation's completion, which never arrives while the window
        // server is not compositing the panel (an occluded panel).
        if !collapsing {
            DispatchQueue.main.asyncAfter(deadline: .now() + fly.duration) { [weak self] in
                guard let self, self.flyToken == token else { return }
                for (card, _) in moves { card.setCompact(false, depth: 0) }
            }
        }
        return fly.duration
    }

    /// Resume a new card's capture highlight on this (possibly rebuilt) stack.
    func playCaptureHighlight(for artifactID: String, startedAgo: Double = 0) {
        cards[artifactID]?.playCaptureHighlight(startedAgo: startedAgo)
    }

    func setCopyFailed(_ failed: Bool, for artifactID: String) { cards[artifactID]?.copyFailed = failed }

    func warningText(for artifactID: String) -> String? { cards[artifactID]?.warningText }

    /// Focus-driven chrome follows the panel's key state (`MiniPreviewPanel`).
    func keyStateChanged() { cards.values.forEach { $0.refreshChrome() } }

    /// Shipping DOM order: the collapsed pile's expand control, the stack
    /// toolbar (Clear all, Minimize), the overflow cues, then each card and
    /// its controls (Close, Delete, Edit, Copy, Save file or Show in Folder).
    /// Focus starts on the first expanded card, else the expand control.
    func installKeyViewLoop(in window: NSWindow) {
        var order = ([pileExpandButton, clearButton, collapseButton] as [NSView?]).compactMap { $0 }
        order += overflowCues.map { $0 as NSView }
        order.append(scroll)
        let card = KeyViewLoop.candidates(in: scroll).first { $0 is MiniPreviewCardView }
        KeyViewLoop.install(order, window: window, initial: card)
    }

    func setEditorPhase(_ phase: UInt32, for artifactID: String, animated: Bool) {
        cards[artifactID]?.setEditorPhase(phase, animated: animated)
    }

    func editControl(for artifactID: String) -> MiniPreviewButton? { cards[artifactID]?.editControl }

    func card(for artifactID: String) -> MiniPreviewCardView? { cards[artifactID] }

    /// Shipping instant glass tip under (or, in bottom-anchored stacks, over)
    /// a hovered or focused icon. It fades and nudges 2 pt over the shipping
    /// tooltip transition and never takes the mouse.
    func setTooltip(for button: MiniPreviewButton, visible: Bool) {
        let spec = NativeMotion.transition(button.kind == .clear ? "preview_stack_tooltip"
                                           : "preview_icon_tooltip", tokens: tokens)
        let duration = window?.isVisible == true ? spec.duration : 0
        guard visible, let text = button.tooltipText, !button.isHidden else {
            guard tooltipOwner === button else { return }
            tooltipOwner = nil
            guard duration > 0 else {
                tooltipView.alphaValue = 0; tooltipView.isHidden = true
                return
            }
            NSAnimationContext.runAnimationGroup({ context in
                context.duration = duration; context.timingFunction = spec.timing
                tooltipView.animator().alphaValue = 0
            }, completionHandler: { [weak self] in
                guard let self, self.tooltipOwner == nil else { return }
                self.tooltipView.isHidden = true
            })
            return
        }
        tooltipOwner = button
        tooltipView.label.stringValue = text
        let size = tooltipView.label.intrinsicContentSize
        let anchor = button.convert(button.bounds, to: self)
        // Shared rule: bottom-anchored stacks open tips above their icons.
        let above = !anchoredAtTop
        func tipFrame(_ progress: Double) -> NSRect {
            let value = captures_preview_icon_tooltip_frame_v1(
                CapturesTrayNoticeRect(x: Double(anchor.minX), y: Double(anchor.minY),
                                       width: Double(anchor.width), height: Double(anchor.height)),
                Double(size.width), Double(size.height), above, progress)
            return NSRect(x: value.x, y: value.y, width: value.width, height: value.height)
        }
        let wasHidden = tooltipView.isHidden || tooltipView.alphaValue == 0
        tooltipView.isHidden = false
        tooltipView.needsLayout = true
        guard duration > 0 else {
            tooltipView.frame = tipFrame(1); tooltipView.alphaValue = 1
            return
        }
        if wasHidden { tooltipView.frame = tipFrame(0); tooltipView.alphaValue = 0 }
        NSAnimationContext.runAnimationGroup { context in
            context.duration = duration; context.timingFunction = spec.timing
            tooltipView.animator().frame = tipFrame(1)
            tooltipView.animator().alphaValue = 1
        }
    }

    override func updateTrackingAreas() {
        if let pointerTracking { removeTrackingArea(pointerTracking) }
        let next = NSTrackingArea(rect: .zero,
            options: [.mouseMoved, .mouseEnteredAndExited, .activeAlways, .inVisibleRect],
            owner: self, userInfo: nil)
        addTrackingArea(next); pointerTracking = next
        super.updateTrackingAreas()
    }

    override func mouseMoved(with event: NSEvent) {
        samplePointer(convert(event.locationInWindow, from: nil))
    }

    override func mouseExited(with event: NSEvent) {
        // Only this view's own area means the pointer left the stack; other
        // views forward their exits up the responder chain.
        guard event.type == .mouseExited, event.trackingArea === pointerTracking else { return }
        samplePointer(nil)
    }

    /// Hold card hover off after an expand or a new capture until the pointer
    /// really moves. A pointer already outside the stack releases it at once.
    func lockCardHover(releaseIfPointerOutside: Bool = true) {
        _ = captures_preview_hover_lock_v1(&hoverLock, UInt32(CAPTURES_HOVER_LOCK_LOCK), 0, 0)
        cards.values.forEach { $0.hoverLockChanged() }
        if releaseIfPointerOutside, let window {
            let point = convert(window.mouseLocationOutsideOfEventStream, from: nil)
            samplePointer(bounds.contains(point) ? point : nil)
        }
    }

    /// Feed a pointer sample in this view's coordinates (nil when outside).
    func samplePointer(_ point: NSPoint?) {
        let wasLocked = hoverLock.locked
        if let point {
            _ = captures_preview_hover_lock_v1(&hoverLock, UInt32(CAPTURES_HOVER_LOCK_POINTER),
                                               Double(point.x), Double(point.y))
        } else {
            _ = captures_preview_hover_lock_v1(&hoverLock, UInt32(CAPTURES_HOVER_LOCK_POINTER_OUTSIDE), 0, 0)
        }
        if wasLocked != hoverLock.locked { cards.values.forEach { $0.hoverLockChanged() } }
    }

    @objc private func scrollBoundsChanged(_ notification: Notification) { updateOverflowCues() }

    /// Show a cue only on an edge that hides cards (shared 1 px tolerance).
    func updateOverflowCues() {
        let edges = captures_preview_overflow_v1(Double(scrollOffsetY), Double(documentHeight),
                                                 Double(viewportHeight))
        for cue in overflowCues {
            let bit = UInt32(cue.above ? CAPTURES_PREVIEW_OVERFLOW_ABOVE : CAPTURES_PREVIEW_OVERFLOW_BELOW)
            cue.isHidden = edges & bit == 0
        }
    }

    /// Shipping `scrollStackBy`: scroll whole card slots, clamped to the content.
    func scrollStack(bySlots slots: Int32) {
        let target = captures_preview_scroll_target_v1(Double(scrollOffsetY), Double(documentHeight),
                                                       Double(viewportHeight), slots)
        let clip = scroll.contentView
        let duration = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
            ? 0 : Double(tokens.number("dur-3")) / 1000
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = duration
            context.timingFunction = CAMediaTimingFunction(name: .easeOut)
            clip.animator().setBoundsOrigin(NSPoint(x: 0, y: CGFloat(target)))
        }, completionHandler: { [weak self] in
            guard let self else { return }
            self.scroll.reflectScrolledClipView(self.scroll.contentView)
            self.updateOverflowCues()
        })
    }

    func updateSavedState(_ saved: Bool, for artifactID: String) {
        cards[artifactID]?.updateSaveButton(saved: saved)
    }

    func showSavedFeedback(for artifactID: String) { cards[artifactID]?.showSavedFeedback() }

    func setClipboardOwner(_ owner: String?) {
        for (id, card) in cards { card.setClipboardCurrent(id == owner) }
    }

    func isClipboardCurrent(for artifactID: String) -> Bool { cards[artifactID]?.clipboardCurrent == true }

    func metadataText(for artifactID: String) -> String? { cards[artifactID]?.metadataText }

    func setPreparedDragPath(_ path: String?, for artifactID: String) {
        cards[artifactID]?.preparedDragPath = path
    }

    func containsPreparedDragPath(_ path: String) -> Bool {
        cards.values.contains { $0.preparedDragPath == path }
    }

    func setDragEnded(_ callback: @escaping (String, NSPoint, NSDragOperation) -> Void) {
        for (id, card) in cards {
            card.dragEnded = { point, operation in callback(id, point, operation) }
        }
    }

    func rejectDrop(for artifactID: String) { cards[artifactID]?.rejectDrop() }

    /// Shipping `thumbnail-arrive`: a newly decoded card rises, fades in and
    /// sharpens from its 3 px blur. Presentation-only; skipped under Reduce Motion.
    @discardableResult func playArrival(for artifactID: String) -> Bool {
        guard let card = cards[artifactID] else { return false }
        card.playArrivalBlur()
        return NativeMotion.play("preview_card_arrive", on: card, tokens: tokens) > 0
    }

    func statusText(for artifactID: String) -> String? { cards[artifactID]?.statusText }

    func activatePileExpand() { pileExpandButton?.performClick(nil) }

    /// Shipping `thumbnail-stack-sparkle`: two dot layers drift up over the
    /// hovered pile, looping until the pointer leaves.
    private func setSparkling(_ active: Bool) {
        guard let groups = sparkles?.content.sublayers else { return }
        sparkling = false
        for group in groups {
            group.removeAnimation(forKey: "preview-sparkle")
            guard active else { continue }
            let name = group.name == "early" ? "preview_pile_sparkle" : "preview_pile_sparkle_late"
            if NativeMotion.play(name, onLayer: group, down: 1, tokens: tokens, repeats: true,
                                 key: "preview-sparkle") > 0 {
                sparkling = true
            }
        }
    }

    func setPileHovered(_ hovered: Bool) {
        guard hovered != pileHovered else { return }
        pileHovered = hovered
        setSparkling(hovered)
        animatePile(hovered: hovered)
    }

    /// Ease every card to its rest or fanned pose from where it is now over
    /// `--stack-fan-dur` / `--stack-fan-ease`, each layer 16 ms later per
    /// depth: the lift cascades instead of moving as a slab.
    private func animatePile(hovered: Bool) {
        let fan = NativeMotion.transition("preview_stack_fan", tokens: tokens)
        for (id, card) in cards {
            guard let layout = (hovered ? hoverLayouts[id] : restLayouts[id]) else { continue }
            let origin = pileOrigin(id, layout: layout, hovered: hovered)
            guard fan.duration > 0, window?.isVisible == true, let layer = card.layer else {
                card.setFrameOrigin(origin)
                continue
            }
            let from = (layer.presentation() ?? layer).position
            CATransaction.begin(); CATransaction.setDisableActions(true)
            card.setFrameOrigin(origin)
            CATransaction.commit()
            let move = CABasicAnimation(keyPath: "position")
            move.fromValue = NSValue(point: from)
            move.toValue = NSValue(point: layer.position)
            move.duration = fan.duration
            move.timingFunction = fan.timing
            move.beginTime = layer.convertTime(CACurrentMediaTime(), from: nil)
                + captures_preview_fan_delay_ms_v1(layout.depth, false) / 1000
            move.fillMode = .backwards
            layer.add(move, forKey: "pile-fan-position")
        }
        applyPilePoses(hovered: hovered, duration: fan.duration, timing: fan.timing)
    }

    /// Dragging the pile re-derives gravity (shipping updates it per move),
    /// so the spin fades in toward the middle of the screen.
    func updatePileGravity(_ gravity: Double) {
        guard stackCollapsed, gravity != pileGravity else { return }
        pileGravity = gravity
        for (id, card) in cards {
            guard let layout = (pileHovered ? hoverLayouts[id] : restLayouts[id]) else { continue }
            card.setFrameOrigin(pileOrigin(id, layout: layout, hovered: pileHovered))
        }
        applyPilePoses(hovered: pileHovered, duration: 0)
        if carryReady { applyCarryPoses() }
    }

    // Shipping drag sway (`.thumbnail-stack-drag-sway`): once a carry has
    // held the fan open for its gather (`thumbnailStackFanCollapseMs`), the
    // rear cards lean with the pointer's velocity through the shared spring;
    // dropping eases them back over the fan's staggered transition.
    private var carryStarted: CFTimeInterval?
    private var carryReady = false
    private var carrySway = CapturesDragSway()
    private var carryPointer: NSPoint?
    private var carrySample: (point: NSPoint, time: CFTimeInterval)?
    private var carryTimer: Timer?
    /// The carried pile's lean now, in points (y down), for tests.
    var pileLean: (x: Double, y: Double) { (carrySway.position_x, carrySway.position_y) }

    /// A carry's desktop pointer (screen points), or nil when it ends.
    func carryPile(at point: NSPoint?) {
        guard stackCollapsed else { return }
        guard let point else { endCarry(); return }
        carryPointer = point
        if carryStarted == nil {
            carryStarted = CACurrentMediaTime(); carryReady = false
            carrySway = CapturesDragSway(); carrySample = nil
        }
        guard !NativeMotion.reduceMotion, carryTimer == nil else { return }
        let timer = Timer(timeInterval: 1.0 / 60, repeats: true) { [weak self] _ in self?.stepCarry() }
        RunLoop.main.add(timer, forMode: .common)
        carryTimer = timer
    }

    /// One display frame of the carry: wait out the gather, then tick the
    /// lean with the pointer's step since the last frame.
    func stepCarry(now: CFTimeInterval = CACurrentMediaTime()) {
        guard let started = carryStarted, let pointer = carryPointer else { stopCarryTimer(); return }
        if !carryReady {
            let fan = NativeMotion.transition("preview_stack_fan", tokens: tokens)
            let deepest = restLayouts.values.map(\.depth).max() ?? 0
            guard now - started >= fan.duration + captures_preview_fan_delay_ms_v1(deepest, false) / 1000
            else { return }
            carryReady = true; carrySway = CapturesDragSway(); carrySample = (point: pointer, time: now)
        }
        let last = carrySample ?? (point: pointer, time: now - 0.016)
        // Screen y points up; the shipping lean's y points down.
        let settled = captures_preview_drag_sway_tick_v1(&carrySway, Double(pointer.x - last.point.x),
            Double(last.point.y - pointer.y), (now - last.time) * 1000, NativeMotion.reduceMotion)
        carrySample = (point: pointer, time: now)
        applyCarryPoses()
        // A still pointer lets the timer stop; the next sample restarts it.
        if settled { stopCarryTimer() }
    }

    private func stopCarryTimer() { carryTimer?.invalidate(); carryTimer = nil }

    private func endCarry() {
        let leaned = carryReady && (carrySway.position_x != 0 || carrySway.position_y != 0)
        stopCarryTimer()
        carryStarted = nil; carryReady = false; carrySway = CapturesDragSway()
        carrySample = nil; carryPointer = nil
        if leaned { animatePile(hovered: pileHovered) }
    }

    /// The fanned pile with the current lean, applied at once.
    private func applyCarryPoses() {
        CATransaction.begin(); CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }
        for (id, card) in cards {
            guard let layout = hoverLayouts[id], layout.depth > 0, let layer = card.layer else { continue }
            var pose = CapturesPreviewPilePose()
            var matrix = [Double](repeating: 0, count: 9)
            let ok = id.withCString { name in
                matrix.withUnsafeMutableBufferPointer {
                    captures_preview_pile_sway_pose_v1(name, layout.depth, pileGravity, anchoredAtTop,
                        carrySway.position_x, carrySway.position_y, &pose, $0.baseAddress)
                }
            }
            guard ok else { continue }
            layer.removeAnimation(forKey: "pile-pose"); layer.removeAnimation(forKey: "pile-fan-position")
            card.setFrameOrigin(NSPoint(x: CGFloat(geometry.padding) + CGFloat(pose.dx),
                                        y: CGFloat(layout.y) + CGFloat(pose.slot_dy)))
            layer.transform = Self.pileTransform(projection: matrix, size: card.bounds.size,
                                                 anchorPoint: layer.anchorPoint, flipped: document.isFlipped)
        }
    }

    /// Shipping rear-card pile pose (`captures_preview_pile_pose_v1`).
    func pilePose(for id: String, depth: Int, hovered: Bool) -> CapturesPreviewPilePose? {
        guard depth > 0 else { return nil }
        var pose = CapturesPreviewPilePose()
        let ok = id.withCString {
            captures_preview_pile_pose_v1($0, depth, hovered, pileGravity, anchoredAtTop, &pose)
        }
        return ok ? pose : nil
    }

    /// The card origin: its slot plus the pose's recession, jitter and drift.
    private func pileOrigin(_ id: String, layout: CapturesPreviewCardLayout, hovered: Bool) -> NSPoint {
        let pose = pilePose(for: id, depth: layout.depth, hovered: hovered)
        return NSPoint(x: CGFloat(geometry.padding) + CGFloat(pose?.dx ?? 0),
                       y: CGFloat(layout.y) + CGFloat(pose?.slot_dy ?? 0))
    }

    /// Shipping rear-card 3D pose (`captures_preview_pile_projection_v1`).
    func pileProjection(for id: String, depth: Int, hovered: Bool) -> [Double]? {
        guard depth > 0 else { return nil }
        var matrix = [Double](repeating: 0, count: 9)
        let ok = id.withCString { name in
            matrix.withUnsafeMutableBufferPointer {
                captures_preview_pile_projection_v1(name, depth, hovered, pileGravity, anchoredAtTop, $0.baseAddress)
            }
        }
        return ok ? matrix : nil
    }

    /// The shared pose's projective map (row-major 3×3 on card-centre points,
    /// y down) as a layer transform about the card centre. Core Animation
    /// divides by w, so the `rotateX` tilt keeps its keystone.
    static func pileTransform(projection m: [Double], size: CGSize, anchorPoint: CGPoint,
                              flipped: Bool) -> CATransform3D {
        guard m.count == 9 else { return CATransform3DIdentity }
        let centre = CGPoint(x: (0.5 - anchorPoint.x) * size.width, y: (0.5 - anchorPoint.y) * size.height)
        // A y-up layer sees the y-down map conjugated by a y flip.
        let f: Double = flipped ? 1 : -1
        var map = CATransform3DIdentity
        map.m11 = CGFloat(m[0]); map.m21 = CGFloat(m[1] * f); map.m41 = CGFloat(m[2])
        map.m12 = CGFloat(m[3] * f); map.m22 = CGFloat(m[4]); map.m42 = CGFloat(m[5] * f)
        map.m14 = CGFloat(m[6]); map.m24 = CGFloat(m[7] * f); map.m44 = CGFloat(m[8])
        var transform = CATransform3DMakeTranslation(-centre.x, -centre.y, 0)
        transform = CATransform3DConcat(transform, map)
        return CATransform3DConcat(transform, CATransform3DMakeTranslation(centre.x, centre.y, 0))
    }

    private func applyPilePoses(hovered: Bool, duration: CFTimeInterval,
                                timing: CAMediaTimingFunction? = nil) {
        guard stackCollapsed else { return }
        for (id, card) in cards {
            guard let layout = (hovered ? hoverLayouts[id] : restLayouts[id]),
                  let layer = card.layer else { continue }
            let target = pileProjection(for: id, depth: layout.depth, hovered: hovered).map {
                Self.pileTransform(projection: $0, size: card.bounds.size, anchorPoint: layer.anchorPoint,
                                   flipped: document.isFlipped)
            } ?? CATransform3DIdentity
            let delay = captures_preview_fan_delay_ms_v1(layout.depth, false) / 1000
            card.setDepthBlur(captures_preview_pile_media_blur_v1(layout.depth, hovered), duration: duration,
                              timing: timing, delay: captures_preview_fan_delay_ms_v1(layout.depth, true) / 1000)
            card.setPileGlow(hovered, duration: duration, timing: timing, delay: delay)
            if duration > 0 {
                let animation = CABasicAnimation(keyPath: "transform")
                animation.fromValue = NSValue(caTransform3D: layer.presentation()?.transform ?? layer.transform)
                animation.toValue = NSValue(caTransform3D: target)
                animation.duration = duration
                animation.timingFunction = timing ?? CAMediaTimingFunction(name: .easeInEaseOut)
                animation.beginTime = layer.convertTime(CACurrentMediaTime(), from: nil) + delay
                animation.fillMode = .backwards
                layer.add(animation, forKey: "pile-pose")
            }
            layer.transform = target
        }
    }
}

final class MiniPreviewPanel: NSPanel {
    let previewView: MiniPreviewView
    /// Shipping's nonactivating panel is key-capable, so Tab reaches the card
    /// controls. Here it takes keyboard focus only while Captures is already
    /// active (Ctrl-F6, Cmd-`), never from a click over another app, and
    /// `becomesKeyOnlyIfNeeded` keeps clicks from taking focus from an editor.
    override var canBecomeKey: Bool { NSApp.isActive }
    override var canBecomeMain: Bool { false }
    /// Ordering the panel in already makes its initial card first responder;
    /// that focus shows chrome only once the panel is key (Ctrl-F6, Cmd-`).
    override func becomeKey() { super.becomeKey(); previewView.keyStateChanged() }
    override func resignKey() { super.resignKey(); previewView.keyStateChanged() }

    init(frame: NSRect, geometry: CapturesPreviewGeometry, contentHeight: Double,
         resources: [String: MiniPreviewResource], ids: [String],
         layouts: [String: CapturesPreviewCardLayout],
         hoverLayouts: [String: CapturesPreviewCardLayout] = [:], collapsed: Bool,
         topAnchor: Bool, rightAnchor: Bool, tokens: Tokens, pileGravity: Double? = nil,
         copy: @escaping (String) -> Void,
         save: @escaping (String) -> Void, open: @escaping (String) -> Void,
         trash: @escaping (String) -> Void, dismiss: @escaping (String) -> Void,
         discard: @escaping (String) -> Void = { _ in },
         setCollapsed: @escaping (Bool) -> Void,
         clearAll: @escaping () -> Void, move: @escaping (NSPoint) -> Void = { _ in }) {
        previewView = MiniPreviewView(geometry: geometry, contentHeight: contentHeight,
            resources: resources, ids: ids,
            layouts: layouts, hoverLayouts: hoverLayouts, collapsed: collapsed,
            topAnchor: topAnchor, rightAnchor: rightAnchor, tokens: tokens, pileGravity: pileGravity,
            copy: copy, save: save, open: open, trash: trash, dismiss: dismiss, discard: discard,
            setCollapsed: setCollapsed, clearAll: clearAll, move: move)
        super.init(contentRect: frame, styleMask: [.borderless, .nonactivatingPanel],
                   backing: .buffered, defer: false)
        isReleasedWhenClosed = false; isOpaque = false; backgroundColor = .clear
        hasShadow = true; level = .floating
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        hidesOnDeactivate = false; isMovable = false; contentView = previewView
        becomesKeyOnlyIfNeeded = true
        previewView.installKeyViewLoop(in: self)
        registerForDraggedTypes([.fileURL])
        setAccessibilityLabel(ids.count == 1 ? "Screenshot mini preview" : "Screenshot mini previews")
    }

    @objc func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        guard let url = sender.draggingPasteboard.string(forType: .fileURL).flatMap(URL.init(string:)),
              previewView.containsPreparedDragPath(url.path) else { return [] }
        return .copy
    }

    @objc func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        draggingEntered(sender).contains(.copy)
    }
}

/// How a card leaves the stack: Close's streak, or Delete's dust.
enum MiniPreviewExitKind { case dismiss, dust }

/// How a History Restore ended (`MiniPreviewController.restore`).
enum MiniPreviewRestoreOutcome {
    case shown
    /// The card was already in the stack; nothing moved.
    case alreadyShowing
    /// The card was dismissed, cleared or replaced before it decoded.
    case cancelled
    case failed(Error)
}

/// AppKit presentation for recent screenshots. Rust owns visibility,
/// membership/order/collapse, card layout, and monitor-relative placement.
final class MiniPreviewController {
    typealias ArtifactAction = (CaptureArtifact) -> Void

    private let policy: NativePreviewPolicy
    private let stack: NativePreviewStack
    private let tokens: Tokens
    private let imageLoader: (String) throws -> NSImage
    private let screenProvider: () -> [NSScreen]
    private var panel: MiniPreviewPanel?
    private var resources: [String: MiniPreviewResource] = [:]
    private var settings = MiniPreviewSettings(enabled: false, placement: "bottom_right",
                                               includeInCaptures: false)
    private var screenID: String?
    private(set) var stackOrigin: CapturesPreviewOrigin?
    private var pendingDecodes: [String: Int] = [:]
    private var cardGenerations: [String: Int] = [:]
    private var preparedDrags: [String: (imagePath: String, previewPath: String, path: String)] = [:]
    private var visibilityPendingArtifactID: String?
    private var nextDecodeToken = 0
    /// The capture this app last copied, valid until the pasteboard changes.
    private var clipboardOwner: (pasteboard: NSPasteboard, changeCount: Int, artifactID: String)?
    private var clipboardTimer: Timer?
    /// Captures an open screenshot editor window shows (visible or minimized).
    private var editorArtifactIDs: Set<String> = []
    private var editorPresence: [String: CapturesEditorPresence] = [:]
    private var editorPresenceWake: DispatchWorkItem?
    /// The rebuild that ends running exits and flights (the panel keeps its
    /// old layout until then), and when it is due.
    private var viewTransition: DispatchWorkItem?
    private var viewTransitionDeadline: CFTimeInterval = 0
    /// When each card arrived, so a rebuilt stack resumes its highlight.
    private var arrivals: [String: CFTimeInterval] = [:]
    /// Captures whose last clipboard copy failed ("Clipboard unavailable").
    private var copyFailures: Set<String> = []
    /// One dust renderer per preview surface, made on the first Delete.
    private lazy var dustTextures: DustTextures? = try? DustTextures()
    /// Saved cards whose Delete dissolved before their export moves to the
    /// Trash (shipping's order); a failed Trash puts them back in their slot.
    private var trashing: [String: (resource: MiniPreviewResource, index: Int, generation: Int?,
                                    screenID: String?, origin: CapturesPreviewOrigin?)] = [:]
    var copyArtifact: ArtifactAction = { _ in }
    var saveArtifact: ArtifactAction = { _ in }
    var openArtifact: ArtifactAction = { _ in }
    var trashArtifact: ArtifactAction = { _ in }
    var prepareDrag: (CaptureArtifact, @escaping (String?) -> Void) -> Void = { _, completion in
        completion(nil)
    }
    var presentedArtifactID: String? { stack.ids.last }
    var presentedArtifactIDs: [String] { stack.ids }
    var decodedArtifactIDs: [String] { stack.ids.filter { resources[$0] != nil } }
    var isCollapsed: Bool { stack.isCollapsed }
    var isPanelVisible: Bool { panel?.isVisible == true }
    /// The stack on screen, for tests (it may still show exiting cards).
    var previewView: MiniPreviewView? { panel?.previewView }
    func statusText(for artifactID: String) -> String? {
        panel?.previewView.statusText(for: artifactID)
    }
    func isClipboardCurrent(for artifactID: String) -> Bool {
        panel?.previewView.isClipboardCurrent(for: artifactID) == true
    }
    func metadataText(for artifactID: String) -> String? {
        panel?.previewView.metadataText(for: artifactID)
    }

    /// Record that `artifactID` was just written to `pasteboard`.
    func recordClipboardCopy(artifactID: String, pasteboard: NSPasteboard) {
        precondition(Thread.isMainThread)
        clipboardOwner = (pasteboard, pasteboard.changeCount, artifactID)
        refreshClipboardOwner()
    }

    func showSavedFeedback(for artifactID: String) {
        panel?.previewView.showSavedFeedback(for: artifactID)
    }

    /// A copy of `artifactID` failed or succeeded: shipping warns
    /// "Clipboard unavailable" on the card until a copy works.
    func recordCopyResult(artifactID: String, succeeded: Bool) {
        precondition(Thread.isMainThread)
        if succeeded { copyFailures.remove(artifactID) } else if stack.ids.contains(artifactID) {
            copyFailures.insert(artifactID)
        }
        panel?.previewView.setCopyFailed(!succeeded && stack.ids.contains(artifactID), for: artifactID)
    }

    func warningText(for artifactID: String) -> String? { panel?.previewView.warningText(for: artifactID) }

    /// Whether exits or a flight are still playing before the stack rebuilds.
    var isTransitioning: Bool { viewTransition != nil }

    /// Rebuild once every running exit or flight has ended.
    private func scheduleRebuild(after seconds: Double) {
        let deadline = CACurrentMediaTime() + seconds
        guard viewTransition == nil || deadline > viewTransitionDeadline else { return }
        viewTransition?.cancel()
        viewTransitionDeadline = deadline
        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.viewTransition = nil
            self.finishView()
        }
        viewTransition = work
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: work)
    }

    /// End running exits and flights now.
    private func settleTransitions() {
        guard let work = viewTransition else { return }
        work.cancel(); viewTransition = nil
        finishView()
    }

    /// Show the stack as it now is: rebuilt, or closed when empty.
    private func finishView() {
        if stack.ids.isEmpty { panel?.close(); panel = nil; screenID = nil; stackOrigin = nil }
        else { makePanel(); updateVisibility() }
    }

    /// The host's open screenshot editors changed: cards whose capture is in
    /// one show the "In editor" pill and ring; a closed editor plays the
    /// shared leave and Edit linger.
    func setEditorArtifacts(_ ids: Set<String>) {
        precondition(Thread.isMainThread)
        editorArtifactIDs = ids
        updateEditorPresence(animated: true)
    }

    func editorPhase(for artifactID: String) -> UInt32 {
        editorPresence[artifactID]?.phase ?? UInt32(CAPTURES_EDITOR_PHASE_IDLE)
    }

    private static var presenceClockMs: Double { ProcessInfo.processInfo.systemUptime * 1000 }

    private func updateEditorPresence(animated: Bool) {
        editorPresenceWake?.cancel(); editorPresenceWake = nil
        let now = Self.presenceClockMs
        let reduced = NativeMotion.reduceMotion
        var wake: Double?
        var next: [String: CapturesEditorPresence] = [:]
        for id in stack.ids {
            var presence = editorPresence[id]
                ?? CapturesEditorPresence(active: false, phase: UInt32(CAPTURES_EDITOR_PHASE_IDLE), since_ms: now)
            if captures_preview_editor_presence_update_v1(&presence, editorArtifactIDs.contains(id), now, reduced) {
                panel?.previewView.setEditorPhase(presence.phase, for: id, animated: animated)
            }
            next[id] = presence
            let wait = captures_preview_editor_presence_next_v1(presence, now, reduced)
            if wait >= 0 { wake = min(wake ?? wait, wait) }
        }
        editorPresence = next
        guard let wake else { return }
        let work = DispatchWorkItem { [weak self] in self?.updateEditorPresence(animated: true) }
        editorPresenceWake = work
        DispatchQueue.main.asyncAfter(deadline: .now() + wake / 1000 + 0.001, execute: work)
    }

    /// Forget ownership once another write changes the pasteboard, then show
    /// the shipping chip only on the card that still owns it.
    func refreshClipboardOwner() {
        if let owner = clipboardOwner, owner.pasteboard.changeCount != owner.changeCount {
            clipboardOwner = nil
        }
        panel?.previewView.setClipboardOwner(clipboardOwner?.artifactID)
        if clipboardOwner != nil, panel != nil {
            guard clipboardTimer == nil else { return }
            let timer = Timer(timeInterval: 1, repeats: true) { [weak self] _ in
                self?.refreshClipboardOwner()
            }
            RunLoop.main.add(timer, forMode: .common); clipboardTimer = timer
        } else {
            clipboardTimer?.invalidate(); clipboardTimer = nil
        }
    }

    init(tokens: Tokens, policy: NativePreviewPolicy = NativePreviewPolicy(),
         stack: NativePreviewStack = NativePreviewStack(),
         screenProvider: @escaping () -> [NSScreen] = { NSScreen.screens },
         imageLoader: @escaping (String) throws -> NSImage = MiniPreviewController.loadImage) {
        self.tokens = tokens; self.policy = policy; self.stack = stack
        self.screenProvider = screenProvider; self.imageLoader = imageLoader
    }

    func beginCapture(settings: MiniPreviewSettings) -> UInt64? {
        precondition(Thread.isMainThread)
        if !settings.enabled || self.settings.placement != settings.placement { stackOrigin = nil }
        self.settings = settings
        guard let generation = policy.beginCapture() else { return nil }
        policy.suppressCaptureUI(true)
        updateVisibility()
        return generation
    }

    func restoreCapture(generation: UInt64?) {
        precondition(Thread.isMainThread)
        if let generation, policy.restore(generation: generation) {
            visibilityPendingArtifactID = nil
        }
        policy.suppressCaptureUI(false)
        updateVisibility()
    }

    func present(_ artifact: CaptureArtifact, on screenID: String,
                 settings: MiniPreviewSettings, generation: UInt64?) {
        precondition(Thread.isMainThread)
        self.settings = settings; self.screenID = screenID
        guard let generation, policy.wait(generation: generation, artifact: artifact.id) else {
            restoreCapture(generation: generation); return
        }
        guard stack.insert(artifact.id) else {
            restoreCapture(generation: generation); return
        }
        visibilityPendingArtifactID = artifact.id
        policy.suppressCaptureUI(false)
        decode(artifact) { _ in }
    }

    /// Shipping History Restore (`restore_history_artifact`): bring a
    /// screenshot back as the front card without a capture generation or
    /// clipboard copy. A card already in the stack stays where it is, like
    /// shipping, which never duplicates or reorders it. An empty stack opens
    /// on `screenID`; otherwise the pile keeps its display and position.
    /// `completion` runs on the main queue once the card decoded, failed,
    /// or was dismissed first.
    func restore(_ artifact: CaptureArtifact, on screenID: String?, settings: MiniPreviewSettings,
                 completion: @escaping (MiniPreviewRestoreOutcome) -> Void) {
        precondition(Thread.isMainThread)
        guard !stack.ids.contains(artifact.id) else { completion(.alreadyShowing); return }
        if stack.ids.isEmpty {
            if !settings.enabled || self.settings.placement != settings.placement { stackOrigin = nil }
            self.screenID = screenID
        }
        self.settings = settings
        guard stack.insert(artifact.id) else { completion(.alreadyShowing); return }
        decode(artifact, completion: completion)
    }

    /// Decode a card just inserted into `stack` and present it.
    private func decode(_ artifact: CaptureArtifact,
                        completion: @escaping (MiniPreviewRestoreOutcome) -> Void) {
        nextDecodeToken &+= 1
        let decodeToken = nextDecodeToken
        pendingDecodes[artifact.id] = decodeToken
        cardGenerations[artifact.id] = decodeToken
        preparedDrags[artifact.id] = nil
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self else { return }
            let result = Result { try self.imageLoader(artifact.previewPath) }
            DispatchQueue.main.async { [weak self] in
                guard let self, self.pendingDecodes[artifact.id] == decodeToken,
                      self.stack.ids.contains(artifact.id) else { completion(.cancelled); return }
                self.pendingDecodes[artifact.id] = nil
                switch result {
                case .success(let image):
                    if self.policy.ready(artifact: artifact.id) {
                        self.visibilityPendingArtifactID = nil
                    }
                    self.resources[artifact.id] = MiniPreviewResource(artifact: artifact, image: image)
                    self.arrivals[artifact.id] = CACurrentMediaTime()
                    self.makePanel()
                    self.updateVisibility()
                    self.panel?.previewView.playArrival(for: artifact.id)
                    // A card appearing under a resting pointer waits for it to move.
                    self.panel?.previewView.lockCardHover()
                    self.prepareFileDrag(for: artifact)
                    completion(.shown)
                case .failure(let error):
                    if self.visibilityPendingArtifactID == artifact.id,
                       self.policy.stopWaiting() {
                        self.visibilityPendingArtifactID = nil
                    }
                    _ = self.stack.remove(artifact.id)
                    self.resources[artifact.id] = nil
                    self.makePanel()
                    self.updateVisibility()
                    completion(.failed(error))
                }
            }
        }
    }

    func updateSettings(_ settings: MiniPreviewSettings) {
        precondition(Thread.isMainThread)
        if !settings.enabled || self.settings.placement != settings.placement { stackOrigin = nil }
        self.settings = settings
        if !settings.enabled, stack.isCollapsed { stack.setCollapsed(false) }
        if !stack.ids.isEmpty { makePanel() }
        updateVisibility()
    }

    func reconcileHistory(ids: Set<String>) {
        precondition(Thread.isMainThread)
        var changed = false
        for id in Array(pendingDecodes.keys) where !ids.contains(id) {
            pendingDecodes[id] = nil
            changed = stack.remove(id) || changed
            if visibilityPendingArtifactID == id, policy.stopWaiting() {
                visibilityPendingArtifactID = nil
                changed = true
            }
        }
        let removed = stack.ids.filter { !ids.contains($0) }
        for id in removed {
            _ = stack.remove(id); resources[id] = nil; preparedDrags[id] = nil
            cardGenerations[id] = nil
        }
        if changed || !removed.isEmpty { makePanel(); updateVisibility() }
    }

    func setStatus(_ value: String, detail: String? = nil, for artifactID: String) {
        guard resources[artifactID] != nil else { return }
        panel?.previewView.setStatus(value, detail: detail, for: artifactID)
    }

    @discardableResult
    func updateSavedPath(_ path: String, for artifact: CaptureArtifact) -> Bool {
        precondition(Thread.isMainThread)
        guard var resource = resources[artifact.id],
              resource.artifact.imagePath == artifact.imagePath,
              resource.artifact.previewPath == artifact.previewPath,
              stack.ids.contains(artifact.id) else { return false }
        resource.artifact.savedPath = path
        resources[artifact.id] = resource
        panel?.previewView.updateSavedState(true, for: artifact.id)
        prepareFileDrag(for: resource.artifact)
        return true
    }

    func refreshArtifacts(_ artifacts: [CaptureArtifact]) {
        precondition(Thread.isMainThread)
        for artifact in artifacts {
            guard var resource = resources[artifact.id] else { continue }
            var refreshed = artifact
            // A History request may have started before an export completed. Do not let
            // that stale response turn a freshly saved preview back into Save.
            if refreshed.savedPath == nil { refreshed.savedPath = resource.artifact.savedPath }
            let changedExport = refreshed.savedPath != resource.artifact.savedPath
            resource.artifact = refreshed
            resources[artifact.id] = resource
            panel?.previewView.updateSavedState(refreshed.savedPath != nil, for: artifact.id)
            if changedExport { prepareFileDrag(for: refreshed) }
        }
    }

    private func prepareFileDrag(for artifact: CaptureArtifact) {
        guard let generation = cardGenerations[artifact.id] else { return }
        preparedDrags[artifact.id] = nil
        panel?.previewView.setPreparedDragPath(nil, for: artifact.id)
        prepareDrag(artifact) { [weak self] path in
            guard let self, let path, self.cardGenerations[artifact.id] == generation,
                  self.contains(artifact, savedPath: artifact.savedPath) else { return }
            self.preparedDrags[artifact.id] = (artifact.imagePath, artifact.previewPath, path)
            self.panel?.previewView.setPreparedDragPath(path, for: artifact.id)
        }
    }

    func contains(_ artifact: CaptureArtifact) -> Bool {
        guard let current = resources[artifact.id]?.artifact else { return false }
        return stack.ids.contains(artifact.id) && current.imagePath == artifact.imagePath
            && current.previewPath == artifact.previewPath
    }

    func contains(_ artifact: CaptureArtifact, savedPath: String?) -> Bool {
        contains(artifact) && resources[artifact.id]?.artifact.savedPath == savedPath
    }

    /// Remove a card. Close plays shipping's streak and Delete its dust in
    /// the card's slot while older cards settle into it; the stack rebuilds
    /// once the exit ends. `exit: nil` (a drag onto another app, a replaced
    /// original) removes it at once.
    @discardableResult
    func dismiss(_ artifactID: String, exit: MiniPreviewExitKind? = .dismiss) -> Double {
        precondition(Thread.isMainThread)
        let liveBefore = stack.ids.count
        guard stack.remove(artifactID) else { return 0 }
        pendingDecodes[artifactID] = nil
        if visibilityPendingArtifactID == artifactID, policy.stopWaiting() {
            visibilityPendingArtifactID = nil
        }
        resources[artifactID] = nil
        preparedDrags[artifactID] = nil
        cardGenerations[artifactID] = nil
        arrivals[artifactID] = nil; copyFailures.remove(artifactID)
        if let exit, let view = panel?.previewView,
           let played = view.playExit(for: artifactID, kind: exit,
                                      textures: exit == .dust ? dustTextures : nil) {
            // Fewer than two live cards: the stack toolbar leaves with the card.
            if liveBefore >= 2 && stack.ids.count < 2 { view.playToolbar("preview_toolbar_exit", holdEnd: true) }
            view.settleSurvivors(into: artifactID, after: played.settleDelay)
            if stack.ids.isEmpty { screenID = nil; stackOrigin = nil }
            scheduleRebuild(after: played.hold)
            return played.hold
        }
        finishView()
        return 0
    }

    /// A saved card's Delete: shipping dissolves the card first and moves its
    /// export to the Trash once the dust has played and the stack settled.
    /// Returns the seconds to wait before the Trash request, or nil when the
    /// card is not in the stack.
    func beginTrash(_ artifactID: String) -> Double? {
        precondition(Thread.isMainThread)
        guard let resource = resources[artifactID],
              let index = stack.ids.firstIndex(of: artifactID) else { return nil }
        trashing[artifactID] = (resource, index, cardGenerations[artifactID], screenID, stackOrigin)
        return dismiss(artifactID, exit: .dust)
    }

    /// The export is in the Trash: forget the dissolved card.
    func finishTrash(_ artifactID: String) {
        precondition(Thread.isMainThread)
        trashing[artifactID] = nil
    }

    /// The Trash failed: like shipping's unlocked card, it returns to its
    /// slot with the error, ready to retry. A card presented again under the
    /// same ID in the meantime stays as it is.
    @discardableResult
    func restoreTrashed(_ artifactID: String, status: String, detail: String? = nil) -> Bool {
        precondition(Thread.isMainThread)
        guard let pending = trashing.removeValue(forKey: artifactID), resources[artifactID] == nil,
              stack.restore(artifactID, at: pending.index) else { return false }
        resources[artifactID] = pending.resource
        cardGenerations[artifactID] = pending.generation
        if screenID == nil { screenID = pending.screenID }
        if stackOrigin == nil { stackOrigin = pending.origin }
        if viewTransition != nil { settleTransitions() } else { finishView() }
        prepareFileDrag(for: pending.resource.artifact)
        setStatus(status, detail: detail, for: artifactID)
        return true
    }

    /// Clear all: every card streaks out, bottom first, without settling, and
    /// the stack toolbar leaves with the first streak.
    func clearAll() {
        precondition(Thread.isMainThread)
        let snapshot = stack.ids
        guard !snapshot.isEmpty else { return }
        _ = stack.removeAll(snapshot)
        snapshot.forEach { id in
            resources[id] = nil; pendingDecodes[id] = nil
            preparedDrags[id] = nil; cardGenerations[id] = nil
            arrivals[id] = nil; copyFailures.remove(id)
        }
        if let pending = visibilityPendingArtifactID, snapshot.contains(pending),
           policy.stopWaiting() {
            visibilityPendingArtifactID = nil
        }
        let topAnchor = settings.placement.hasPrefix("top_")
        var hold: Double?
        if let view = panel?.previewView {
            for (index, id) in snapshot.enumerated() {
                let delay = captures_preview_clear_delay_ms_v1(snapshot.count, index, topAnchor) / 1000
                if let played = view.playExit(for: id, kind: .dismiss, delay: delay) {
                    hold = max(hold ?? 0, played.hold)
                }
            }
            if hold != nil { view.playToolbar("preview_toolbar_clear", holdEnd: true) }
        }
        if stack.ids.isEmpty { screenID = nil; stackOrigin = nil }
        if let hold { scheduleRebuild(after: hold) } else { finishView() }
    }

    /// Show less and expand. The cards fly between the list and the pile
    /// (shipping `thumbnail-card-expand` and the minimize run) while the stack
    /// toolbar leaves or enters; collapsing rebuilds once the pile is reached.
    func setCollapsed(_ collapsed: Bool) {
        precondition(Thread.isMainThread)
        guard stack.isCollapsed != collapsed else { return }
        settleTransitions()
        let before = panel?.isVisible == true ? panel?.previewView.cardScreenFrames() ?? [:] : [:]
        let blurs = panel?.isVisible == true ? panel?.previewView.cardMediaBlurs() ?? [:] : [:]
        stack.setCollapsed(collapsed)
        let depths = cardDepths()
        if collapsed, let view = panel?.previewView, let pile = pileScreenFrames(),
           view.playFly(pile, collapsing: true, depths: depths) > 0 {
            view.playToolbar("preview_toolbar_out", holdEnd: true)
            scheduleRebuild(after: NativeMotion.transition("preview_stack_fly", tokens: tokens).duration)
            return
        }
        makePanel(); updateVisibility()
        if !collapsed, !before.isEmpty, let view = panel?.previewView,
           view.playFly(before, collapsing: false, depths: depths, blurs: blurs) > 0 {
            view.playToolbar("preview_toolbar_in", holdEnd: false)
        }
        // Expanding leaves the pointer over a card it never hovered.
        if !collapsed { panel?.previewView.lockCardHover() }
    }

    private func cardDepths() -> [String: Int] {
        let count = stack.ids.count
        return Dictionary(uniqueKeysWithValues: stack.ids.enumerated().map { ($1, count - 1 - $0) })
    }

    /// Where each card sits in the collapsed pile, in screen coordinates.
    private func pileScreenFrames() -> [String: NSRect]? {
        let ids = stack.ids
        guard !ids.isEmpty, let screen = targetScreen(), let monitor = Self.monitor(for: screen),
              let geometry = NativePreviewLayout.geometry(monitor: monitor, count: ids.count,
                  collapsed: true, origin: stackOrigin, placement: settings.placement) else { return nil }
        let frame = Self.appKitFrame(geometry: geometry, monitor: monitor, screenFrame: screen.frame)
        let topAnchor = settings.placement.hasPrefix("top_")
        let padding = CGFloat(geometry.padding), height = CGFloat(geometry.card_height)
        var frames: [String: NSRect] = [:]
        for (index, id) in ids.enumerated() {
            guard let layout = stack.cardLayout(index: index, topAnchor: topAnchor) else { continue }
            frames[id] = NSRect(x: frame.minX + padding, y: frame.maxY - CGFloat(layout.y) - height,
                                width: frame.width - padding * 2, height: height)
        }
        return frames
    }

    func close() {
        precondition(Thread.isMainThread)
        viewTransition?.cancel(); viewTransition = nil
        _ = policy.stopWaiting(); visibilityPendingArtifactID = nil; screenID = nil
        stackOrigin = nil
        let ids = stack.ids; _ = stack.removeAll(ids)
        resources.removeAll(); pendingDecodes.removeAll()
        preparedDrags.removeAll(); cardGenerations.removeAll()
        arrivals.removeAll(); copyFailures.removeAll()
        panel?.close(); panel = nil
    }

    private func makePanel() {
        // A rebuild shows the current stack, ending any exit or flight.
        viewTransition?.cancel(); viewTransition = nil
        panel?.close(); panel = nil
        let ids = stack.ids
        if ids.isEmpty { stackOrigin = nil }
        guard !ids.isEmpty, let screen = targetScreen(),
              let monitor = Self.monitor(for: screen),
              let geometry = NativePreviewLayout.geometry(monitor: monitor, count: ids.count,
                  collapsed: stack.isCollapsed, origin: stackOrigin,
                  placement: settings.placement) else { return }
        let topAnchor = settings.placement.hasPrefix("top_")
        let rightAnchor = settings.placement.hasSuffix("_right")
        let layouts = Dictionary(uniqueKeysWithValues: ids.enumerated().compactMap { index, id in
            stack.cardLayout(index: index, topAnchor: topAnchor).map { (id, $0) }
        })
        let hoverLayouts = Dictionary(uniqueKeysWithValues: ids.enumerated().compactMap { index, id in
            stack.cardLayout(index: index, topAnchor: topAnchor, hovered: true).map { (id, $0) }
        })
        let frame = Self.appKitFrame(geometry: geometry, monitor: monitor,
                                     screenFrame: screen.frame)
        let gravity = NativePreviewLayout.gravity(monitor: monitor, count: ids.count,
            origin: stackOrigin, placement: settings.placement)
        let next = MiniPreviewPanel(frame: frame, geometry: geometry,
            contentHeight: stack.contentHeight,
            resources: resources, ids: ids, layouts: layouts, hoverLayouts: hoverLayouts,
            collapsed: stack.isCollapsed, topAnchor: topAnchor, rightAnchor: rightAnchor, tokens: tokens,
            pileGravity: gravity,
            copy: { [weak self] in self?.perform(\.copyArtifact, artifactID: $0) },
            save: { [weak self] in self?.perform(\.saveArtifact, artifactID: $0) },
            open: { [weak self] in self?.perform(\.openArtifact, artifactID: $0) },
            trash: { [weak self] in self?.perform(\.trashArtifact, artifactID: $0) },
            dismiss: { [weak self] in self?.dismiss($0) },
            discard: { [weak self] in self?.dismiss($0, exit: .dust) },
            setCollapsed: { [weak self] in self?.setCollapsed($0) },
            clearAll: { [weak self] in self?.clearAll() },
            move: { [weak self] in self?.moveStack(to: $0) })
        next.sharingType = settings.includeInCaptures ? .readOnly : .none
        for id in ids {
            guard let artifact = resources[id]?.artifact, let prepared = preparedDrags[id],
                  prepared.imagePath == artifact.imagePath,
                  prepared.previewPath == artifact.previewPath else { continue }
            next.previewView.setPreparedDragPath(prepared.path, for: id)
        }
        let sourceArtifacts = resources.mapValues(\.artifact)
        let sourceGenerations = cardGenerations
        next.previewView.setDragEnded { [weak self] id, point, operation in
            guard let self, let source = sourceArtifacts[id], self.contains(source),
                  self.cardGenerations[id] == sourceGenerations[id] else { return }
            if operation.contains(.copy), self.panel?.frame.contains(point) == true {
                self.panel?.previewView.rejectDrop(for: id)
            } else if operation.contains(.copy) && !NSApp.windows.contains(where: {
                $0 !== self.panel && $0.isVisible && $0.frame.contains(point)
            }) {
                self.dismiss(id, exit: nil)
            }
        }
        panel = next
        refreshClipboardOwner()
        updateEditorPresence(animated: false)
        for id in ids { next.previewView.setEditorPhase(editorPhase(for: id), for: id, animated: false) }
        let now = CACurrentMediaTime()
        let highlight = NativeMotion.duration("preview_capture_highlight", tokens: tokens)
        for id in ids {
            next.previewView.setCopyFailed(copyFailures.contains(id), for: id)
            // A card keeps its capture highlight across rebuilds.
            if let arrived = arrivals[id], now - arrived < highlight {
                next.previewView.playCaptureHighlight(for: id, startedAgo: now - arrived)
            }
        }
    }

    private func perform(_ action: KeyPath<MiniPreviewController, ArtifactAction>,
                         artifactID: String) {
        guard stack.ids.contains(artifactID), let artifact = resources[artifactID]?.artifact else { return }
        self[keyPath: action](artifact)
    }

    private func targetScreen() -> NSScreen? {
        let screens = screenProvider()
        if let screenID {
            return screens.first { Self.displayID(for: $0) == screenID } ?? screens.first
        }
        return screens.first
    }

    private func updateVisibility() {
        guard let panel else { return }
        panel.sharingType = settings.includeInCaptures ? .readOnly : .none
        // A panel playing exits or a flight keeps its frame until it rebuilds.
        if viewTransition == nil, let screen = targetScreen() { position(panel: panel, on: screen) }
        if policy.visible(count: stack.ids.count, enabled: settings.enabled,
                          includeInCaptures: settings.includeInCaptures) {
            panel.orderFrontRegardless()
        } else {
            panel.orderOut(nil)
        }
    }

    private func position(panel: NSPanel, on screen: NSScreen) {
        guard let monitor = Self.monitor(for: screen),
              let geometry = NativePreviewLayout.geometry(monitor: monitor, count: stack.ids.count,
                  collapsed: stack.isCollapsed, origin: stackOrigin,
                  placement: settings.placement) else { return }
        panel.setFrame(Self.appKitFrame(geometry: geometry, monitor: monitor,
                                       screenFrame: screen.frame), display: panel.isVisible)
    }

    func moveStack(to position: NSPoint) {
        settleTransitions()
        guard stack.isCollapsed, settings.enabled, let panel, panel.isVisible,
              let screen = targetScreen(), let monitor = Self.monitor(for: screen) else { return }
        let scale = max(1, monitor.scale_factor)
        let logical = NSPoint(x: Double(monitor.full_x) / scale + position.x - screen.frame.minX,
            y: Double(monitor.full_y) / scale + screen.frame.maxY - position.y - panel.frame.height)
        stackOrigin = NativePreviewLayout.movedOrigin(monitor: monitor, count: stack.ids.count,
            frameOrigin: logical, placement: settings.placement)
        self.position(panel: panel, on: screen)
        if let gravity = NativePreviewLayout.gravity(monitor: monitor, count: stack.ids.count,
                                                     origin: stackOrigin, placement: settings.placement) {
            panel.previewView.updatePileGravity(gravity)
        }
    }

    static func displayID(for screen: NSScreen) -> String? {
        (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.stringValue
    }

    static func monitor(for screen: NSScreen) -> CapturesPreviewMonitor? {
        guard let number = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber else { return nil }
        let display = CGDirectDisplayID(number.uint32Value)
        let full = CGDisplayBounds(display)
        let pixelsWide = CGFloat(CGDisplayPixelsWide(display))
        let scale = full.width > 0 ? max(1, pixelsWide / full.width) : max(1, screen.backingScaleFactor)
        let visible = screen.visibleFrame
        let workX = (full.minX + visible.minX - screen.frame.minX) * scale
        let workY = (full.minY + screen.frame.maxY - visible.maxY) * scale
        return CapturesPreviewMonitor(work_x: Int32(workX.rounded()), work_y: Int32(workY.rounded()),
            work_width: UInt32(max(0, (visible.width * scale).rounded())),
            work_height: UInt32(max(0, (visible.height * scale).rounded())),
            full_x: Int32((full.minX * scale).rounded()),
            full_y: Int32((full.minY * scale).rounded()),
            full_width: UInt32(max(0, (full.width * scale).rounded())),
            full_height: UInt32(max(0, (full.height * scale).rounded())),
            scale_factor: Double(scale))
    }

    static func appKitFrame(geometry: CapturesPreviewGeometry, monitor: CapturesPreviewMonitor,
                            screenFrame: NSRect) -> NSRect {
        let scale = max(1, monitor.scale_factor)
        let localX = geometry.x - Double(monitor.full_x) / scale
        let localTop = geometry.y - Double(monitor.full_y) / scale
        return NSRect(x: screenFrame.minX + localX,
            y: screenFrame.maxY - localTop - geometry.height,
            width: geometry.width, height: geometry.height)
    }

    static func loadImage(path: String) throws -> NSImage {
        guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
              let image = CGImageSourceCreateImageAtIndex(source, 0,
                  [kCGImageSourceShouldCacheImmediately: true] as CFDictionary)
        else { throw AppBridgeError.invalidResponse }
        return NSImage(cgImage: image, size: NSSize(width: image.width, height: image.height))
    }
}

/// Copy/save/trash jobs outlive the workspace scene so the nonactivating preview
/// remains useful while Preferences owns the root window.
final class MiniPreviewActions {
    private let transport: AppTransport
    private let loadPreferences: () throws -> CapturePreferences
    private let pasteboard: () -> NSPasteboard
    private let fileExists: (String) -> Bool
    private let revealFiles: ([URL]) -> Void
    private weak var previews: MiniPreviewController?
    private var boundToPreviews = false
    private var historyRoot: String?
    private var inFlight: Set<String> = []

    init(settingsPath: String?, transport: AppTransport = AppBridge(),
         loadPreferences: (() throws -> CapturePreferences)? = nil,
         pasteboard: @escaping () -> NSPasteboard = { .general },
         fileExists: @escaping (String) -> Bool = {
             var directory: ObjCBool = false
             return FileManager.default.fileExists(atPath: $0, isDirectory: &directory)
                 && !directory.boolValue
         },
         revealFiles: @escaping ([URL]) -> Void = { NSWorkspace.shared.activateFileViewerSelecting($0) }) {
        self.transport = transport
        self.loadPreferences = loadPreferences ?? { try CapturePreferences.load(path: settingsPath) }
        self.pasteboard = pasteboard
        self.fileExists = fileExists; self.revealFiles = revealFiles
    }

    func bind(previews: MiniPreviewController) {
        self.previews = previews; boundToPreviews = true
    }
    func configure(historyRoot: String) {
        // Style changes reconstruct LiveCaptureController, but retained exports
        // must survive for the whole process, not just one workspace render.
        guard self.historyRoot != historyRoot else { return }
        self.historyRoot = historyRoot
        LiveCaptureController.queue.async { [transport] in
            // Serialized ahead of preparation; never clear a live OS drag.
            _ = try? transport.request(["operation": "clear_previous_preview_drags",
                                        "root": historyRoot])
        }
    }

    func prepareDrag(_ artifact: CaptureArtifact, completion: @escaping (String?) -> Void) {
        guard let historyRoot, !artifact.id.isEmpty else { completion(nil); return }
        LiveCaptureController.queue.async { [transport] in
            let response = try? transport.request(["operation": "prepare_preview_drag",
                                                   "root": historyRoot, "id": artifact.id])
            let path = response?["id"] as? String == artifact.id
                ? response?["path"] as? String : nil
            DispatchQueue.main.async { completion(path) }
        }
    }

    func copy(_ artifact: CaptureArtifact) {
        previews?.setStatus("", for: artifact.id)
        LiveCaptureController.queue.async { [weak self] in
            let result = Result { try Data(contentsOf: URL(fileURLWithPath: artifact.imagePath)) }
            DispatchQueue.main.async {
                guard let self else { return }
                switch result {
                case .success(let png):
                    let pasteboard = self.pasteboard(); pasteboard.clearContents()
                    let copied = pasteboard.setData(png, forType: .png)
                    // Success shows the shipping "Copied to clipboard" chip.
                    self.previews?.setStatus(copied ? "" : "Copy failed", for: artifact.id)
                    self.previews?.recordCopyResult(artifactID: artifact.id, succeeded: copied)
                    if copied {
                        self.previews?.recordClipboardCopy(artifactID: artifact.id, pasteboard: pasteboard)
                    }
                case .failure:
                    self.previews?.setStatus("Copy failed", for: artifact.id)
                    self.previews?.recordCopyResult(artifactID: artifact.id, succeeded: false)
                }
            }
        }
    }

    func save(_ artifact: CaptureArtifact) {
        guard (!boundToPreviews || previews?.contains(artifact) == true),
              !inFlight.contains(artifact.id) else { return }
        inFlight.insert(artifact.id)
        if let path = artifact.savedPath {
            previews?.setStatus("", for: artifact.id)
            LiveCaptureController.queue.async { [weak self] in
                guard let self else { return }
                let exists = self.fileExists(path)
                DispatchQueue.main.async { [weak self] in
                    guard let self else { return }
                    self.inFlight.remove(artifact.id)
                    guard !self.boundToPreviews || self.previews?.contains(artifact) == true else { return }
                    if exists {
                        self.revealFiles([URL(fileURLWithPath: path)])
                        self.previews?.setStatus("", for: artifact.id)
                    } else {
                        self.previews?.setStatus("Export missing", for: artifact.id)
                    }
                }
            }
            return
        }
        guard let historyRoot else {
            inFlight.remove(artifact.id)
            previews?.setStatus("Save unavailable", for: artifact.id); return
        }
        previews?.setStatus("", for: artifact.id)
        LiveCaptureController.queue.async { [weak self] in
            guard let self else { return }
            let result = Result { () throws -> String in
                let preferences = try self.loadPreferences()
                let response = try self.transport.request(["operation": "save_screenshot",
                    "root": historyRoot, "id": artifact.id,
                    "directory": preferences.directory, "format": preferences.format])
                guard let path = response["path"] as? String,
                      !path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                else { throw AppBridgeError.invalidResponse }
                return path
            }
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.inFlight.remove(artifact.id)
                guard !self.boundToPreviews || self.previews?.contains(artifact) == true else { return }
                switch result {
                case .success(let path):
                    guard !self.boundToPreviews
                            || self.previews?.updateSavedPath(path, for: artifact) == true else { return }
                    self.previews?.setStatus("", for: artifact.id)
                    self.previews?.showSavedFeedback(for: artifact.id)
                case .failure: self.previews?.setStatus("Save failed", for: artifact.id)
                }
            }
        }
    }

    func trash(_ artifact: CaptureArtifact) {
        let savedPath = artifact.savedPath
        guard (!boundToPreviews || previews?.contains(artifact, savedPath: savedPath) == true),
              !inFlight.contains(artifact.id) else { return }
        inFlight.insert(artifact.id)
        guard let historyRoot else {
            inFlight.remove(artifact.id)
            previews?.setStatus("Trash unavailable", for: artifact.id)
            return
        }
        // Shipping dissolves the card first and moves the export to the Trash
        // once the dust has played; a failure brings the card back.
        let wait = boundToPreviews ? previews?.beginTrash(artifact.id) ?? 0 : 0
        LiveCaptureController.queue.asyncAfter(deadline: .now() + wait) { [weak self] in
            guard let self else { return }
            let result = Result { () throws -> Void in
                let response = try self.transport.request(["operation": "trash_preview",
                    "root": historyRoot, "id": artifact.id,
                    "saved_path": savedPath.map { $0 as Any } ?? NSNull()])
                guard response["kind"] as? String == "preview_trashed",
                      response["id"] as? String == artifact.id else {
                    throw AppBridgeError.invalidResponse
                }
            }
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.inFlight.remove(artifact.id)
                guard self.boundToPreviews else { return }
                switch result {
                case .success: self.previews?.finishTrash(artifact.id)
                case .failure(let error):
                    self.previews?.restoreTrashed(artifact.id, status: "Trash failed",
                                                  detail: error.localizedDescription)
                }
            }
        }
    }
}
