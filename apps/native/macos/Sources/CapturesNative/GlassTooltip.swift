import AppKit
import QuartzCore

/// Fixed-glass tooltip chip that replaces the delayed system tooltip on
/// floating surfaces: the recording HUD (`.recording-tooltip`) and mini-preview
/// icons (`.icon-button::after`). It appears with no delay on hover or keyboard
/// focus and never takes the mouse; the owning surface places and animates it.
class GlassTooltipView: NSView {
    struct Style {
        let fontToken: String
        let radiusToken: String
        /// Label inset: the shipping padding plus any border.
        let inset: NSSize
        let bordered: Bool
        let shadowed: Bool

        /// `.recording-tooltip > [role="tooltip"]`: xs medium text in an `--r-sm`
        /// pill with a glass border.
        static let recordingHUD = Style(fontToken: "text-xs", radiusToken: "r-lg",
                                        inset: NSSize(width: 9, height: 6),
                                        bordered: true, shadowed: false)
        /// `.icon-button::after`: 2xs medium text, `padding: 4px 7px`, `--r-xs`,
        /// `--shadow-sm` and no border.
        static let previewIcon = Style(fontToken: "text-2xs", radiusToken: "r-xs",
                                       inset: NSSize(width: 7, height: 4),
                                       bordered: false, shadowed: true)
    }

    var text = "" { didSet { needsDisplay = true } }
    private let chipStyle: Style
    private let textAttributes: [NSAttributedString.Key: Any]
    private var transitionID = 0
    private var attributedText: NSAttributedString { NSAttributedString(string: text, attributes: textAttributes) }
    var textSize: NSSize { textSize(for: text) }
    func textSize(for string: String) -> NSSize {
        let measured = NSAttributedString(string: string, attributes: textAttributes).size()
        return NSSize(width: ceil(measured.width), height: ceil(measured.height))
    }
    override var isFlipped: Bool { true }

