import AppKit
import AVFoundation
import Combine
import ImageIO
import SwiftUI

private enum PreviewGeometry {
    static let windowWidth: CGFloat = 340
    static let cardWidth: CGFloat = 284
    static let cardHeight: CGFloat = 160
    static let gap: CGFloat = 24
    static let slot: CGFloat = cardHeight + gap
    static let padding: CGFloat = 28
    static let controlGutter: CGFloat = 52
    static let collapsedHeight: CGFloat = cardHeight + controlGutter * 2
    static let cornerRadius: CGFloat = 12
    static let expandDuration = 0.52
    static let fanDuration = 0.20
    static let fanStagger = 0.016
    static let dismissTravel: CGFloat = 118
    static let dismissMotion = 0.45
    static let survivorSettle = 0.58
    static let deleteMotionDelay = 1.8
    static let deleteDuration = 2.9
    static let dragThreshold: CGFloat = 8

    static func pose(_ depth: Int) -> CGFloat {
        let n = CGFloat(max(0, depth))
        guard n > 0 else { return 0 }
        return n * (24 + 0.55 * n) / (n + 24)
    }

    static func jitter(_ depth: Int) -> CGFloat {
        guard depth > 0 else { return 0 }
        var hashed = UInt32(truncatingIfNeeded: depth) &* 0x9e37_79b1 ^ 0x7f4a_7c15
        hashed = hashed &* 0x85eb_ca6b
        let unit = CGFloat(Double(hashed) / pow(2, 32) * 2 - 1)
        return unit * 0.4 * CGFloat(pow(0.58, Double(depth - 1)))
    }

    static func rotation(id: UUID, depth: Int) -> Double {
        guard depth > 0 else { return 0 }
        var hash: UInt32 = 0x811c9dc5
        for byte in id.uuidString.utf8 {
            hash ^= UInt32(byte)
            hash = hash &* 0x01000193
        }
        let sign = hash & 1 == 1 ? 1.0 : -1.0
        let magnitude = 2.7 + Double(hash >> 1) / pow(2, 31) * 0.3
        return sign * magnitude
    }
}

private final class PreviewPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

final class PreviewController {
    static let shared = PreviewController()

    private var panel: PreviewPanel?
    private var model: PreviewStackModel?

    private init() {
        let center = NSWorkspace.shared.notificationCenter
        center.addObserver(forName: NSWorkspace.sessionDidResignActiveNotification, object: nil, queue: .main) { _ in
            PreviewController.shared.hideForPrivacy()
        }
        DistributedNotificationCenter.default().addObserver(
            forName: Notification.Name("com.apple.screenIsLocked"),
            object: nil,
            queue: .main
        ) { _ in PreviewController.shared.hideForPrivacy() }
    }

    func refresh() {
        DispatchQueue.main.async {
            let artifacts = AppStore.shared.previews
            guard AppStore.shared.settings.showPreviews, !artifacts.isEmpty else {
                self.panel?.orderOut(nil)
                self.model?.stopSafetyMonitoring()
                self.model?.update(artifacts: [])
                return
            }

            let model = self.model ?? PreviewStackModel()
            self.model = model
            model.update(artifacts: artifacts)
            let panel = self.panel ?? self.makePanel(model)
            self.panel = panel
            self.resizeAndPlace(panel, model: model, initial: !panel.isVisible)
            panel.orderFrontRegardless()
            model.startSafetyMonitoring()
        }
    }

    fileprivate func hideForPrivacy() {
        model?.stopSafetyMonitoring()
        panel?.orderOut(nil)
    }

    fileprivate func layoutChanged() {
        guard let panel, let model else { return }
        resizeAndPlace(panel, model: model, initial: false)
    }

    fileprivate func beginPileDrag() -> NSPoint? {
        panel?.frame.origin
    }

    fileprivate func movePile(from origin: NSPoint, by delta: CGSize) {
        guard let panel, let screen = panel.screen ?? NSScreen.main else { return }
        let visible = screen.visibleFrame
        let x = min(max(visible.minX, origin.x + delta.width), visible.maxX - panel.frame.width)
        let y = min(max(visible.minY, origin.y - delta.height), visible.maxY - panel.frame.height)
        panel.setFrameOrigin(NSPoint(x: x, y: y))
        model?.updateGravity(panelFrame: panel.frame, workArea: visible)
    }

    fileprivate func finishPileDrag() {
        guard let panel, let model, let visible = panel.screen?.visibleFrame else { return }
        model.updateGravity(panelFrame: panel.frame, workArea: visible)
    }

    fileprivate func panelContains(screenPoint: NSPoint) -> Bool {
        panel?.frame.contains(screenPoint) ?? false
    }

