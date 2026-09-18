import AppKit
import QuartzCore

enum Metrics {
    static func emit(_ event: String, milliseconds: Double, detail: String = "") {
        write(["event": event, "milliseconds": milliseconds, "detail": detail])
    }
    static func write(_ fields: [String: Any]) {
        var row = fields
        row["schema"] = 1
        row["pid"] = ProcessInfo.processInfo.processIdentifier
        row["uptime"] = ProcessInfo.processInfo.systemUptime
        if let data = try? JSONSerialization.data(withJSONObject: row, options: [.sortedKeys]) {
            FileHandle.standardOutput.write(data + Data([10]))
        }
    }
}

final class Surface: NSView {
    override var isFlipped: Bool { true }
}

// NSButton keeps keyboard activation, target/action and accessibility behavior;
// only the visual treatment is custom. No stock Aqua bezel in the content UI.
final class CaptureButton: NSButton {
    var tokens: Tokens!
    var selected = false
    var glass = false
    var actionBlock: (() -> Void)?

    init(_ title: String, frame: NSRect, tokens: Tokens, glass: Bool = false, action: @escaping () -> Void) {
        super.init(frame: frame)
        self.tokens = tokens
        self.glass = glass
        self.title = title
        actionBlock = action
        isBordered = false
        setButtonType(.momentaryPushIn)
        target = self
        self.action = #selector(activate)
        setAccessibilityLabel(title)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func activate() { actionBlock?() }

    override func becomeFirstResponder() -> Bool {
        let accepted = super.becomeFirstResponder()
        needsDisplay = true
        return accepted
    }

    override func resignFirstResponder() -> Bool {
        let accepted = super.resignFirstResponder()
        needsDisplay = true
        return accepted
    }

    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1),
            xRadius: tokens.number("r-md"), yRadius: tokens.number("r-md"))
        let fill = !isEnabled ? (glass ? "glass" : "surface-sunken")
            : cell?.isHighlighted == true ? (glass ? "glass-active" : "surface-active")
            : selected ? "surface-selected" : (glass ? "glass-raised" : "control")
        tokens.color(fill).setFill()
        path.fill()
        tokens.color(selected && isEnabled ? "theme-accent" : (glass ? "glass-border" : "control-border")).setStroke()
        path.lineWidth = 1
        path.stroke()
        let font = NSFont.systemFont(ofSize: tokens.number("text-md"), weight: .medium)
        let attributes: [NSAttributedString.Key: Any] = [
            .font: font, .foregroundColor: tokens.color(isEnabled ? (glass ? "glass-text" : "text") : (glass ? "glass-text-subtle" : "text-faint")),
        ]
        let size = (title as NSString).size(withAttributes: attributes)
        (title as NSString).draw(at: CGPoint(x: (bounds.width - size.width) / 2,
            y: (bounds.height - size.height) / 2), withAttributes: attributes)
        if window?.firstResponder === self {
            tokens.color("theme-accent").setStroke()
            path.lineWidth = 2
            path.stroke()
        }
    }
}

final class Workbench: NSObject, NSApplicationDelegate, NSTableViewDataSource, NSTableViewDelegate {
    let options: Options
    private var window: NSWindow!
    private var content: Surface!
    private var preview: PreviewView?
    private var table: NSTableView?
    private var preferencesController: PreferencesController?
    private var liveController: LiveCaptureController?
    private var scene: String
    private var appearance: String
    private var theme: String
    private var customTheme: [String: Any] = [:]
    private var paused = false
    private var historyCount: Int
    private lazy var resolvedTokens = makeTokens()
    private var exerciseDirectory: URL?
    private var tokens: Tokens { resolvedTokens }
    private func makeTokens() -> Tokens {
        let mode = appearance == "system"
            ? (NSApp.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? "dark" : "light")
            : appearance
        let base = Tokens.variants["\(mode)-\(theme)"] ?? Tokens.variants["\(mode)-mustard"]!
        return theme == "custom" ? base.applyingCustomTheme(customTheme, light: mode == "light") : base
    }