    init(tokens: Tokens, style: Style) {
        chipStyle = style
        let paragraph = NSMutableParagraphStyle()
        paragraph.alignment = .center
        paragraph.lineBreakMode = .byTruncatingTail
        textAttributes = [.font: NSFont.capturesUI(ofSize: tokens.number(style.fontToken), weight: .medium),
                          .foregroundColor: tokens.color("glass-text"), .paragraphStyle: paragraph]
        super.init(frame: .zero)
        wantsLayer = true
        disableImplicitLayerActions()
        layer?.backgroundColor = tokens.color("glass-strong").cgColor
        layer?.cornerRadius = tokens.number(style.radiusToken)
        if style.bordered {
            layer?.borderColor = tokens.color("glass-border").cgColor
            layer?.borderWidth = 1
        }
        if style.shadowed {
            // `--shadow-sm`: 0 2px 6px rgba(0, 0, 0, .32).
            layer?.masksToBounds = false
            layer?.shadowColor = NSColor.black.cgColor
            layer?.shadowOpacity = 0.32
            layer?.shadowRadius = 3
            layer?.shadowOffset = CGSize(width: 0, height: -2)
        }
        isHidden = true
        alphaValue = 0
        setAccessibilityElement(false)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    /// Prevent Core Animation from ever implicitly interpolating `position` or
    /// `bounds` across buttons when the single chip moves to a new owner.
    private func disableImplicitLayerActions() {
        let none: [String: CAAction] = [
            "position": NSNull(),
            "bounds": NSNull(),
            "frame": NSNull(),
            "opacity": NSNull(),
            "transform": NSNull(),
            "hidden": NSNull(),
        ]
        layer?.actions = none
    }

    /// Place the tooltip synchronously at `shown`. Only when arriving from a
    /// fully hidden state does it fade in with a strictly vertical `nudgeY`
    /// transform (in the superview's coordinate space, `hidden.minY - shown.minY`) —
    /// `frame`/`position` are never animated, so switching between buttons can
    /// never slide the chip across the surface.
    func present(text: String, frame shown: NSRect, nudgeY: CGFloat,
                 duration: Double, timing: CAMediaTimingFunction?, wasHidden: Bool) {
        transitionID += 1
        let shouldAnimate = wasHidden && duration > 0 && !NativeMotion.reduceMotion
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        NSAnimationContext.beginGrouping()
        NSAnimationContext.current.duration = 0
        NSAnimationContext.current.allowsImplicitAnimation = false
        wantsLayer = true
        disableImplicitLayerActions()
        layer?.removeAllAnimations()
        self.text = text
        self.frame = shown
        isHidden = false
        alphaValue = 1
        layer?.opacity = 1
        layer?.transform = CATransform3DIdentity
        needsLayout = true
        needsDisplay = true
        NSAnimationContext.endGrouping()
        if shouldAnimate, let layer {
            let opacity = CABasicAnimation(keyPath: "opacity")
            opacity.fromValue = 0.0
            opacity.toValue = 1.0
            opacity.duration = duration
            opacity.timingFunction = timing
            layer.add(opacity, forKey: "captures.tooltip.opacity")
            if nudgeY != 0 {
                let slide = CABasicAnimation(keyPath: "transform.translation.y")
                slide.fromValue = nudgeY
                slide.toValue = 0.0
                slide.duration = duration
                slide.timingFunction = timing
                layer.add(slide, forKey: "captures.tooltip.nudge")
            }
        }
        CATransaction.commit()
    }

    /// Fade the tooltip out in place (or hide immediately when `duration <= 0`
    /// or already hidden).
    func dismiss(duration: Double, timing: CAMediaTimingFunction?, shouldHide: @escaping () -> Bool) {
        transitionID += 1
        let id = transitionID
        guard !isHidden, duration > 0, !NativeMotion.reduceMotion, let layer else {
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            NSAnimationContext.beginGrouping()
            NSAnimationContext.current.duration = 0
            NSAnimationContext.current.allowsImplicitAnimation = false
            self.layer?.removeAllAnimations()
            alphaValue = 0
            self.layer?.opacity = 0
            self.layer?.transform = CATransform3DIdentity
            isHidden = true
            NSAnimationContext.endGrouping()
            CATransaction.commit()
            return
        }
        let currentOpacity = layer.presentation()?.opacity ?? Float(alphaValue)
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        disableImplicitLayerActions()
        CATransaction.setCompletionBlock { [weak self] in
            guard let self, self.transitionID == id, shouldHide() else { return }
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            NSAnimationContext.beginGrouping()
            NSAnimationContext.current.duration = 0
            NSAnimationContext.current.allowsImplicitAnimation = false
            self.layer?.removeAllAnimations()
            self.alphaValue = 0
            self.layer?.opacity = 0
            self.layer?.transform = CATransform3DIdentity
            self.isHidden = true
            NSAnimationContext.endGrouping()
            CATransaction.commit()
        }
        layer.removeAllAnimations()
        let fade = CABasicAnimation(keyPath: "opacity")
        fade.fromValue = currentOpacity
        fade.toValue = 0.0
        fade.duration = duration
        fade.timingFunction = timing
        fade.fillMode = .forwards
        fade.isRemovedOnCompletion = false
        layer.add(fade, forKey: "captures.tooltip.opacity")
        CATransaction.commit()
    }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    var textBounds: NSRect {
        // An oversized CGRect inset is null, with infinite x/y origins. Hidden,
        // unsized chips have no drawable text area.
        let content = bounds.insetBy(dx: chipStyle.inset.width, dy: chipStyle.inset.height)
        return content.isNull ? .zero : content
    }

    override func draw(_ dirtyRect: NSRect) {
        let content = textBounds
        guard content.width > 0, content.height > 0 else { return }
        // Measure and paint the same glyphs. A separate NSTextField cell can
        // truncate even when its intrinsic width was used to place this chip.
        attributedText.draw(in: content)
    }
}
