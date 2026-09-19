import AppKit

final class ClosurePopUpButton: NSPopUpButton {
    var tokens: Tokens!
    var change: ((Int) -> Void)?
    @objc func selectedValue() { change?(indexOfSelectedItem) }

    override func draw(_ dirtyRect: NSRect) {
        let outline = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1), xRadius: tokens.number("r-md"), yRadius: tokens.number("r-md"))
        tokens.color("control").setFill(); outline.fill()
        tokens.color(window?.firstResponder === self ? "theme-accent" : "control-border").setStroke(); outline.stroke()
        let attributes: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: tokens.number("text-md")),
            .foregroundColor: tokens.color(isEnabled ? "text" : "text-muted")]
        (title as NSString).draw(at: NSPoint(x: 12, y: 7), withAttributes: attributes)
        ("⌄" as NSString).draw(at: NSPoint(x: bounds.width - 22, y: 7), withAttributes: attributes)
    }
}

final class ClosureColorWell: NSColorWell {
    var change: ((NSColor) -> Void)?
    @objc func selectedColor() { change?(color) }
}

final class PreferencesController: NSObject, NSTextFieldDelegate {
    private let root: Surface
    private let store: SettingsStore
    private let tokensProvider: () -> Tokens
    private let appearanceChanged: (String, String, [String: Any]) -> Void
    private let settingsChanged: ([String: Any]) -> Void
    private let settingsPersisted: ([String: Any]) -> Void
    private let showHistory: () -> Void
    private let liveCaptureAvailable: Bool
    private var settings: [String: Any] = [:]
    private var scroll = NSScrollView()
    private var document = Surface()
    private var status = NSTextField(labelWithString: "Loading preferences…")
    private var retryButton: CaptureButton?
    private var findBar: Surface?
    private var findField: NSTextField?
    private var findCount: NSTextField?
    private var matches: [NSView] = []
    private var matchIndex = 0
    private var latestRevision = 0
    private var saveFailed = false
    private var rebuilding = false
    private let sections = [("appearance", "Appearance"), ("capture", "Capture"),
                            ("shortcuts", "Shortcuts"),
                            ("recording", "Recording"), ("gif", "GIF export"),
                            ("updates", "Updates"), ("about", "About")]
    private var sectionViews: [String: NSView] = [:]
    private var searchable: [(NSView, String)] = []

