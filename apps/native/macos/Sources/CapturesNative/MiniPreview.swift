import AppKit
import ImageIO
import CCapturesSettings

struct MiniPreviewSettings: Equatable {
    let enabled: Bool
    let placement: String
    let includeInCaptures: Bool
}

final class MiniPreviewView: NSView {
    private let tokens: Tokens
    private let imageView = NSImageView()
    private let title = NSTextField(labelWithString: "Latest screenshot")
    private let status = NSTextField(labelWithString: "")
    private(set) var artifactID: String
    var copyAction: () -> Void
    var saveAction: () -> Void
    var openAction: () -> Void
    var dismissAction: () -> Void
    override var isFlipped: Bool { true }

    init(frame: NSRect, artifactID: String, image: NSImage, tokens: Tokens,
         copy: @escaping () -> Void, save: @escaping () -> Void,
         open: @escaping () -> Void, dismiss: @escaping () -> Void) {
        self.artifactID = artifactID; self.tokens = tokens
        copyAction = copy; saveAction = save; openAction = open; dismissAction = dismiss
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = tokens.color("glass-strong").cgColor
        layer?.cornerRadius = tokens.number("r-xl")
        layer?.borderWidth = 1; layer?.borderColor = tokens.color("glass-border").cgColor

        let padding: CGFloat = 28, cardHeight: CGFloat = 160
        imageView.frame = NSRect(x: padding, y: padding, width: frame.width - padding * 2,
                                 height: cardHeight)
        imageView.image = image; imageView.imageScaling = .scaleProportionallyUpOrDown
        imageView.wantsLayer = true; imageView.layer?.cornerRadius = tokens.number("r-lg")
        imageView.layer?.masksToBounds = true
        imageView.setAccessibilityLabel("Latest screenshot thumbnail")
        addSubview(imageView)

        title.frame = NSRect(x: padding, y: 5, width: 170, height: 19)
        title.font = .systemFont(ofSize: tokens.number("text-sm"), weight: .medium)
        title.textColor = tokens.color("glass-text"); addSubview(title)
        status.frame = NSRect(x: 198, y: 5, width: 114, height: 19)
        status.alignment = .right; status.lineBreakMode = .byTruncatingTail
        status.font = .systemFont(ofSize: tokens.number("text-sm"))
        status.textColor = tokens.color("glass-text-muted"); addSubview(status)

        let actions: [(String, () -> Void)] = [("Copy", copy), ("Save", save),
            ("Open", open), ("Dismiss", dismiss)]
        for (index, action) in actions.enumerated() {
            let button = CaptureButton(action.0,
                frame: NSRect(x: padding + CGFloat(index) * 72, y: 194, width: 64, height: 34),
                tokens: tokens, glass: true, action: action.1)
            addSubview(button)
        }
        setAccessibilityRole(.group); setAccessibilityLabel("Latest screenshot mini preview")
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func setStatus(_ value: String) {
        status.stringValue = value
        status.setAccessibilityLabel(value.isEmpty ? nil : value)
    }
}

final class MiniPreviewPanel: NSPanel {
    let previewView: MiniPreviewView
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    init(frame: NSRect, artifactID: String, image: NSImage, tokens: Tokens,
         copy: @escaping () -> Void, save: @escaping () -> Void,
         open: @escaping () -> Void, dismiss: @escaping () -> Void) {
        previewView = MiniPreviewView(frame: NSRect(origin: .zero, size: frame.size),
            artifactID: artifactID, image: image, tokens: tokens,
            copy: copy, save: save, open: open, dismiss: dismiss)
        super.init(contentRect: frame, styleMask: [.borderless, .nonactivatingPanel],
                   backing: .buffered, defer: false)
        isReleasedWhenClosed = false; isOpaque = false; backgroundColor = .clear
        hasShadow = true; level = .floating
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        hidesOnDeactivate = false; isMovable = false; contentView = previewView
        setAccessibilityLabel("Latest screenshot mini preview")
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
    private var decodeGeneration = 0
    var copyArtifact: ArtifactAction = { _ in }
    var saveArtifact: ArtifactAction = { _ in }
    var openArtifact: ArtifactAction = { _ in }

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
                    self.artifact = artifact
                    self.makePanel(image: image)
                    self.updateVisibility()
                case .failure:
                    _ = self.policy.stopWaiting()
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
        artifact = nil; screenID = nil
        panel?.close(); panel = nil
    }

    func close() { dismiss() }

    private func makePanel(image: NSImage) {
        panel?.close(); panel = nil
        guard let artifact, let screen = targetScreen() else { return }
        let size = NSSize(width: 340, height: 240)
        let next = MiniPreviewPanel(frame: NSRect(origin: .zero, size: size),
            artifactID: artifact.id, image: image, tokens: tokens,
            copy: { [weak self] in self?.perform(\.copyArtifact) },
            save: { [weak self] in self?.perform(\.saveArtifact) },
            open: { [weak self] in self?.perform(\.openArtifact) },
            dismiss: { [weak self] in self?.dismiss() })
        next.sharingType = settings.includeInCaptures ? .readOnly : .none
        panel = next
        position(panel: next, on: screen)
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
        let workX = full.minX + (visible.minX - screen.frame.minX) * scale
        let workY = full.minY + (screen.frame.maxY - visible.maxY) * scale
        return CapturesPreviewMonitor(work_x: Int32(workX.rounded()), work_y: Int32(workY.rounded()),
            work_width: UInt32(max(0, (visible.width * scale).rounded())),
            work_height: UInt32(max(0, (visible.height * scale).rounded())),
            full_x: Int32(full.minX.rounded()), full_y: Int32(full.minY.rounded()),
            full_width: UInt32(max(0, full.width.rounded())),
            full_height: UInt32(max(0, full.height.rounded())), scale_factor: Double(scale))
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

    private static func loadImage(path: String) throws -> NSImage {
        guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
              let image = CGImageSourceCreateImageAtIndex(source, 0,
                  [kCGImageSourceShouldCacheImmediately: true] as CFDictionary)
        else { throw AppBridgeError.invalidResponse }
        return NSImage(cgImage: image, size: NSSize(width: image.width, height: image.height))
    }
}