    init(options: Options) {
        self.options = options
        scene = options.live ? "live" : options.scene
        appearance = options.appearance
        theme = options.theme
        historyCount = options.historyCount
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let menu = NSMenu()
        let item = NSMenuItem()
        menu.addItem(item)
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "Quit Captures Native Workbench", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        item.submenu = appMenu
        let editItem = NSMenuItem()
        menu.addItem(editItem)
        let editMenu = NSMenu(title: "Edit")
        editMenu.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editMenu.addItem(.separator())
        let find = editMenu.addItem(withTitle: "Find…", action: #selector(showFind), keyEquivalent: "f")
        find.target = self
        let next = editMenu.addItem(withTitle: "Find Next", action: #selector(findNext), keyEquivalent: "g")
        next.target = self
        let previous = editMenu.addItem(withTitle: "Find Previous", action: #selector(findPrevious), keyEquivalent: "G")
        previous.keyEquivalentModifierMask = [.command, .shift]
        previous.target = self
        editItem.submenu = editMenu
        NSApp.mainMenu = menu
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1000, height: 720),
            styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
        window.title = options.live ? "Captures Native — capture workspace" : "Captures Native — development fixtures"
        window.isReleasedWhenClosed = false
        window.center()
        render()
        if scene != "idle" { window.makeKeyAndOrderFront(nil) }
        NSApp.activate(ignoringOtherApps: true)
        Metrics.write(["event": "ready", "scene": scene, "window": window.windowNumber,
            "scale": window.backingScaleFactor, "appearance": appearance, "theme": theme,
            "historyCount": historyCount, "referenceChips": options.referenceChips])
        DistributedNotificationCenter.default().addObserver(self, selector: #selector(systemAppearanceChanged),
            name: NSNotification.Name("AppleInterfaceThemeChangedNotification"), object: nil)
        if let path = options.screenshot {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in self?.captureWindow(to: path) }
        }
        if options.exercise {
            for cycle in 0..<6 {
                DispatchQueue.main.asyncAfter(deadline: .now() + 2 + Double(cycle) * 4) { [weak self] in
                    self?.exercise(cycle)
                }
            }
        }
        if let seconds = options.quitAfter {
            DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { NSApp.terminate(nil) }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        preferencesController?.flush()
        LiveCaptureController.flush()
        if let exerciseDirectory { try? FileManager.default.removeItem(at: exerciseDirectory) }
        return .terminateNow
    }

    @objc private func showFind() { if scene == "preferences" { preferencesController?.showFind() } }
    @objc private func findNext() { preferencesController?.stepFind(1) }
    @objc private func findPrevious() { preferencesController?.stepFind(-1) }
    @objc private func systemAppearanceChanged() {
        guard appearance == "system" else { return }
        resolvedTokens = makeTokens()
        preferencesController?.restyle()
    }

    private func render() {
        let started = CACurrentMediaTime()
        preview = nil
        table = nil
        liveController = nil
        content = Surface(frame: NSRect(x: 0, y: 0, width: 1000, height: 720))
        content.wantsLayer = true
        content.layer!.backgroundColor = tokens.color("surface-canvas").cgColor
        window.appearance = NSAppearance(named: tokens.color("text").brightnessComponent > 0.5 ? .darkAqua : .aqua)
        window.contentView = content
        if scene == "preferences" {
            do {
                let path = options.exercise
                    ? exerciseSettingsPath()
                    : options.settingsFile
                let store = try SettingsStore(path: path)
                preferencesController = PreferencesController(root: content, store: store, tokens: { [weak self] in self?.tokens ?? Tokens.variants["dark-mustard"]! }, appearanceChanged: { [weak self] appearance, theme, customTheme in
                    guard let self else { return }
                    let changed = self.appearance != appearance || self.theme != theme || !NSDictionary(dictionary: self.customTheme).isEqual(to: customTheme)
                    self.appearance = appearance
                    self.theme = theme
                    self.customTheme = customTheme
                    if changed {
                        self.resolvedTokens = self.makeTokens()
                    }
                    self.window.appearance = appearance == "system" ? nil : NSAppearance(named: appearance == "dark" ? .darkAqua : .aqua)
                }, showHistory: { [weak self] in self?.scene = self?.options.live == true ? "live" : "history"; self?.render() },
                   liveCaptureAvailable: options.live,
                   initialAppearance: options.appearanceOverride ? options.appearance : nil,
                   initialTheme: options.themeOverride ? options.theme : nil)
            } catch {
                label("Preferences unavailable: \(error.localizedDescription)", x: 32, y: 32, width: 900)
            }
            Metrics.emit("scene-construction", milliseconds: (CACurrentMediaTime() - started) * 1000, detail: scene)
            return
        }
        preferencesController = nil
        if scene == "live" {
            liveController = LiveCaptureController(root: content, window: window, tokens: tokens,
                historyRoot: options.historyRoot, settingsPath: options.settingsFile) { [weak self] in self?.scene = "preferences"; self?.render() }
            Metrics.emit("scene-construction", milliseconds: (CACurrentMediaTime() - started) * 1000, detail: scene)
            return
        }
        let sidebar = Surface(frame: NSRect(x: 0, y: 0, width: 196, height: 720))
        sidebar.wantsLayer = true
        sidebar.layer!.backgroundColor = tokens.color("surface-sunken").cgColor
        content.addSubview(sidebar)
        if let url = Bundle.module.url(forResource: "icon", withExtension: "svg"),
           let image = NSImage(contentsOf: url) {
            let icon = NSImageView(frame: NSRect(x: 20, y: 20, width: 28, height: 28))
            icon.image = image
            icon.setAccessibilityElement(false)
            sidebar.addSubview(icon)
        }
        label("Captures", x: 58, y: 22, width: 125, size: "text-xl", parent: sidebar)
        for (i, name) in ["preferences", "history", "hud", "preview"].enumerated() {
            let button = CaptureButton(name == "hud" ? "Recording controls" : name.capitalized,
                frame: NSRect(x: 12, y: 70 + i * 44, width: 172, height: 34), tokens: tokens) { [weak self] in
                    self?.scene = name
                    self?.render()
                }
            button.selected = scene == name
            sidebar.addSubview(button)
        }
        label("Fixture mode", x: 24, y: 640, width: 155, size: "text-sm", muted: true, parent: sidebar)
        label("No capture access", x: 24, y: 663, width: 155, size: "text-sm", muted: true, parent: sidebar)
        label(scene == "hud" ? "Recording controls" : scene.capitalized,
            x: 220, y: 20, width: 650, size: "text-xl")
        label("Native rendering workbench · synthetic data, not functional parity",
            x: 220, y: 47, width: 740, size: "text-sm", muted: true)
        switch scene {
        case "history": history()
        case "hud": hud()
        case "preview": previews()
        default: break
        }
        Metrics.emit("scene-construction", milliseconds: (CACurrentMediaTime() - started) * 1000, detail: scene)
    }

    private func exerciseSettingsPath() -> String {
        if exerciseDirectory == nil {
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent("captures-native-exercise-\(ProcessInfo.processInfo.processIdentifier)")
            try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            exerciseDirectory = directory
        }
        return exerciseDirectory!.appendingPathComponent("settings.json").path
    }

    @discardableResult private func label(_ text: String, x: CGFloat, y: CGFloat, width: CGFloat,
        size: String = "text-md", muted: Bool = false, glass: Bool = false, parent: NSView? = nil) -> NSTextField {
        let field = NSTextField(labelWithString: text)
        field.frame = NSRect(x: x, y: y, width: width, height: 26)
        field.font = NSFont.systemFont(ofSize: tokens.number(size), weight: size == "text-xl" ? .semibold : .regular)
        field.textColor = tokens.color(glass ? "glass-text" : (muted ? "text-muted" : "text"))
        (parent ?? content).addSubview(field)
        return field
    }

    private func panel(_ rect: NSRect, glass: Bool = false) -> Surface {
        let view = Surface(frame: rect)
        view.wantsLayer = true
        view.layer!.backgroundColor = tokens.color(glass ? "glass-strong" : "surface-raised").cgColor
        view.layer!.cornerRadius = tokens.number("r-xl")
        view.layer!.borderWidth = 1
        view.layer!.borderColor = tokens.color(glass ? "glass-border" : "border").cgColor
        content.addSubview(view)
        return view
    }

    private func captureWindow(to path: String) {
        guard let view = window.contentView else { return }
        let representation = view.bitmapImageRepForCachingDisplay(in: view.bounds)
        guard let representation else { return }
        view.cacheDisplay(in: view.bounds, to: representation)
        guard let data = representation.representation(using: .png, properties: [:]) else { return }
        do {
            try data.write(to: URL(fileURLWithPath: path), options: .atomic)
            Metrics.write(["event": "screenshot", "path": path, "window": window.windowNumber])
        } catch { Metrics.write(["event": "screenshot-error", "detail": error.localizedDescription]) }
    }

    private func history() {
        label("\(historyCount) synthetic captures · reusable native rows", x: 220, y: 90, width: 620, muted: true)
        let empty = CaptureButton(historyCount == 0 ? "Load 1,000" : "Empty state",
            frame: NSRect(x: 836, y: 82, width: 136, height: 34), tokens: tokens) { [weak self] in
                guard let self else { return }
                self.historyCount = self.historyCount == 0 ? 1000 : 0
                self.render()
            }
        content.addSubview(empty)
        if historyCount == 0 {
            label("No captures yet", x: 240, y: 170, width: 650, size: "text-xl")
            return
        }
        let scroll = NSScrollView(frame: NSRect(x: 220, y: 130, width: 752, height: 550))
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        let table = NSTableView(frame: scroll.bounds)
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("capture"))
        column.width = 728
        table.addTableColumn(column)
        table.headerView = nil
        table.rowHeight = 68
        table.backgroundColor = tokens.color("surface-canvas")
        table.selectionHighlightStyle = .none
        table.dataSource = self
        table.delegate = self
        table.setAccessibilityLabel("Capture history fixtures")
        scroll.documentView = table
        content.addSubview(scroll)
        self.table = table
    }

    func numberOfRows(in tableView: NSTableView) -> Int { historyCount }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let id = NSUserInterfaceItemIdentifier("capture-row")
        let view = tableView.makeView(withIdentifier: id, owner: self) as? NSTableCellView ?? NSTableCellView()
        view.identifier = id
        view.subviews.forEach { $0.removeFromSuperview() }
        view.wantsLayer = true
        view.layer!.backgroundColor = tokens.color("surface-raised").cgColor
        view.layer!.cornerRadius = tokens.number("r-lg")
        let title = NSTextField(labelWithString: "\(row % 3 == 0 ? "Recording" : "Screenshot") \(row + 1)")
        title.frame = NSRect(x: 16, y: 32, width: 690, height: 22)
        title.font = .systemFont(ofSize: tokens.number("text-md"), weight: .medium)
        title.textColor = tokens.color("text")
        let subtitle = NSTextField(labelWithString: row % 3 == 0 ? "Video · 00:24 · fixture" : "1920 × 1080 · PNG · fixture")
        subtitle.frame = NSRect(x: 16, y: 10, width: 690, height: 20)
        subtitle.font = .systemFont(ofSize: tokens.number("text-sm"))
        subtitle.textColor = tokens.color("text-muted")
        view.addSubview(title)
        view.addSubview(subtitle)
        view.textField = title
        return view
    }