    init(root: Surface, store: SettingsStore, tokens: @escaping () -> Tokens,
         appearanceChanged: @escaping (String, String, [String: Any]) -> Void,
         settingsChanged: @escaping ([String: Any]) -> Void = { _ in },
         settingsPersisted: @escaping ([String: Any]) -> Void = { _ in },
         showHistory: @escaping () -> Void, liveCaptureAvailable: Bool = false,
         initialAppearance: String? = nil, initialTheme: String? = nil) {
        self.root = root; self.store = store; tokensProvider = tokens
        self.appearanceChanged = appearanceChanged; self.showHistory = showHistory
        self.settingsChanged = settingsChanged
        self.settingsPersisted = settingsPersisted
        self.liveCaptureAvailable = liveCaptureAvailable
        super.init()
        buildShell()
        store.load { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let value):
                self.settings = value
                if let initialAppearance { self.settings["appearance"] = initialAppearance }
                if let initialTheme { self.settings["theme"] = initialTheme }
                self.appearanceChanged(self.settings.string("appearance", "system"), self.settings.string("theme", "mustard"), value["custom_theme"] as? [String: Any] ?? [:])
                self.settingsChanged(self.settings)
                self.settingsPersisted(self.settings)
                self.restyle()
                self.setStatus("", kind: "idle")
            case .failure(let error):
                self.setStatus("Couldn’t load preferences: \(error.localizedDescription)", kind: "error")
            }
        }
    }

    private var tokens: Tokens { tokensProvider() }

    private func buildShell() {
        let nav = Surface(frame: NSRect(x: 0, y: 0, width: 196, height: root.bounds.height))
        nav.autoresizingMask = [.height]
        nav.wantsLayer = true; nav.layer?.backgroundColor = tokens.color("surface-sunken").cgColor
        root.addSubview(nav)
        let mark = Surface(frame: NSRect(x: 24, y: 24, width: 26, height: 26))
        mark.wantsLayer = true; mark.layer?.cornerRadius = tokens.number("r-md")
        mark.layer?.backgroundColor = tokens.color("theme-accent").cgColor; nav.addSubview(mark)
        addLabel("⌖", frame: mark.bounds, size: 16, parent: mark, centered: true, color: "theme-accent-ink")
        addLabel("Captures", frame: NSRect(x: 62, y: 24, width: 112, height: 26), size: 14, weight: .semibold, parent: nav)
        for (index, section) in sections.enumerated() {
            let button = CaptureButton(section.1, frame: NSRect(x: 16, y: 76 + index * 42, width: 164, height: 34), tokens: tokens) { [weak self] in
                self?.reveal(section.0)
            }
            button.alignment = .left; button.setAccessibilityRole(.button); nav.addSubview(button)
        }
        addLabel("Native development build", frame: NSRect(x: 22, y: root.bounds.height - 62, width: 160, height: 20),
                 size: 11, muted: true, parent: nav).autoresizingMask = [.minYMargin]
        addLabel(liveCaptureAvailable ? "Display capture enabled" : "Capture engine not connected", frame: NSRect(x: 22, y: root.bounds.height - 40, width: 166, height: 20),
                 size: 11, muted: true, parent: nav).autoresizingMask = [.minYMargin]

        addLabel("Preferences", frame: NSRect(x: 224, y: 18, width: 220, height: 28), size: 20, weight: .semibold, parent: root)
        addLabel("Changes save automatically.", frame: NSRect(x: 224, y: 43, width: 240, height: 20), size: 12, muted: true, parent: root)
        let history = actionButton(liveCaptureAvailable ? "Capture Workspace" : "Capture History", x: root.bounds.width - 574, y: 22, width: 174, parent: root) { [weak self] in
            self?.flush(); self?.showHistory()
        }
        history.autoresizingMask = [.minXMargin]
        status.frame = NSRect(x: root.bounds.width - 390, y: 25, width: 360, height: 28)
        status.autoresizingMask = [.minXMargin]; status.alignment = .right; root.addSubview(status)
        retryButton = actionButton("Retry", x: root.bounds.width - 88, y: 23, width: 58, parent: root) { [weak self] in self?.retrySave() }
        retryButton?.autoresizingMask = [.minXMargin]; retryButton?.isHidden = true
        scroll = NSScrollView(frame: NSRect(x: 196, y: 72, width: root.bounds.width - 196, height: root.bounds.height - 72))
        scroll.autoresizingMask = [.width, .height]; scroll.hasVerticalScroller = true; scroll.drawsBackground = false
        document = Surface(frame: NSRect(x: 0, y: 0, width: scroll.bounds.width, height: 400))
        scroll.documentView = document; root.addSubview(scroll)
    }

    private func rebuildCards() {
        let oldY = scroll.contentView.bounds.origin.y
        document.subviews.forEach { $0.removeFromSuperview() }; sectionViews.removeAll(); searchable.removeAll()
        var y: CGFloat = 28
        y = appearanceCard(y); y = captureCard(y); y = shortcutsCard(y); y = recordingCard(y); y = gifCard(y)
        y = updatesCard(y); y = aboutCard(y)
        document.frame.size = NSSize(width: scroll.contentSize.width, height: y + 42)
        scroll.contentView.scroll(to: NSPoint(x: 0, y: min(oldY, max(0, y - scroll.contentSize.height))))
        updateFind()
    }

    private func card(_ id: String, title: String, description: String, y: CGFloat, height: CGFloat) -> Surface {
        let card = Surface(frame: NSRect(x: 28, y: y, width: min(720, scroll.contentSize.width - 56), height: height))
        card.wantsLayer = true; card.layer?.cornerRadius = tokens.number("r-xl")
        card.layer?.backgroundColor = tokens.color("surface-raised").cgColor
        card.layer?.borderWidth = 1; card.layer?.borderColor = tokens.color("border").cgColor
        document.addSubview(card); sectionViews[id] = card
        let heading = addLabel(title, frame: NSRect(x: 22, y: 18, width: 650, height: 24), size: 16, weight: .semibold, parent: card)
        let detail = addLabel(description, frame: NSRect(x: 22, y: 43, width: 650, height: 34), size: 12, muted: true, parent: card)
        searchable += [(heading, title), (detail, description)]
        return card
    }

    private func appearanceCard(_ y: CGFloat) -> CGFloat {
        let card = card("appearance", title: "Appearance", description: "One look across every Captures window. Capture overlays stay dark so they read on any desktop.", y: y, height: settings.string("theme") == "custom" ? 382 : 290)
        rowTitle("Interface theme", detail: "Follow the system setting, or lock Captures to light or dark.", y: 88, parent: card)
        let selected = settings.string("appearance", "system")
        for (i, value) in ["system", "light", "dark"].enumerated() {
            let button = actionButton(value.capitalized, x: 414 + CGFloat(i) * 86, y: 91, width: 78, parent: card) { [weak self] in self?.set(value, for: "appearance", rerender: true) }
            button.selected = selected == value
        }
        rowTitle("Accent color", detail: "Used for capture, selection and focus. Status colors keep their meaning.", y: 142, parent: card)
        let themes = Options.themes + ["custom"]
        let descriptions = ["mustard": "Captures mustard and signal red", "ember": "Warm orange and electric pink",
            "rose": "Bright rose and coral", "violet": "Orchid violet and raspberry", "cobalt": "True blue and coral",
            "aqua": "Clear cyan and watermelon", "mint": "Fresh mint and vermilion", "lime": "Crisp lime and vermilion",
            "mono": "Vercel-like black and white", "custom": "Build your own RGB palette"]
        for (i, value) in themes.enumerated() {
            let button = actionButton(value.capitalized, x: 22 + CGFloat(i % 5) * 134, y: 194 + CGFloat(i / 5) * 43, width: 124, parent: card) { [weak self] in self?.set(value, for: "theme", rerender: true) }
            button.selected = settings.string("theme", "mustard") == value
            button.setAccessibilityLabel("\(value.capitalized): \(descriptions[value]!)")
            let mode = tokens.color("text").brightnessComponent > 0.5 ? "dark" : "light"
            let preview = value == "custom" ? tokens : (Tokens.variants["\(mode)-\(value)"] ?? tokens)
            for (index, token) in ["theme-accent", "theme-signal"].enumerated() {
                let swatch = Surface(frame: NSRect(x: 7 + index * 10, y: 11, width: 8, height: 10)); swatch.wantsLayer = true
                swatch.layer?.backgroundColor = preview.color(token).cgColor; swatch.layer?.cornerRadius = 2; button.addSubview(swatch)
            }
        }
        if settings.string("theme") == "custom" {
            let custom = settings["custom_theme"] as? [String: Any] ?? [:]
            rowTitle("Custom colors", detail: "Enter six-digit RGB hex values. Supporting shades stay readable.", y: 286, parent: card)
            colorField("Accent", value: custom.string("accent", "#32d3ff"), key: "accent", x: 318, y: 300, parent: card)
            colorField("Signal", value: custom.string("signal", "#ff4fc3"), key: "signal", x: 472, y: 300, parent: card)
            _ = actionButton("Reset colors", x: 566, y: 342, width: 112, parent: card) { [weak self] in
                self?.settings["custom_theme"] = ["accent": "#32d3ff", "signal": "#ff4fc3"]
                self?.changed(rerender: true)
            }
        }
        return y + card.frame.height + 22
    }

    private func captureCard(_ y: CGFloat) -> CGFloat {
        let card = card("capture", title: "Capture", description: "Where captures go and what happens right after you take one.", y: y, height: 820)
        rowTitle("Save captures to", detail: "Enter a folder or choose one.", y: 86, parent: card)
        let directory = NSTextField(string: settings.string("output_directory")); directory.frame = NSRect(x: 290, y: 96, width: 278, height: 30)
        directory.identifier = NSUserInterfaceItemIdentifier("setting.output_directory"); directory.delegate = self; styleField(directory); card.addSubview(directory)
        _ = actionButton("Choose…", x: 578, y: 96, width: 100, parent: card) { [weak self] in self?.chooseFolder() }
        toggle("Automatically copy captures to the clipboard", detail: "Turn this off to preserve existing clipboard contents.", key: "auto_copy_to_clipboard", y: 150, parent: card)
        toggle("Start capture as soon as a target is selected", detail: "Region, window, or Full screen selection immediately starts capture.", key: "auto_start_on_selection", y: 214, parent: card)
        toggle("Show mini previews after screenshots", detail: "Turn this off to keep the quick-access preview stack hidden.", key: "show_mini_previews", y: 278, parent: card)
        menuSetting("Mini preview position", detail: "Choose the screen corner for the preview stack.", key: "mini_preview_placement", values: ["bottom_left", "bottom_right", "top_left", "top_right"], y: 342, parent: card, enabled: settings.bool("show_mini_previews"))
        toggle("Show mini previews in screenshots and recordings", detail: "Mini previews must be enabled above.", key: "include_mini_previews_in_captures", y: 406, parent: card, enabled: settings.bool("show_mini_previews"))
        toggle("Show recording controls in screenshots and recordings", detail: "This native fixture cannot exclude recording controls yet.", key: "include_recording_controls_in_captures", y: 470, parent: card, enabled: false)
        toggle("Freeze screen when capturing", detail: "Holds hover states, menus and motion still while selecting.", key: "freeze_screen", y: 534, parent: card)
        toggle("Show cursor in screenshots", detail: "Includes the pointer in still captures.", key: "show_cursor_in_screenshots", y: 598, parent: card)
        menuSetting("Screenshot format", detail: "Used when you save or export.", key: "screenshot_format", values: ["png", "jpeg", "webp"], y: 662, parent: card)
        menuSetting("Screenshot countdown", detail: "Wait before capturing; Escape cancels.", key: "screenshot_countdown_seconds", values: Array(0...10), y: 726, parent: card)
        return y + card.frame.height + 22
    }

    private func shortcutsCard(_ y: CGFloat) -> CGFloat {
        let description = liveCaptureAvailable
            ? "Saved bindings for native region, window and display captures."
            : "Global capture shortcuts are not connected in this native development build."
        let card = card("shortcuts", title: "Shortcuts", description: description, y: y, height: 164)
        disabledRow(liveCaptureAvailable ? "Capture shortcuts" : "Global capture shortcuts",
            detail: liveCaptureAvailable ? "Shortcut editing is not available in the native workspace yet."
                : "Use the installed Preview for capture shortcuts.", y: 88, parent: card)
        return y + card.frame.height + 22
    }

    private func recordingCard(_ y: CGFloat) -> CGFloat {
        let card = card("recording", title: "Recording", description: "Defaults for new screen recordings. The native capture engine is not connected yet.", y: y, height: 626)
        recordingMenu("Recording format", detail: "MP4, GIF or WebM", key: "video_format", values: ["mp4", "gif", "webm"], y: 88, parent: card)
        recordingMenu("Frames per second", detail: "Default recording frame rate", key: "video_fps", values: [60, 30, 15], y: 146, parent: card)
        recordingMenu("Maximum resolution", detail: "Original, 1080p or 720p", key: "video_max_resolution", values: ["original", "p1080", "p720"], y: 204, parent: card)
        recordingMenu("Countdown", detail: "Delay before a recording starts", key: "countdown_seconds", values: Array(0...10), y: 262, parent: card)
        toggleRecording("Record desktop audio", key: "capture_system_audio", y: 320, parent: card)
        disabledRow("Microphone", detail: "Unavailable until native microphone capture is connected.", y: 366, parent: card)
        toggleRecording("Show cursor in recordings", key: "show_cursor", y: 412, parent: card)
        toggleRecording("Open the editor after recording", key: "open_editor_after_recording", y: 458, parent: card)
        toggleRecording("Export recording audio in mono", key: "mono_audio", y: 504, parent: card)
        toggleRecording("Show clicks in recordings", key: "highlight_clicks", y: 550, parent: card)
        return y + card.frame.height + 22
    }

    private func gifCard(_ y: CGFloat) -> CGFloat {
        let card = card("gif", title: "GIF export", description: "GIF-specific export defaults. GIFs do not include recorded audio.", y: y, height: 210)
        disabledRow("GIF quality", detail: "Available when the native encoder is connected.", y: 88, parent: card)
        disabledRow("GIF dimensions", detail: "Uses the recording size for now.", y: 146, parent: card)
        return y + card.frame.height + 22
    }
    private func updatesCard(_ y: CGFloat) -> CGFloat {
        let card = card("updates", title: "Updates", description: "Preview build update preferences.", y: y, height: 150)
        toggle("Show release notes with updates", detail: "Show what changed when an update is available.", key: "show_update_changelog", y: 88, parent: card)
        return y + card.frame.height + 22
    }
    private func aboutCard(_ y: CGFloat) -> CGFloat {
        let card = card("about", title: "About", description: "Captures native development fixture.", y: y, height: 150)
        disabledRow("Capture History", detail: "Existing synthetic fixture — open with --scene history.", y: 88, parent: card)
        return y + card.frame.height + 22
    }

    private func rowTitle(_ title: String, detail: String, y: CGFloat, parent: NSView) {
        let first = addLabel(title, frame: NSRect(x: 22, y: y, width: 470, height: 21), size: 13, weight: .medium, parent: parent)
        let second = addLabel(detail, frame: NSRect(x: 22, y: y + 21, width: 470, height: 31), size: 11, muted: true, parent: parent)
        second.maximumNumberOfLines = 2; searchable += [(first, title), (second, detail)]
    }

    private func toggle(_ title: String, detail: String, key: String, y: CGFloat, parent: NSView, enabled: Bool = true) {
        rowTitle(title, detail: detail, y: y, parent: parent)
        let button = actionButton(settings.bool(key) ? "On" : "Off", x: 600, y: y + 8, width: 78, parent: parent) { [weak self] in
            guard let self else { return }; self.set(!self.settings.bool(key), for: key, rerender: true)
        }
        button.selected = settings.bool(key); button.setAccessibilityRole(.checkBox); button.setAccessibilityValue(settings.bool(key))
        button.setAccessibilityLabel(title); button.isEnabled = enabled; button.alphaValue = enabled ? 1 : 0.55
    }

    private func toggleRecording(_ title: String, key: String, y: CGFloat, parent: NSView) {
        let recording = settings["recording"] as? [String: Any] ?? [:]
        rowTitle(title, detail: "Recording default", y: y, parent: parent)
        let button = actionButton(recording.bool(key) ? "On" : "Off", x: 600, y: y + 5, width: 78, parent: parent) { [weak self] in
            guard let self else { return }; var value = self.settings["recording"] as? [String: Any] ?? [:]
            value[key] = !value.bool(key); self.settings["recording"] = value; self.changed(rerender: true)
        }
        button.selected = recording.bool(key); button.setAccessibilityRole(.checkBox)
        button.setAccessibilityLabel(title)
    }

    private func recordingMenu<T: Equatable>(_ title: String, detail: String, key: String, values: [T], y: CGFloat, parent: NSView) {
        let recording = settings["recording"] as? [String: Any] ?? [:]
        rowTitle(title, detail: detail, y: y, parent: parent)
        let current = recording[key] as? T ?? values[0]
        addMenu(values.map { String(describing: $0) }, selected: String(describing: current), title: title, y: y, parent: parent) { [weak self] index in
            guard let self else { return }; var recording = self.settings["recording"] as? [String: Any] ?? [:]
            recording[key] = values[index]; self.settings["recording"] = recording; self.changed(rerender: false)
        }
    }

    private func menuSetting<T: Equatable>(_ title: String, detail: String, key: String, values: [T], y: CGFloat, parent: NSView, enabled: Bool = true) {
        rowTitle(title, detail: detail, y: y, parent: parent)
        let current = settings[key] as? T ?? values[0]
        let menu = addMenu(values.map { String(describing: $0) }, selected: String(describing: current), title: title, y: y, parent: parent) { [weak self] index in
            self?.set(values[index], for: key, rerender: false)
        }; menu.isEnabled = enabled
    }

    private func addMenu(_ values: [String], selected: String, title: String, y: CGFloat, parent: NSView, change: @escaping (Int) -> Void) -> NSPopUpButton {
        let menu = ClosurePopUpButton(frame: NSRect(x: 528, y: y + 7, width: 150, height: 30), pullsDown: false)
        menu.tokens = tokens
        menu.addItems(withTitles: values.map { $0.replacingOccurrences(of: "_", with: " ").capitalized })
        menu.selectItem(at: values.firstIndex(of: selected) ?? 0); menu.setAccessibilityLabel(title)
        menu.change = change; menu.target = menu; menu.action = #selector(ClosurePopUpButton.selectedValue); parent.addSubview(menu); return menu
    }

    private func disabledRow(_ title: String, detail: String, y: CGFloat, parent: NSView) {
        rowTitle(title, detail: detail, y: y, parent: parent)
        let button = actionButton("Unavailable", x: 558, y: y + 7, width: 120, parent: parent) {}
        button.isEnabled = false; button.alphaValue = 0.55
    }

    private func colorField(_ label: String, value: String, key: String, x: CGFloat, y: CGFloat, parent: NSView) {
        addLabel(label, frame: NSRect(x: x, y: y - 20, width: 140, height: 18), size: 11, muted: true, parent: parent)
        let well = ClosureColorWell(frame: NSRect(x: x, y: y, width: 30, height: 30)); well.color = NSColor(hex: value) ?? .white
        let field = NSTextField(string: value.uppercased()); field.frame = NSRect(x: x + 36, y: y, width: 104, height: 30)
        field.identifier = NSUserInterfaceItemIdentifier("custom.\(key)"); styleField(field); field.delegate = self; parent.addSubview(field)
        well.change = { [weak self, weak field] color in
            guard let self, let field, let normalized = color.rgbHex else { return }
            field.stringValue = normalized; var custom = self.settings["custom_theme"] as? [String: Any] ?? [:]
            custom[key] = normalized; self.settings["custom_theme"] = custom; self.changed(rerender: true)
        }; well.target = well; well.action = #selector(ClosureColorWell.selectedColor); parent.addSubview(well)
    }

    func controlTextDidEndEditing(_ obj: Notification) {
        guard !rebuilding else { return }
        guard let field = obj.object as? NSTextField else { return }
        if field.identifier?.rawValue.hasPrefix("custom.") == true {
            var custom = settings["custom_theme"] as? [String: Any] ?? [:]
            guard let normalized = Self.normalizeHex(field.stringValue) else { field.stringValue = custom.string(String(field.identifier!.rawValue.dropFirst(7))); return }
            field.stringValue = normalized; custom[String(field.identifier!.rawValue.dropFirst(7))] = normalized
            settings["custom_theme"] = custom; changed(rerender: true)
        } else if field.identifier?.rawValue == "setting.output_directory" {
            set(field.stringValue, for: "output_directory", rerender: false)
        }
    }

    static func normalizeHex(_ input: String) -> String? {
        var value = input.trimmingCharacters(in: .whitespacesAndNewlines).uppercased()
        guard value.hasPrefix("#") else { return nil }
        if value.count == 4 { value = "#" + value.dropFirst().map { "\($0)\($0)" }.joined() }
        guard value.utf8.count == 7, value.dropFirst().allSatisfy({ $0.isASCII && $0.isHexDigit }) else { return nil }
        return value
    }

    private func chooseFolder() {
        let picker = NSOpenPanel(); picker.canChooseDirectories = true; picker.canChooseFiles = false
        picker.allowsMultipleSelection = false; picker.prompt = "Choose"; picker.message = "Choose capture folder"
        picker.beginSheetModal(for: root.window!) { [weak self] response in
            if response == .OK, let path = picker.url?.path { self?.set(path, for: "output_directory", rerender: true) }
        }
    }

    private func set(_ value: Any, for key: String, rerender: Bool) { settings[key] = value; changed(rerender: rerender) }
    private func changed(rerender: Bool) {
        setStatus("Saving changes…", kind: "saving"); saveFailed = false
        settingsChanged(settings)
        latestRevision = store.save(settings) { [weak self] revision, result in
            guard let self, revision == self.latestRevision else { return }
            switch result {
            case .success(let saved):
                self.settings = saved; self.settingsChanged(saved)
                self.settingsPersisted(saved)
                self.setStatus("✓  Changes saved", kind: "saved")
            case .failure(let error): self.saveFailed = true; self.setStatus("Couldn’t save changes: \(error.localizedDescription) — Retry", kind: "error")
            }
        }
        if rerender {
            appearanceChanged(settings.string("appearance", "system"), settings.string("theme", "mustard"), settings["custom_theme"] as? [String: Any] ?? [:]); restyle()
        }
    }

    private func setStatus(_ text: String, kind: String) {
        status.stringValue = text; status.font = .systemFont(ofSize: 11, weight: .medium)
        status.textColor = tokens.color(kind == "error" ? "danger-text" : kind == "saved" ? "positive-text" : "text-muted")
        status.setAccessibilityLabel(text)
        retryButton?.isHidden = kind != "error" || !saveFailed
    }

    func retrySave() { if saveFailed { changed(rerender: false) } }
    func flush() {
        if let field = root.window?.firstResponder as? NSTextView,
           let editor = field.delegate as? NSTextField {
            controlTextDidEndEditing(Notification(name: NSControl.textDidEndEditingNotification, object: editor))
        }
        store.flush()
    }
    private func reveal(_ id: String) { if let view = sectionViews[id] { scroll.contentView.scroll(to: NSPoint(x: 0, y: max(0, view.frame.minY - 20))) } }

    func showFind() {
        if findBar != nil { windowFocusFind(); return }
        let bar = Surface(frame: NSRect(x: 212, y: 64, width: root.bounds.width - 228, height: 44)); bar.autoresizingMask = [.width]
        bar.wantsLayer = true; bar.layer?.backgroundColor = tokens.color("surface-canvas").cgColor
        let field = NSTextField(string: ""); field.placeholderString = "Find settings"; field.frame = NSRect(x: 12, y: 7, width: bar.bounds.width - 286, height: 30)
        field.autoresizingMask = [.width]; field.identifier = NSUserInterfaceItemIdentifier("find"); styleField(field); field.delegate = self; bar.addSubview(field)
        let count = addLabel("", frame: NSRect(x: bar.bounds.width - 266, y: 11, width: 126, height: 22), size: 11, muted: true, parent: bar)
        count.alignment = .right; count.autoresizingMask = [.minXMargin]; findCount = count
        for (i, data) in [("↑", -1), ("↓", 1), ("×", 0)].enumerated() {
            _ = actionButton(data.0, x: bar.bounds.width - 132 + CGFloat(i * 40), y: 7, width: 34, parent: bar) { [weak self] in
                if data.1 == 0 { self?.closeFind() } else { self?.stepFind(data.1) }
            }
        }
        root.addSubview(bar); findBar = bar; findField = field; scroll.frame.origin.y = 108; scroll.frame.size.height -= 36
        windowFocusFind()
    }

    private func windowFocusFind() { root.window?.makeFirstResponder(findField) }
    func controlTextDidChange(_ obj: Notification) { if (obj.object as? NSTextField)?.identifier?.rawValue == "find" { updateFind() } }
    private func updateFind() {
        for (view, _) in searchable { view.wantsLayer = true; view.layer?.backgroundColor = NSColor.clear.cgColor; view.layer?.cornerRadius = 4 }
        let query = findField?.stringValue.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() ?? ""
        matches = query.isEmpty ? [] : searchable.filter { $0.1.lowercased().contains(query) }.map(\.0)
        matchIndex = min(matchIndex, max(0, matches.count - 1)); highlightMatch()
        findCount?.stringValue = query.isEmpty ? "" : matches.isEmpty ? "No matches" : "\(matchIndex + 1) of \(matches.count)"
    }

    func stepFind(_ delta: Int) { guard !matches.isEmpty else { return }; matchIndex = (matchIndex + delta + matches.count) % matches.count; highlightMatch(); findCount?.stringValue = "\(matchIndex + 1) of \(matches.count)" }
    private func highlightMatch() {
        for (index, view) in matches.enumerated() { view.layer?.backgroundColor = tokens.color(index == matchIndex ? "surface-active" : "surface-selected").cgColor }
        if !matches.isEmpty {
            let target = matches[matchIndex].convert(matches[matchIndex].bounds, to: document)
            document.scrollToVisible(target.insetBy(dx: -20, dy: -40))
        }
    }
    func closeFind() { findBar?.removeFromSuperview(); findBar = nil; findField = nil; findCount = nil; matches = []; scroll.frame.origin.y = 72; scroll.frame.size.height += 36; updateFind() }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
        if commandSelector == #selector(NSResponder.cancelOperation(_:)), findBar != nil { closeFind(); return true }
        return false
    }

    func exerciseAppearance(_ value: String) { set(value, for: "appearance", rerender: true) }

    func restyle() {
        guard !rebuilding else { return }
        rebuilding = true
        defer { rebuilding = false }
        let oldY = scroll.contentView.bounds.origin.y
        let query = findField?.stringValue
        let focused = (root.window?.firstResponder as? NSView)?.accessibilityLabel()
        root.subviews.forEach { $0.removeFromSuperview() }
        findBar = nil; findField = nil; findCount = nil
        root.layer?.backgroundColor = tokens.color("surface-canvas").cgColor
        buildShell()
        rebuildCards()
        scroll.contentView.scroll(to: NSPoint(x: 0, y: oldY))
        if let query { showFind(); findField?.stringValue = query; updateFind() }
        else if let focused {
            func restore(_ view: NSView) -> NSView? {
                if view.accessibilityLabel() == focused, view.acceptsFirstResponder { return view }
                for child in view.subviews { if let found = restore(child) { return found } }
                return nil
            }
            if let view = restore(root) { root.window?.makeFirstResponder(view) }
        }
        setStatus(status.stringValue, kind: saveFailed ? "error" : "idle")
    }

    @discardableResult private func addLabel(_ text: String, frame: NSRect, size: CGFloat, weight: NSFont.Weight = .regular,
        muted: Bool = false, parent: NSView, centered: Bool = false, color: String? = nil) -> NSTextField {
        let label = NSTextField(labelWithString: text); label.frame = frame; label.font = .systemFont(ofSize: size, weight: weight)
        label.textColor = tokens.color(color ?? (muted ? "text-muted" : "text")); label.alignment = centered ? .center : .left
        parent.addSubview(label); return label
    }
    private func actionButton(_ title: String, x: CGFloat, y: CGFloat, width: CGFloat, parent: NSView, action: @escaping () -> Void) -> CaptureButton {
        let button = CaptureButton(title, frame: NSRect(x: x, y: y, width: width, height: 32), tokens: tokens, action: action); parent.addSubview(button); return button
    }
    private func styleField(_ field: NSTextField) {
        field.isBordered = false; field.drawsBackground = true; field.backgroundColor = tokens.color("surface-field")
        field.textColor = tokens.color("text"); field.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        field.wantsLayer = true; field.layer?.cornerRadius = tokens.number("r-md"); field.layer?.borderWidth = 1; field.layer?.borderColor = tokens.color("control-border").cgColor
    }
}
