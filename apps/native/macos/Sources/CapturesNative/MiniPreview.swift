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

enum MiniPreviewButtonKind { case close, trash, edit, copy, save, folder, collapse, clear, check }

/// Preview-only control so this floating chrome does not inherit Workbench button styling.
final class MiniPreviewButton: NSButton {
    var kind: MiniPreviewButtonKind
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
    @objc private func activate() {
        if kind == .edit { editorJustOpened = true; needsDisplay = true }
        actionBlock()
    }
    override var isHidden: Bool {
        didSet {
            guard isHidden, !oldValue else { return }
            // Hidden chrome cannot stay hovered or keep its tip on screen.
            hovered = false; editorJustOpened = false
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
        hovered = true; applyHoverShape(); needsDisplay = true
        tooltipChanged?(self, showsTooltip)
    }
    override func mouseExited(with event: NSEvent) {
        hovered = false; editorJustOpened = false; applyHoverShape(); needsDisplay = true
        tooltipChanged?(self, showsTooltip)
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
        let width = showsHoverLabel ? max(hoverWidth, rest.width) : rest.width
        let next = NSRect(x: growsFromTrailingEdge ? rest.maxX - width : rest.minX,
                          y: rest.minY, width: width, height: rest.height)
        if frame != next { frame = next }
        needsDisplay = true
    }
    override func draw(_ dirtyRect: NSRect) {
        if kind == .edit && editorPresent { drawEditorPill(); return }
        let active = cell?.isHighlighted == true
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5),
                                xRadius: tokens.number("r-md"), yRadius: tokens.number("r-md"))
        if showsHoverLabel, let hoverLabel {
            tokens.color("glass-strong").setFill(); path.fill()
            tokens.color("glass-border").setStroke(); path.stroke()
            let text = NSAttributedString(string: hoverLabel, attributes: [
                .font: NSFont.systemFont(ofSize: tokens.number("text-2xs"), weight: .semibold),
                .foregroundColor: tokens.color("glass-text")])
            let size = text.size()
            text.draw(at: NSPoint(x: (bounds.width - size.width) / 2, y: (bounds.height - size.height) / 2))
            if focused {
                tokens.color("theme-accent").setStroke()
                let focus = NSBezierPath(roundedRect: bounds.insetBy(dx: 2, dy: 2), xRadius: 5, yRadius: 5)
                focus.lineWidth = 2; focus.stroke()
            }
            return
        }
        (primary ? tokens.color("theme-accent") : tokens.color(hovered || active ? "glass-raised" : "glass-strong")).setFill()
        path.fill(); (primary ? NSColor.clear : tokens.color("glass-border")).setStroke(); path.stroke()
        let color = primary ? tokens.color("theme-accent-ink")
            : kind == .trash ? tokens.color("theme-signal-text") : tokens.color("glass-text")
        color.setStroke(); color.setFill()
        var iconX: CGFloat = 6
        if !title.isEmpty && bounds.width > 40 {
            let text = NSAttributedString(string: title, attributes: [.font: NSFont.systemFont(ofSize: tokens.number("text-xs"), weight: .semibold), .foregroundColor: color])
            let size = text.size()
            let gap = tokens.number("s-3")
            iconX = (bounds.width - size.width - gap - 16) / 2
            text.draw(at: NSPoint(x: iconX + 16 + gap, y: (bounds.height - size.height) / 2))
        }
        drawIcon(in: NSRect(x: iconX, y: (bounds.height - 16) / 2, width: 16, height: 16))
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
        drawIcon(in: icon, lineWidth: 2.2 * 11 / 24)
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
        let p = NSBezierPath(); p.lineWidth = lineWidth; p.lineCapStyle = .round; p.lineJoinStyle = .round
        func line(_ a: NSPoint, _ b: NSPoint) { p.move(to: a); p.line(to: b) }
        switch kind {
        case .close, .clear: line(NSPoint(x:r.minX+3,y:r.minY+3), NSPoint(x:r.maxX-3,y:r.maxY-3)); line(NSPoint(x:r.maxX-3,y:r.minY+3), NSPoint(x:r.minX+3,y:r.maxY-3))
        case .trash: p.appendRoundedRect(NSRect(x:r.minX+4,y:r.minY+2,width:8,height:10), xRadius: 1, yRadius: 1); line(NSPoint(x:r.minX+2,y:r.maxY-3),NSPoint(x:r.maxX-2,y:r.maxY-3)); line(NSPoint(x:r.minX+6,y:r.maxY-1),NSPoint(x:r.minX+10,y:r.maxY-1))
        case .edit: line(NSPoint(x:r.minX+3,y:r.minY+3),NSPoint(x:r.maxX-3,y:r.maxY-3)); line(NSPoint(x:r.minX+2,y:r.minY+2),NSPoint(x:r.minX+6,y:r.minY+3))
        case .copy: p.appendRoundedRect(NSRect(x:r.minX+2,y:r.minY+2,width:9,height:10),xRadius:1,yRadius:1); p.appendRoundedRect(NSRect(x:r.minX+5,y:r.minY+5,width:9,height:9),xRadius:1,yRadius:1)
        case .save: p.appendRoundedRect(r.insetBy(dx:2,dy:2),xRadius:1,yRadius:1); line(NSPoint(x:r.midX,y:r.maxY-3),NSPoint(x:r.midX,y:r.minY+5)); line(NSPoint(x:r.midX-3,y:r.minY+8),NSPoint(x:r.midX,y:r.minY+5)); line(NSPoint(x:r.midX+3,y:r.minY+8),NSPoint(x:r.midX,y:r.minY+5))
        case .folder: p.appendRoundedRect(NSRect(x:r.minX+1,y:r.minY+3,width:14,height:10),xRadius:2,yRadius:2); line(NSPoint(x:r.minX+2,y:r.maxY-3),NSPoint(x:r.minX+7,y:r.maxY-3))
        case .check: MiniPreviewClipboardChip.appendCheck(to: p, in: r)
        case .collapse:
            p.move(to: NSPoint(x: r.minX + 2, y: r.maxY - 5))
            for point in [NSPoint(x: r.midX, y: r.maxY - 1), NSPoint(x: r.maxX - 2, y: r.maxY - 5),
                          NSPoint(x: r.midX, y: r.maxY - 9), NSPoint(x: r.minX + 2, y: r.maxY - 5)] { p.line(to: point) }
            for y in [r.minY + 6, r.minY + 3] {
                line(NSPoint(x: r.minX + 2, y: y), NSPoint(x: r.midX, y: y - 4))
                line(NSPoint(x: r.midX, y: y - 4), NSPoint(x: r.maxX - 2, y: y))
            }
        }
        p.stroke()
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