    private func makePanel(_ model: PreviewStackModel) -> PreviewPanel {
        let panel = PreviewPanel(
            contentRect: NSRect(x: 0, y: 0, width: PreviewGeometry.windowWidth, height: model.contentHeight),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        panel.level = .floating
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.hidesOnDeactivate = false
        panel.acceptsMouseMovedEvents = true
        panel.contentView = NSHostingView(rootView: PreviewStackView(model: model))
        return panel
    }

    private func resizeAndPlace(_ panel: PreviewPanel, model: PreviewStackModel, initial: Bool) {
        let screen = panel.screen ?? NSScreen.main ?? NSScreen.screens.first
        guard let visible = screen?.visibleFrame else { return }
        let oldFrame = panel.frame
        let height = min(visible.height, model.contentHeight)
        var origin = oldFrame.origin
        if initial {
            let placement = AppStore.shared.settings.previewPlacement.replacingOccurrences(of: "-", with: "_")
            model.topAnchored = placement.hasPrefix("top")
            model.rightAnchored = placement.hasSuffix("right")
            origin.x = model.rightAnchored ? visible.maxX - PreviewGeometry.windowWidth : visible.minX
            origin.y = model.topAnchored ? visible.maxY - height : visible.minY
        } else {
            if model.topAnchored { origin.y = oldFrame.maxY - height }
            origin.x = min(max(visible.minX, origin.x), visible.maxX - PreviewGeometry.windowWidth)
            origin.y = min(max(visible.minY, origin.y), visible.maxY - height)
        }
        panel.setFrame(NSRect(x: origin.x, y: origin.y, width: PreviewGeometry.windowWidth, height: height), display: true, animate: false)
        model.viewportHeight = height
        model.updateGravity(panelFrame: panel.frame, workArea: visible)
    }
}

private enum PreviewExit: Equatable {
    case dismiss
    case delete
}

private struct DustParticle: Identifiable {
    let id: Int
    let image: NSImage
    let origin: CGPoint
    let size: CGSize
    let delta: CGSize
    let rotation: Double
    let delay: Double
    let duration: Double
}

private struct PreviewCardState: Identifiable {
    let artifact: Artifact
    var image: NSImage?
    var exit: PreviewExit?
    var dust: [DustParticle]
    var exitingAt: Date?

    var id: UUID { artifact.id }
}

private final class PreviewStackModel: ObservableObject {
    @Published var cards: [PreviewCardState] = []
    @Published var expanded = false
    @Published var pileHovered = false
    @Published var topAnchored = false
    @Published var rightAnchored = false
    @Published var gravity: CGFloat = 1
    @Published var centerProximity: CGFloat = 0
    @Published var viewportHeight: CGFloat = PreviewGeometry.collapsedHeight
    @Published var scrollOffset = 0
    @Published var draggingPile = false
    @Published var sway = CGSize.zero
    @Published var settleRevision = 0

    private var loadGeneration = UUID()
    private var pileOrigin: NSPoint?
    private var pileStart = CGPoint.zero
    private var lastDrag = CGSize.zero
    private var lastDragTime = Date()
    private var swayVelocity = CGSize.zero
    private var swayDrive = CGSize.zero
    private var swayTimer: Timer?
    private var safetyTimer: Timer?
    private var safetyCheckInFlight = false
    private var safetyGeneration = UUID()

    var reduceMotion: Bool { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }

    var contentHeight: CGFloat {
        if !expanded { return PreviewGeometry.collapsedHeight }
        guard !cards.isEmpty else { return PreviewGeometry.collapsedHeight }
        return PreviewGeometry.padding + PreviewGeometry.controlGutter
            + CGFloat(cards.count) * PreviewGeometry.cardHeight
            + CGFloat(max(0, cards.count - 1)) * PreviewGeometry.gap
    }

    var visibleCards: [PreviewCardState] {
        let maxCards = max(1, Int((viewportHeight - PreviewGeometry.padding - PreviewGeometry.controlGutter + PreviewGeometry.gap) / PreviewGeometry.slot))
        let upper = min(cards.count, scrollOffset + maxCards)
        guard scrollOffset < upper else { return [] }
        return Array(cards[scrollOffset..<upper])
    }

    var canScrollOlder: Bool { scrollOffset > 0 }
    var canScrollNewer: Bool {
        guard expanded else { return false }
        return scrollOffset + visibleCards.count < cards.count
    }

    func update(artifacts: [Artifact]) {
        let existing = Dictionary(uniqueKeysWithValues: cards.map { ($0.id, $0) })
        cards = artifacts.map { artifact in
            if var state = existing[artifact.id] {
                state = PreviewCardState(artifact: artifact, image: state.image, exit: state.exit, dust: state.dust, exitingAt: state.exitingAt)
                return state
            }
            return PreviewCardState(artifact: artifact, image: nil, exit: nil, dust: [], exitingAt: nil)
        }
        scrollOffset = min(scrollOffset, max(0, cards.count - 1))
        loadMissingImages()
    }

    func setExpanded(_ value: Bool) {
        guard value != expanded else { return }
        withAnimation(reduceMotion ? nil : .timingCurve(0.16, 1, 0.3, 1, duration: PreviewGeometry.expandDuration)) {
            expanded = value
            pileHovered = false
            scrollOffset = 0
        }
        DispatchQueue.main.async { PreviewController.shared.layoutChanged() }
    }

