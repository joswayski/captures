import AppKit
import ImageIO
import CCapturesSettings

struct MiniPreviewSettings: Equatable {
    let enabled: Bool
    let placement: String
    let includeInCaptures: Bool
}

final class MiniPreviewCardView: NSView {
    private let tokens: Tokens
    private let imageView = NSImageView()
    private let title = NSTextField(labelWithString: "Screenshot")
    private let status = NSTextField(labelWithString: "")
    private(set) var artifactID: String
    override var isFlipped: Bool { true }

    init(frame: NSRect, artifactID: String, image: NSImage, tokens: Tokens,
         copy: @escaping () -> Void, save: @escaping () -> Void,
         open: @escaping () -> Void, dismiss: @escaping () -> Void) {
        self.artifactID = artifactID; self.tokens = tokens
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = tokens.color("glass-strong").cgColor
        layer?.cornerRadius = tokens.number("r-lg")
        layer?.borderWidth = 1; layer?.borderColor = tokens.color("glass-border").cgColor

        imageView.frame = bounds
        imageView.image = image; imageView.imageScaling = .scaleProportionallyUpOrDown
        imageView.wantsLayer = true; imageView.layer?.cornerRadius = tokens.number("r-lg")
        imageView.layer?.masksToBounds = true
        imageView.setAccessibilityLabel("Screenshot thumbnail")
        addSubview(imageView)

        let inset = tokens.number("s-2")
        title.frame = NSRect(x: inset, y: inset, width: 150, height: 19)
        title.font = .systemFont(ofSize: tokens.number("text-sm"), weight: .medium)
        title.textColor = tokens.color("glass-text"); addSubview(title)
        status.frame = NSRect(x: bounds.width - 126, y: inset, width: 118, height: 19)
        status.alignment = .right; status.lineBreakMode = .byTruncatingTail
        status.font = .systemFont(ofSize: tokens.number("text-sm"))
        status.textColor = tokens.color("glass-text-muted"); addSubview(status)

        let actions: [(String, () -> Void)] = [("Copy", copy), ("Save", save),
            ("Open", open), ("Dismiss", dismiss)]
        let buttonHeight = tokens.number("h-md"), gap = tokens.number("s-2")
        let actionWidth = (bounds.width - inset * 2 - gap * 3) / 4
        for (index, action) in actions.enumerated() {
            let button = CaptureButton(action.0,
                frame: NSRect(x: inset + CGFloat(index) * (actionWidth + gap),
                    y: bounds.height - buttonHeight - inset, width: actionWidth, height: buttonHeight),
                tokens: tokens, glass: true, action: action.1)
            addSubview(button)
        }
        setAccessibilityRole(.group); setAccessibilityLabel("Screenshot mini preview")
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func setStatus(_ value: String) {
        status.stringValue = value
        status.setAccessibilityLabel(value.isEmpty ? nil : value)
    }
}

private struct MiniPreviewResource {
    let artifact: CaptureArtifact
    let image: NSImage
}

final class MiniPreviewView: NSView {
    private let geometry: CapturesPreviewGeometry
    private let tokens: Tokens
    private let scroll = NSScrollView()
    private let document = FlippedView()
    private var cards: [String: MiniPreviewCardView] = [:]
    private var expandButton: CaptureButton?
    private var collapseButton: CaptureButton?
    private var clearButton: CaptureButton?
    private(set) var artifactIDs: [String] = []
    override var isFlipped: Bool { true }

