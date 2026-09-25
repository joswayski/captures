import AppKit
import ImageIO
import QuartzCore
import CCapturesSettings

struct MiniPreviewSettings: Equatable {
    let enabled: Bool
    let placement: String
    let includeInCaptures: Bool
}

final class MiniPreviewCardView: NSView {
    private let tokens: Tokens
    private let imageView = NSImageView()
    private let depthShade = NSView()
    private let title = NSTextField(labelWithString: "Screenshot")
    private let status = NSTextField(labelWithString: "")
    private var actionButtons: [CaptureButton] = []
    private var saveButton: CaptureButton?
    private var compact = false
    private(set) var artifactID: String
    var hasVisibleLabels: Bool { !title.isHidden || !status.isHidden }
    override var isFlipped: Bool { true }

    init(frame: NSRect, artifactID: String, image: NSImage, tokens: Tokens,
         saved: Bool, copy: @escaping () -> Void, save: @escaping () -> Void,
         open: @escaping () -> Void, trash: @escaping () -> Void,
         dismiss: @escaping () -> Void) {
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

        depthShade.frame = bounds; depthShade.wantsLayer = true
        depthShade.layer?.cornerRadius = tokens.number("r-lg")
        depthShade.isHidden = true; depthShade.setAccessibilityElement(false)
        addSubview(depthShade)

        let inset = tokens.number("s-2")
        title.frame = NSRect(x: inset, y: inset, width: 150, height: 19)
        title.font = .systemFont(ofSize: tokens.number("text-sm"), weight: .medium)
        title.textColor = tokens.color("glass-text"); styleLabelBacking(title); addSubview(title)
        status.frame = NSRect(x: bounds.width - 126, y: inset, width: 118, height: 19)
        status.alignment = .right; status.lineBreakMode = .byTruncatingTail
        status.font = .systemFont(ofSize: tokens.number("text-sm"))
        status.textColor = tokens.color("glass-text-muted"); styleLabelBacking(status)
        status.isHidden = true; addSubview(status)

        let actions: [(String, () -> Void)] = [("Copy", copy), (saved ? "Reveal" : "Save", save),
            ("Edit", open), ("Trash", trash), ("×", dismiss)]
        let buttonHeight = tokens.number("h-md"), gap = tokens.number("s-2")
        let actionWidth = (bounds.width - inset * 2 - gap * 4) / 5
        for (index, action) in actions.enumerated() {
            let button = CaptureButton(action.0,
                frame: NSRect(x: inset + CGFloat(index) * (actionWidth + gap),
                    y: bounds.height - buttonHeight - inset, width: actionWidth, height: buttonHeight),
                tokens: tokens, glass: true, action: action.1)
            addSubview(button)
            actionButtons.append(button)
            if index == 1 { saveButton = button; updateSaveButton(saved: saved) }
            if index == 2 {
                button.setAccessibilityLabel("Edit screenshot")
                button.toolTip = "Edit screenshot"
            }
            if index == 3 {
                button.signal = true
                button.toolTip = "Move saved export to Trash and dismiss preview; keep private History"
            }
            if index == 4 {
                button.setAccessibilityLabel("Dismiss preview")
                button.toolTip = "Dismiss preview"
            }
        }
        setAccessibilityRole(.group); setAccessibilityLabel("Screenshot mini preview")
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func setStatus(_ value: String, detail: String? = nil) {
        status.stringValue = value
        status.isHidden = compact || value.isEmpty
        status.setAccessibilityLabel(value.isEmpty ? nil : value)
        status.toolTip = detail
    }

    func updateSaveButton(saved: Bool) {
        saveButton?.title = saved ? "Reveal" : "Save"
        saveButton?.setAccessibilityLabel(saved ? "Show in Folder" : "Save")
        saveButton?.toolTip = saved ? "Show in Folder" : "Save"
    }

    var statusText: String { status.stringValue }

    func setCompact(_ compact: Bool, depth: Int) {
        self.compact = compact
        depthShade.isHidden = !compact || depth == 0
        depthShade.layer?.backgroundColor = tokens.color("glass-strong-solid")
            .withAlphaComponent(CGFloat(captures_preview_dim_opacity_v1(depth))).cgColor
        title.isHidden = compact
        status.isHidden = compact || status.stringValue.isEmpty
        actionButtons.forEach { $0.isHidden = compact }
    }

    private func styleLabelBacking(_ label: NSTextField) {
        label.wantsLayer = true
        label.layer?.backgroundColor = tokens.color("glass-strong").cgColor
        label.layer?.cornerRadius = tokens.number("r-xs")
        label.layer?.masksToBounds = true
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
        setAccessibilityLabel(count == 1 ? "Expand preview" : "Expand \(count) previews")
        toolTip = "Click to expand; drag to move the preview pile"
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
         topAnchor: Bool, tokens: Tokens, copy: @escaping (String) -> Void,
         save: @escaping (String) -> Void, open: @escaping (String) -> Void,
         trash: @escaping (String) -> Void, dismiss: @escaping (String) -> Void,
         setCollapsed: @escaping (Bool) -> Void,
         clearAll: @escaping () -> Void, move: @escaping (NSPoint) -> Void = { _ in }) {
        self.geometry = geometry; self.tokens = tokens; self.restLayouts = layouts
        self.hoverLayouts = hoverLayouts
        artifactIDs = ids
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
                saved: resource.artifact.savedPath != nil,
                copy: { copy(id) }, save: { save(id) }, open: { open(id) },
                trash: { trash(id) }, dismiss: { dismiss(id) })
            card.isHidden = false
            card.setCompact(collapsed, depth: layout.depth)
            card.setAccessibilityElement(layout.interactive)
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

        let controlY = topAnchor ? 16 : bounds.height - 44
        if !collapsed && ids.count >= 2 {
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

    func setStatus(_ value: String, detail: String? = nil, for artifactID: String) {
        cards[artifactID]?.setStatus(value, detail: detail)
    }

    func updateSavedState(_ saved: Bool, for artifactID: String) {
        cards[artifactID]?.updateSaveButton(saved: saved)
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
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    init(frame: NSRect, geometry: CapturesPreviewGeometry, contentHeight: Double,
         resources: [String: MiniPreviewResource], ids: [String],
         layouts: [String: CapturesPreviewCardLayout],
         hoverLayouts: [String: CapturesPreviewCardLayout] = [:], collapsed: Bool,
         topAnchor: Bool, tokens: Tokens, copy: @escaping (String) -> Void,
         save: @escaping (String) -> Void, open: @escaping (String) -> Void,
         trash: @escaping (String) -> Void, dismiss: @escaping (String) -> Void,
         setCollapsed: @escaping (Bool) -> Void,
         clearAll: @escaping () -> Void, move: @escaping (NSPoint) -> Void = { _ in }) {
        previewView = MiniPreviewView(geometry: geometry, contentHeight: contentHeight,
            resources: resources, ids: ids,
            layouts: layouts, hoverLayouts: hoverLayouts, collapsed: collapsed,
            topAnchor: topAnchor, tokens: tokens,
            copy: copy, save: save, open: open, trash: trash, dismiss: dismiss,
            setCollapsed: setCollapsed, clearAll: clearAll, move: move)
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
    private(set) var stackOrigin: CapturesPreviewOrigin?
    private var pendingDecodes: [String: Int] = [:]
    private var visibilityPendingArtifactID: String?
    private var nextDecodeToken = 0
    var copyArtifact: ArtifactAction = { _ in }
    var saveArtifact: ArtifactAction = { _ in }
    var openArtifact: ArtifactAction = { _ in }
    var trashArtifact: ArtifactAction = { _ in }
    var presentedArtifactID: String? { stack.ids.last }
    var presentedArtifactIDs: [String] { stack.ids }
    var decodedArtifactIDs: [String] { stack.ids.filter { resources[$0] != nil } }
    var isCollapsed: Bool { stack.isCollapsed }
    var isPanelVisible: Bool { panel?.isVisible == true }
    func statusText(for artifactID: String) -> String? {
        panel?.previewView.statusText(for: artifactID)
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
            _ = stack.remove(id); resources[id] = nil
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
            resource.artifact = refreshed
            resources[artifact.id] = resource
            panel?.previewView.updateSavedState(refreshed.savedPath != nil, for: artifact.id)
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
        if stack.ids.isEmpty { panel?.close(); panel = nil; screenID = nil; stackOrigin = nil }
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
        if stack.ids.isEmpty { panel?.close(); panel = nil; screenID = nil; stackOrigin = nil }
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
        stackOrigin = nil
        let ids = stack.ids; _ = stack.removeAll(ids)
        resources.removeAll(); pendingDecodes.removeAll()
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
            collapsed: stack.isCollapsed, topAnchor: topAnchor, tokens: tokens,
            copy: { [weak self] in self?.perform(\.copyArtifact, artifactID: $0) },
            save: { [weak self] in self?.perform(\.saveArtifact, artifactID: $0) },
            open: { [weak self] in self?.perform(\.openArtifact, artifactID: $0) },
            trash: { [weak self] in self?.perform(\.trashArtifact, artifactID: $0) },
            dismiss: { [weak self] in self?.dismiss($0) },
            setCollapsed: { [weak self] in self?.setCollapsed($0) },
            clearAll: { [weak self] in self?.clearAll() },
            move: { [weak self] in self?.moveStack(to: $0) })
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
        guard (!boundToPreviews || previews?.contains(artifact) == true),
              !inFlight.contains(artifact.id) else { return }
        inFlight.insert(artifact.id)
        if let path = artifact.savedPath {
            previews?.setStatus("Finding…", for: artifact.id)
            LiveCaptureController.queue.async { [weak self] in
                guard let self else { return }
                let exists = self.fileExists(path)
                DispatchQueue.main.async { [weak self] in
                    guard let self else { return }
                    self.inFlight.remove(artifact.id)
                    guard !self.boundToPreviews || self.previews?.contains(artifact) == true else { return }
                    if exists {
                        self.revealFiles([URL(fileURLWithPath: path)])
                        self.previews?.setStatus("Shown in Folder", for: artifact.id)
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
        previews?.setStatus("Saving…", for: artifact.id)
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
                    self.previews?.setStatus("Saved", for: artifact.id)
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