    func scroll(_ slots: Int) {
        let maxOffset = max(0, cards.count - max(1, visibleCards.count))
        withAnimation(reduceMotion ? nil : .easeOut(duration: 0.38)) {
            scrollOffset = min(maxOffset, max(0, scrollOffset + slots))
        }
    }

    func open(_ artifact: Artifact) {
        AppStore.shared.open(artifact)
    }

    func copy(_ artifact: Artifact) {
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        if artifact.kind == "image", let image = NSImage(contentsOf: artifact.url) { pasteboard.writeObjects([image]) }
        else { pasteboard.writeObjects([artifact.url as NSURL]) }
    }

    func reveal(_ artifact: Artifact) {
        NSWorkspace.shared.activateFileViewerSelecting([artifact.url])
    }

    func dismiss(_ artifact: Artifact) {
        beginExit(artifact, kind: .dismiss)
    }

    func delete(_ artifact: Artifact) {
        let alert = NSAlert()
        alert.messageText = "Delete capture?"
        alert.informativeText = "This file will be deleted permanently."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Delete")
        alert.addButton(withTitle: "Cancel")
        alert.buttons.first?.hasDestructiveAction = true
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        beginExit(artifact, kind: .delete)
    }

    func clear() {
        let live = cards.filter { $0.exit == nil }.map(\.artifact)
        for (index, artifact) in live.enumerated() {
            DispatchQueue.main.asyncAfter(deadline: .now() + Double(index) * (reduceMotion ? 0 : 0.07)) {
                self.beginExit(artifact, kind: .dismiss)
            }
        }
    }

    func beginPileDrag(at location: CGPoint) {
        guard !expanded else { return }
        pileOrigin = PreviewController.shared.beginPileDrag()
        pileStart = location
        lastDrag = .zero
        lastDragTime = Date()
        draggingPile = false
        sway = .zero
        swayVelocity = .zero
        swayDrive = .zero
    }

    func updatePileDrag(_ translation: CGSize) {
        guard let origin = pileOrigin else { return }
        if !draggingPile && hypot(translation.width, translation.height) < PreviewGeometry.dragThreshold { return }
        draggingPile = true
        PreviewController.shared.movePile(from: origin, by: translation)
        let now = Date()
        let dt = min(0.048, max(0.001, now.timeIntervalSince(lastDragTime)))
        tickSway(step: CGSize(width: translation.width - lastDrag.width, height: translation.height - lastDrag.height), dt: dt)
        lastDrag = translation
        lastDragTime = now
        startSwayTimer()
    }

    func endPileDrag() {
        let dragged = draggingPile
        pileOrigin = nil
        draggingPile = false
        PreviewController.shared.finishPileDrag()
        if !dragged { setExpanded(true) }
        else { settleSway() }
    }

    func updateGravity(panelFrame: NSRect, workArea: NSRect) {
        let travel = max(1, workArea.height - panelFrame.height)
        let normalizedFromBottom = min(1, max(0, (panelFrame.minY - workArea.minY) / travel))
        gravity = min(1, max(-1, 1 - 2 * normalizedFromBottom))
        centerProximity = 1 - abs(gravity)
        if !topAnchored && gravity <= -0.2 { topAnchored = true }
        if topAnchored && gravity >= 0.2 { topAnchored = false }
        let horizontalTravel = max(1, workArea.width - panelFrame.width)
        let bias = 2 * min(1, max(0, (panelFrame.minX - workArea.minX) / horizontalTravel)) - 1
        if !rightAnchored && bias >= 0.2 { rightAnchored = true }
        if rightAnchored && bias <= -0.2 { rightAnchored = false }
    }

    func startSafetyMonitoring() {
        guard safetyTimer == nil else { return }
        let generation = UUID()
        safetyGeneration = generation
        safetyTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            guard let self, !self.safetyCheckInFlight else { return }
            self.safetyCheckInFlight = true
            Backend.shared.call("session_status") { result in
                guard self.safetyGeneration == generation else { return }
                self.safetyCheckInFlight = false
                if (try? result.get()["available"] as? Bool) != true { PreviewController.shared.hideForPrivacy() }
            }
        }
    }

    func stopSafetyMonitoring() {
        safetyGeneration = UUID()
        safetyTimer?.invalidate()
        safetyTimer = nil
        safetyCheckInFlight = false
    }

    func survivorOffset(for id: UUID) -> CGFloat {
        guard expanded, let index = cards.firstIndex(where: { $0.id == id }), cards[index].exit == nil else { return 0 }
        let now = Date()
        let ready: (PreviewCardState) -> Bool = { state in
            guard let exit = state.exit, let started = state.exitingAt else { return false }
            let delay = exit == .delete ? PreviewGeometry.deleteMotionDelay : PreviewGeometry.dismissMotion
            return now.timeIntervalSince(started) >= delay
        }
        if topAnchored {
            return -CGFloat(cards[..<index].filter(ready).count) * PreviewGeometry.slot
        }
        guard index + 1 < cards.count else { return 0 }
        return CGFloat(cards[(index + 1)...].filter(ready).count) * PreviewGeometry.slot
    }