    init(geometry: CapturesPreviewGeometry, resources: [String: MiniPreviewResource],
         ids: [String], layouts: [String: CapturesPreviewCardLayout], collapsed: Bool,
         topAnchor: Bool, tokens: Tokens, copy: @escaping (String) -> Void,
         save: @escaping (String) -> Void, open: @escaping (String) -> Void,
         dismiss: @escaping (String) -> Void, setCollapsed: @escaping (Bool) -> Void,
         clearAll: @escaping () -> Void) {
        self.geometry = geometry; self.tokens = tokens; artifactIDs = ids
        super.init(frame: NSRect(x: 0, y: 0, width: geometry.width, height: geometry.height))
        wantsLayer = true

        scroll.frame = bounds; scroll.autoresizingMask = [.width, .height]
        scroll.drawsBackground = false; scroll.hasVerticalScroller = !collapsed
        scroll.scrollerStyle = .overlay; scroll.borderType = .noBorder
        scroll.contentView.drawsBackground = false
        addSubview(scroll)

        let padding = CGFloat(geometry.padding), cardHeight = CGFloat(geometry.card_height)
        let cardWidth = bounds.width - padding * 2
        let contentBottom = layouts.values.map { CGFloat($0.y) + cardHeight }.max() ?? 0
        document.frame = NSRect(x: 0, y: 0, width: bounds.width,
            height: max(bounds.height, contentBottom + padding))
        scroll.documentView = document

        for id in ids {
            guard let resource = resources[id], let layout = layouts[id] else { continue }
            let card = MiniPreviewCardView(frame: NSRect(x: padding, y: CGFloat(layout.y),
                width: cardWidth, height: cardHeight), artifactID: id,
                image: resource.image, tokens: tokens,
                copy: { copy(id) }, save: { save(id) }, open: { open(id) },
                dismiss: { dismiss(id) })
            card.layer?.zPosition = CGFloat(layout.depth)
            card.isHidden = false
            card.subviews.compactMap { $0 as? NSControl }.forEach { $0.isEnabled = layout.interactive }
            card.setAccessibilityElement(layout.interactive)
            document.addSubview(card); cards[id] = card
        }

        let controlY = topAnchor ? 16 : bounds.height - 44
        if collapsed {
            let button = CaptureButton(ids.count == 1 ? "Show preview" : "Show all",
                frame: NSRect(x: padding, y: controlY, width: 100, height: 28),
                tokens: tokens, glass: true) { setCollapsed(false) }
            button.setAccessibilityLabel(ids.count == 1 ? "Expand preview" : "Expand \(ids.count) previews")
            addSubview(button); expandButton = button
        } else if ids.count >= 2 {
            let collapse = CaptureButton("Show less",
                frame: NSRect(x: padding, y: controlY, width: 92, height: 28),
                tokens: tokens, glass: true) { setCollapsed(true) }
            let clear = CaptureButton("Clear all",
                frame: NSRect(x: padding + 100, y: controlY, width: 82, height: 28),
                tokens: tokens, glass: true, action: clearAll)
            addSubview(collapse); addSubview(clear)
            collapseButton = collapse; clearButton = clear
        }

        if !collapsed {
            let newestAtTop = topAnchor
            let destinationY = newestAtTop ? 0 : max(0, document.bounds.height - scroll.contentView.bounds.height)
            scroll.contentView.scroll(to: NSPoint(x: 0, y: destinationY))
            scroll.reflectScrolledClipView(scroll.contentView)
        }
        setAccessibilityRole(.group)
        setAccessibilityLabel(ids.count == 1 ? "Screenshot mini preview" : "\(ids.count) screenshot mini previews")
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func setStatus(_ value: String, for artifactID: String) {
        cards[artifactID]?.setStatus(value)
    }
}

final class MiniPreviewPanel: NSPanel {
    let previewView: MiniPreviewView
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    init(frame: NSRect, geometry: CapturesPreviewGeometry,
         resources: [String: MiniPreviewResource], ids: [String],
         layouts: [String: CapturesPreviewCardLayout], collapsed: Bool,
         topAnchor: Bool, tokens: Tokens, copy: @escaping (String) -> Void,
         save: @escaping (String) -> Void, open: @escaping (String) -> Void,
         dismiss: @escaping (String) -> Void, setCollapsed: @escaping (Bool) -> Void,
         clearAll: @escaping () -> Void) {
        previewView = MiniPreviewView(geometry: geometry, resources: resources, ids: ids,
            layouts: layouts, collapsed: collapsed, topAnchor: topAnchor, tokens: tokens,
            copy: copy, save: save, open: open, dismiss: dismiss,
            setCollapsed: setCollapsed, clearAll: clearAll)
        super.init(contentRect: frame, styleMask: [.borderless, .nonactivatingPanel],
                   backing: .buffered, defer: false)
        isReleasedWhenClosed = false; isOpaque = false; backgroundColor = .clear
        hasShadow = true; level = .floating
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        hidesOnDeactivate = false; isMovable = false; contentView = previewView
        setAccessibilityLabel(ids.count == 1 ? "Screenshot mini preview" : "Screenshot mini previews")
    }
}

/// AppKit presentation for one latest screenshot. Rust owns visibility,
/// replacement generations and monitor-relative placement policy.
final class MiniPreviewController {
    typealias ArtifactAction = (CaptureArtifact) -> Void

