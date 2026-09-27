import AppKit

/// Style fields for one selected annotation. Shipping applies stroke, opacity,
/// fill and shadow changes as they are made; Rust owns defaults and the
/// document transaction, and this view reports only values the user changed.
final class EditorAnnotationControls: NSView, NSTextFieldDelegate {
    override var isFlipped: Bool { true }
    /// A changed style: the minimal patch against the published style and the
    /// live-undo field it belongs to. A burst in one field (typing, swatches)
    /// folds into one undo step; a toggle (`nil`) is its own step.
    var apply: (_ patch: [String: Any], _ field: String?) -> Void = { _, _ in }
    /// Shipping's Opacity slider (0–100), applied live like the layer menu's.
    var opacityChanged: (Double) -> Void = { _ in }
    var reportError: (String) -> Void = { _ in }
    var resized: (CGFloat) -> Void = { _ in }
    private let tokens: Tokens
    private let formatter: NumberFormatter
    private var original: NativeAnnotationStyle?
    private var draft: NativeAnnotationStyle?
    private var ready = false
    private enum Group { case always, closed, stroke, fill, shadow }
    private var rows: [(view: NSView, group: Group)] = []
    private var fields: [String: NSTextField] = [:]
    /// Shipping `ColorField` rows keyed by their label.
    private var swatchRows: [String: ColorSwatchRow] = [:]
    private var toggles: [String: CaptureButton] = [:]
    private let opacitySlider = TokenSlider(value: 100, minValue: 0, maxValue: 100, target: nil, action: nil)
    private let opacityValue = NSTextField(labelWithString: "100%")
    private let numbers: [(String, WritableKeyPath<NativeAnnotationStyle, Double>)] = [
        ("Stroke width", \.strokeWidth), ("Shadow opacity", \.shadowOpacity),
        ("Shadow blur", \.shadowBlur), ("Shadow X", \.shadowX), ("Shadow Y", \.shadowY),
    ]