    private func beginExit(_ artifact: Artifact, kind: PreviewExit) {
        guard let index = cards.firstIndex(where: { $0.id == artifact.id }), cards[index].exit == nil else { return }
        cards[index].exit = kind
        cards[index].exitingAt = Date()
        if kind == .delete, !reduceMotion, let image = cards[index].image {
            cards[index].dust = Self.makeDust(image: image, fromRight: rightAnchored)
        }

        let shiftDelay = kind == .delete ? PreviewGeometry.deleteMotionDelay : PreviewGeometry.dismissMotion
        DispatchQueue.main.asyncAfter(deadline: .now() + (reduceMotion ? 0 : shiftDelay)) {
            withAnimation(
                self.reduceMotion
                    ? nil
                    : .timingCurve(0.16, 1, 0.3, 1, duration: PreviewGeometry.survivorSettle)
            ) {
                self.settleRevision += 1
            }
        }
        let total = reduceMotion ? 0.12 : (kind == .delete ? PreviewGeometry.deleteDuration : PreviewGeometry.dismissMotion + PreviewGeometry.survivorSettle)
        DispatchQueue.main.asyncAfter(deadline: .now() + total) {
            guard self.cards.first(where: { $0.id == artifact.id })?.exit == kind else { return }
            switch kind {
            case .dismiss: AppStore.shared.dismissPreview(artifact)
            case .delete: AppStore.shared.deleteArtifact(artifact)
            }
            self.cards.removeAll { $0.id == artifact.id }
            PreviewController.shared.refresh()
            PreviewController.shared.layoutChanged()
        }
    }

    private func loadMissingImages() {
        let generation = UUID()
        loadGeneration = generation
        let pending = cards.filter { $0.image == nil }.map(\.artifact)
        guard !pending.isEmpty else { return }
        DispatchQueue.global(qos: .userInitiated).async {
            let loaded = pending.map { ($0.id, Self.previewImage(for: $0)) }
            DispatchQueue.main.async {
                guard self.loadGeneration == generation else { return }
                for (id, image) in loaded where image != nil {
                    if let index = self.cards.firstIndex(where: { $0.id == id }) { self.cards[index].image = image }
                }
            }
        }
    }

    private static func previewImage(for artifact: Artifact) -> NSImage? {
        if artifact.kind != "video" {
            guard let source = CGImageSourceCreateWithURL(artifact.url as CFURL, nil) else { return nil }
            let options: [CFString: Any] = [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceThumbnailMaxPixelSize: 640,
                kCGImageSourceShouldCacheImmediately: true,
            ]
            guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else { return nil }
            return NSImage(cgImage: image, size: NSSize(width: image.width, height: image.height))
        }
        let asset = AVURLAsset(url: artifact.url)
        let generator = AVAssetImageGenerator(asset: asset)
        generator.appliesPreferredTrackTransform = true
        generator.maximumSize = NSSize(width: 568, height: 320)
        guard let image = try? generator.copyCGImage(at: .zero, actualTime: nil) else { return nil }
        return NSImage(cgImage: image, size: NSSize(width: image.width, height: image.height))
    }

    private static func makeDust(image: NSImage, fromRight: Bool) -> [DustParticle] {
        var proposed = NSRect(origin: .zero, size: image.size)
        guard
            let original = image.cgImage(forProposedRect: &proposed, context: nil, hints: nil),
            let source = rasterizedCardImage(original)
        else { return [] }
        let columns = 20
        let rows = 11
        let chipW = CGFloat(source.width) / CGFloat(columns)
        let chipH = CGFloat(source.height) / CGFloat(rows)
        let originX: CGFloat = fromRight ? PreviewGeometry.cardWidth - 22.5 : 22.5
        let originY: CGFloat = 22.5
        let maxDistance = hypot(max(originX, PreviewGeometry.cardWidth - originX), max(originY, PreviewGeometry.cardHeight - originY))
        var random = SeededRandom(seed: 739)
        var particles: [DustParticle] = []
        particles.reserveCapacity(columns * rows)
        for row in 0..<rows {
            for column in 0..<columns {
                let sourceRect = CGRect(
                    x: CGFloat(column) * chipW,
                    y: CGFloat(rows - row - 1) * chipH,
                    width: ceil(chipW),
                    height: ceil(chipH)
                ).intersection(CGRect(x: 0, y: 0, width: source.width, height: source.height))
                guard let chip = source.cropping(to: sourceRect) else { continue }
                let x = CGFloat(column) * PreviewGeometry.cardWidth / CGFloat(columns)
                let y = CGFloat(row) * PreviewGeometry.cardHeight / CGFloat(rows)
                let size = CGSize(width: PreviewGeometry.cardWidth / CGFloat(columns) + 0.55, height: PreviewGeometry.cardHeight / CGFloat(rows) + 0.55)
                let center = CGPoint(x: x + size.width / 2, y: y + size.height / 2)
                let distance = hypot(center.x - originX, center.y - originY)
                let wave = distance / maxDistance
                let angle = atan2(center.y - originY, center.x - originX)
                let delayNorm = min(1.12, max(0, wave + sin(angle * 2.7 + wave * 5.5) * 0.07 * wave + (random.next() - 0.5) * 0.34 * wave * wave))
                let distanceScale = 12 + random.next() * 26
                particles.append(DustParticle(
                    id: row * columns + column,
                    image: NSImage(cgImage: chip, size: size),
                    origin: CGPoint(x: x, y: y),
                    size: size,
                    delta: CGSize(
                        width: (center.x - originX) / maxDistance * distanceScale + (random.next() - 0.5) * 22,
                        height: -36 - random.next() * 58
                    ),
                    rotation: Double((random.next() - 0.5) * 120),
                    delay: Double(delayNorm * 0.72 + random.next() * (0.018 + wave * 0.14)),
                    duration: Double(0.78 + random.next() * 0.32 + wave * 0.08)
                ))
            }
        }
        return particles
    }

