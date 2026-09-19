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
    private var actionButtons: [CaptureButton] = []
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
        title.textColor = tokens.color("glass-text"); styleLabelBacking(title); addSubview(title)
        status.frame = NSRect(x: bounds.width - 126, y: inset, width: 118, height: 19)
        status.alignment = .right; status.lineBreakMode = .byTruncatingTail
        status.font = .systemFont(ofSize: tokens.number("text-sm"))
        status.textColor = tokens.color("glass-text-muted"); styleLabelBacking(status)
        status.isHidden = true; addSubview(status)

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
            actionButtons.append(button)
        }
        setAccessibilityRole(.group); setAccessibilityLabel("Screenshot mini preview")
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func setStatus(_ value: String) {
        status.stringValue = value
        status.isHidden = value.isEmpty
        status.setAccessibilityLabel(value.isEmpty ? nil : value)
    }

    func setActionsVisible(_ visible: Bool) {
        actionButtons.forEach { $0.isHidden = !visible }
    }

    private func styleLabelBacking(_ label: NSTextField) {
        label.wantsLayer = true
        label.layer?.backgroundColor = tokens.color("glass-strong").cgColor
        label.layer?.cornerRadius = tokens.number("r-xs")
        label.layer?.masksToBounds = true
    }
}

struct MiniPreviewResource {
    let artifact: CaptureArtifact
    let image: NSImage
}

private final class MiniPreviewDocumentView: NSView {
    override var isFlipped: Bool { true }
}

private final class MiniPreviewExpandButton: NSButton {
    private let actionBlock: () -> Void

    init(frame: NSRect, count: Int, action: @escaping () -> Void) {
        actionBlock = action
        super.init(frame: frame)
        title = ""; isBordered = false; setButtonType(.momentaryPushIn)
        target = self; self.action = #selector(activate)
        setAccessibilityLabel(count == 1 ? "Expand preview" : "Expand \(count) previews")
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func activate() { actionBlock() }
    override func draw(_ dirtyRect: NSRect) {}
}

final class MiniPreviewView: NSView {
    private let geometry: CapturesPreviewGeometry
    private let tokens: Tokens
    private let scroll = NSScrollView()
    private let document = MiniPreviewDocumentView()
    private var cards: [String: MiniPreviewCardView] = [:]
    private var pileExpandButton: MiniPreviewExpandButton?
    private var expandButton: CaptureButton?
    private var collapseButton: CaptureButton?
    private var clearButton: CaptureButton?
    private(set) var artifactIDs: [String] = []
    var renderedArtifactIDs: [String] { artifactIDs.filter { cards[$0] != nil } }
    var cardPaintOrder: [String] {
        document.subviews.compactMap { ($0 as? MiniPreviewCardView)?.artifactID }
    }
    var visibleCardActionTitles: [String] {
        cards.values.flatMap { card in
            card.subviews.compactMap { $0 as? CaptureButton }
                .filter { !$0.isHidden }.map(\.title)
        }
    }
    var pileExpandAccessibilityLabel: String? { pileExpandButton?.accessibilityLabel() }
    var documentHeight: CGFloat { document.frame.height }
    var viewportHeight: CGFloat { scroll.contentView.bounds.height }
    var scrollOffsetY: CGFloat { scroll.contentView.bounds.minY }
    override var isFlipped: Bool { true }