    /// The shipping check glyph (24-unit viewBox `m5 12 4 4L19 6`) in an
    /// unflipped rect.
    static func appendCheck(to path: NSBezierPath, in r: NSRect) {
        func point(_ x: CGFloat, _ y: CGFloat) -> NSPoint {
            NSPoint(x: r.minX + r.width * x / 24, y: r.maxY - r.height * y / 24)
        }
        path.move(to: point(5, 12)); path.line(to: point(9, 16)); path.line(to: point(19, 6))
    }

    override func draw(_ dirtyRect: NSRect) {
        let pill = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5),
                                xRadius: bounds.height / 2, yRadius: bounds.height / 2)
        NSColor(srgbRed: 10 / 255, green: 22 / 255, blue: 15 / 255, alpha: 0.9).setFill(); pill.fill()
        NSColor(srgbRed: 53 / 255, green: 163 / 255, blue: 93 / 255, alpha: 0.55).setStroke()
        pill.lineWidth = 1; pill.stroke()
        let icon = NSRect(x: tokens.number("s-4"), y: (bounds.height - 12) / 2, width: 12, height: 12)
        let check = NSBezierPath(); check.lineWidth = 1.2
        check.lineCapStyle = .round; check.lineJoinStyle = .round
        Self.appendCheck(to: check, in: icon)
        NSColor(srgbRed: 0x7f / 255, green: 0xd7 / 255, blue: 0x9c / 255, alpha: 1).setStroke(); check.stroke()
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
    private let dimensions: NSTextField
    private let status = NSTextField(labelWithString: "")
    private var actionButtons: [MiniPreviewButton] = []
    private var saveButton: MiniPreviewButton?
    private var copyButton: MiniPreviewButton?
    private var closeButton: MiniPreviewButton?
    private var editButton: MiniPreviewButton?
    private let clipboardChip: MiniPreviewClipboardChip
    /// The clipboard still holds this capture: Copy hides and the chip shows.
    private(set) var clipboardCurrent = false
    private var savedFeedbackActive = false
    private var savedFeedbackToken = 0
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
         dismiss: @escaping () -> Void) {
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
        let delete = addButton("Delete", .trash, x: groupStart + (saved ? 28 + gap : 0), y: inset) { [weak self] in
            self?.saved == true ? trash() : dismiss()
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
        let saveControl = addButton(saved ? "Show in Folder" : "Save file", saved ? .folder : .save,
                                    x: centerX, y: centerTop + 32 + gap, width: 140, primary: true, action: save)
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
        setAccessibilityRole(.group); setAccessibilityLabel("Screenshot mini preview")
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
    func hoverLockChanged() { refreshPointerChrome() }

    private func refreshPointerChrome() {
        let focusedControl = window?.firstResponder === self
            || actionButtons.contains { $0.window?.firstResponder === $0 }
        setChromeVisible((pointerInside && !isHoverLocked()) || focusedControl)
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
        setMediaHovered(!compact && chromeVisible, animated: false)
    }

    @discardableResult private func addButton(_ title: String, _ kind: MiniPreviewButtonKind, x: CGFloat,
        y: CGFloat, width: CGFloat = 28, primary: Bool = false, action: @escaping () -> Void) -> MiniPreviewButton {
        let button = MiniPreviewButton(title, kind: kind, frame: NSRect(x: x, y: y, width: width, height: kind == .copy || kind == .save || kind == .folder ? 32 : 28), tokens: tokens, primary: primary, action: action)
        addSubview(button); actionButtons.append(button); return button
    }

    private func setChromeVisible(_ visible: Bool) {
        guard !compact else { return }
        chromeVisible = visible
        actionButtons.forEach { if $0 !== editButton { $0.isHidden = !visible } }
        if clipboardCurrent { copyButton?.isHidden = true }
        closeButton?.isHidden = !visible || !saved
        // An open, leaving or lingering editor keeps the control pinned.
        editButton?.isHidden = !visible && !editorPinned
        dimensions.isHidden = visible
        setMediaHovered(visible)
    }
    override var acceptsFirstResponder: Bool { !compact }
    override func becomeFirstResponder() -> Bool { let result = super.becomeFirstResponder(); if result { setChromeVisible(true) }; return result }
    override func resignFirstResponder() -> Bool {
        let result = super.resignFirstResponder()
        if result { focusWithinChanged() }
        return result
    }
    /// Shipping `:focus-within`: keyboard focus on the card or one of its
    /// controls keeps the chrome up. Once focus leaves, the chrome hides
    /// unless the pointer is still over the card.
    fileprivate func focusWithinChanged() {
        DispatchQueue.main.async { [weak self] in
            guard let self, let window = self.window else { return }
            if let responder = window.firstResponder as? NSView, responder.isDescendant(of: self) {
                self.setChromeVisible(true); return
            }
            // Same rule as pointer chrome: hover counts only once unlocked.
            self.setChromeVisible(self.pointerInside && !self.isHoverLocked())
        }
    }
    override func updateTrackingAreas() {
        if let tracking { removeTrackingArea(tracking) }
        tracking = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self, userInfo: nil)
        addTrackingArea(tracking!); super.updateTrackingAreas()
    }
    override func mouseEntered(with event: NSEvent) {
        pointerInside = true
        refreshPointerChrome()
    }
    override func mouseExited(with event: NSEvent) {
        pointerInside = false
        if !actionButtons.contains(where: { $0.window?.firstResponder === $0 }) { setChromeVisible(false) }
    }

    private func styleLabelBacking(_ label: NSTextField) {
        label.wantsLayer = true
        label.layer?.backgroundColor = tokens.color("glass-strong").cgColor
        label.layer?.cornerRadius = tokens.number("r-xs")
        label.layer?.masksToBounds = true
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        guard let hit = super.hitTest(point) else { return nil }
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
    private var tracking: NSTrackingArea?

    init(frame: NSRect, count: Int, move: @escaping (NSPoint) -> Void,
         fan: @escaping (Bool) -> Void,
         action: @escaping () -> Void) {
        actionBlock = action; self.move = move; self.fan = fan
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
    }
    override func mouseUp(with event: NSEvent) {
        let clicked = press != nil && !dragging && bounds.contains(convert(event.locationInWindow, from: nil))
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
        let ys: (CGFloat, CGFloat) = above ? (10, 5.5) : (6, 10.5)
        let chevron = NSBezierPath(); chevron.lineWidth = 2
        chevron.lineCapStyle = .round; chevron.lineJoinStyle = .round
        chevron.move(to: NSPoint(x: box.minX + 3.5, y: box.minY + ys.0))
        chevron.line(to: NSPoint(x: box.minX + 8, y: box.minY + ys.1))
        chevron.line(to: NSPoint(x: box.minX + 12.5, y: box.minY + ys.0))
        tokens.color("glass-text").setStroke(); chevron.stroke()
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
         topAnchor: Bool, rightAnchor: Bool, tokens: Tokens, copy: @escaping (String) -> Void,
         save: @escaping (String) -> Void, open: @escaping (String) -> Void,
         trash: @escaping (String) -> Void, dismiss: @escaping (String) -> Void,
         setCollapsed: @escaping (Bool) -> Void,
         clearAll: @escaping () -> Void, move: @escaping (NSPoint) -> Void = { _ in }) {
        self.geometry = geometry; self.tokens = tokens; self.restLayouts = layouts
        self.hoverLayouts = hoverLayouts; anchoredAtTop = topAnchor
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
                trash: { trash(id) }, dismiss: { dismiss(id) })
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

        if collapsed, let front = ids.compactMap({ id in
            layouts[id].map { (id, $0) }
        }).first(where: { $0.1.interactive }), let card = cards[front.0] {
            let expand = MiniPreviewExpandButton(frame: card.frame, count: ids.count, move: move,
                fan: { [weak self] hovered in self?.setPileHovered(hovered) }) {
                setCollapsed(false)
            }
            document.addSubview(expand); pileExpandButton = expand
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

    func setStatus(_ value: String, detail: String? = nil, for artifactID: String) {
        cards[artifactID]?.setStatus(value, detail: detail)
    }

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

    /// Shipping `thumbnail-arrive`: a newly decoded card rises and fades in
    /// (the 3 px blur is omitted). Presentation-only; skipped under Reduce Motion.
    @discardableResult func playArrival(for artifactID: String) -> Bool {
        guard let card = cards[artifactID] else { return false }
        return NativeMotion.play("preview_card_arrive", on: card, tokens: tokens) > 0
    }

    func statusText(for artifactID: String) -> String? { cards[artifactID]?.statusText }

    func activatePileExpand() { pileExpandButton?.performClick(nil) }

    func setPileHovered(_ hovered: Bool) {
        guard hovered != pileHovered else { return }
        pileHovered = hovered
        let duration = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
            ? 0 : Double(tokens.number("dur-3")) / 1000
        NSAnimationContext.runAnimationGroup { context in
            context.duration = duration
            context.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
            for (id, card) in cards {
                guard let layout = (hovered ? hoverLayouts[id] : restLayouts[id]) else { continue }
                card.animator().setFrameOrigin(NSPoint(x: card.frame.origin.x,
                    y: CGFloat(layout.y)))
            }
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

    init(frame: NSRect, geometry: CapturesPreviewGeometry, contentHeight: Double,
         resources: [String: MiniPreviewResource], ids: [String],
         layouts: [String: CapturesPreviewCardLayout],
         hoverLayouts: [String: CapturesPreviewCardLayout] = [:], collapsed: Bool,
         topAnchor: Bool, rightAnchor: Bool, tokens: Tokens, copy: @escaping (String) -> Void,
         save: @escaping (String) -> Void, open: @escaping (String) -> Void,
         trash: @escaping (String) -> Void, dismiss: @escaping (String) -> Void,
         setCollapsed: @escaping (Bool) -> Void,
         clearAll: @escaping () -> Void, move: @escaping (NSPoint) -> Void = { _ in }) {
        previewView = MiniPreviewView(geometry: geometry, contentHeight: contentHeight,
            resources: resources, ids: ids,
            layouts: layouts, hoverLayouts: hoverLayouts, collapsed: collapsed,
            topAnchor: topAnchor, rightAnchor: rightAnchor, tokens: tokens,
            copy: copy, save: save, open: open, trash: trash, dismiss: dismiss,
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

    func dismiss(_ artifactID: String) {
        precondition(Thread.isMainThread)
        guard stack.remove(artifactID) else { return }
        pendingDecodes[artifactID] = nil
        if visibilityPendingArtifactID == artifactID, policy.stopWaiting() {
            visibilityPendingArtifactID = nil
        }
        resources[artifactID] = nil
        preparedDrags[artifactID] = nil
        cardGenerations[artifactID] = nil
        if stack.ids.isEmpty { panel?.close(); panel = nil; screenID = nil; stackOrigin = nil }
        else { makePanel(); updateVisibility() }
    }

    func clearAll() {
        precondition(Thread.isMainThread)
        let snapshot = stack.ids
        guard !snapshot.isEmpty else { return }
        _ = stack.removeAll(snapshot)
        snapshot.forEach { id in
            resources[id] = nil; pendingDecodes[id] = nil
            preparedDrags[id] = nil; cardGenerations[id] = nil
        }
        if let pending = visibilityPendingArtifactID, snapshot.contains(pending),
           policy.stopWaiting() {
            visibilityPendingArtifactID = nil
        }
        if stack.ids.isEmpty { panel?.close(); panel = nil; screenID = nil; stackOrigin = nil }
        else { makePanel(); updateVisibility() }
    }

    func setCollapsed(_ collapsed: Bool) {
        precondition(Thread.isMainThread)
        guard stack.isCollapsed != collapsed else { return }
        stack.setCollapsed(collapsed)
        makePanel(); updateVisibility()
        // Expanding leaves the pointer over a card it never hovered.
        if !collapsed { panel?.previewView.lockCardHover() }
    }

    func close() {
        precondition(Thread.isMainThread)
        _ = policy.stopWaiting(); visibilityPendingArtifactID = nil; screenID = nil
        stackOrigin = nil
        let ids = stack.ids; _ = stack.removeAll(ids)
        resources.removeAll(); pendingDecodes.removeAll()
        preparedDrags.removeAll(); cardGenerations.removeAll()
        panel?.close(); panel = nil
    }

    private func makePanel() {
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
        let next = MiniPreviewPanel(frame: frame, geometry: geometry,
            contentHeight: stack.contentHeight,
            resources: resources, ids: ids, layouts: layouts, hoverLayouts: hoverLayouts,
            collapsed: stack.isCollapsed, topAnchor: topAnchor, rightAnchor: rightAnchor, tokens: tokens,
            copy: { [weak self] in self?.perform(\.copyArtifact, artifactID: $0) },
            save: { [weak self] in self?.perform(\.saveArtifact, artifactID: $0) },
            open: { [weak self] in self?.perform(\.openArtifact, artifactID: $0) },
            trash: { [weak self] in self?.perform(\.trashArtifact, artifactID: $0) },
            dismiss: { [weak self] in self?.dismiss($0) },
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
                self.dismiss(id)
            }
        }
        panel = next
        refreshClipboardOwner()
        updateEditorPresence(animated: false)
        for id in ids { next.previewView.setEditorPhase(editorPhase(for: id), for: id, animated: false) }
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
        if let screen = targetScreen() { position(panel: panel, on: screen) }
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
        guard stack.isCollapsed, settings.enabled, let panel, panel.isVisible,
              let screen = targetScreen(), let monitor = Self.monitor(for: screen) else { return }
        let scale = max(1, monitor.scale_factor)
        let logical = NSPoint(x: Double(monitor.full_x) / scale + position.x - screen.frame.minX,
            y: Double(monitor.full_y) / scale + screen.frame.maxY - position.y - panel.frame.height)
        stackOrigin = NativePreviewLayout.movedOrigin(monitor: monitor, count: stack.ids.count,
            frameOrigin: logical, placement: settings.placement)
        self.position(panel: panel, on: screen)
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
                    if copied {
                        self.previews?.recordClipboardCopy(artifactID: artifact.id, pasteboard: pasteboard)
                    }
                case .failure:
                    self.previews?.setStatus("Copy failed", for: artifact.id)
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
        previews?.setStatus("Moving to Trash…", for: artifact.id)
        LiveCaptureController.queue.async { [weak self] in
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
                guard !self.boundToPreviews
                        || self.previews?.contains(artifact, savedPath: savedPath) == true else { return }
                switch result {
                case .success: self.previews?.dismiss(artifact.id)
                case .failure(let error):
                    self.previews?.setStatus("Trash failed", detail: error.localizedDescription,
                                             for: artifact.id)
                }
            }
        }
    }
}