    private static func rasterizedCardImage(_ source: CGImage) -> CGImage? {
        let width = Int(PreviewGeometry.cardWidth)
        let height = Int(PreviewGeometry.cardHeight)
        guard let context = CGContext(
            data: nil,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: width * 4,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ) else { return nil }
        let scale = max(CGFloat(width) / CGFloat(source.width), CGFloat(height) / CGFloat(source.height))
        let renderedWidth = CGFloat(source.width) * scale
        let renderedHeight = CGFloat(source.height) * scale
        let destination = CGRect(
            x: (CGFloat(width) - renderedWidth) / 2,
            y: (CGFloat(height) - renderedHeight) / 2,
            width: renderedWidth,
            height: renderedHeight
        )
        context.interpolationQuality = .high
        context.draw(source, in: destination)
        return context.makeImage()
    }

    private func tickSway(step: CGSize, dt: TimeInterval) {
        guard !reduceMotion else { sway = .zero; return }
        func advance(position: CGFloat, velocity: CGFloat, drive: CGFloat, pointerStep: CGFloat, limit: CGFloat) -> (CGFloat, CGFloat, CGFloat) {
            let pointerSpeed = pointerStep / CGFloat(dt)
            let desired = min(limit, max(-limit, -pointerSpeed * 0.0012))
            let driveRate: CGFloat = pointerStep == 0 ? 10 : 55
            let nextDrive = drive + (desired - drive) * (1 - exp(-driveRate * CGFloat(dt)))
            let nextVelocity = velocity * exp(-32 * CGFloat(dt)) + (nextDrive - position) * 500 * CGFloat(dt)
            let nextPosition = min(limit, max(-limit, position + nextVelocity * CGFloat(dt)))
            return (nextPosition, abs(nextPosition) == limit ? 0 : nextVelocity, nextDrive)
        }
        let x = advance(position: sway.width, velocity: swayVelocity.width, drive: swayDrive.width, pointerStep: step.width, limit: 3)
        let y = advance(position: sway.height, velocity: swayVelocity.height, drive: swayDrive.height, pointerStep: step.height, limit: 2)
        sway = CGSize(width: x.0, height: y.0)
        swayVelocity = CGSize(width: x.1, height: y.1)
        swayDrive = CGSize(width: x.2, height: y.2)
    }

    private func startSwayTimer() {
        guard swayTimer == nil, !reduceMotion else { return }
        swayTimer = Timer.scheduledTimer(withTimeInterval: 1 / 60, repeats: true) { [weak self] _ in
            guard let self, self.draggingPile else { return }
            self.tickSway(step: .zero, dt: 1 / 60)
        }
    }

    private func settleSway() {
        swayTimer?.invalidate()
        swayTimer = nil
        withAnimation(reduceMotion ? nil : .timingCurve(0.16, 1, 0.3, 1, duration: 0.32)) { sway = .zero }
    }
}

private struct SeededRandom {
    var seed: UInt32
    mutating func next() -> CGFloat {
        seed = seed &* 1_664_525 &+ 1_013_904_223
        return CGFloat(Double(seed) / Double(UInt32.max))
    }
}

private struct PreviewStackView: View {
    @ObservedObject var model: PreviewStackModel

    var body: some View {
        ZStack {
            if model.expanded { expandedStack } else { collapsedStack }
            overflowControls
        }
        .frame(width: PreviewGeometry.windowWidth, height: model.viewportHeight)
        .background(Color.clear)
        .preferredColorScheme(.dark)
    }

