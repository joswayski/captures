import AppKit
import SwiftUI
import UniformTypeIdentifiers

enum EditorViewportMath {
    static let minimumZoom: CGFloat = 0.05
    static let maximumZoom: CGFloat = 8

    static func clampedZoom(_ value: CGFloat) -> CGFloat {
        min(maximumZoom, max(minimumZoom, value))
    }

    static func wheelZoomFactor(scrollingDeltaY: CGFloat, precise: Bool) -> CGFloat {
        let pixels = min(240, max(-240, scrollingDeltaY * (precise ? 1 : 16)))
        return exp(pixels * 0.002)
    }

    static func sliderPosition(for zoom: CGFloat) -> Double {
        let span = log(maximumZoom / minimumZoom)
        return Double(log(clampedZoom(zoom) / minimumZoom) / span)
    }

    static func zoom(forSliderPosition position: Double) -> CGFloat {
        let bounded = min(1, max(0, position))
        return clampedZoom(minimumZoom * exp(CGFloat(bounded) * log(maximumZoom / minimumZoom)))
    }

    static func documentPoint(anchor: CGPoint, canvasOrigin: CGPoint, zoom: CGFloat) -> CGPoint {
        CGPoint(x: (anchor.x - canvasOrigin.x) / zoom, y: (anchor.y - canvasOrigin.y) / zoom)
    }

    static func anchorCorrection(
        anchor: CGPoint, canvasOrigin: CGPoint, documentPoint: CGPoint, zoom: CGFloat
    ) -> CGSize {
        CGSize(
            width: anchor.x - (canvasOrigin.x + documentPoint.x * zoom),
            height: anchor.y - (canvasOrigin.y + documentPoint.y * zoom)
        )
    }

    static func frame(_ frame: CGRect, matches documentSize: CGSize, zoom: CGFloat) -> Bool {
        abs(frame.width - documentSize.width * zoom) < 0.5
            && abs(frame.height - documentSize.height * zoom) < 0.5
    }

    static func isMostlyOffscreen(viewportSize: CGSize, canvasFrame: CGRect) -> Bool {
        guard viewportSize.width > 0, viewportSize.height > 0,
              canvasFrame.width > 0, canvasFrame.height > 0 else { return false }
        let viewport = CGRect(origin: .zero, size: viewportSize)
        let intersection = viewport.intersection(canvasFrame)
        guard !intersection.isNull, intersection.width > 0, intersection.height > 0 else { return true }
        let overlapArea = intersection.width * intersection.height
        let canvasArea = max(1, canvasFrame.width * canvasFrame.height)
        return overlapArea < min(48 * 48, canvasArea * 0.04)
    }
}

private struct EditorCanvasFramePreference: PreferenceKey {
    static var defaultValue = CGRect.zero
    static func reduce(value: inout CGRect, nextValue: () -> CGRect) { value = nextValue() }
}

private struct EditorCombineFramePreference: PreferenceKey {
    static var defaultValue = CGRect.zero
    static func reduce(value: inout CGRect, nextValue: () -> CGRect) { value = nextValue() }
}

struct EditorZoomAnchor: Equatable {
    let viewportPoint: CGPoint
    let documentPoint: CGPoint
    let zoom: CGFloat
    var correctedFrameOrigin: CGPoint?
}

extension EditorViewportMath {
    static func pendingAnchor(
        replacing pending: EditorZoomAnchor?,
        viewportPoint: CGPoint,
        measuredCanvasOrigin: CGPoint,
        currentZoom: CGFloat,
        nextZoom: CGFloat
    ) -> EditorZoomAnchor {
        let effectiveOrigin: CGPoint
        if let pending, abs(pending.zoom - currentZoom) < 0.0001 {
            effectiveOrigin = CGPoint(
                x: pending.viewportPoint.x - pending.documentPoint.x * currentZoom,
                y: pending.viewportPoint.y - pending.documentPoint.y * currentZoom
            )
        } else {
            effectiveOrigin = measuredCanvasOrigin
        }
        return EditorZoomAnchor(
            viewportPoint: viewportPoint,
            documentPoint: documentPoint(
                anchor: viewportPoint,
                canvasOrigin: effectiveOrigin,
                zoom: currentZoom
            ),
            zoom: nextZoom,
            correctedFrameOrigin: nil
        )
    }

    static func resolvePendingAnchor(
        _ anchor: EditorZoomAnchor, canvasOrigin: CGPoint
    ) -> (correction: CGSize, pending: EditorZoomAnchor?) {
        let correction = anchorCorrection(
            anchor: anchor.viewportPoint,
            canvasOrigin: canvasOrigin,
            documentPoint: anchor.documentPoint,
            zoom: anchor.zoom
        )
        if abs(correction.width) <= 0.01, abs(correction.height) <= 0.01 {
            return (.zero, nil)
        }
        if anchor.correctedFrameOrigin == canvasOrigin {
            return (.zero, anchor)
        }
        var pending = anchor
        pending.correctedFrameOrigin = canvasOrigin
        return (correction, pending)
    }
}

func routeEditorViewportEvent(
    _ event: NSEvent,
    handler: ((NSEvent) -> NSEvent?)?
) -> NSEvent? {
    guard let handler else { return event }
    return handler(event)
}

private enum ImageEditorTool: String, CaseIterable, Identifiable {
    case select = "Select"
    case crop = "Crop"
    case text = "Text"
    case shape = "Shape"
    case arrow = "Arrow"
    case pen = "Freehand"
    case erase = "Erase"
    case restore = "Restore"
    case wand = "Background wand"

    var id: String { rawValue }
    var symbol: String {
        switch self {
        case .select: return "arrow.up.left.and.arrow.down.right"
        case .crop: return "crop"
        case .text: return "textformat"
        case .shape: return "square.on.circle"
        case .arrow: return "arrow.up.right"
        case .pen: return "pencil.tip"
        case .erase: return "eraser"
        case .restore: return "paintbrush"
        case .wand: return "wand.and.stars"
        }
    }
}

private enum ImageResizeCorner: CaseIterable, Equatable {
    case northWest, northEast, southEast, southWest
}

private final class EditorViewportMonitorView: NSView {
    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

private struct EditorViewportEvents: NSViewRepresentable {
    var zoomBy: (CGFloat, CGPoint) -> Void
    var zoomActual: () -> Void
    var panBegan: () -> Void
    var panChanged: (CGSize) -> Void
    var panEnded: () -> Void

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeNSView(context: Context) -> NSView {
        let view = EditorViewportMonitorView()
        context.coordinator.view = view
        context.coordinator.install()
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        context.coordinator.parent = self
    }

    static func dismantleNSView(_ nsView: NSView, coordinator: Coordinator) {
        coordinator.remove()
    }

    final class Coordinator {
        var parent: EditorViewportEvents
        weak var view: NSView?
        private var monitor: Any?
        private var panStart: CGPoint?
        private var magnifyAnchor: CGPoint?

        init(_ parent: EditorViewportEvents) { self.parent = parent }

        func install() {
            monitor = NSEvent.addLocalMonitorForEvents(matching: [
                .scrollWheel, .magnify, .keyDown,
                .leftMouseDown, .leftMouseDragged, .leftMouseUp,
                .otherMouseDown, .otherMouseDragged, .otherMouseUp,
            ]) { [weak self] event in
                guard let self else { return event }
                return routeEditorViewportEvent(event, handler: self.handle)
            }
        }

        func remove() {
            if let monitor { NSEvent.removeMonitor(monitor) }
            monitor = nil
        }

        private func handle(_ event: NSEvent) -> NSEvent? {
            guard let view, event.window === view.window else { return event }
            let location = view.convert(event.locationInWindow, from: nil)
            switch event.type {
            case .scrollWheel:
                guard view.bounds.contains(location),
                      event.modifierFlags.contains(.command) || event.modifierFlags.contains(.control),
                      event.scrollingDeltaY != 0 else { return event }
                if magnifyAnchor != nil { return nil }
                parent.zoomBy(
                    EditorViewportMath.wheelZoomFactor(
                        scrollingDeltaY: event.scrollingDeltaY,
                        precise: event.hasPreciseScrollingDeltas
                    ),
                    location
                )
                return nil
            case .magnify:
                if event.phase.contains(.began), view.bounds.contains(location) {
                    magnifyAnchor = location
                }
                guard let anchor = magnifyAnchor ?? (view.bounds.contains(location) ? location : nil) else {
                    return event
                }
                magnifyAnchor = anchor
                if event.magnification.isFinite, event.magnification != 0 {
                    parent.zoomBy(max(0.01, 1 + event.magnification), anchor)
                }
                if event.phase.contains(.ended) || event.phase.contains(.cancelled) {
                    magnifyAnchor = nil
                }
                return nil
            case .keyDown:
                guard event.modifierFlags.contains(.command) || event.modifierFlags.contains(.control),
                      !(view.window?.firstResponder is NSTextView) else { return event }
                switch event.charactersIgnoringModifiers {
                case "+", "=": parent.zoomBy(1.25, CGPoint(x: view.bounds.midX, y: view.bounds.midY))
                case "-": parent.zoomBy(0.8, CGPoint(x: view.bounds.midX, y: view.bounds.midY))
                case "0": parent.zoomActual()
                default: return event
                }
                return nil
            case .leftMouseDown, .otherMouseDown:
                let modifierPan = event.modifierFlags.contains(.command) || event.modifierFlags.contains(.control)
                guard view.bounds.contains(location), event.buttonNumber == 2 || (event.buttonNumber == 0 && modifierPan) else {
                    return event
                }
                panStart = location
                parent.panBegan()
                return nil
            case .leftMouseDragged, .otherMouseDragged:
                guard let start = panStart else { return event }
                parent.panChanged(CGSize(width: location.x - start.x, height: location.y - start.y))
                return nil
            case .leftMouseUp, .otherMouseUp:
                guard panStart != nil else { return event }
                panStart = nil
                parent.panEnded()
                return nil
            default:
                return event
            }
        }
    }
}

struct ImageEditorView: View {
    let artifact: Artifact
    @State private var loadAttempted = false
    @State private var model: EditorModel?
    @State private var error: Error?

