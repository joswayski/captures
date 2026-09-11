import AppKit
import SwiftUI
import UniformTypeIdentifiers

private enum ImageEditorTool: String, CaseIterable, Identifiable {
    case select = "Select"
    case crop = "Crop"
    case text = "Text"
    case rectangle = "Rectangle"
    case ellipse = "Ellipse"
    case arrow = "Arrow"
    case pen = "Freehand"
    case erase = "Erase"
    case restore = "Restore"

    var id: String { rawValue }
    var symbol: String {
        switch self {
        case .select: return "arrow.up.left.and.arrow.down.right"
        case .crop: return "crop"
        case .text: return "textformat"
        case .rectangle: return "rectangle"
        case .ellipse: return "circle"
        case .arrow: return "arrow.up.right"
        case .pen: return "pencil.tip"
        case .erase: return "eraser"
        case .restore: return "paintbrush"
        }
    }
}

private enum ImageResizeCorner: CaseIterable, Equatable {
    case northWest, northEast, southEast, southWest
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
    @ObservedObject private var store = AppStore.shared
    @Environment(\.colorScheme) private var colorScheme
    @State private var tool: ImageEditorTool = .select
    @State private var zoom: CGFloat = 1
    @State private var widthText = ""
    @State private var heightText = ""
    @State private var strokeWidth: CGFloat = 6
    @State private var brushSize: CGFloat = 32
    @State private var fillShapes = false
    @State private var draftPoints: [CGPoint] = []
    @State private var cropStart: CGPoint?
    @State private var cropRect: CGRect?
    @State private var dragOrigin: CGPoint?
    @State private var dragLayerFrame: EditorRect?
    @State private var resizeLayerFrame: EditorRect?
    @State private var exportFormat = "png"
    @State private var quality = 0.92
    @State private var exportQualityMode = "preserve"
    @State private var maximumSizeMB = 10.0
    @State private var exporting = false
    @State private var comparisonPending = false
    @State private var comparisonBefore: NSImage?
    @State private var comparisonAfter: NSImage?
    @State private var comparisonSplit: CGFloat = 0.5
    @State private var exportSettingsOpen = false
    @State private var notice = ""