    private var expandedStack: some View {
        VStack(spacing: 0) {
            if model.topAnchored { toolbar; Spacer().frame(height: 8) }
            VStack(spacing: PreviewGeometry.gap) {
                ForEach(model.visibleCards) { state in
                    PreviewCardView(state: state, model: model, depth: 0, compact: false)
                }
            }
            if !model.topAnchored { Spacer(minLength: 8); toolbar }
        }
        .padding(.horizontal, PreviewGeometry.padding)
        .padding(.top, model.topAnchored ? 16 : PreviewGeometry.padding)
        .padding(.bottom, model.topAnchored ? PreviewGeometry.padding : 16)
        .transition(.opacity)
    }

    private var collapsedStack: some View {
        let indices = Array(model.cards.indices.prefix(12).reversed())
        return ZStack {
            ForEach(indices, id: \.self) { index in
                PreviewCardView(state: model.cards[index], model: model, depth: index, compact: true)
            }
        }
        .frame(width: PreviewGeometry.cardWidth, height: PreviewGeometry.cardHeight)
        .contentShape(Rectangle())
        .onHover { inside in
            guard !model.draggingPile else { return }
            withAnimation(model.reduceMotion ? nil : .timingCurve(0.16, 1, 0.3, 1, duration: PreviewGeometry.fanDuration)) {
                model.pileHovered = inside
            }
        }
        .gesture(
            DragGesture(minimumDistance: 0, coordinateSpace: .global)
                .onChanged { value in
                    if value.translation == .zero { model.beginPileDrag(at: value.startLocation) }
                    model.updatePileDrag(value.translation)
                }
                .onEnded { _ in model.endPileDrag() }
        )
        .position(
            x: PreviewGeometry.windowWidth / 2,
            y: model.topAnchored ? PreviewGeometry.controlGutter + PreviewGeometry.cardHeight / 2 : model.viewportHeight - PreviewGeometry.controlGutter - PreviewGeometry.cardHeight / 2
        )
        .transition(.opacity)
    }

    private var toolbar: some View {
        HStack(spacing: 5) {
            if model.cards.filter({ $0.exit == nil }).count >= 2 {
                Button(action: model.clear) { Image(systemName: "xmark") }
                    .help("Clear all previews")
            }
            Button(action: { model.setExpanded(false) }) {
                Label("Show less", systemImage: "square.3.layers.3d")
            }
        }
        .font(.system(size: 11, weight: .semibold))
        .foregroundColor(NativeTheme.glassText)
        .buttonStyle(.plain)
        .padding(.horizontal, 9)
        .frame(height: 28)
        .background(.ultraThinMaterial, in: Capsule())
        .background(NativeTheme.glass.opacity(0.76), in: Capsule())
        .overlay(Capsule().stroke(Color.white.opacity(0.14)))
        .frame(maxWidth: .infinity, alignment: model.rightAnchored ? .trailing : .leading)
    }

    @ViewBuilder private var overflowControls: some View {
        if model.expanded && model.canScrollOlder {
            Button(action: { model.scroll(-1) }) { Image(systemName: "chevron.up") }
                .buttonStyle(PreviewOverflowButtonStyle())
                .position(x: PreviewGeometry.windowWidth / 2, y: 11)
        }
        if model.expanded && model.canScrollNewer {
            Button(action: { model.scroll(1) }) { Image(systemName: "chevron.down") }
                .buttonStyle(PreviewOverflowButtonStyle())
                .position(x: PreviewGeometry.windowWidth / 2, y: model.viewportHeight - 11)
        }
    }
}

private struct PreviewOverflowButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 11, weight: .semibold))
            .foregroundColor(NativeTheme.glassText)
            .frame(width: 46, height: 22)
            .background(.ultraThinMaterial, in: Capsule())
            .background(NativeTheme.glass.opacity(0.8), in: Capsule())
            .scaleEffect(configuration.isPressed ? 0.95 : 1)
    }
}

private struct PreviewCardView: View {
    let state: PreviewCardState
    @ObservedObject var model: PreviewStackModel
    let depth: Int
    let compact: Bool
    @State private var hovered = false

    private var pose: CGFloat { PreviewGeometry.pose(depth) }
    private var fan: CGFloat { model.pileHovered || model.draggingPile ? 1 : 0 }
    private var exitProgress: CGFloat {
        guard let started = state.exitingAt else { return 0 }
        let elapsed = Date().timeIntervalSince(started)
        let duration = state.exit == .dismiss ? PreviewGeometry.dismissMotion : 0.55
        return min(1, max(0, elapsed / duration))
    }