    var body: some View {
        Group {
            if let model {
                ImageEditorSurface(artifact: artifact, model: model)
            } else if let error {
                VStack(spacing: 14) {
                    Image(systemName: "exclamationmark.triangle")
                        .font(.system(size: 28))
                    Text("Couldn’t open this screenshot").font(.headline)
                    Text(error.localizedDescription).foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ProgressView("Loading screenshot…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .onAppear {
            guard !loadAttempted else { return }
            loadAttempted = true
            do { model = try EditorModel(artifact: artifact) } catch { self.error = error }
        }
    }
}

private struct ImageEditorSurface: View {
    let artifact: Artifact
    @ObservedObject var model: EditorModel
    let initialPropertiesAnchor: String?
    let onCombineControlsVisibilityChanged: ((Bool) -> Void)?
    let onViewportOffscreenChanged: ((Bool) -> Void)?
    @ObservedObject private var store = AppStore.shared
    @Environment(\.colorScheme) private var colorScheme
    @State private var tool: ImageEditorTool = .select
    @State private var zoom: CGFloat = 1
    @State private var zoomMode = "fit"
    @State private var viewportSize: CGSize = .zero
    @State private var viewPan = CGSize.zero
    @State private var panGestureOrigin = CGSize.zero
    @State private var canvasViewportFrame = CGRect.zero
    @State private var pendingZoomAnchor: EditorZoomAnchor?
    @State private var canvasOffscreen = false
    @State private var selectedShape: EditorShape = .rectangle
    @State private var shapeFlyoutOpen = false
    @State private var backgroundFlyoutOpen = false
    @State private var widthText = ""
    @State private var heightText = ""
    @State private var strokeWidth: CGFloat = 6
    @State private var defaultOpacity: CGFloat = 1
    @State private var brushSize: CGFloat = 32
    @State private var brushSoftness: CGFloat = 0.35
    @State private var wandTolerance: CGFloat = 0.12
    @State private var wandContiguous = true
    @State private var brushLastPoint: CGPoint?
    @State private var brushLayerID: UUID?
    @State private var fillShapes = false
    @State private var draftPoints: [CGPoint] = []
    @State private var cropStart: CGPoint?
    @State private var cropRect: CGRect?
    @State private var dragOrigin: CGPoint?
    @State private var dragLayerFrame: EditorRect?
    @State private var alignmentGuides: [EditorAlignmentGuide] = []
    @State private var resizeLayerFrame: EditorRect?
    @State private var rotatingLayer = false
    @State private var exportFormat: String
    @State private var quality = "92"
    @State private var exportQualityMode = "preserve"
    @State private var maximumSizeMB = 10.0
    @State private var exporting = false
    @State private var comparisonPending = false
    @State private var comparisonBefore: NSImage?
    @State private var comparisonAfter: NSImage?
    @State private var comparisonSplit: CGFloat = 0.5
    @State private var comparisonGeneration = UUID()
    @State private var comparisonWork: DispatchWorkItem?
    @State private var comparisonExpanded = true
    @State private var exportSettingsOpen = false
    @State private var notice = ""
    @State private var editorHex = EditorColor.signal.hex
    @State private var makeCopy: Bool
    @State private var exportDirectory: URL
    @State private var exportFilename: String

    init(
        artifact: Artifact,
        model: EditorModel,
        initialTool: ImageEditorTool = .select,
        shapeFlyoutOpen: Bool = false,
        initialAlignmentGuides: [EditorAlignmentGuide] = [],
        initialZoom: CGFloat? = nil,
        initialViewPan: CGSize = .zero,
        initialPropertiesAnchor: String? = nil,
        onCombineControlsVisibilityChanged: ((Bool) -> Void)? = nil,
        onViewportOffscreenChanged: ((Bool) -> Void)? = nil
    ) {
        self.artifact = artifact
        self.model = model
        self.initialPropertiesAnchor = initialPropertiesAnchor
        self.onCombineControlsVisibilityChanged = onCombineControlsVisibilityChanged
        self.onViewportOffscreenChanged = onViewportOffscreenChanged
        _tool = State(initialValue: initialTool)
        _shapeFlyoutOpen = State(initialValue: shapeFlyoutOpen)
        _alignmentGuides = State(initialValue: initialAlignmentGuides)
        _zoom = State(initialValue: initialZoom ?? 1)
        _zoomMode = State(initialValue: initialZoom == nil ? "fit" : "custom")
        _viewPan = State(initialValue: initialViewPan)
        let sourceExtension = artifact.url.pathExtension.lowercased()
        let sourceFormat = sourceExtension == "jpg" ? "jpeg" : sourceExtension
        let canReplaceSource = ["png", "jpeg", "webp"].contains(sourceFormat)
        _exportFormat = State(initialValue: canReplaceSource ? sourceFormat : "png")
        _makeCopy = State(initialValue: !canReplaceSource)
        _exportDirectory = State(initialValue: artifact.url.deletingLastPathComponent())
        _exportFilename = State(initialValue: artifact.url.deletingPathExtension().lastPathComponent)
    }

    var body: some View {
        VStack(spacing: 0) {
            header
                .zIndex(20)
            if model.restoredDraft {
                HStack {
                    Text("Unsaved editing draft restored — export, save, or keep editing.")
                    Spacer()
                    Button("Discard draft") { discardDraft() }
                        .buttonStyle(CaptureButtonStyle())
                }
                .font(.callout.weight(.medium))
                .padding(.horizontal, NativeTheme.metric("s-6")).padding(.vertical, NativeTheme.metric("s-3"))
                .background(Color.orange.opacity(0.12))
            }
            HStack(spacing: 0) {
                toolRail
                    .zIndex(20)
                canvasViewport
                inspector
            }
            exportFooter
                .zIndex(20)
        }
        .background(NativeTheme.canvas(colorScheme))
        .foregroundStyle(NativeTheme.text(colorScheme))
        .disabled(exporting)
        .onAppear {
            syncDimensions()
            syncEditorColor()
            if [.wand, .erase, .restore].contains(tool) { selectBackgroundImageIfNeeded() }
        }
        .onChange(of: model.document.width) { _ in syncDimensions() }
        .onChange(of: model.document.height) { _ in syncDimensions() }
        .onChange(of: model.document) { _ in scheduleComparison() }
        .onChange(of: model.selectedLayerID) { _ in syncEditorColor() }
        .onChange(of: exportSettingsOpen) { if $0 { scheduleComparison() } else { dismissComparison() } }
        .onChange(of: exportFormat) { _ in
            if formatRequiresCopy { makeCopy = true }
            scheduleComparison()
        }
        .onChange(of: exportFilename) { value in
            if value != model.sourceURL.deletingPathExtension().lastPathComponent { makeCopy = true }
        }
        .onChange(of: makeCopy) { enabled in
            if !enabled {
                exportDirectory = model.sourceURL.deletingLastPathComponent()
                exportFilename = model.sourceURL.deletingPathExtension().lastPathComponent
            }
        }
        .onChange(of: exportQualityMode) { _ in scheduleComparison() }
        .onChange(of: quality) { _ in scheduleComparison() }
        .onChange(of: maximumSizeMB) { _ in scheduleComparison() }
        .onDisappear { comparisonWork?.cancel(); dismissComparison() }
        .animation(NativeTheme.motion, value: tool)
        .animation(NativeTheme.standard, value: exportSettingsOpen)
    }

    private var header: some View {
        HStack(spacing: 10) {
            HStack(spacing: 5) {
                Text("Canvas").font(.caption).foregroundStyle(NativeTheme.muted(colorScheme))
                TextField("W", text: $widthText).frame(width: 58).textFieldStyle(.roundedBorder)
                    .onSubmit(applyCanvasSize)
                Text("×")
                TextField("H", text: $heightText).frame(width: 58).textFieldStyle(.roundedBorder)
                    .onSubmit(applyCanvasSize)
                Divider().frame(height: 22)
                Button { perform { try model.trimTransparentEdges() } } label: {
                    Label("Trim edges", systemImage: "arrow.down.right.and.arrow.up.left")
                }.buttonStyle(CaptureButtonStyle())
                if let layerID = model.selectedLayerID,
                   model.canvasExpansion(for: layerID) != nil {
                    Button { perform { try model.expandCanvasToFit(layerID: layerID) } } label: {
                        Label("Expand canvas", systemImage: "arrow.up.left.and.arrow.down.right")
                    }
                    .buttonStyle(CaptureButtonStyle(primary: true))
                }
                CaptureChoice(title: "Background color", selection: backgroundSelection, options: [
                    CaptureOption(label: "Transparent", value: "transparent"),
                    CaptureOption(label: "White", value: "white"),
                    CaptureOption(label: "Black", value: "black"),
                ]).frame(width: 145)
            }
            Spacer()
            Button { model.undo() } label: { Image(systemName: "arrow.uturn.backward") }
                .buttonStyle(CaptureButtonStyle()).disabled(!model.canUndo).help("Undo")
            Button { model.redo() } label: { Image(systemName: "arrow.uturn.forward") }
                .buttonStyle(CaptureButtonStyle()).disabled(!model.canRedo).help("Redo")
            HStack(spacing: 6) {
                Button { zoomMode = "fit"; applyZoomMode("fit") } label: { Image(systemName: "arrow.up.left.and.arrow.down.right") }
                    .buttonStyle(CaptureButtonStyle(primary: zoomMode == "fit")).help("Fit canvas")
                Button { zoomBy(0.8) } label: { Image(systemName: "minus") }
                    .buttonStyle(CaptureButtonStyle()).help("Zoom out").disabled(zoom <= EditorViewportMath.minimumZoom)
                CaptureSlider(
                    value: Binding(
                        get: { EditorViewportMath.sliderPosition(for: zoom) },
                        set: { setManualZoom(EditorViewportMath.zoom(forSliderPosition: $0)) }
                    ),
                    range: 0...1
                ).frame(width: 90)
                Button { zoomBy(1.25) } label: { Image(systemName: "plus") }
                    .buttonStyle(CaptureButtonStyle()).help("Zoom in").disabled(zoom >= EditorViewportMath.maximumZoom)
                CaptureChoice(title: "Zoom mode", selection: $zoomMode, options: [
                    CaptureOption(label: "Fit", value: "fit"),
                    CaptureOption(label: "100%", value: "actual"),
                    CaptureOption(label: "\(Int((zoom * 100).rounded()))%", value: "custom"),
                ]).frame(width: 92)
                .onChange(of: zoomMode) { applyZoomMode($0) }
            }
            Button { chooseImage() } label: { Label("Add images", systemImage: "photo.on.rectangle.angled") }
                .buttonStyle(CaptureButtonStyle()).keyboardShortcut("i", modifiers: [.command])
        }
        .padding(.horizontal, NativeTheme.metric("s-5")).frame(minHeight: 52)
        .background(NativeTheme.raised(colorScheme))
        .overlay(alignment: .bottom) { Divider() }
    }

    private var toolRail: some View {
        VStack(spacing: 4) {
            ForEach(ImageEditorTool.allCases) { item in
                if item == .shape {
                    shapeTool
                } else if item == .erase {
                    backgroundTool
                } else if item != .restore && item != .wand {
                    toolButton(item)
                }
            }
            Spacer()
        }
        .padding(NativeTheme.metric("s-3")).frame(minWidth: 56, maxWidth: 56, maxHeight: .infinity)
        .background(NativeTheme.raised(colorScheme))
        .overlay(alignment: .trailing) { Divider() }
    }

    private func toolButton(_ item: ImageEditorTool) -> some View {
        Button {
            tool = item
            if [.wand, .erase, .restore].contains(item) { selectBackgroundImageIfNeeded() }
            else if item != .select { model.selectedLayerID = nil }
            shapeFlyoutOpen = false
            backgroundFlyoutOpen = false
        } label: {
            Image(systemName: item.symbol).frame(width: 28, height: 28)
        }
        .buttonStyle(CaptureButtonStyle(primary: tool == item))
        .help(item.rawValue)
    }

    private var backgroundTool: some View {
        Button {
            if ![.wand, .erase, .restore].contains(tool) { tool = .wand }
            selectBackgroundImageIfNeeded()
            shapeFlyoutOpen = false
            backgroundFlyoutOpen.toggle()
        } label: {
            Image(systemName: tool == .restore ? "paintbrush" : tool == .wand ? "wand.and.stars" : "eraser")
                .frame(width: 28, height: 28)
                .overlay(alignment: .bottomTrailing) {
                    Image(systemName: "chevron.right").font(.system(size: 7, weight: .bold))
                }
        }
        .buttonStyle(CaptureButtonStyle(primary: [.wand, .erase, .restore].contains(tool)))
        .help("Remove background")
        .overlay(alignment: .leading) {
            if backgroundFlyoutOpen {
                HStack(spacing: 5) {
                    ForEach([ImageEditorTool.wand, .erase, .restore]) { item in
                        toolButton(item)
                    }
                }
                .padding(6)
                .background(NativeTheme.raised(colorScheme), in: RoundedRectangle(cornerRadius: NativeTheme.metric("r-lg")))
                .overlay(RoundedRectangle(cornerRadius: NativeTheme.metric("r-lg")).stroke(NativeTheme.border(colorScheme)))
                .shadow(color: .black.opacity(0.32), radius: 14, y: 5)
                .offset(x: 52)
                .zIndex(100)
            }
        }
        .zIndex(backgroundFlyoutOpen ? 100 : 0)
    }

    private var backgroundSelection: Binding<String> {
        Binding(
            get: {
                if model.document.background == .white { return "white" }
                if model.document.background == EditorColor(red: 0, green: 0, blue: 0) { return "black" }
                return "transparent"
            },
            set: { value in
                model.mutate {
                    $0.background = value == "white" ? .white
                        : value == "black" ? EditorColor(red: 0, green: 0, blue: 0) : nil
                }
            }
        )
    }

    private func shapeSymbol(_ shape: EditorShape) -> String {
        switch shape {
        case .rectangle: return "rectangle"
        case .ellipse: return "circle"
        case .line: return "line.diagonal"
        case .triangle: return "triangle"
        case .diamond: return "diamond"
        case .star: return "star"
        case .arrow: return "arrow.up.right"
        }
    }

    private var shapeTool: some View {
        Button {
            tool = .shape
            model.selectedLayerID = nil
            backgroundFlyoutOpen = false
            shapeFlyoutOpen.toggle()
        } label: {
            Image(systemName: shapeSymbol(selectedShape))
                .frame(width: 28, height: 28)
                .overlay(alignment: .bottomTrailing) {
                    Image(systemName: "chevron.right").font(.system(size: 7, weight: .bold))
                }
        }
        .buttonStyle(CaptureButtonStyle(primary: tool == .shape))
        .help("Shapes")
        .overlay(alignment: .leading) {
            if shapeFlyoutOpen {
                HStack(spacing: 5) {
                    ForEach(EditorShape.allCases.filter { $0 != .arrow }, id: \.self) { shape in
                        Button {
                            selectedShape = shape
                            tool = .shape
                            shapeFlyoutOpen = false
                        } label: {
                            Image(systemName: shapeSymbol(shape))
                                .frame(width: 28, height: 28)
                        }
                        .buttonStyle(CaptureButtonStyle(primary: selectedShape == shape))
                        .help(shape.rawValue.capitalized)
                    }
                }
                .padding(6)
                .background(NativeTheme.raised(colorScheme), in: RoundedRectangle(cornerRadius: NativeTheme.metric("r-lg")))
                .overlay(RoundedRectangle(cornerRadius: NativeTheme.metric("r-lg")).stroke(NativeTheme.border(colorScheme)))
                .shadow(color: .black.opacity(0.32), radius: 14, y: 5)
                .offset(x: 52)
                .zIndex(100)
            }
        }
        .zIndex(shapeFlyoutOpen ? 100 : 0)
    }

    private var canvasViewport: some View {
        GeometryReader { geometry in
            ScrollView([.horizontal, .vertical]) {
                ZStack {
                    checkerboard
                    renderedCanvas
                    alignmentGuideOverlay
                    selectionOverlay
                    cropOverlay
                    if !draftPoints.isEmpty { freehandPreview }
                    if comparisonBefore != nil { comparisonOverlay }
                }
                .frame(width: CGFloat(model.document.width) * zoom, height: CGFloat(model.document.height) * zoom)
                .coordinateSpace(name: "image-editor-canvas")
                .contentShape(Rectangle())
                .gesture(canvasGesture)
                .shadow(color: .black.opacity(0.22), radius: 18, y: 8)
                .background(GeometryReader { canvas in
                    Color.clear.preference(
                        key: EditorCanvasFramePreference.self,
                        value: canvas.frame(in: .named("image-editor-viewport"))
                    )
                })
                .offset(viewPan)
                .padding(32)
                .frame(minWidth: geometry.size.width, minHeight: geometry.size.height)
            }
            .onAppear { updateViewport(geometry.size) }
            .onChange(of: geometry.size) { updateViewport($0) }
            .coordinateSpace(name: "image-editor-viewport")
            .onPreferenceChange(EditorCanvasFramePreference.self, perform: updateCanvasFrame)
            .overlay {
                ZStack(alignment: .top) {
                    EditorViewportEvents(
                        zoomBy: { factor, point in zoomBy(factor, anchor: point) },
                        zoomActual: { setManualZoom(1) },
                        panBegan: { panGestureOrigin = viewPan },
                        panChanged: { delta in
                            viewPan = CGSize(
                                width: panGestureOrigin.width + delta.width,
                                height: panGestureOrigin.height + delta.height
                            )
                        },
                        panEnded: {}
                    )
                    if canvasOffscreen {
                        Button("Recenter") {
                            pendingZoomAnchor = nil
                            viewPan = .zero
                        }
                        .buttonStyle(CaptureButtonStyle())
                        .padding(.top, NativeTheme.metric("s-4"))
                    }
                }
            }
        }
        .background(NativeTheme.field(colorScheme))
    }

    private var checkerboard: some View {
        Canvas { context, size in
            let tile: CGFloat = 12
            for y in stride(from: 0 as CGFloat, to: size.height, by: tile) {
                for x in stride(from: 0 as CGFloat, to: size.width, by: tile) {
                    let alternate = (Int(x / tile) + Int(y / tile)).isMultiple(of: 2)
                    context.fill(Path(CGRect(x: x, y: y, width: tile, height: tile)), with: .color(alternate ? .white : Color(nsColor: .lightGray).opacity(0.45)))
                }
            }
        }
    }

    @ViewBuilder private var renderedCanvas: some View {
        if let image = try? model.renderedImage() {
            Image(nsImage: image).resizable().interpolation(.high)
        }
    }

    @ViewBuilder private var selectionOverlay: some View {
        if let index = model.selectedLayerIndex, tool == .select {
            let frame = model.document.layers[index].frame.cgRect
            ZStack {
                Rectangle()
                    .stroke(NativeTheme.accent, style: StrokeStyle(lineWidth: 2, dash: [5, 4]))
                    .frame(width: frame.width * zoom, height: frame.height * zoom)
                    .position(x: frame.midX * zoom, y: frame.midY * zoom)
                    .rotationEffect(Angle(radians: Double(model.document.layers[index].rotation)))
                    .allowsHitTesting(false)
                ForEach(Array(ImageResizeCorner.allCases.enumerated()), id: \.offset) { _, corner in
                    selectionResizeHandle(corner, frame: frame, rotation: model.document.layers[index].rotation)
                }
                rotationHandle(frame: frame, rotation: model.document.layers[index].rotation)
            }
        }
    }

    private var alignmentGuideOverlay: some View {
        ZStack {
            ForEach(Array(alignmentGuides.enumerated()), id: \.offset) { _, guide in
                if guide.axis == .vertical {
                    Rectangle().fill(NativeTheme.accent)
                        .frame(width: 1, height: CGFloat(model.document.height) * zoom)
                        .position(x: guide.position * zoom, y: CGFloat(model.document.height) * zoom / 2)
                } else {
                    Rectangle().fill(NativeTheme.accent)
                        .frame(width: CGFloat(model.document.width) * zoom, height: 1)
                        .position(x: CGFloat(model.document.width) * zoom / 2, y: guide.position * zoom)
                }
            }
        }
        .allowsHitTesting(false)
    }

    @ViewBuilder private var cropOverlay: some View {
        if tool == .crop, let cropRect {
            Rectangle()
                .stroke(NativeTheme.accent, lineWidth: 2)
                .background(Color.clear)
                .frame(width: cropRect.width * zoom, height: cropRect.height * zoom)
                .position(x: cropRect.midX * zoom, y: cropRect.midY * zoom)
                .overlay(alignment: .topLeading) {
                    Text("\(Int(cropRect.width)) × \(Int(cropRect.height))")
                        .font(.caption.monospaced()).padding(5)
                        .foregroundStyle(NativeTheme.glassText).background(NativeTheme.glass).cornerRadius(5)
                }
                .allowsHitTesting(false)
        }
    }

    private var freehandPreview: some View {
        Canvas { context, _ in
            var path = Path()
            if let first = draftPoints.first {
                path.move(to: CGPoint(x: first.x * zoom, y: first.y * zoom))
                for point in draftPoints.dropFirst() { path.addLine(to: CGPoint(x: point.x * zoom, y: point.y * zoom)) }
            }
            context.stroke(path, with: .color(NativeTheme.signal), style: StrokeStyle(lineWidth: strokeWidth * zoom, lineCap: .round, lineJoin: .round))
        }.allowsHitTesting(false)
    }

    private var canvasGesture: some Gesture {
        DragGesture(minimumDistance: tool == .select ? 2 : 0)
            .onChanged { value in
                let point = CGPoint(x: value.location.x / zoom, y: value.location.y / zoom)
                let start = CGPoint(x: value.startLocation.x / zoom, y: value.startLocation.y / zoom)
                switch tool {
                case .select:
                    if dragOrigin == nil {
                        selectLayer(at: start)
                        dragOrigin = start
                        if let index = model.selectedLayerIndex {
                            dragLayerFrame = model.document.layers[index].frame
                            if !model.document.layers[index].locked { model.beginInteractiveEdit() }
                        }
                    }
                    if let origin = dragOrigin, let initial = dragLayerFrame,
                       let index = model.selectedLayerIndex, !model.document.layers[index].locked {
                        let delta = CGSize(width: point.x - origin.x, height: point.y - origin.y)
                        let proposed = EditorRect(
                            x: initial.x + delta.width, y: initial.y + delta.height,
                            width: initial.width, height: initial.height
                        )
                        let snapped = model.snapTranslatedFrame(
                            proposed, layerID: model.document.layers[index].id,
                            threshold: 10 / max(0.01, zoom)
                        )
                        alignmentGuides = snapped.guides
                        model.updateSelectedLive { $0.frame = snapped.frame }
                    }
                case .crop:
                    cropStart = cropStart ?? start
                    cropRect = boundedRect(from: cropStart!, to: point)
                case .pen:
                    if draftPoints.isEmpty { draftPoints = [start] }
                    draftPoints.append(point)
                case .erase, .restore:
                    if brushLayerID == nil {
                        guard selectBackgroundImage(at: start) else { break }
                        brushLayerID = model.selectedLayerID
                        brushLastPoint = start
                        model.beginInteractiveEdit()
                    }
                    guard let layerID = brushLayerID, let previous = brushLastPoint else { break }
                    model.selectedLayerID = layerID
                    perform {
                        try model.eraseStroke(
                            from: previous, to: point, radius: brushSize / 2,
                            softness: brushSoftness, restore: tool == .restore
                        )
                    }
                    brushLastPoint = point
                default: break
                }
            }
            .onEnded { value in
                let point = CGPoint(x: value.location.x / zoom, y: value.location.y / zoom)
                let start = CGPoint(x: value.startLocation.x / zoom, y: value.startLocation.y / zoom)
                switch tool {
                case .select:
                    model.endInteractiveEdit()
                    alignmentGuides = []
                case .crop:
                    cropRect = boundedRect(from: cropStart ?? start, to: point)
                    cropStart = nil
                case .text:
                    model.addText("Text", at: point)
                case .shape:
                    model.addShape(
                        selectedShape, at: point, color: defaultEditorColor,
                        lineWidth: strokeWidth, fill: fillShapes, opacity: defaultOpacity
                    )
                case .arrow:
                    model.addShape(
                        .arrow, at: point, color: defaultEditorColor,
                        lineWidth: strokeWidth, opacity: defaultOpacity
                    )
                case .pen:
                    model.addStroke(
                        draftPoints, color: defaultEditorColor,
                        lineWidth: strokeWidth, opacity: defaultOpacity
                    )
                    draftPoints.removeAll()
                case .wand:
                    if selectBackgroundImage(at: point) {
                        perform {
                            try model.removeBackgroundColor(
                                at: point, tolerance: wandTolerance, contiguous: wandContiguous
                            )
                        }
                    }
                case .erase, .restore:
                    brushLastPoint = nil
                    brushLayerID = nil
                    model.endInteractiveEdit()
                }
                dragOrigin = nil
                dragLayerFrame = nil
            }
    }

    private var inspector: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                SectionTitle("Layers", subtitle: "\(model.document.layers.count)")
                Spacer()
                Button { chooseImage() } label: { Image(systemName: "plus") }.buttonStyle(CaptureButtonStyle())
            }.padding(16)
            ScrollView {
                LazyVStack(spacing: 5) {
                    ForEach(Array(model.document.layers.enumerated().reversed()), id: \.element.id) { _, layer in
                        layerRow(layer)
                    }
                }.padding(.horizontal, 10)
            }.frame(minHeight: 150, maxHeight: 280)
            Divider().padding(.top, 10)
            GeometryReader { viewport in
                ScrollViewReader { proxy in
                    ScrollView { properties.padding(NativeTheme.metric("s-5")) }
                        .coordinateSpace(name: "image-editor-properties")
                        .onAppear {
                            guard let initialPropertiesAnchor else { return }
                            DispatchQueue.main.async {
                                proxy.scrollTo(initialPropertiesAnchor, anchor: .bottom)
                            }
                        }
                        .onPreferenceChange(EditorCombineFramePreference.self) { frame in
                            guard onCombineControlsVisibilityChanged != nil else { return }
                            onCombineControlsVisibilityChanged?(
                                frame.width > 0 && frame.height > 0
                                    && frame.minY >= -0.5
                                    && frame.maxY <= viewport.size.height + 0.5
                            )
                        }
                }
            }
        }
        .frame(minWidth: 320, maxWidth: 320, maxHeight: .infinity)
        .background(NativeTheme.raised(colorScheme))
        .overlay(alignment: .leading) { Divider() }
    }