    var body: some View {
        VStack(spacing: 0) {
            header
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
                canvasViewport
                inspector
            }
            exportFooter
        }
        .background(NativeTheme.canvas(colorScheme))
        .foregroundStyle(NativeTheme.text(colorScheme))
        .onAppear {
            syncDimensions()
            let preferred = store.settings.screenshotFormat.lowercased() == "jpg" ? "jpeg" : store.settings.screenshotFormat.lowercased()
            if ["png", "jpeg", "webp"].contains(preferred) { exportFormat = preferred }
        }
        .onChange(of: model.document.width) { _ in syncDimensions() }
        .onChange(of: model.document.height) { _ in syncDimensions() }
        .animation(NativeTheme.motion, value: tool)
        .animation(NativeTheme.standard, value: exportSettingsOpen)
    }

    private var header: some View {
        HStack(spacing: NativeTheme.metric("s-4")) {
            HStack(spacing: 6) {
                Text("Canvas").font(.caption).foregroundStyle(NativeTheme.muted(colorScheme))
                TextField("W", text: $widthText).frame(width: 64).textFieldStyle(.roundedBorder)
                Text("×")
                TextField("H", text: $heightText).frame(width: 64).textFieldStyle(.roundedBorder)
                Button("Resize") { applyCanvasSize() }.buttonStyle(CaptureButtonStyle())
                Button("Trim edges") { perform { try model.trimTransparentEdges() } }
                    .buttonStyle(CaptureButtonStyle())
            }
            Spacer()
            Button { model.undo() } label: { Image(systemName: "arrow.uturn.backward") }
                .buttonStyle(CaptureButtonStyle()).disabled(!model.canUndo).help("Undo")
            Button { model.redo() } label: { Image(systemName: "arrow.uturn.forward") }
                .buttonStyle(CaptureButtonStyle()).disabled(!model.canRedo).help("Redo")
            HStack(spacing: 6) {
                Button("Fit") { zoom = 1 }.buttonStyle(CaptureButtonStyle())
                Button { zoom = max(0.05, zoom - 0.1) } label: { Image(systemName: "minus") }
                    .buttonStyle(CaptureButtonStyle()).help("Zoom out")
                Slider(value: $zoom, in: 0.05...4).frame(width: 90)
                Button { zoom = min(4, zoom + 0.1) } label: { Image(systemName: "plus") }
                    .buttonStyle(CaptureButtonStyle()).help("Zoom in")
                Text("\(Int(zoom * 100))%").font(.system(.caption, design: .monospaced)).frame(width: 46)
            }
            Button("Add images") { chooseImage() }
                .buttonStyle(CaptureButtonStyle()).keyboardShortcut("i", modifiers: [.command])
        }
        .padding(.horizontal, NativeTheme.metric("s-5")).frame(minHeight: 52)
        .background(NativeTheme.raised(colorScheme))
        .overlay(alignment: .bottom) { Divider() }
    }

    private var toolRail: some View {
        VStack(spacing: 4) {
            ForEach(ImageEditorTool.allCases) { item in
                Button { tool = item } label: {
                    Image(systemName: item.symbol).frame(width: 28, height: 28)
                }
                .buttonStyle(CaptureButtonStyle(primary: tool == item))
                .help(item.rawValue)
            }
            Spacer()
        }
        .padding(NativeTheme.metric("s-3")).frame(minWidth: 56, maxWidth: 56, maxHeight: .infinity)
        .background(NativeTheme.raised(colorScheme))
        .overlay(alignment: .trailing) { Divider() }
    }

    private var canvasViewport: some View {
        GeometryReader { geometry in
            ScrollView([.horizontal, .vertical]) {
                ZStack {
                    checkerboard
                    renderedCanvas
                    selectionOverlay
                    cropOverlay
                    if !draftPoints.isEmpty { freehandPreview }
                    if comparisonBefore != nil { comparisonOverlay }
                }
                .frame(width: CGFloat(model.document.width) * zoom, height: CGFloat(model.document.height) * zoom)
                .contentShape(Rectangle())
                .gesture(canvasGesture)
                .shadow(color: .black.opacity(0.22), radius: 18, y: 8)
                .padding(50)
                .frame(minWidth: geometry.size.width, minHeight: geometry.size.height)
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
                    .allowsHitTesting(false)
                ForEach(Array(ImageResizeCorner.allCases.enumerated()), id: \.offset) { _, corner in
                    selectionResizeHandle(corner, frame: frame)
                }
            }
        }
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
                        if let index = model.selectedLayerIndex { dragLayerFrame = model.document.layers[index].frame }
                    }
                case .crop:
                    cropStart = cropStart ?? start
                    cropRect = boundedRect(from: cropStart!, to: point)
                case .pen:
                    if draftPoints.isEmpty { draftPoints = [start] }
                    draftPoints.append(point)
                case .erase, .restore:
                    perform { try model.erase(at: point, radius: brushSize / 2, restore: tool == .restore) }
                default: break
                }
            }
            .onEnded { value in
                let point = CGPoint(x: value.location.x / zoom, y: value.location.y / zoom)
                let start = CGPoint(x: value.startLocation.x / zoom, y: value.startLocation.y / zoom)
                switch tool {
                case .select:
                    if let origin = dragOrigin, let initial = dragLayerFrame, let index = model.selectedLayerIndex,
                       !model.document.layers[index].locked {
                        let delta = CGSize(width: point.x - origin.x, height: point.y - origin.y)
                        model.updateSelected { $0.frame = EditorRect(x: initial.x + delta.width, y: initial.y + delta.height, width: initial.width, height: initial.height) }
                    }
                case .crop:
                    cropRect = boundedRect(from: cropStart ?? start, to: point)
                    cropStart = nil
                case .text:
                    model.addText("Text", at: point)
                case .rectangle: model.addShape(.rectangle, at: point)
                case .ellipse: model.addShape(.ellipse, at: point)
                case .arrow: model.addShape(.arrow, at: point)
                case .pen:
                    model.addStroke(draftPoints)
                    draftPoints.removeAll()
                case .erase, .restore: break
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
            ScrollView { properties.padding(NativeTheme.metric("s-5")) }
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
        .onTapGesture { model.selectedLayerID = layer.id }
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
                HStack {
                    Text("Opacity")
                    Slider(value: Binding(get: { layer.opacity }, set: { value in model.updateSelected { $0.opacity = value } }), in: 0...1)
                    Text("\(Int(layer.opacity * 100))%").monospacedDigit().frame(width: 42)
                }
                HStack {
                    Text("Rotation")
                    Slider(value: Binding(get: { layer.rotation * 180 / .pi }, set: { value in model.updateSelected { $0.rotation = value * .pi / 180 } }), in: -180...180)
                }
                if case .shape = layer.content {
                    Toggle("Fill shape", isOn: Binding(get: { layer.fill != nil }, set: { enabled in model.updateSelected { $0.fill = enabled ? $0.color : nil } }))
                }
                if case .image = layer.content, tool == .erase || tool == .restore {
                    Text("Background brush").font(.headline)
                    Slider(value: $brushSize, in: 4...160)
                    Text("Drag over the image to \(tool == .restore ? "restore original pixels" : "make pixels transparent").")
                        .font(.caption).foregroundStyle(NativeTheme.muted(colorScheme))
                }
                HStack {
                    Button { model.reorderSelected(by: 1) } label: { Image(systemName: "arrow.up") }.buttonStyle(CaptureButtonStyle()).help("Move layer up")
                    Button { model.reorderSelected(by: -1) } label: { Image(systemName: "arrow.down") }.buttonStyle(CaptureButtonStyle()).help("Move layer down")
                    Button { model.duplicateSelected() } label: { Image(systemName: "plus.square.on.square") }.buttonStyle(CaptureButtonStyle()).help("Duplicate layer")
                    Button { model.deleteSelected() } label: { Image(systemName: "trash") }.buttonStyle(CaptureButtonStyle(destructive: true)).disabled(layer.locked).help("Delete layer")
                }
            }.padding(.top, 14)
        } else if tool == .crop {
            SectionTitle("Crop", subtitle: "Drag on the canvas")
            Button("Apply crop") {
                if let cropRect { model.crop(to: cropRect); self.cropRect = nil; tool = .select }
            }.buttonStyle(CaptureButtonStyle(primary: true)).disabled(cropRect == nil).padding(.top, 12)
        } else {
            SectionTitle("Properties", subtitle: "Select a layer to edit it")
            VStack(alignment: .leading, spacing: 10) {
                Button("Rotate left") { model.rotateCanvas(clockwise: false) }.buttonStyle(CaptureButtonStyle())
                Button("Rotate right") { model.rotateCanvas(clockwise: true) }.buttonStyle(CaptureButtonStyle())
            }.padding(.top, 12)
        }
    }

    private var exportFooter: some View {
        VStack(spacing: 10) {
            if exportSettingsOpen {
                HStack(spacing: 18) {
                    Picker("Format", selection: $exportFormat) {
                        Text("PNG").tag("png"); Text("JPEG").tag("jpeg"); Text("WebP").tag("webp")
                    }.frame(width: 180)
                    Picker("Save quality", selection: $exportQualityMode) {
                        Text("Preserve quality").tag("preserve")
                        Text("Compress").tag("compress")
                        Text("Maximum file size").tag("maximum")
                    }.frame(width: 220)
                    if exportQualityMode == "compress" {
                        HStack { Text("Quality"); Slider(value: $quality, in: 0.55...1).frame(width: 160); Text("\(Int(quality * 100))%") }
                    }
                    if exportQualityMode == "maximum" {
                        HStack { Text("Maximum"); TextField("MB", value: $maximumSizeMB, format: .number).frame(width: 70); Text("MB") }
                    }
                    if exportFormat == "png", exportQualityMode != "preserve" {
                        Text(exportQualityMode == "compress"
                             ? "PNG remains lossless; quality does not currently change its pixels."
                             : "PNG is not resized to meet a limit; export fails if the lossless file is too large.")
                            .font(.caption)
                            .foregroundStyle(NativeTheme.muted(colorScheme))
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: 260, alignment: .leading)
                    }
                    Spacer()
                    Button(comparisonPending ? "Encoding…" : "Compare") { prepareComparison() }
                        .buttonStyle(CaptureButtonStyle()).disabled(comparisonPending)
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
                Spacer()
                if !notice.isEmpty { Text(notice).font(.callout).foregroundStyle(NativeTheme.muted(colorScheme)) }
                Button("Copy image") { copyImage() }.buttonStyle(CaptureButtonStyle())
                Button(exporting ? "Exporting…" : "Export…") { exportAsNewFile() }
                    .buttonStyle(CaptureButtonStyle()).disabled(exporting)
                Button("Save") { saveSource() }.buttonStyle(CaptureButtonStyle(primary: true))
                    .help("Explicitly replace the source image with the edited image")
            }
        }
        .padding(NativeTheme.metric("s-4")).background(NativeTheme.raised(colorScheme)).overlay(alignment: .top) { Divider() }
    }

    private func selectLayer(at point: CGPoint) {
        model.selectedLayerID = model.document.layers.reversed().first(where: { $0.visible && $0.frame.cgRect.contains(point) })?.id
    }

    private func selectionResizeHandle(_ corner: ImageResizeCorner, frame: CGRect) -> some View {
        let point: CGPoint
        switch corner {
        case .northWest: point = CGPoint(x: frame.minX, y: frame.minY)
        case .northEast: point = CGPoint(x: frame.maxX, y: frame.minY)
        case .southEast: point = CGPoint(x: frame.maxX, y: frame.maxY)
        case .southWest: point = CGPoint(x: frame.minX, y: frame.maxY)
        }
        return Circle().fill(Color.white).overlay(Circle().stroke(NativeTheme.accent, lineWidth: 2))
            .frame(width: 12, height: 12).position(x: point.x * zoom, y: point.y * zoom)
            .gesture(DragGesture(minimumDistance: 0).onChanged { value in
                if resizeLayerFrame == nil, let index = model.selectedLayerIndex {
                    resizeLayerFrame = model.document.layers[index].frame
                    model.beginInteractiveEdit()
                }
                guard let initial = resizeLayerFrame else { return }
                let dx = value.translation.width / zoom
                let dy = value.translation.height / zoom
                var left = initial.x
                var top = initial.y
                var right = initial.x + initial.width
                var bottom = initial.y + initial.height
                if corner == .northWest || corner == .southWest { left = min(right - 4, left + dx) }
                if corner == .northEast || corner == .southEast { right = max(left + 4, right + dx) }
                if corner == .northWest || corner == .northEast { top = min(bottom - 4, top + dy) }
                if corner == .southWest || corner == .southEast { bottom = max(top + 4, bottom + dy) }
                model.updateSelectedLive { $0.frame = EditorRect(x: left, y: top, width: right - left, height: bottom - top) }
            }.onEnded { _ in resizeLayerFrame = nil; model.endInteractiveEdit() })
            .help("Resize layer")
    }

    private func boundedRect(from start: CGPoint, to end: CGPoint) -> CGRect {
        CGRect(x: min(start.x, end.x), y: min(start.y, end.y), width: abs(end.x - start.x), height: abs(end.y - start.y))
            .intersection(CGRect(x: 0, y: 0, width: CGFloat(model.document.width), height: CGFloat(model.document.height)))
    }

    private func syncDimensions() {
        widthText = String(model.document.width)
        heightText = String(model.document.height)
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

    private func copyImage() {
        perform {
            let image = try model.renderedImage()
            NSPasteboard.general.clearContents()
            NSPasteboard.general.writeObjects([image])
            notice = "Copied"
        }
    }

    private func saveSource() {
        let alert = NSAlert()
        alert.messageText = "Replace the original screenshot?"
        alert.informativeText = "This is the only action that overwrites the source file. Export creates a separate file."
        alert.addButton(withTitle: "Replace Original")
        alert.addButton(withTitle: "Cancel")
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let sourceExtension = model.sourceURL.pathExtension.lowercased()
        let sourceFormat = sourceExtension == "jpg" ? "jpeg" : sourceExtension
        guard ["png", "jpeg", "webp"].contains(sourceFormat) else {
            showExportError("The source format \(sourceExtension.uppercased()) cannot be replaced by the native editor. Export a PNG, JPEG, or WebP copy instead.")
            return
        }
        do {
            let directory = AppStore.dataDirectory.appendingPathComponent("image-editor-work/\(UUID().uuidString)", isDirectory: true)
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
            store.report(error)
            notice = error.localizedDescription
        }
    }

    private func exportAsNewFile() {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "\(artifact.url.deletingPathExtension().lastPathComponent) edited.\(exportFormat)"
        panel.canCreateDirectories = true
        guard panel.runModal() == .OK, let url = panel.url else { return }
        guard !sameFile(url, model.sourceURL) else {
            showExportError("Export cannot replace the source image. Use Save when you explicitly want to replace the original.")
            return
        }
        if FileManager.default.fileExists(atPath: url.path) {
            showExportError("Export only creates new files. Choose a filename that does not already exist.")
            return
        }
        do {
            let directory = AppStore.dataDirectory.appendingPathComponent("image-editor-work/\(UUID().uuidString)", isDirectory: true)
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
                Button { comparisonBefore = nil; comparisonAfter = nil } label: { Image(systemName: "xmark") }
                    .buttonStyle(CaptureButtonStyle(glass: true))
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing).padding(10)
            }
        }
    }

    private func prepareComparison() {
        guard !comparisonPending else { return }
        do {
            let before = try model.renderedImage()
            let directory = AppStore.dataDirectory.appendingPathComponent("image-comparisons/\(UUID().uuidString)", isDirectory: true)
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            let input = directory.appendingPathComponent("before.png")
            let output = directory.appendingPathComponent("after.\(exportFormat)")
            try model.exportData(format: "png").write(to: input, options: [.atomic])
            comparisonBefore = before
            comparisonAfter = nil
            comparisonPending = true
            Backend.shared.call("image_encode", imageEncodeFields(input: input, output: output)) { result in
                comparisonPending = false
                defer { try? FileManager.default.removeItem(at: directory) }
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
            store.report(error)
            notice = error.localizedDescription
        }
    }

    private func imageEncodeFields(input: URL, output: URL, format: String? = nil) -> [String: Any] {
        var fields: [String: Any] = ["path": input.path, "output": output.path, "format": format ?? exportFormat]
        if exportQualityMode == "compress" { fields["quality"] = Int((quality * 100).rounded()) }
        if exportQualityMode == "maximum" { fields["max_bytes"] = max(10_000, Int((maximumSizeMB * 1_000_000).rounded())) }
        return fields
    }

    private func sameFile(_ lhs: URL, _ rhs: URL) -> Bool {
        lhs.standardizedFileURL.resolvingSymlinksInPath() == rhs.standardizedFileURL.resolvingSymlinksInPath()
    }

    private func showExportError(_ message: String) {
        let alert = NSAlert()
        alert.messageText = "Choose another export location"
        alert.informativeText = message
        alert.runModal()
        notice = message
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