    var body: some View {
        ZStack {
            cardMedia
            if !compact {
                FileDragSource(
                    artifact: state.artifact,
                    image: state.image,
                    onOpen: { model.open(state.artifact) },
                    onExternalDrop: { model.dismiss(state.artifact) }
                )
                .frame(width: PreviewGeometry.cardWidth, height: PreviewGeometry.cardHeight)
            }
            if !compact && hovered && state.exit == nil { hoverChrome }
            if state.exit == .delete, !state.dust.isEmpty { DustView(particles: state.dust, startedAt: state.exitingAt ?? Date()) }
        }
        .frame(width: PreviewGeometry.cardWidth, height: PreviewGeometry.cardHeight)
        .background(
            state.exit == .delete && !state.dust.isEmpty ? Color.clear : Color.black.opacity(0.4),
            in: RoundedRectangle(cornerRadius: PreviewGeometry.cornerRadius)
        )
        .overlay(
            RoundedRectangle(cornerRadius: PreviewGeometry.cornerRadius)
                .stroke(Color.white.opacity(state.exit == .delete ? 0 : 0.08))
        )
        .shadow(color: .black.opacity(state.exit == .delete ? 0 : 0.38), radius: 7, y: 6)
        .opacity(state.exit == .dismiss ? 0 : 1)
        .offset(x: dismissOffset)
        .scaleEffect(state.exit == .delete && state.dust.isEmpty ? 0.8 : compactScale)
        .rotation3DEffect(.degrees(compact ? -Double(pose * (0.8 - 0.1 * fan) * model.gravity) : 0), axis: (x: 1, y: 0, z: 0), perspective: 1 / 900)
        .rotationEffect(.degrees(compactRotation))
        .offset(x: compactOffset.width, y: compactOffset.height + model.survivorOffset(for: state.id))
        .zIndex(Double(80 - depth))
        .onHover { inside in
            guard !compact, state.exit == nil else { return }
            withAnimation(model.reduceMotion ? nil : .easeOut(duration: 0.18)) { hovered = inside }
        }
        .animation(model.reduceMotion ? nil : .timingCurve(0.16, 1, 0.3, 1, duration: PreviewGeometry.expandDuration), value: compact)
        .animation(
            model.reduceMotion
                ? nil
                : .timingCurve(0.16, 1, 0.3, 1, duration: PreviewGeometry.fanDuration)
                    .delay(Double(pose) * PreviewGeometry.fanStagger),
            value: model.pileHovered
        )
        .animation(model.reduceMotion ? nil : .timingCurve(0.4, 0, 0.2, 1, duration: state.exit == .dismiss ? PreviewGeometry.dismissMotion : 0.68), value: state.exit != nil)
    }

    private var cardMedia: some View {
        Group {
            if let image = state.image {
                Image(nsImage: image).resizable().aspectRatio(contentMode: .fill)
            } else {
                ZStack {
                    NativeTheme.glass
                    Image(systemName: state.artifact.kind == "video" ? "film" : state.artifact.kind == "gif" ? "photo.stack" : "photo")
                        .font(.system(size: 28)).foregroundColor(NativeTheme.glassMuted)
                }
            }
        }
        .frame(width: PreviewGeometry.cardWidth, height: PreviewGeometry.cardHeight)
        .clipShape(RoundedRectangle(cornerRadius: PreviewGeometry.cornerRadius))
        .opacity(state.exit == .delete && !state.dust.isEmpty ? 0 : 1)
        .blur(radius: mediaBlur)
        .brightness(hovered ? -0.34 : 0)
        .scaleEffect(hovered ? 1.015 : 1)
        .overlay(alignment: .bottomLeading) {
            if !hovered && !compact {
                Text(metadata)
                    .font(.system(size: 9, weight: .medium, design: .rounded)).monospacedDigit()
                    .foregroundColor(NativeTheme.glassText)
                    .padding(.horizontal, 7).padding(.vertical, 3)
                    .background(Color.black.opacity(0.66), in: RoundedRectangle(cornerRadius: 5))
                    .padding(8)
            }
        }
    }

    private var hoverChrome: some View {
        ZStack {
            HStack {
                Button(action: { model.dismiss(state.artifact) }) { Image(systemName: "xmark") }
                    .help("Close preview")
                Spacer()
                Button(action: { model.delete(state.artifact) }) { Image(systemName: "trash") }
                    .foregroundColor(NativeTheme.signal)
                    .help("Delete capture")
            }
            .buttonStyle(PreviewIconButtonStyle())
            .padding(8)
            .frame(maxHeight: .infinity, alignment: .top)

            VStack(spacing: 8) {
                Button(action: { model.copy(state.artifact) }) { Label("Copy", systemImage: "doc.on.doc") }
                Button(action: { model.reveal(state.artifact) }) { Label("Show in Folder", systemImage: "folder") }
                    .foregroundColor(.black.opacity(0.86))
                    .background(NativeTheme.accent, in: RoundedRectangle(cornerRadius: 7))
            }
            .buttonStyle(PreviewActionButtonStyle())
            .frame(width: 140)
        }
        .transition(.opacity)
    }

    private var metadata: String {
        if state.artifact.width > 0 && state.artifact.height > 0 {
            return "\(state.artifact.width) × \(state.artifact.height)"
        }
        return state.artifact.kind.uppercased()
    }

    private var mediaBlur: CGFloat {
        if compact { return pose * (model.pileHovered ? 0.75 : 1.15) }
        return hovered ? 2 : 0
    }