    private func layerRow(_ layer: EditorLayer) -> some View {
        HStack(spacing: 8) {
            Button { model.updateLayer(id: layer.id) { $0.visible.toggle() } } label: {
                Image(systemName: layer.visible ? "eye" : "eye.slash")
            }.buttonStyle(.plain).help(layer.visible ? "Hide layer" : "Show layer")
            VStack(alignment: .leading, spacing: 2) {
                Text(layer.name).lineLimit(1)
                Text(layerKind(layer)).font(.caption).foregroundStyle(NativeTheme.muted(colorScheme))
            }
            Spacer()
            Button { model.updateLayer(id: layer.id) { $0.locked.toggle() } } label: {
                Image(systemName: layer.locked ? "lock.fill" : "lock.open")
            }.buttonStyle(.plain).help(layer.locked ? "Unlock layer" : "Lock layer")
        }
        .padding(9)
        .background(model.selectedLayerID == layer.id ? NativeTheme.accent.opacity(0.16) : NativeTheme.field(colorScheme))
        .clipShape(RoundedRectangle(cornerRadius: 7))
        .contentShape(Rectangle())
        .onTapGesture { model.selectedLayerID = layer.id; editorHex = layer.color.hex }
    }

    private func layerColorControls(_ layer: EditorLayer) -> some View {
        let swatches: [EditorColor] = [
            .signal,
            EditorColor(red: 1, green: 0.79, blue: 0.16),
            EditorColor(red: 0.24, green: 0.48, blue: 0.95),
            EditorColor(red: 0.21, green: 0.78, blue: 0.55),
            .white,
            EditorColor(red: 0.05, green: 0.05, blue: 0.06),
        ]
        return VStack(alignment: .leading, spacing: 8) {
            Text("Color").font(.headline)
            HStack(spacing: 7) {
                ForEach(swatches, id: \.self) { color in
                    Button {
                        model.updateSelected { $0.color = color }
                        editorHex = color.hex
                    } label: {
                        Circle().fill(Color(nsColor: color.nsColor))
                            .frame(width: 24, height: 24)
                            .overlay(Circle().stroke(Color.primary, lineWidth: layer.color == color ? 3 : 0))
                            .overlay(Circle().stroke(Color.white.opacity(0.5), lineWidth: 1).padding(2))
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(color.hex)
                    .accessibilityAddTraits(layer.color == color ? .isSelected : [])
                }
                TextField("#RRGGBB", text: $editorHex)
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 88)
                    .onSubmit(commitEditorHex)
            }
        }
    }