    init(geometry: CapturesPreviewGeometry, contentHeight: Double,
         resources: [String: MiniPreviewResource],
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
        let documentHeight = collapsed ? bounds.height : max(bounds.height, CGFloat(contentHeight))
        document.frame = NSRect(x: 0, y: 0, width: bounds.width,
            height: documentHeight)
        scroll.documentView = document

        for id in ids {
            guard let resource = resources[id], let layout = layouts[id] else { continue }
            let card = MiniPreviewCardView(frame: NSRect(x: padding, y: CGFloat(layout.y),
                width: cardWidth, height: cardHeight), artifactID: id,
                image: resource.image, tokens: tokens,
                copy: { copy(id) }, save: { save(id) }, open: { open(id) },
                dismiss: { dismiss(id) })
            card.isHidden = false
            card.setActionsVisible(!collapsed)
            card.setAccessibilityElement(layout.interactive)
            document.addSubview(card); cards[id] = card
        }

        if collapsed, let front = ids.compactMap({ id in
            layouts[id].map { (id, $0) }
        }).first(where: { $0.1.interactive }), let card = cards[front.0] {
            let expand = MiniPreviewExpandButton(frame: card.frame, count: ids.count) {
                setCollapsed(false)
            }
            document.addSubview(expand); pileExpandButton = expand
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

    func activatePileExpand() { pileExpandButton?.performClick(nil) }
}

final class MiniPreviewPanel: NSPanel {
    let previewView: MiniPreviewView
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    init(frame: NSRect, geometry: CapturesPreviewGeometry, contentHeight: Double,
         resources: [String: MiniPreviewResource], ids: [String],
         layouts: [String: CapturesPreviewCardLayout], collapsed: Bool,
         topAnchor: Bool, tokens: Tokens, copy: @escaping (String) -> Void,
         save: @escaping (String) -> Void, open: @escaping (String) -> Void,
         dismiss: @escaping (String) -> Void, setCollapsed: @escaping (Bool) -> Void,
         clearAll: @escaping () -> Void) {
        previewView = MiniPreviewView(geometry: geometry, contentHeight: contentHeight,
            resources: resources, ids: ids,
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
    private var pendingDecodes: [String: Int] = [:]
    private var visibilityPendingArtifactID: String?
    private var nextDecodeToken = 0
    var copyArtifact: ArtifactAction = { _ in }
    var saveArtifact: ArtifactAction = { _ in }
    var openArtifact: ArtifactAction = { _ in }
    var presentedArtifactID: String? { stack.ids.last }
    var presentedArtifactIDs: [String] { stack.ids }
    var decodedArtifactIDs: [String] { stack.ids.filter { resources[$0] != nil } }
    var isCollapsed: Bool { stack.isCollapsed }
    var isPanelVisible: Bool { panel?.isVisible == true }

    init(tokens: Tokens, policy: NativePreviewPolicy = NativePreviewPolicy(),
         stack: NativePreviewStack = NativePreviewStack(),
         screenProvider: @escaping () -> [NSScreen] = { NSScreen.screens },
         imageLoader: @escaping (String) throws -> NSImage = MiniPreviewController.loadImage) {
        self.tokens = tokens; self.policy = policy; self.stack = stack
        self.screenProvider = screenProvider; self.imageLoader = imageLoader
    }

    func beginCapture(settings: MiniPreviewSettings) -> UInt64? {
        precondition(Thread.isMainThread)
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
        nextDecodeToken &+= 1
        let decodeToken = nextDecodeToken
        pendingDecodes[artifact.id] = decodeToken
        policy.suppressCaptureUI(false)
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self else { return }
            let result = Result { try self.imageLoader(artifact.previewPath) }
            DispatchQueue.main.async { [weak self] in
                guard let self, self.pendingDecodes[artifact.id] == decodeToken,
                      self.stack.ids.contains(artifact.id) else { return }
                self.pendingDecodes[artifact.id] = nil
                switch result {
                case .success(let image):
                    if self.policy.ready(artifact: artifact.id) {
                        self.visibilityPendingArtifactID = nil
                    }
                    self.resources[artifact.id] = MiniPreviewResource(artifact: artifact, image: image)
                    self.makePanel()
                    self.updateVisibility()
                case .failure:
                    if self.visibilityPendingArtifactID == artifact.id,
                       self.policy.stopWaiting() {
                        self.visibilityPendingArtifactID = nil
                    }
                    _ = self.stack.remove(artifact.id)
                    self.resources[artifact.id] = nil
                    self.makePanel()
                    self.updateVisibility()
                }
            }
        }
    }

    func updateSettings(_ settings: MiniPreviewSettings) {
        precondition(Thread.isMainThread)
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
            _ = stack.remove(id); resources[id] = nil
        }
        if changed || !removed.isEmpty { makePanel(); updateVisibility() }
    }

    func setStatus(_ value: String, for artifactID: String) {
        guard resources[artifactID] != nil else { return }
        panel?.previewView.setStatus(value, for: artifactID)
    }

    func dismiss(_ artifactID: String) {
        precondition(Thread.isMainThread)
        guard stack.remove(artifactID) else { return }
        pendingDecodes[artifactID] = nil
        if visibilityPendingArtifactID == artifactID, policy.stopWaiting() {
            visibilityPendingArtifactID = nil
        }
        resources[artifactID] = nil
        if stack.ids.isEmpty { panel?.close(); panel = nil; screenID = nil }
        else { makePanel(); updateVisibility() }
    }

    func clearAll() {
        precondition(Thread.isMainThread)
        let snapshot = stack.ids
        guard !snapshot.isEmpty else { return }
        _ = stack.removeAll(snapshot)
        snapshot.forEach { id in resources[id] = nil; pendingDecodes[id] = nil }
        if let pending = visibilityPendingArtifactID, snapshot.contains(pending),
           policy.stopWaiting() {
            visibilityPendingArtifactID = nil
        }
        if stack.ids.isEmpty { panel?.close(); panel = nil; screenID = nil }
        else { makePanel(); updateVisibility() }
    }

    func setCollapsed(_ collapsed: Bool) {
        precondition(Thread.isMainThread)
        guard stack.isCollapsed != collapsed else { return }
        stack.setCollapsed(collapsed)
        makePanel(); updateVisibility()
    }

    func close() {
        precondition(Thread.isMainThread)
        _ = policy.stopWaiting(); visibilityPendingArtifactID = nil; screenID = nil
        let ids = stack.ids; _ = stack.removeAll(ids)
        resources.removeAll(); pendingDecodes.removeAll()
        panel?.close(); panel = nil
    }

    private func makePanel() {
        panel?.close(); panel = nil
        let ids = stack.ids
        guard !ids.isEmpty, let screen = targetScreen(),
              let monitor = Self.monitor(for: screen),
              let geometry = NativePreviewLayout.geometry(monitor: monitor, count: ids.count,
                  collapsed: stack.isCollapsed,
                  placement: settings.placement) else { return }
        let topAnchor = settings.placement.hasPrefix("top_")
        let layouts = Dictionary(uniqueKeysWithValues: ids.enumerated().compactMap { index, id in
            stack.cardLayout(index: index, topAnchor: topAnchor).map { (id, $0) }
        })
        let frame = Self.appKitFrame(geometry: geometry, monitor: monitor,
                                     screenFrame: screen.frame)
        let next = MiniPreviewPanel(frame: frame, geometry: geometry,
            contentHeight: stack.contentHeight,
            resources: resources, ids: ids, layouts: layouts,
            collapsed: stack.isCollapsed, topAnchor: topAnchor, tokens: tokens,
            copy: { [weak self] in self?.perform(\.copyArtifact, artifactID: $0) },
            save: { [weak self] in self?.perform(\.saveArtifact, artifactID: $0) },
            open: { [weak self] in self?.perform(\.openArtifact, artifactID: $0) },
            dismiss: { [weak self] in self?.dismiss($0) },
            setCollapsed: { [weak self] in self?.setCollapsed($0) },
            clearAll: { [weak self] in self?.clearAll() })
        next.sharingType = settings.includeInCaptures ? .readOnly : .none
        panel = next
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
                  collapsed: stack.isCollapsed,
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