    private var compactScale: CGFloat {
        guard compact else { return 1 }
        return 1 - pose * (0.025 - 0.005 * fan) + pose * model.centerProximity * (0.025 - 0.005 * fan)
    }

    private var compactOffset: CGSize {
        guard compact else { return .zero }
        let x = pose * (-0.8 + 0.2 * fan) + model.sway.width * pose * 0.7
        let peek = 13 + 3 * fan
        let y = (-pose * peek + PreviewGeometry.jitter(depth) + model.sway.height * pose * 0.2) * model.gravity
        return CGSize(width: x, height: y)
    }

    private var compactRotation: Double {
        guard compact else { return 0 }
        return PreviewGeometry.rotation(id: state.id, depth: depth) * Double(model.centerProximity)
            + Double(model.sway.width * pose * 0.12)
    }

    private var dismissOffset: CGFloat {
        guard state.exit == .dismiss else { return 0 }
        return (model.rightAnchored ? 1 : -1) * PreviewGeometry.dismissTravel
    }
}

private struct PreviewIconButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: .semibold))
            .frame(width: 28, height: 28)
            .foregroundColor(NativeTheme.glassText)
            .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 7))
            .background(NativeTheme.glass.opacity(0.8), in: RoundedRectangle(cornerRadius: 7))
            .overlay(RoundedRectangle(cornerRadius: 7).stroke(Color.white.opacity(0.14)))
            .scaleEffect(configuration.isPressed ? 0.96 : 1)
    }
}

private struct PreviewActionButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 11, weight: .semibold))
            .frame(maxWidth: .infinity)
            .frame(height: 32)
            .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 7))
            .background(NativeTheme.glass.opacity(0.76), in: RoundedRectangle(cornerRadius: 7))
            .overlay(RoundedRectangle(cornerRadius: 7).stroke(Color.white.opacity(0.16)))
            .scaleEffect(configuration.isPressed ? 0.97 : 1)
    }
}

private struct DustView: View {
    let particles: [DustParticle]
    let startedAt: Date

    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 60)) { timeline in
            let elapsed = timeline.date.timeIntervalSince(startedAt)
            ZStack(alignment: .topLeading) {
                ForEach(particles) { particle in
                    let progress = min(1, max(0, (elapsed - particle.delay) / particle.duration))
                    Image(nsImage: particle.image)
                        .resizable()
                        .frame(width: particle.size.width, height: particle.size.height)
                        .position(x: particle.origin.x + particle.size.width / 2, y: particle.origin.y + particle.size.height / 2)
                        .offset(x: particle.delta.width * eased(progress), y: particle.delta.height * eased(progress))
                        .rotationEffect(.degrees(particle.rotation * eased(progress)))
                        .scaleEffect(1 - 0.18 * progress)
                        .opacity(progress <= 0 ? 1 : 1 - progress)
                }
            }
        }
        .frame(width: PreviewGeometry.cardWidth, height: PreviewGeometry.cardHeight, alignment: .topLeading)
        .allowsHitTesting(false)
    }

    private func eased(_ value: Double) -> CGFloat {
        let t = CGFloat(value)
        return 1 - pow(1 - t, 3)
    }
}

private struct FileDragSource: NSViewRepresentable {
    let artifact: Artifact
    let image: NSImage?
    let onOpen: () -> Void
    let onExternalDrop: () -> Void

    func makeNSView(context: Context) -> FileDragNSView {
        let view = FileDragNSView()
        update(view)
        return view
    }

    func updateNSView(_ nsView: FileDragNSView, context: Context) { update(nsView) }

    private func update(_ view: FileDragNSView) {
        view.url = artifact.url
        view.dragImage = image
        view.onOpen = onOpen
        view.onExternalDrop = onExternalDrop
    }
}

private final class FileDragNSView: NSView, NSDraggingSource {
    var url: URL?
    var dragImage: NSImage?
    var onOpen: (() -> Void)?
    var onExternalDrop: (() -> Void)?
    private var mouseDownEvent: NSEvent?

    override func mouseDown(with event: NSEvent) {
        mouseDownEvent = event
        if event.clickCount == 2 { onOpen?() }
    }

    override func mouseDragged(with event: NSEvent) {
        guard let initial = mouseDownEvent, let url else { return }
        let delta = hypot(event.locationInWindow.x - initial.locationInWindow.x, event.locationInWindow.y - initial.locationInWindow.y)
        guard delta >= 4 else { return }
        mouseDownEvent = nil
        let item = NSDraggingItem(pasteboardWriter: url as NSURL)
        let image = dragImage ?? NSWorkspace.shared.icon(forFile: url.path)
        item.setDraggingFrame(bounds, contents: image)
        beginDraggingSession(with: [item], event: event, source: self)
    }

    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        .copy
    }

    func draggingSession(_ session: NSDraggingSession, endedAt screenPoint: NSPoint, operation: NSDragOperation) {
        guard operation.contains(.copy), !PreviewController.shared.panelContains(screenPoint: screenPoint) else { return }
        onExternalDrop?()
    }
}