    @ViewBuilder private var properties: some View {
        if let index = model.selectedLayerIndex {
            let layer = model.document.layers[index]
            SectionTitle("Properties", subtitle: layer.name)
            VStack(alignment: .leading, spacing: 12) {
                if case let .text(text) = layer.content {
                    TextField("Text", text: Binding(get: { text }, set: { value in model.updateSelected { $0.content = .text(value) } }))
                        .textFieldStyle(.roundedBorder)
                }
                if case .image = layer.content { EmptyView() }
                else { layerColorControls(layer) }
                if case .shape = layer.content {
                    HStack {
                        Text("Stroke width")
                        CaptureSlider(value: Binding(
                            get: { Double(layer.lineWidth) },
                            set: { value in model.updateSelected { $0.lineWidth = CGFloat(value) } }
                        ), range: 2...40)
                        Text("\(Int(layer.lineWidth.rounded())) px").monospacedDigit().frame(width: 46)
                    }
                } else if case .freehand = layer.content {
                    HStack {
                        Text("Stroke width")
                        CaptureSlider(value: Binding(
                            get: { Double(layer.lineWidth) },
                            set: { value in model.updateSelected { $0.lineWidth = CGFloat(value) } }
                        ), range: 2...40)
                        Text("\(Int(layer.lineWidth.rounded())) px").monospacedDigit().frame(width: 46)
                    }
                }
                HStack {
                    Text("Opacity")
                    CaptureSlider(value: Binding(
                        get: { Double(layer.opacity) },
                        set: { value in model.updateSelected { $0.opacity = CGFloat(value) } }
                    ), range: 0...1)
                    Text("\(Int(layer.opacity * 100))%").monospacedDigit().frame(width: 42)
                }
                HStack {
                    Text("Blend mode")
                    Spacer()
                    Picker("Blend mode", selection: Binding(
                        get: { layer.blendMode ?? .normal },
                        set: { mode in model.updateSelected { $0.blendMode = mode == .normal ? nil : mode } }
                    )) {
                        ForEach(EditorBlendMode.allCases) { mode in Text(mode.label).tag(mode) }
                    }
                    .labelsHidden()
                    .pickerStyle(.menu)
                    .frame(width: 130)
                }
                HStack {
                    Text("Rotation")
                    CaptureSlider(value: Binding(
                        get: { Double(layer.rotation * 180 / .pi) },
                        set: { value in model.updateSelected { $0.rotation = CGFloat(value) * .pi / 180 } }
                    ), range: -180...180)
                }
                if case .shape = layer.content {
                    CaptureCheckboxRow(title: "Filled shape", isOn: Binding(
                        get: { layer.fill != nil },
                        set: { enabled in model.updateSelected { $0.fill = enabled ? $0.color : nil } }
                    ))
                }
                CaptureCheckboxRow(title: "Shadow", isOn: Binding(
                    get: { layer.shadow != nil },
                    set: { enabled in model.updateSelected { $0.shadow = enabled ? EditorShadow() : nil } }
                ))
                if let shadow = layer.shadow {
                    HStack {
                        Text("Shadow blur")
                        CaptureSlider(value: Binding(
                            get: { Double(shadow.radius) },
                            set: { value in model.updateSelected { $0.shadow?.radius = CGFloat(value) } }
                        ), range: 0...40)
                        Text("\(Int(shadow.radius.rounded())) px").monospacedDigit().frame(width: 46)
                    }
                    HStack {
                        Text("Opacity")
                        CaptureSlider(value: Binding(
                            get: { Double(shadow.opacity) },
                            set: { value in model.updateSelected { $0.shadow?.opacity = CGFloat(value) } }
                        ), range: 0...1)
                        Text("\(Int((shadow.opacity * 100).rounded()))%").monospacedDigit().frame(width: 42)
                    }
                    HStack(spacing: 12) {
                        shadowOffsetField("X offset", value: shadow.offsetX, keyPath: \EditorShadow.offsetX)
                        shadowOffsetField("Y offset", value: shadow.offsetY, keyPath: \EditorShadow.offsetY)
                    }
                }
                if case .image = layer.content, tool == .erase || tool == .restore {
                    Text("Background brush").font(.headline)
                    CaptureSlider(
                        value: Binding(get: { Double(brushSize) }, set: { brushSize = CGFloat($0) }),
                        range: 4...160
                    )
                    HStack {
                        Text("Softness")
                        CaptureSlider(
                            value: Binding(get: { Double(brushSoftness) }, set: { brushSoftness = CGFloat($0) }),
                            range: 0...1
                        )
                        Text("\(Int((brushSoftness * 100).rounded()))%").monospacedDigit().frame(width: 42)
                    }
                    Text("Drag over the image to \(tool == .restore ? "restore original pixels" : "make pixels transparent").")
                        .font(.caption).foregroundStyle(NativeTheme.muted(colorScheme))
                }
                if case .image = layer.content, tool == .wand {
                    Text("Color tolerance").font(.headline)
                    CaptureSlider(
                        value: Binding(get: { Double(wandTolerance) }, set: { wandTolerance = CGFloat($0) }),
                        range: 0.01...0.5
                    )
                    CaptureCheckboxRow(title: "Contiguous only", isOn: $wandContiguous)
                    Text(wandContiguous
                         ? "Click a color to remove that area. Undo restores it."
                         : "Click a color to remove it everywhere in the layer. Undo restores it.")
                        .font(.caption).foregroundStyle(NativeTheme.muted(colorScheme))
                }
                HStack {
                    Button { model.reorderSelected(by: 1) } label: { Image(systemName: "arrow.up") }.buttonStyle(CaptureButtonStyle()).help("Move layer up")
                    Button { model.reorderSelected(by: -1) } label: { Image(systemName: "arrow.down") }.buttonStyle(CaptureButtonStyle()).help("Move layer down")
                    Button { model.duplicateSelected() } label: { Image(systemName: "plus.square.on.square") }.buttonStyle(CaptureButtonStyle()).help("Duplicate layer")
                    Button { model.deleteSelected() } label: { Image(systemName: "trash") }.buttonStyle(CaptureButtonStyle(destructive: true)).disabled(layer.locked).help("Delete layer")
                }
                VStack(alignment: .leading, spacing: 12) {
                    Divider()
                    Text("Combine").font(.headline)
                    HStack {
                        Button("Merge down") { perform { try model.mergeSelectedDown() } }
                            .buttonStyle(CaptureButtonStyle())
                            .disabled(!model.canMergeSelectedDown)
                        Button("Merge visible") { perform { try model.mergeVisible() } }
                            .buttonStyle(CaptureButtonStyle())
                            .disabled(!model.canMergeVisible)
                    }
                    Button("Flatten image") { perform { try model.flatten() } }
                        .buttonStyle(CaptureButtonStyle())
                        .disabled(!model.canFlatten)
                        .help("Bake the canvas background and visible layers into one locked layer; discard hidden layers")
                }
                .id("layer-combine-controls")
                .background(GeometryReader { geometry in
                    Color.clear.preference(
                        key: EditorCombineFramePreference.self,
                        value: geometry.frame(in: .named("image-editor-properties"))
                    )
                })
            }.padding(.top, 14)
        } else if tool == .crop {
            SectionTitle("Crop", subtitle: "Drag on the canvas")
            Button("Apply crop") {
                if let cropRect { model.crop(to: cropRect); self.cropRect = nil; tool = .select }
            }.buttonStyle(CaptureButtonStyle(primary: true)).disabled(cropRect == nil).padding(.top, 12)
        } else if [.shape, .arrow, .pen].contains(tool) {
            SectionTitle(tool == .pen ? "Freehand" : tool.rawValue)
            VStack(alignment: .leading, spacing: 12) {
                defaultColorControls
                HStack {
                    Text("Size")
                    CaptureSlider(
                        value: Binding(get: { Double(strokeWidth) }, set: { strokeWidth = CGFloat($0) }),
                        range: 2...40
                    )
                    Text("\(Int(strokeWidth.rounded())) px").monospacedDigit().frame(width: 46)
                }
                HStack {
                    Text("Opacity")
                    CaptureSlider(
                        value: Binding(get: { Double(defaultOpacity) }, set: { defaultOpacity = CGFloat($0) }),
                        range: 0...1
                    )
                    Text("\(Int((defaultOpacity * 100).rounded()))%").monospacedDigit().frame(width: 42)
                }
                if tool == .shape, ![.line, .arrow].contains(selectedShape) {
                    CaptureCheckboxRow(title: "Filled shape", isOn: $fillShapes)
                }
                Text("These settings apply to the next annotation.")
                    .font(.caption).foregroundStyle(NativeTheme.muted(colorScheme))
            }.padding(.top, 12)
        } else if tool == .select {
            EmptyView()
        } else {
            SectionTitle(tool.rawValue)
            Text("Choose a tool or select a layer to edit its properties.")
                .font(.caption).foregroundStyle(NativeTheme.muted(colorScheme)).padding(.top, 12)
        }
    }

