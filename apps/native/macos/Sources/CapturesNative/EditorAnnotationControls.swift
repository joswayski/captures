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
    private var wells: [String: ClosureColorWell] = [:]
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
        color("Stroke color", group: .stroke)
        number(numbers[0].0, group: .stroke)
        toggle("Fill", group: .closed)
        color("Fill color", group: .fill)
        toggle("Shadow", group: .always)
        color("Shadow color", group: .shadow)
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
        wells.values.forEach { $0.deactivate() }
        original = style; draft = style
        if let style {
            fields["Stroke color"]?.stringValue = style.color
            fields["Fill color"]?.stringValue = style.fill ?? style.color
            fields["Shadow color"]?.stringValue = style.shadowColor
            for (name, key) in numbers { fields[name]?.stringValue = format(style[keyPath: key]) }
            for (name, well) in wells {
                well.color = NSColor(hex: fields[name]!.stringValue) ?? tokens.color("text")
            }
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
        let controls: [NSControl] = Array(fields.values) + Array(wells.values)
            + Array(toggles.values) + actions
        for control in controls { control.isEnabled = ready && draft != nil }
        for well in wells.values where !ready || well.isHiddenOrHasHiddenAncestor { well.deactivate() }
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
        for name in ["Stroke color", "Fill color", "Shadow color"]
            where fields[name]?.superview?.isHidden == false {
            let previous = name == "Stroke color" ? original.color
                : name == "Fill color" ? original.fill : original.shadowColor
            let text = fields[name]!.stringValue
            guard text != previous else { continue }
            guard let value = PreferencesController.normalizeHex(text) else {
                reportError("Enter \(name.lowercased()) as #RGB or #RRGGBB."); return
            }
            switch name {
            case "Stroke color": edited.color = value
            case "Fill color": edited.fill = value
            default: edited.shadowColor = value
            }
        }
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

    private func color(_ name: String, group: Group) {
        let parent = row(group, title: name)
        let field = NSTextField(frame: NSRect(x: 130, y: 8, width: 100, height: 30))
        field.setAccessibilityLabel(name); fields[name] = field; parent.addSubview(field)
        let well = ClosureColorWell(frame: NSRect(x: 238, y: 8, width: 34, height: 30))
        well.setAccessibilityLabel("Choose \(name.lowercased())")
        well.change = { [weak field] color in
            if let value = color.rgbHex { field?.stringValue = value }
        }
        well.target = well; well.action = #selector(ClosureColorWell.selectedColor)
        wells[name] = well; parent.addSubview(well)
    }

    private func toggle(_ name: String, group: Group) {
        let parent = row(group, title: name)
        let button = CaptureButton("Off", frame: NSRect(x: 194, y: 8, width: 78, height: 30),
                                   tokens: tokens) { [weak self] in
            guard let self, var draft = self.draft, self.ready else { return }
            switch name {
            case "Stroke": draft.strokeEnabled.toggle()
            case "Fill": draft.fill = draft.fill == nil ? self.fields["Fill color"]?.stringValue : nil
            default: draft.dropShadow.toggle()
            }
            self.draft = draft; self.refresh()
        }
        button.setAccessibilityRole(.checkBox); button.setAccessibilityLabel(name)
        toggles[name] = button; parent.addSubview(button)
    }
}