    init(tokens: Tokens, formatter: NumberFormatter) {
        self.tokens = tokens; self.formatter = formatter
        super.init(frame: NSRect(x: 0, y: 550, width: 272, height: 0))
        setAccessibilityLabel("Annotation style controls")
        let heading = NSTextField(labelWithString: "Annotation style")
        heading.font = .systemFont(ofSize: 13, weight: .semibold)
        heading.textColor = tokens.color("text")
        heading.frame = NSRect(x: 0, y: 0, width: 272, height: 26)
        addSubview(heading); rows.append((heading, .always))
        toggle("Stroke", group: .closed)
        color(EditorColors.text("stroke_color"), field: "stroke-color", group: .stroke) { $0.color = $1 }
        number(numbers[0].0, group: .stroke)
        opacityRow()
        toggle("Fill", group: .closed)
        color(EditorColors.text("fill_color"), field: "fill-color", group: .fill) { $0.fill = $1 }
        toggle("Shadow", group: .always)
        color(EditorColors.text("shadow_color"), field: "shadow-color", group: .shadow) { $0.shadowColor = $1 }
        for (name, _) in numbers.dropFirst() { number(name, group: .shadow) }
        setStyle(nil)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func setStyle(_ style: NativeAnnotationStyle?) {
        // A live edit landed: the published style caught up with the fields, so
        // keep the field being typed in (and its caret) as it is.
        if let style, let draft, original != nil, style == draft {
            original = style; refresh(); return
        }
        // End editing before replacing fields, so a field editor cannot later
        // commit an old layer's text into the new selection.
        if fields.values.contains(where: { $0.currentEditor() != nil && $0.currentEditor() === window?.firstResponder }) {
            window?.makeFirstResponder(nil)
        }
        swatchRows.values.forEach { $0.deactivate() }
        original = style; draft = style
        if let style {
            swatchRows[EditorColors.text("stroke_color")]?.selectedHex = style.color
            swatchRows[EditorColors.text("fill_color")]?.selectedHex = style.fill ?? style.color
            swatchRows[EditorColors.text("shadow_color")]?.selectedHex = style.shadowColor
            for (name, key) in numbers { fields[name]?.stringValue = format(style[keyPath: key]) }
        }
        refresh()
    }

    /// The selected layer's opacity, unless the slider is being dragged.
    func setOpacity(_ value: Double?) {
        guard let value else { return }
        if window?.firstResponder !== opacitySlider { opacitySlider.doubleValue = value }
        opacityValue.stringValue = "\(Int(opacitySlider.doubleValue.rounded()))%"
    }

    func setReady(_ value: Bool) { ready = value; refresh() }

    private func refresh() {
        isHidden = draft == nil
        var y: CGFloat = 0
        for (view, group) in rows {
            let visible: Bool
            switch group {
            case .always: visible = draft != nil
            case .closed: visible = draft?.closed == true
            case .stroke: visible = draft != nil && (draft?.closed == false || draft?.strokeEnabled == true)
            case .fill: visible = draft?.closed == true && draft?.fill != nil
            case .shadow: visible = draft?.dropShadow == true
            }
            view.isHidden = !visible
            if visible { view.frame.origin.y = y; y += view.frame.height }
        }
        for (name, button) in toggles {
            let enabled = name == "Stroke" ? draft?.strokeEnabled == true
                : name == "Fill" ? draft?.fill != nil : draft?.dropShadow == true
            button.title = enabled ? "On" : "Off"; button.selected = enabled
            button.setAccessibilityValue(enabled)
        }
        let controls: [NSControl] = Array(fields.values) + Array(toggles.values)
        for control in controls { control.isEnabled = ready && draft != nil }
        // Like the layer menu's slider, Opacity stays live while an edit
        // applies; its changes queue.
        opacitySlider.isEnabled = draft != nil
        for swatches in swatchRows.values {
            swatches.isEnabled = ready && draft != nil
            if !ready || swatches.isHiddenOrHasHiddenAncestor { swatches.deactivate() }
        }
        frame.size.height = y
        resized(y)
    }

    /// Report the draft's changes against the published style, if any.
    private func emit(field: String?) {
        guard ready, let original, let draft else { return }
        let patch = draft.patch(from: original)
        if !patch.isEmpty { apply(patch, field) }
    }

    func controlTextDidChange(_ obj: Notification) {
        guard let field = obj.object as? NSTextField else { return }
        numberChanged(field, reportInvalid: false)
    }

    @objc private func numberCommitted(_ sender: NSTextField) {
        numberChanged(sender, reportInvalid: true)
    }

    /// Shipping NumberInput: a valid value applies as it is typed; a partial
    /// or invalid entry waits (Enter explains why).
    private func numberChanged(_ field: NSTextField, reportInvalid: Bool) {
        guard ready, var edited = draft, let original,
              let entry = numbers.first(where: { fields[$0.0] === field }) else { return }
        let (name, key) = entry
        let text = field.currentEditor()?.string ?? field.stringValue
        let value: Double
        if text == format(original[keyPath: key]) {
            // Preserve full precision when only the rounded display is unchanged.
            value = original[keyPath: key]
        } else {
            guard let parsed = formatter.number(from: text)?.doubleValue, parsed.isFinite,
                  name != "Stroke width" || parsed > 0 else {
                if reportInvalid { reportError("Enter a valid \(name.lowercased()).") }
                return
            }
            value = parsed
        }
        guard value != edited[keyPath: key] else { return }
        edited[keyPath: key] = value
        draft = edited
        emit(field: name.lowercased().replacingOccurrences(of: " ", with: "-"))
    }

    @objc private func opacitySliderChanged() {
        let value = opacitySlider.doubleValue.rounded()
        opacityValue.stringValue = "\(Int(value))%"
        opacityChanged(value)
    }

    private func format(_ value: Double) -> String {
        formatter.string(from: NSNumber(value: value)) ?? String(value)
    }

    private func row(_ group: Group, title: String? = nil) -> NSView {
        let row = NSView(frame: NSRect(x: 0, y: 0, width: 272, height: 38))
        if let title {
            let label = NSTextField(labelWithString: title)
            label.frame = NSRect(x: 0, y: 12, width: 124, height: 20)
            label.font = .systemFont(ofSize: 12); label.textColor = tokens.color("text-muted")
            row.addSubview(label)
        }
        addSubview(row); rows.append((row, group)); return row
    }

    private func number(_ name: String, group: Group) {
        let parent = row(group, title: name)
        let field = NSTextField(frame: NSRect(x: 130, y: 8, width: 142, height: 30))
        field.formatter = formatter; field.alignment = .right
        field.delegate = self; field.target = self; field.action = #selector(numberCommitted(_:))
        field.setAccessibilityLabel(name); fields[name] = field; parent.addSubview(field)
    }

    /// Shipping's Opacity `RangeSlider`: label, live slider and percentage.
    private func opacityRow() {
        let parent = row(.always, title: "Opacity")
        opacityValue.frame = NSRect(x: 226, y: 12, width: 46, height: 20)
        opacityValue.alignment = .right
        opacityValue.font = .monospacedDigitSystemFont(ofSize: tokens.number("text-xs"), weight: .regular)
        opacityValue.textColor = tokens.color("text-muted")
        parent.addSubview(opacityValue)
        opacitySlider.frame = NSRect(x: 130, y: 10, width: 92, height: 24)
        opacitySlider.tokens = tokens
        opacitySlider.isContinuous = true
        opacitySlider.target = self; opacitySlider.action = #selector(opacitySliderChanged)
        opacitySlider.setAccessibilityLabel("Opacity")
        parent.addSubview(opacitySlider)
    }

    /// A shipping `ColorField`: legend, eight swatches and a custom tile. A
    /// choice applies at once; choices in one field fold into one undo step.
    private func color(_ name: String, field: String, group: Group,
                       stage: @escaping (inout NativeAnnotationStyle, String) -> Void) {
        let parent = row(group)
        let legend = NSTextField(labelWithString: name)
        legend.frame = NSRect(x: 0, y: 8, width: 272, height: 20)
        legend.font = .systemFont(ofSize: 12); legend.textColor = tokens.color("text-muted")
        legend.setAccessibilityElement(false)
        parent.addSubview(legend)
        let swatches = ColorSwatchRow(tokens: tokens, label: name, compact: false)
        let height = ColorSwatchRow.height(width: 272, compact: false, tokens: tokens)
        swatches.frame = NSRect(x: 0, y: legend.frame.maxY + tokens.number("s-2"), width: 272, height: height)
        swatches.changed = { [weak self] value in
            guard let self, self.ready, var draft = self.draft else { return }
            stage(&draft, value); self.draft = draft
            self.emit(field: field)
        }
        parent.addSubview(swatches)
        parent.frame.size.height = swatches.frame.maxY
        swatchRows[name] = swatches
    }

    private func toggle(_ name: String, group: Group) {
        let parent = row(group, title: name)
        let button = CaptureButton("Off", frame: NSRect(x: 194, y: 8, width: 78, height: 30),
                                   tokens: tokens) { [weak self] in
            guard let self, var draft = self.draft, self.ready else { return }
            switch name {
            case "Stroke": draft.strokeEnabled.toggle()
            case "Fill":
                let seed = self.swatchRows[EditorColors.text("fill_color")]?.selectedHex
                draft.fill = draft.fill == nil ? (seed?.isEmpty == false ? seed : draft.color) : nil
            default: draft.dropShadow.toggle()
            }
            self.draft = draft; self.refresh()
            self.emit(field: nil)
        }
        button.setAccessibilityRole(.checkBox); button.setAccessibilityLabel(name)
        toggles[name] = button; parent.addSubview(button)
    }
}