    private var defaultColorControls: some View {
        let swatches: [EditorColor] = [
            .signal,
            EditorColor(red: 1, green: 0.79, blue: 0.16),
            EditorColor(red: 0.24, green: 0.48, blue: 0.95),
            EditorColor(red: 0.21, green: 0.78, blue: 0.55),
            .white,
            EditorColor(red: 0.05, green: 0.05, blue: 0.06),
        ]
        return VStack(alignment: .leading, spacing: 8) {
            Text("Color").font(.headline)
            HStack(spacing: 7) {
                ForEach(swatches, id: \.self) { color in
                    Button {
                        editorHex = color.hex
                    } label: {
                        Circle().fill(Color(nsColor: color.nsColor))
                            .frame(width: 24, height: 24)
                            .overlay(Circle().stroke(Color.primary, lineWidth: defaultEditorColor == color ? 3 : 0))
                            .overlay(Circle().stroke(Color.white.opacity(0.5), lineWidth: 1).padding(2))
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(color.hex)
                    .accessibilityAddTraits(defaultEditorColor == color ? .isSelected : [])
                }
                TextField("#RRGGBB", text: $editorHex)
                    .textFieldStyle(.roundedBorder).frame(width: 88)
                    .onSubmit(commitEditorHex)
            }
        }
    }

    private func shadowOffsetField(
        _ title: String,
        value: CGFloat,
        keyPath: WritableKeyPath<EditorShadow, CGFloat>
    ) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.caption).foregroundStyle(NativeTheme.muted(colorScheme))
            TextField(title, value: Binding(
                get: { Double(value) },
                set: { next in
                    model.updateSelected {
                        $0.shadow?[keyPath: keyPath] = min(100, max(-100, CGFloat(next.rounded())))
                    }
                }
            ), format: .number.precision(.fractionLength(0)))
            .textFieldStyle(.roundedBorder)
        }
    }

    private var exportFooter: some View {
        VStack(spacing: 10) {
            if exportSettingsOpen {
                HStack(spacing: 18) {
                    CaptureSegments(selection: $exportFormat, options: [
                        CaptureOption(label: "PNG", value: "png"),
                        CaptureOption(label: "JPEG", value: "jpeg"),
                        CaptureOption(label: "WebP", value: "webp"),
                    ]).frame(width: 180)
                    CaptureSegments(selection: $exportQualityMode, options: [
                        CaptureOption(label: "Preserve", value: "preserve"),
                        CaptureOption(label: "Compress", value: "compress"),
                        CaptureOption(label: "Maximum", value: "maximum"),
                    ]).frame(width: 220)
                    if exportQualityMode == "compress" {
                        CaptureChoice(title: "Compression quality", selection: $quality, options: [
                            CaptureOption(label: "Tiny", value: "55"),
                            CaptureOption(label: "Smaller", value: "70"),
                            CaptureOption(label: "Balanced", value: "85"),
                            CaptureOption(label: "High", value: "92"),
                            CaptureOption(label: "Highest", value: "98"),
                        ], opensAbove: true).frame(width: 170)
                    }
                    if exportQualityMode == "maximum" {
                        HStack { Text("Maximum"); TextField("MB", value: $maximumSizeMB, format: .number).frame(width: 70); Text("MB") }
                    }
                    if exportFormat == "png", exportQualityMode != "preserve" {
                        Text(exportQualityMode == "compress"
                             ? "PNG uses tighter lossless packing in this native build."
                             : "PNG is not resized to meet a limit; export fails if the lossless file is too large.")
                            .font(.caption)
                            .foregroundStyle(NativeTheme.muted(colorScheme))
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: 260, alignment: .leading)
                    }
                    Spacer()
                    if exportQualityMode != "preserve" {
                        Button {
                            comparisonExpanded.toggle()
                            if comparisonExpanded { scheduleComparison() } else { dismissComparison() }
                        } label: {
                            HStack(spacing: 6) {
                                Image(systemName: "chevron.right")
                                    .rotationEffect(comparisonExpanded ? .degrees(90) : .zero)
                                Text("Compression comparison")
                                Text(comparisonPending ? "Encoding…" : "Updates automatically")
                                    .foregroundColor(NativeTheme.muted(colorScheme))
                            }
                        }
                        .buttonStyle(CaptureButtonStyle())
                    }
                }
            }
            HStack(spacing: 12) {
                Button { exportSettingsOpen.toggle() } label: {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Export settings").font(.callout.weight(.medium))
                        Text("\(exportFormat.uppercased()) · \(model.document.width) × \(model.document.height)")
                            .font(.caption.monospaced()).foregroundStyle(NativeTheme.muted(colorScheme))
                    }
                }.buttonStyle(CaptureButtonStyle())
                VStack(alignment: .leading, spacing: 3) {
                    Text("Filename").font(.caption).foregroundStyle(NativeTheme.muted(colorScheme))
                    HStack(spacing: 6) {
                        Text("Saving to \(exportDirectory.path)")
                            .font(.caption).foregroundStyle(NativeTheme.muted(colorScheme)).lineLimit(1)
                        Button("Change…", action: chooseExportDirectory).buttonStyle(.plain)
                    }
                    HStack(spacing: 5) {
                        TextField("Filename", text: $exportFilename).textFieldStyle(.roundedBorder)
                        CaptureChoice(title: "File format", selection: $exportFormat, options: [
                            CaptureOption(label: ".png", value: "png"),
                            CaptureOption(label: ".jpg", value: "jpeg"),
                            CaptureOption(label: ".webp", value: "webp"),
                        ], opensAbove: true).frame(width: 82)
                    }
                }
                .frame(width: 280)
                Spacer()
                if !notice.isEmpty { Text(notice).font(.callout).foregroundStyle(NativeTheme.muted(colorScheme)) }
                Button { copyImage() } label: { Label("Copy image", systemImage: "doc.on.doc") }
                    .buttonStyle(CaptureButtonStyle())
                Text(makeCopy ? "Save creates a new file." : "Save overwrites the original after confirmation.")
                    .font(.caption).foregroundStyle(NativeTheme.muted(colorScheme))
                    .frame(maxWidth: 185, alignment: .leading)
                HStack(spacing: 8) {
                    CaptureToggle(title: "Save as new file", isOn: $makeCopy, compact: true)
                    Text("Save as new file")
                }
                .disabled(formatRequiresCopy || exporting)
                Button {
                    if makeCopy || formatRequiresCopy { exportAsNewFile() } else { saveSource() }
                } label: { Label(exporting ? "Saving…" : "Save", systemImage: "square.and.arrow.down") }
                .buttonStyle(CaptureButtonStyle(primary: true))
                .disabled(exporting)
            }
        }
        .padding(NativeTheme.metric("s-4")).background(NativeTheme.raised(colorScheme)).overlay(alignment: .top) { Divider() }
    }

    private func selectLayer(at point: CGPoint) {
        model.selectedLayerID = model.document.layers.reversed().first(where: {
            $0.visible && layerContains($0, point: point)
        })?.id
    }

    @discardableResult
    private func selectBackgroundImage(at point: CGPoint) -> Bool {
        guard let layer = model.document.layers.reversed().first(where: {
            guard $0.visible, case .image = $0.content else { return false }
            return layerContains($0, point: point)
        }) else { return false }
        model.selectedLayerID = layer.id
        return true
    }

    private func selectBackgroundImageIfNeeded() {
        if let index = model.selectedLayerIndex, case .image = model.document.layers[index].content { return }
        model.selectedLayerID = model.document.layers.reversed().first(where: {
            guard $0.visible, case .image = $0.content else { return false }
            return true
        })?.id
    }

    private func layerContains(_ layer: EditorLayer, point: CGPoint) -> Bool {
        let frame = layer.frame.cgRect
        let center = CGPoint(x: frame.midX, y: frame.midY)
        return frame.contains(rotatedPoint(point, around: center, radians: -layer.rotation))
    }

    private func selectionResizeHandle(_ corner: ImageResizeCorner, frame: CGRect, rotation: CGFloat) -> some View {
        let point: CGPoint
        switch corner {
        case .northWest: point = CGPoint(x: frame.minX, y: frame.minY)
        case .northEast: point = CGPoint(x: frame.maxX, y: frame.minY)
        case .southEast: point = CGPoint(x: frame.maxX, y: frame.maxY)
        case .southWest: point = CGPoint(x: frame.minX, y: frame.maxY)
        }
        let rotated = rotatedPoint(point, around: CGPoint(x: frame.midX, y: frame.midY), radians: rotation)
        return Circle().fill(Color.white).overlay(Circle().stroke(NativeTheme.accent, lineWidth: 2))
            .frame(width: 12, height: 12).position(x: rotated.x * zoom, y: rotated.y * zoom)
            .gesture(DragGesture(minimumDistance: 0).onChanged { value in
                if resizeLayerFrame == nil, let index = model.selectedLayerIndex {
                    resizeLayerFrame = model.document.layers[index].frame
                    model.beginInteractiveEdit()
                }
                guard let initial = resizeLayerFrame else { return }
                let dx = value.translation.width / zoom
                let dy = value.translation.height / zoom
                let cosine = cos(rotation)
                let sine = sin(rotation)
                let localDelta = CGSize(
                    width: dx * cosine + dy * sine,
                    height: -dx * sine + dy * cosine
                )
                let initialRect = initial.cgRect
                let initialCenter = CGPoint(x: initialRect.midX, y: initialRect.midY)
                var dragged = point
                dragged.x += localDelta.width
                dragged.y += localDelta.height
                let opposite = CGPoint(
                    x: corner == .northWest || corner == .southWest ? initialRect.maxX : initialRect.minX,
                    y: corner == .northWest || corner == .northEast ? initialRect.maxY : initialRect.minY
                )
                if corner == .northWest || corner == .southWest {
                    dragged.x = min(opposite.x - 4, dragged.x)
                } else {
                    dragged.x = max(opposite.x + 4, dragged.x)
                }
                if corner == .northWest || corner == .northEast {
                    dragged.y = min(opposite.y - 4, dragged.y)
                } else {
                    dragged.y = max(opposite.y + 4, dragged.y)
                }
                let localCenter = CGPoint(x: (dragged.x + opposite.x) / 2, y: (dragged.y + opposite.y) / 2)
                let worldCenter = rotatedPoint(localCenter, around: initialCenter, radians: rotation)
                let width = abs(dragged.x - opposite.x)
                let height = abs(dragged.y - opposite.y)
                model.updateSelectedLive {
                    $0.frame = EditorRect(
                        x: worldCenter.x - width / 2,
                        y: worldCenter.y - height / 2,
                        width: width,
                        height: height
                    )
                }
            }.onEnded { _ in resizeLayerFrame = nil; model.endInteractiveEdit() })
            .help("Resize layer")
    }

    private func rotationHandle(frame: CGRect, rotation: CGFloat) -> some View {
        let center = CGPoint(x: frame.midX * zoom, y: frame.midY * zoom)
        let stemStart = rotatedPoint(CGPoint(x: frame.midX, y: frame.minY), around: CGPoint(x: frame.midX, y: frame.midY), radians: rotation)
        let handlePoint = rotatedPoint(CGPoint(x: frame.midX, y: frame.minY - 28), around: CGPoint(x: frame.midX, y: frame.midY), radians: rotation)
        let handle = CGPoint(x: handlePoint.x * zoom, y: handlePoint.y * zoom)
        return ZStack {
            Path { path in
                path.move(to: CGPoint(x: stemStart.x * zoom, y: stemStart.y * zoom))
                path.addLine(to: handle)
            }
                .stroke(NativeTheme.accent, lineWidth: 1)
                .allowsHitTesting(false)
            Circle().fill(NativeTheme.raised(colorScheme))
                .overlay(Circle().stroke(NativeTheme.accent, lineWidth: 2))
                .frame(width: 15, height: 15)
                .position(handle)
                .gesture(DragGesture(minimumDistance: 0, coordinateSpace: .named("image-editor-canvas"))
                    .onChanged { value in
                        if !rotatingLayer { rotatingLayer = true; model.beginInteractiveEdit() }
                        var radians = atan2(value.location.y - center.y, value.location.x - center.x) + .pi / 2
                        let snap = CGFloat.pi / 12
                        let nearest = (radians / snap).rounded() * snap
                        if abs(radians - nearest) < .pi / 60 { radians = nearest }
                        model.updateSelectedLive { $0.rotation = radians }
                    }
                    .onEnded { _ in rotatingLayer = false; model.endInteractiveEdit() })
                .help("Rotate layer · snaps every 15°")
        }
    }

    private func rotatedPoint(_ point: CGPoint, around center: CGPoint, radians: CGFloat) -> CGPoint {
        let dx = point.x - center.x
        let dy = point.y - center.y
        return CGPoint(
            x: center.x + dx * cos(radians) - dy * sin(radians),
            y: center.y + dx * sin(radians) + dy * cos(radians)
        )
    }

    private func boundedRect(from start: CGPoint, to end: CGPoint) -> CGRect {
        CGRect(x: min(start.x, end.x), y: min(start.y, end.y), width: abs(end.x - start.x), height: abs(end.y - start.y))
            .intersection(CGRect(x: 0, y: 0, width: CGFloat(model.document.width), height: CGFloat(model.document.height)))
    }

    private func syncDimensions() {
        widthText = String(model.document.width)
        heightText = String(model.document.height)
        if zoomMode == "fit" { applyZoomMode("fit") }
    }

    private func updateViewport(_ size: CGSize) {
        viewportSize = size
        if zoomMode == "fit" { applyZoomMode("fit") }
        refreshCanvasOffscreen(viewport: size, frame: canvasViewportFrame)
    }

    private func applyZoomMode(_ mode: String) {
        switch mode {
        case "fit":
            pendingZoomAnchor = nil
            viewPan = .zero
            guard viewportSize.width > 64, viewportSize.height > 64 else { return }
            zoom = min(
                1,
                max(EditorViewportMath.minimumZoom, min(
                    (viewportSize.width - 64) / CGFloat(model.document.width),
                    (viewportSize.height - 64) / CGFloat(model.document.height)
                ))
            )
        case "actual": setManualZoom(1)
        default: break
        }
    }

    private func zoomBy(_ factor: CGFloat, anchor: CGPoint? = nil) {
        guard factor.isFinite, factor > 0 else { return }
        setManualZoom(zoom * factor, anchor: anchor)
    }

    private func setManualZoom(_ requested: CGFloat, anchor: CGPoint? = nil) {
        guard requested.isFinite else { return }
        let next = EditorViewportMath.clampedZoom(requested)
        let viewportPoint = anchor ?? CGPoint(x: viewportSize.width / 2, y: viewportSize.height / 2)
        if canvasViewportFrame.width > 0, canvasViewportFrame.height > 0, zoom > 0 {
            pendingZoomAnchor = EditorViewportMath.pendingAnchor(
                replacing: pendingZoomAnchor,
                viewportPoint: viewportPoint,
                measuredCanvasOrigin: canvasViewportFrame.origin,
                currentZoom: zoom,
                nextZoom: next
            )
        }
        zoomMode = "custom"
        zoom = next
    }

    private func updateCanvasFrame(_ frame: CGRect) {
        canvasViewportFrame = frame
        refreshCanvasOffscreen(viewport: viewportSize, frame: frame)
        guard let anchor = pendingZoomAnchor,
              abs(anchor.zoom - zoom) < 0.0001,
              EditorViewportMath.frame(
                frame,
                matches: CGSize(
                    width: CGFloat(model.document.width),
                    height: CGFloat(model.document.height)
                ),
                zoom: zoom
              ) else { return }
        let resolution = EditorViewportMath.resolvePendingAnchor(anchor, canvasOrigin: frame.origin)
        let correction = resolution.correction
        pendingZoomAnchor = resolution.pending
        if abs(correction.width) > 0.01 || abs(correction.height) > 0.01 {
            viewPan = CGSize(
                width: viewPan.width + correction.width,
                height: viewPan.height + correction.height
            )
        }
    }

    private func refreshCanvasOffscreen(viewport: CGSize, frame: CGRect) {
        let offscreen = EditorViewportMath.isMostlyOffscreen(viewportSize: viewport, canvasFrame: frame)
        canvasOffscreen = offscreen
        onViewportOffscreenChanged?(offscreen)
    }

    private func syncEditorColor() {
        guard let index = model.selectedLayerIndex else { return }
        editorHex = model.document.layers[index].color.hex
    }

    private func commitEditorHex() {
        let value = editorHex.trimmingCharacters(in: .whitespacesAndNewlines)
        guard value.range(of: "^#[0-9a-fA-F]{6}$", options: .regularExpression) != nil else {
            syncEditorColor()
            return
        }
        let converted = EditorColor(NSColor(css: value))
        if model.selectedLayerIndex != nil { model.updateSelected { $0.color = converted } }
        editorHex = converted.hex
    }

    private var defaultEditorColor: EditorColor {
        let value = editorHex.trimmingCharacters(in: .whitespacesAndNewlines)
        guard value.range(of: "^#[0-9a-fA-F]{6}$", options: .regularExpression) != nil else { return .signal }
        return EditorColor(NSColor(css: value))
    }

    private func applyCanvasSize() {
        guard let width = Int(widthText), let height = Int(heightText) else { return }
        model.resizeCanvas(width: width, height: height)
    }

    private func chooseImage() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.png, .jpeg, .tiff, .gif, .heic]
        panel.allowsMultipleSelection = true
        guard panel.runModal() == .OK else { return }
        for url in panel.urls { perform { try model.importImage(from: url) } }
    }

    private func chooseExportDirectory() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.allowsMultipleSelection = false
        panel.directoryURL = exportDirectory
        guard panel.runModal() == .OK, let directory = panel.url else { return }
        exportDirectory = directory
        if directory.standardizedFileURL != model.sourceURL.deletingLastPathComponent().standardizedFileURL {
            makeCopy = true
        }
    }

    private func copyImage() {
        perform {
            let image = try model.renderedImage()
            NSPasteboard.general.clearContents()
            NSPasteboard.general.writeObjects([image])
            notice = "Copied"
        }
    }

    private var sourceFormat: String {
        let value = model.sourceURL.pathExtension.lowercased()
        return value == "jpg" ? "jpeg" : value
    }

    private var formatRequiresCopy: Bool { sourceFormat != exportFormat }

    private func saveSource() {
        CaptureDialogController.shared.present(
            title: "Replace the original screenshot?",
            message: "This replaces the source image. Turn on Save as new file to keep the original untouched.",
            action: "Replace Original",
            destructive: true,
            onConfirm: replaceSource
        )
    }

    private func replaceSource() {
        let sourceExtension = model.sourceURL.pathExtension.lowercased()
        let sourceFormat = sourceExtension == "jpg" ? "jpeg" : sourceExtension
        guard ["png", "jpeg", "webp"].contains(sourceFormat) else {
            showExportError("The source format \(sourceExtension.uppercased()) cannot be replaced by the native editor. Export a PNG, JPEG, or WebP copy instead.")
            return
        }
        let directory = AppStore.dataDirectory.appendingPathComponent("image-editor-work/\(UUID().uuidString)", isDirectory: true)
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            let input = directory.appendingPathComponent("flattened.png")
            try model.exportData(format: "png").write(to: input, options: [.atomic])
            let staged = model.sourceURL.deletingLastPathComponent()
                .appendingPathComponent(".captures-\(UUID().uuidString).\(sourceExtension)")
            exporting = true
            Backend.shared.call("image_encode", imageEncodeFields(input: input, output: staged, format: sourceFormat)) { result in
                exporting = false
                defer { try? FileManager.default.removeItem(at: directory) }
                switch result {
                case .success:
                    do {
                        _ = try FileManager.default.replaceItemAt(model.sourceURL, withItemAt: staged)
                        model.clearDraft()
                        notice = "Saved"
                    } catch {
                        try? FileManager.default.removeItem(at: staged)
                        store.report(error)
                        notice = error.localizedDescription
                    }
                case let .failure(error):
                    try? FileManager.default.removeItem(at: staged)
                    store.report(error)
                    notice = error.localizedDescription
                }
            }
        } catch {
            try? FileManager.default.removeItem(at: directory)
            store.report(error)
            notice = error.localizedDescription
        }
    }

    private func exportAsNewFile() {
        let name = exportFilename.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, !name.contains("/"), name != ".", name != ".." else {
            showExportError("Enter a filename without folders or path separators.")
            return
        }
        let url = exportDirectory.appendingPathComponent(name).appendingPathExtension(exportFormat == "jpeg" ? "jpg" : exportFormat)
        guard !sameFile(url, model.sourceURL) else {
            showExportError("Export cannot replace the source image. Use Save when you explicitly want to replace the original.")
            return
        }
        if FileManager.default.fileExists(atPath: url.path) {
            showExportError("Export only creates new files. Choose a filename that does not already exist.")
            return
        }
        let directory = AppStore.dataDirectory.appendingPathComponent("image-editor-work/\(UUID().uuidString)", isDirectory: true)
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            let input = directory.appendingPathComponent("flattened.png")
            try model.exportData(format: "png").write(to: input, options: [.atomic])
            exporting = true
            let fields = imageEncodeFields(input: input, output: url)
            Backend.shared.call("image_encode", fields) { result in
                exporting = false
                try? FileManager.default.removeItem(at: directory)
                switch result {
                case let .success(response):
                    do {
                        let newArtifact = try Artifact(response: response)
                        store.addArtifact(newArtifact)
                        notice = "Exported \(newArtifact.url.lastPathComponent)"
                    } catch { store.report(error); notice = error.localizedDescription }
                case let .failure(error):
                    store.report(error)
                    notice = error.localizedDescription
                }
            }
        } catch {
            try? FileManager.default.removeItem(at: directory)
            store.report(error)
            notice = error.localizedDescription
        }
    }

    private var comparisonOverlay: some View {
        GeometryReader { geometry in
            ZStack {
                if let before = comparisonBefore {
                    Image(nsImage: before).resizable().interpolation(.high)
                }
                if let after = comparisonAfter {
                    Image(nsImage: after).resizable().interpolation(.high)
                        .mask(alignment: .trailing) { Rectangle().frame(width: geometry.size.width * (1 - comparisonSplit)) }
                    Rectangle().fill(Color.white).frame(width: 2)
                        .position(x: geometry.size.width * comparisonSplit, y: geometry.size.height / 2)
                    Circle().fill(NativeTheme.glass)
                        .overlay(Image(systemName: "arrow.left.and.right").foregroundStyle(NativeTheme.glassText))
                        .frame(width: 38, height: 38)
                        .position(x: geometry.size.width * comparisonSplit, y: geometry.size.height / 2)
                        .gesture(DragGesture(minimumDistance: 0).onChanged { value in
                            comparisonSplit = min(0.94, max(0.06, value.location.x / geometry.size.width))
                        })
                    HStack { Text("Before"); Spacer(); Text("After") }
                        .font(.caption.weight(.semibold)).foregroundStyle(NativeTheme.glassText)
                        .padding(12).frame(maxHeight: .infinity, alignment: .bottom)
                } else {
                    ZStack { Color.black.opacity(0.3); ProgressView("Encoding comparison…") }
                }
                Button {
                    comparisonExpanded = false
                    dismissComparison()
                } label: { Image(systemName: "xmark") }
                    .buttonStyle(CaptureButtonStyle(glass: true))
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing).padding(10)
            }
        }
    }

    private func prepareComparison() {
        guard !comparisonPending else { return }
        let generation = UUID()
        comparisonGeneration = generation
        let directory = AppStore.dataDirectory.appendingPathComponent("image-comparisons/\(UUID().uuidString)", isDirectory: true)
        do {
            let before = try model.renderedImage()
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            let input = directory.appendingPathComponent("before.png")
            let output = directory.appendingPathComponent("after.\(exportFormat)")
            try model.exportData(format: "png").write(to: input, options: [.atomic])
            comparisonBefore = before
            comparisonAfter = nil
            comparisonPending = true
            Backend.shared.call("image_encode", imageEncodeFields(input: input, output: output)) { result in
                defer { try? FileManager.default.removeItem(at: directory) }
                guard comparisonGeneration == generation else { return }
                comparisonPending = false
                switch result {
                case .success:
                    do {
                        let data = try Data(contentsOf: output)
                        guard let image = NSImage(data: data) else { throw EditorError.invalidImage }
                        comparisonAfter = image
                    } catch { store.report(error); notice = error.localizedDescription }
                case let .failure(error):
                    store.report(error)
                    notice = error.localizedDescription
                    comparisonBefore = nil
                }
            }
        } catch {
            try? FileManager.default.removeItem(at: directory)
            store.report(error)
            notice = error.localizedDescription
        }
    }

    private func dismissComparison() {
        comparisonGeneration = UUID()
        comparisonPending = false
        comparisonBefore = nil
        comparisonAfter = nil
    }

    private func scheduleComparison() {
        comparisonWork?.cancel()
        guard exportSettingsOpen, comparisonExpanded, exportQualityMode != "preserve" else {
            dismissComparison()
            return
        }
        let work = DispatchWorkItem {
            dismissComparison()
            prepareComparison()
        }
        comparisonWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.35, execute: work)
    }

    private func imageEncodeFields(input: URL, output: URL, format: String? = nil) -> [String: Any] {
        var fields: [String: Any] = ["path": input.path, "output": output.path, "format": format ?? exportFormat]
        if exportQualityMode == "compress" { fields["quality"] = Int(quality) ?? 92 }
        if exportQualityMode == "maximum" { fields["max_bytes"] = max(10_000, Int((maximumSizeMB * 1_000_000).rounded())) }
        return fields
    }

    private func sameFile(_ lhs: URL, _ rhs: URL) -> Bool {
        lhs.standardizedFileURL.resolvingSymlinksInPath() == rhs.standardizedFileURL.resolvingSymlinksInPath()
    }

    private func showExportError(_ message: String) {
        notice = message
        CaptureDialogController.shared.present(
            title: "Choose another export location",
            message: message,
            action: "OK",
            cancel: nil,
            onConfirm: {}
        )
    }

    private func discardDraft() {
        model.clearDraft()
        notice = "Draft removed. Reopen the editor to reload the original."
    }

    private func layerKind(_ layer: EditorLayer) -> String {
        switch layer.content {
        case .image: return "Image"
        case .text: return "Text"
        case let .shape(shape): return shape.rawValue.capitalized
        case .freehand: return "Freehand"
        }
    }

    private func perform(_ work: () throws -> Void) {
        do { try work() } catch { store.report(error); notice = error.localizedDescription }
    }
}