    private let policy: NativePreviewPolicy
    private let tokens: Tokens
    private let imageLoader: (String) throws -> NSImage
    private let screenProvider: () -> [NSScreen]
    private var panel: MiniPreviewPanel?
    private var artifact: CaptureArtifact?
    private var settings = MiniPreviewSettings(enabled: false, placement: "bottom_right",
                                               includeInCaptures: false)
    private var screenID: String?
    private var pendingArtifactID: String?
    private var decodeGeneration = 0
    var copyArtifact: ArtifactAction = { _ in }
    var saveArtifact: ArtifactAction = { _ in }
    var openArtifact: ArtifactAction = { _ in }
    var presentedArtifactID: String? { artifact?.id }
    var isPanelVisible: Bool { panel?.isVisible == true }

    init(tokens: Tokens, policy: NativePreviewPolicy = NativePreviewPolicy(),
         screenProvider: @escaping () -> [NSScreen] = { NSScreen.screens },
         imageLoader: @escaping (String) throws -> NSImage = MiniPreviewController.loadImage) {
        self.tokens = tokens; self.policy = policy
        self.screenProvider = screenProvider; self.imageLoader = imageLoader
    }

    func beginCapture(settings: MiniPreviewSettings) -> UInt64? {
        precondition(Thread.isMainThread)
        self.settings = settings
        guard let generation = policy.beginCapture() else { return nil }
        decodeGeneration += 1
        pendingArtifactID = nil
        policy.suppressCaptureUI(true)
        updateVisibility()
        return generation
    }

    func restoreCapture(generation: UInt64?) {
        precondition(Thread.isMainThread)
        if let generation { _ = policy.restore(generation: generation) }
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
        pendingArtifactID = artifact.id
        policy.suppressCaptureUI(false)
        decodeGeneration += 1
        let decode = decodeGeneration
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self else { return }
            let result = Result { try self.imageLoader(artifact.previewPath) }
            DispatchQueue.main.async { [weak self] in
                guard let self, self.decodeGeneration == decode else { return }
                switch result {
                case .success(let image):
                    guard self.policy.ready(artifact: artifact.id) else { return }
                    self.pendingArtifactID = nil
                    self.artifact = artifact
                    self.makePanel(image: image)
                    self.updateVisibility()
                case .failure:
                    _ = self.policy.stopWaiting()
                    self.pendingArtifactID = nil
                    self.updateVisibility()
                }
            }
        }
    }

    func updateSettings(_ settings: MiniPreviewSettings) {
        precondition(Thread.isMainThread)
        self.settings = settings
        updateVisibility()
    }

    func reconcileHistory(ids: Set<String>) {
        precondition(Thread.isMainThread)
        if let pendingArtifactID, !ids.contains(pendingArtifactID) {
            decodeGeneration += 1
            self.pendingArtifactID = nil
            _ = policy.stopWaiting()
            updateVisibility()
        }
        if let artifact, !ids.contains(artifact.id) { dismiss() }
    }

    func setStatus(_ value: String, for artifactID: String) {
        guard artifact?.id == artifactID else { return }
        panel?.previewView.setStatus(value)
    }

    func dismiss() {
        precondition(Thread.isMainThread)
        decodeGeneration += 1
        _ = policy.stopWaiting()
        artifact = nil; pendingArtifactID = nil; screenID = nil
        panel?.close(); panel = nil
    }

    func close() { dismiss() }

