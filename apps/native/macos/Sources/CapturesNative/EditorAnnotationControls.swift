import AppKit

/// Unpublished fields for one selected annotation. Rust owns defaults and the
/// document transaction; this view submits only values the user changed.
final class EditorAnnotationControls: NSView {
    override var isFlipped: Bool { true }
    var apply: ([String: Any]) -> Void = { _ in }
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
    private var actions: [CaptureButton] = []
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
        color(EditorColors.text("stroke_color"), group: .stroke) { $0.color = $1 }
        number(numbers[0].0, group: .stroke)
        toggle("Fill", group: .closed)
        color(EditorColors.text("fill_color"), group: .fill) { $0.fill = $1 }
        toggle("Shadow", group: .always)
        color(EditorColors.text("shadow_color"), group: .shadow) { $0.shadowColor = $1 }
        for (name, _) in numbers.dropFirst() { number(name, group: .shadow) }
        let buttons = row(.always)
        let apply = CaptureButton("Apply style", frame: NSRect(x: 0, y: 0, width: 128, height: 30),
                                  tokens: tokens) { [weak self] in self?.submit() }
        let reset = CaptureButton("Reset fields", frame: NSRect(x: 144, y: 0, width: 128, height: 30),
                                  tokens: tokens) { [weak self] in self?.setStyle(self?.original) }
        buttons.addSubview(apply); buttons.addSubview(reset); actions = [apply, reset]
        setStyle(nil)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func setStyle(_ style: NativeAnnotationStyle?) {
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
        let controls: [NSControl] = Array(fields.values) + Array(toggles.values) + actions
        for control in controls { control.isEnabled = ready && draft != nil }
        for swatches in swatchRows.values {
            swatches.isEnabled = ready && draft != nil
            if !ready || swatches.isHiddenOrHasHiddenAncestor { swatches.deactivate() }
        }
        frame.size.height = y
        resized(y)
    }

    private func submit() {
        guard ready, let original, var edited = draft else { return }
        // Preserve full-precision values when only their rounded display is unchanged.
        for (name, key) in numbers where fields[name]?.superview?.isHidden == false {
            let text = fields[name]!.stringValue
            if text != format(original[keyPath: key]) {
                guard let value = formatter.number(from: text)?.doubleValue, value.isFinite,
                      name != "Stroke width" || value > 0 else {
                    reportError("Enter a valid \(name.lowercased())."); return
                }
                edited[keyPath: key] = value
            }
        }
        // Swatch and custom colors are staged on the draft; an untouched legacy
        // value is never revalidated.
        let patch = edited.patch(from: original)
        if !patch.isEmpty { apply(patch) }
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
        field.setAccessibilityLabel(name); fields[name] = field; parent.addSubview(field)
    }

    /// A shipping `ColorField`: legend, eight swatches and a custom tile. A
    /// choice stages the draft; Apply still commits the style as one step.
    private func color(_ name: String, group: Group,
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
        }
        button.setAccessibilityRole(.checkBox); button.setAccessibilityLabel(name)
        toggles[name] = button; parent.addSubview(button)
    }
}