    private func hud() {
        let view = panel(NSRect(x: 320, y: 270, width: 550, height: 100), glass: true)
        label(paused ? "Paused · 00:24" : "Recording · 00:24", x: 24, y: 19, width: 250,
            size: "text-lg", glass: true, parent: view)
        label("Static timer fixture — no recording engine", x: 24, y: 53, width: 320,
            size: "text-sm", glass: true, parent: view)
        let button = CaptureButton(paused ? "Resume" : "Pause", frame: NSRect(x: 378, y: 30, width: 145, height: 36), tokens: tokens, glass: true) { [weak self] in
            guard let self else { return }
            self.paused.toggle()
            self.render()
        }
        view.addSubview(button)
    }

    private func previews() {
        do {
            let view = try PreviewView(frame: NSRect(x: 320, y: 80, width: 524, height: 550), atlas: !options.referenceChips)
            content.addSubview(view)
            preview = view
            for (index, name) in ["Cold dissolve", "Warm dissolve", "Reset"].enumerated() {
                content.addSubview(CaptureButton(name, frame: NSRect(x: 280 + index * 220, y: 645, width: 200, height: 34), tokens: tokens) { [weak view] in
                    view?.reset()
                    if index < 2 { view?.dissolve(cold: index == 0) }
                })
            }
        } catch {
            label("Preview unavailable: \(error)", x: 220, y: 140, width: 750)
            Metrics.emit("effect-error", milliseconds: 0, detail: String(describing: error))
        }
    }

    private func exercise(_ cycle: Int) {
        let started = CACurrentMediaTime()
        switch scene {
        case "preferences": preferencesController?.exerciseAppearance(cycle % 2 == 0 ? "light" : "dark")
        case "history": table?.scrollRowToVisible(cycle % 2 == 0 ? max(0, historyCount - 1) : 0)
        case "hud": paused.toggle(); render()
        case "preview": preview?.reset(); preview?.dissolve(cold: cycle % 2 == 0)
        default: break
        }
        Metrics.emit("scripted-action", milliseconds: (CACurrentMediaTime() - started) * 1000,
            detail: "\(scene), cycle=\(cycle); CPU submission, not presentation latency")
    }
}