    private func makePanel(image: NSImage) {
        panel?.close(); panel = nil
        guard let artifact, let screen = targetScreen(),
              let monitor = Self.monitor(for: screen),
              let geometry = NativePreviewLayout.geometry(monitor: monitor, count: 1,
                  placement: settings.placement) else { return }
        let frame = Self.appKitFrame(geometry: geometry, monitor: monitor,
                                     screenFrame: screen.frame)
        let next = MiniPreviewPanel(frame: frame, geometry: geometry,
            artifactID: artifact.id, image: image, tokens: tokens,
            copy: { [weak self] in self?.perform(\.copyArtifact) },
            save: { [weak self] in self?.perform(\.saveArtifact) },
            open: { [weak self] in self?.perform(\.openArtifact) },
            dismiss: { [weak self] in self?.dismiss() })
        next.sharingType = settings.includeInCaptures ? .readOnly : .none
        panel = next
    }

    private func perform(_ action: KeyPath<MiniPreviewController, ArtifactAction>) {
        guard let artifact, panel?.previewView.artifactID == artifact.id else { return }
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
        if policy.visible(count: artifact == nil ? 0 : 1, enabled: settings.enabled,
                          includeInCaptures: settings.includeInCaptures) {
            panel.orderFrontRegardless()
        } else {
            panel.orderOut(nil)
        }
    }

    private func position(panel: NSPanel, on screen: NSScreen) {
        guard let monitor = Self.monitor(for: screen),
              let geometry = NativePreviewLayout.geometry(monitor: monitor, count: 1,
                  placement: settings.placement) else { return }
        panel.setFrame(Self.appKitFrame(geometry: geometry, monitor: monitor,
                                       screenFrame: screen.frame), display: panel.isVisible)
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

/// Copy/save jobs outlive the workspace scene so the nonactivating preview
/// remains useful while Preferences owns the root window.
final class MiniPreviewActions {
    private let transport: AppTransport
    private let loadPreferences: () throws -> CapturePreferences
    private let pasteboard: () -> NSPasteboard
    private weak var previews: MiniPreviewController?
    private var historyRoot: String?

    init(settingsPath: String?, transport: AppTransport = AppBridge(),
         loadPreferences: (() throws -> CapturePreferences)? = nil,
         pasteboard: @escaping () -> NSPasteboard = { .general }) {
        self.transport = transport
        self.loadPreferences = loadPreferences ?? { try CapturePreferences.load(path: settingsPath) }
        self.pasteboard = pasteboard
    }

    func bind(previews: MiniPreviewController) { self.previews = previews }
    func configure(historyRoot: String) { self.historyRoot = historyRoot }

    func copy(_ artifact: CaptureArtifact) {
        previews?.setStatus("Copying…", for: artifact.id)
        LiveCaptureController.queue.async { [weak self] in
            let result = Result { try Data(contentsOf: URL(fileURLWithPath: artifact.imagePath)) }
            DispatchQueue.main.async {
                guard let self else { return }
                switch result {
                case .success(let png):
                    let pasteboard = self.pasteboard(); pasteboard.clearContents()
                    let copied = pasteboard.setData(png, forType: .png)
                    self.previews?.setStatus(copied ? "Copied" : "Copy failed",
                                             for: artifact.id)
                case .failure:
                    self.previews?.setStatus("Copy failed", for: artifact.id)
                }
            }
        }
    }

    func save(_ artifact: CaptureArtifact) {
        guard let historyRoot else {
            previews?.setStatus("Save unavailable", for: artifact.id); return
        }
        previews?.setStatus("Saving…", for: artifact.id)
        LiveCaptureController.queue.async { [weak self] in
            guard let self else { return }
            let result = Result { () throws -> Void in
                let preferences = try self.loadPreferences()
                let response = try self.transport.request(["operation": "save_screenshot",
                    "root": historyRoot, "id": artifact.id,
                    "directory": preferences.directory, "format": preferences.format])
                guard response["path"] as? String != nil else { throw AppBridgeError.invalidResponse }
            }
            DispatchQueue.main.async { [weak self] in
                let status: String
                switch result {
                case .success: status = "Saved"
                case .failure: status = "Save failed"
                }
                self?.previews?.setStatus(status, for: artifact.id)
            }
        }
    }
}