@MainActor
func makeImageEditorReferenceModel(artifact: Artifact) throws -> EditorModel {
    let source = try Data(contentsOf: artifact.url)
    return EditorModel(
        document: try EditorDocument(imageData: source),
        sourceURL: artifact.url
    )
}

@MainActor
func imageEditorReferenceView(artifact: Artifact, state: String) -> AnyView {
    do {
        // Reference states share one shipping image for visual comparison, but
        // each must start from its decoded source rather than the persisted app
        // draft identity used by a real editor window.
        let model = try makeImageEditorReferenceModel(artifact: artifact)
        switch state {
        case "shapes":
            model.selectedLayerID = nil
            return AnyView(ImageEditorSurface(
                artifact: artifact, model: model, initialTool: .shape, shapeFlyoutOpen: true
            ))
        case "properties":
            model.addShape(.rectangle, at: CGPoint(x: 320, y: 220))
            return AnyView(ImageEditorSurface(artifact: artifact, model: model))
        case "overflow":
            model.addShape(.rectangle, at: CGPoint(x: 900, y: 210))
            return AnyView(ImageEditorSurface(artifact: artifact, model: model))
        case "snap-guides":
            model.addShape(.rectangle, at: CGPoint(x: 390, y: 200))
            return AnyView(ImageEditorSurface(
                artifact: artifact, model: model,
                initialAlignmentGuides: [
                    EditorAlignmentGuide(axis: .vertical, position: 480),
                    EditorAlignmentGuide(axis: .horizontal, position: 270),
                ]
            ))
        case "viewport":
            model.addShape(.ellipse, at: CGPoint(x: 620, y: 260))
            return AnyView(ImageEditorSurface(
                artifact: artifact, model: model,
                initialZoom: 1.65,
                // At 165%, the source fixture's only warm feature (the moon at
                // document x ≈ 786) is outside the 904 pt viewport at -170 pt.
                // This still demonstrates a substantial asymmetric pan while
                // keeping the moon, blue scene, and selected ellipse visible.
                initialViewPan: CGSize(width: -500, height: -90)
            ))
        case "layers", "layer-combine":
            model.updateLayer(id: model.document.layers[0].id) { $0.locked = false }
            model.addShape(.rectangle, at: CGPoint(x: 260, y: 180))
            model.updateSelected {
                $0.blendMode = .multiply
                $0.fill = $0.color
                $0.opacity = 0.82
            }
            return AnyView(ImageEditorSurface(
                artifact: artifact,
                model: model,
                initialPropertiesAnchor: state == "layer-combine" ? "layer-combine-controls" : nil,
                onCombineControlsVisibilityChanged: state == "layer-combine" ? {
                    recordNativeReferenceCombineControlsVisible($0)
                } : nil,
                onViewportOffscreenChanged: state == "layers" ? {
                    recordNativeReferenceLayerCanvasOffscreen($0)
                } : nil
            ))
        case "erase":
            return AnyView(ImageEditorSurface(artifact: artifact, model: model, initialTool: .erase))
        case "wand":
            return AnyView(ImageEditorSurface(artifact: artifact, model: model, initialTool: .wand))
        default:
            return AnyView(ImageEditorSurface(artifact: artifact, model: model))
        }
    } catch {
        return AnyView(Text("Reference editor failed to load: \(error.localizedDescription)"))
    }
}
