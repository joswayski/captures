import AppKit

/// Style fields for one selected annotation, laid out like shipping's shape
/// and path properties: the Stroke check row (closed shapes), Stroke color,
/// the Stroke width and Opacity `RangeSlider`s, `DropShadowFields`, then the
/// Filled shape check row and Fill color. Shipping applies every change as it
/// is made; Rust owns defaults and the document transaction, and this view
/// reports only values the user changed. Frames never depend on fonts.
final class EditorAnnotationControls: NSView {
    override var isFlipped: Bool { true }
    /// A changed style: the minimal patch against the published style and the
    /// live-undo field it belongs to. A burst in one field (a slider drag,
    /// swatches) folds into one undo step; a toggle (`nil`) is its own step.
    var apply: (_ patch: [String: Any], _ field: String?) -> Void = { _, _ in }
    /// Shipping's Opacity slider (0–100), applied live like the layer menu's.
    var opacityChanged: (Double) -> Void = { _ in }
    var reportError: (String) -> Void = { _ in }
    var resized: (CGFloat) -> Void = { _ in }
    /// Shipping's Stroke width `RangeSlider` range.
    static let strokeWidthRange: ClosedRange<Double> = 2...40
    private static let gap: CGFloat = 12
    private static let label: CGFloat = 22
    private static let checkHeight: CGFloat = 24
    private let tokens: Tokens
    private var original: NativeAnnotationStyle?
    private var draft: NativeAnnotationStyle?
    private var ready = false
    let strokeCheck = NSButton(checkboxWithTitle: "Stroke", target: nil, action: nil)
    let shadowCheck = NSButton(checkboxWithTitle: "Drop shadow", target: nil, action: nil)
    let fillCheck = NSButton(checkboxWithTitle: "Filled shape", target: nil, action: nil)
    private let strokeLegend = NSTextField(labelWithString: EditorColors.text("stroke_color"))
    private let fillLegend = NSTextField(labelWithString: EditorColors.text("fill_color"))
    let strokeSwatches: ColorSwatchRow
    let fillSwatches: ColorSwatchRow
    let strokeWidthSlider: EditorMarkedSlider
    let opacitySlider: EditorMarkedSlider
    let shadowFields: EditorDropShadowFields

    init(tokens: Tokens, formatter: NumberFormatter, width: CGFloat = ScreenshotEditorController.contentWidth) {
        self.tokens = tokens
        strokeSwatches = ColorSwatchRow(tokens: tokens, label: EditorColors.text("stroke_color"), compact: false)
        fillSwatches = ColorSwatchRow(tokens: tokens, label: EditorColors.text("fill_color"), compact: false)
        strokeWidthSlider = EditorMarkedSlider(tokens: tokens, title: "Stroke width",
            accessibilityLabel: "Stroke width", range: EditorAnnotationControls.strokeWidthRange, value: 4,
            marks: []) {
            "\(Int($0)) px"
        }
        opacitySlider = EditorMarkedSlider(tokens: tokens, title: "Opacity", accessibilityLabel: "Opacity",
                                           range: 0...100, value: 100, marks: []) { "\(Int($0))%" }
        shadowFields = EditorDropShadowFields(tokens: tokens, formatter: formatter)
        super.init(frame: NSRect(x: 0, y: 550, width: width, height: 0))
        setAccessibilityLabel("Annotation style controls")
        for legend in [strokeLegend, fillLegend] {
            legend.font = .systemFont(ofSize: tokens.number("text-sm"))
            legend.textColor = tokens.color("text-muted")
            legend.setAccessibilityElement(false)
        }
        for (check, name) in [(strokeCheck, "Stroke"), (shadowCheck, "Drop shadow"), (fillCheck, "Filled shape")] {
            check.font = .systemFont(ofSize: tokens.number("text-sm"))
            check.setAccessibilityLabel(name)
            check.target = self; check.action = #selector(toggled(_:))
        }
        let views: [NSView] = [strokeCheck, strokeLegend, strokeSwatches, strokeWidthSlider, opacitySlider,
                               shadowCheck, shadowFields, fillCheck, fillLegend, fillSwatches]
        views.forEach { addSubview($0) }
        strokeSwatches.changed = { [weak self] value in self?.stage(field: "stroke-color") { $0.color = value } }
        fillSwatches.changed = { [weak self] value in self?.stage(field: "fill-color") { $0.fill = value } }
        strokeWidthSlider.changed = { [weak self] value in
            self?.stage(field: "stroke-width") { $0.strokeWidth = value }
        }
        opacitySlider.changed = { [weak self] value in self?.opacityChanged(value) }
        shadowFields.changed = { [weak self] key in
            guard let self else { return }
            let shadow = self.shadowFields.shadowValue
            switch key {
            case "color": self.stage(field: "shadow-color") { $0.shadowColor = shadow.color }
            case "opacity": self.stage(field: "shadow-opacity") { $0.shadowOpacity = shadow.opacity }
            case "blur": self.stage(field: "shadow-blur") { $0.shadowBlur = shadow.blur }
            case "offsetX": self.stage(field: "shadow-x") { $0.shadowX = shadow.offsetX }
            default: self.stage(field: "shadow-y") { $0.shadowY = shadow.offsetY }
            }
        }
        setStyle(nil)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func setStyle(_ style: NativeAnnotationStyle?) {
        // A live edit landed: the published style caught up with the fields,
        // so keep what is shown (and any offset being typed) as it is.
        if let style, let draft, original != nil, style == draft {
            original = style; refresh(); return
        }
        // End editing before replacing fields, so a field editor cannot later
        // commit an old layer's offset into the new selection.
        for field in [shadowFields.offsetXField, shadowFields.offsetYField]
            where field.currentEditor() != nil && field.currentEditor() === window?.firstResponder {
            window?.makeFirstResponder(nil)
        }
        strokeSwatches.deactivate(); fillSwatches.deactivate(); shadowFields.swatches.deactivate()
        original = style; draft = style
        if let style {
            strokeSwatches.selectedHex = style.color
            fillSwatches.selectedHex = style.fill ?? style.color
            strokeWidthSlider.value = style.strokeWidth
            shadowFields.show(EditorDropShadowFields.Value(
                color: style.shadowColor, opacity: style.shadowOpacity, blur: style.shadowBlur,
                offsetX: style.shadowX, offsetY: style.shadowY))
        }
        refresh()
    }

    /// The selected layer's opacity, unless the slider is being dragged.
    func setOpacity(_ value: Double?) {
        guard let value else { return }
        if window?.firstResponder !== opacitySlider.slider { opacitySlider.value = value }
    }

    func setReady(_ value: Bool) { ready = value; refresh() }

    private func refresh() {
        isHidden = draft == nil
        let closed = draft?.closed == true
        let stroked = draft != nil && (!closed || draft?.strokeEnabled == true)
        let shadowed = draft?.dropShadow == true
        let filled = closed && draft?.fill != nil
        strokeCheck.state = draft?.strokeEnabled == true ? .on : .off
        shadowCheck.state = shadowed ? .on : .off
        fillCheck.state = draft?.fill != nil ? .on : .off
        let width = bounds.width
        let swatchHeight = ColorSwatchRow.height(width: width, compact: false, tokens: tokens)
        var y: CGFloat = 0
        func place(_ view: NSView, visible: Bool, x: CGFloat = 0, height: CGFloat, below: CGFloat = 0) {
            view.isHidden = !visible
            guard visible else { return }
            view.frame = NSRect(x: x, y: y + below, width: width - x, height: height)
        }
        place(strokeCheck, visible: closed, height: Self.checkHeight)
        if closed { y += Self.checkHeight + Self.gap }
        place(strokeLegend, visible: stroked, height: 20)
        place(strokeSwatches, visible: stroked, height: swatchHeight, below: Self.label)
        if stroked { y += Self.label + swatchHeight + Self.gap }
        place(strokeWidthSlider, visible: stroked, height: EditorMarkedSlider.plainHeight)
        if stroked { y += EditorMarkedSlider.plainHeight + Self.gap }
        place(opacitySlider, visible: draft != nil, height: EditorMarkedSlider.plainHeight)
        y += EditorMarkedSlider.plainHeight + Self.gap
        place(shadowCheck, visible: draft != nil, height: Self.checkHeight)
        y += Self.checkHeight + Self.gap
        let indent = EditorDropShadowFields.indent
        let shadowHeight = EditorDropShadowFields.height(width: width - indent, tokens: tokens)
        place(shadowFields, visible: shadowed, x: indent, height: shadowHeight)
        if shadowed { y += shadowHeight + Self.gap }
        place(fillCheck, visible: closed, height: Self.checkHeight)
        if closed { y += Self.checkHeight + Self.gap }
        place(fillLegend, visible: filled, height: 20)
        place(fillSwatches, visible: filled, height: swatchHeight, below: Self.label)
        if filled { y += Self.label + swatchHeight + Self.gap }
        let height = draft == nil ? 0 : y - Self.gap
        let enabled = ready && draft != nil
        for check in [strokeCheck, shadowCheck, fillCheck] { check.isEnabled = enabled }
        strokeWidthSlider.isEnabled = enabled
        shadowFields.isEnabled = enabled
        // Like the layer menu's slider, Opacity stays live while an edit
        // applies; its changes queue.
        opacitySlider.isEnabled = draft != nil
        for swatches in [strokeSwatches, fillSwatches, shadowFields.swatches] {
            swatches.isEnabled = enabled
            if !ready || swatches.isHiddenOrHasHiddenAncestor { swatches.deactivate() }
        }
        frame.size.height = height
        resized(height)
    }

    /// Stage one field's change on the draft and report the patch.
    private func stage(field: String, _ change: (inout NativeAnnotationStyle) -> Void) {
        guard ready, var edited = draft else { return }
        change(&edited)
        guard edited != draft else { return }
        draft = edited
        emit(field: field)
    }

    /// Report the draft's changes against the published style, if any.
    private func emit(field: String?) {
        guard ready, let original, let draft else { return }
        let patch = draft.patch(from: original)
        if !patch.isEmpty { apply(patch, field) }
    }

    /// Stroke, Drop shadow and Filled shape: each toggle is its own undo step.
    @objc private func toggled(_ sender: NSButton) {
        guard ready, var edited = draft else { return }
        if sender === strokeCheck {
            edited.strokeEnabled = sender.state == .on
        } else if sender === fillCheck {
            // Shipping seeds a new fill from the stroke color.
            edited.fill = sender.state == .on ? edited.color : nil
            if let fill = edited.fill { fillSwatches.selectedHex = fill }
        } else {
            edited.dropShadow = sender.state == .on
        }
        draft = edited
        refresh()
        emit(field: nil)
    }
}
