import AppKit
import Combine
import CoreGraphics
import CoreImage
import CryptoKit
import Foundation

struct EditorPoint: Codable, Equatable, Hashable {
    var x: CGFloat
    var y: CGFloat

    var cgPoint: CGPoint { CGPoint(x: x, y: y) }
}

struct EditorRect: Codable, Equatable, Hashable {
    var x: CGFloat
    var y: CGFloat
    var width: CGFloat
    var height: CGFloat

    init(x: CGFloat, y: CGFloat, width: CGFloat, height: CGFloat) {
        self.x = x
        self.y = y
        self.width = width
        self.height = height
    }

    init(_ rect: CGRect) {
        self.init(x: rect.origin.x, y: rect.origin.y, width: rect.width, height: rect.height)
    }

    var cgRect: CGRect { CGRect(x: x, y: y, width: width, height: height) }
}

struct EditorColor: Codable, Equatable, Hashable {
    var red: CGFloat
    var green: CGFloat
    var blue: CGFloat
    var alpha: CGFloat

    static let signal = EditorColor(red: 0.96, green: 0.22, blue: 0.25, alpha: 1)
    static let white = EditorColor(red: 1, green: 1, blue: 1, alpha: 1)

    var nsColor: NSColor {
        NSColor(srgbRed: red, green: green, blue: blue, alpha: alpha)
    }

    init(red: CGFloat, green: CGFloat, blue: CGFloat, alpha: CGFloat = 1) {
        self.red = red
        self.green = green
        self.blue = blue
        self.alpha = alpha
    }

    init(_ color: NSColor) {
        let converted = color.usingColorSpace(.sRGB) ?? color
        red = converted.redComponent
        green = converted.greenComponent
        blue = converted.blueComponent
        alpha = converted.alphaComponent
    }
}

enum EditorShape: String, Codable, CaseIterable {
    case rectangle, ellipse, line, triangle, diamond, star, arrow
}

enum EditorLayerContent: Codable, Equatable {
    case image(Data, original: Data)
    case text(String)
    case shape(EditorShape)
    case freehand([EditorPoint])

    private enum CodingKeys: String, CodingKey { case type, data, original, text, shape, points }
    private enum Kind: String, Codable { case image, text, shape, freehand }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        switch try values.decode(Kind.self, forKey: .type) {
        case .image:
            self = .image(
                try values.decode(Data.self, forKey: .data),
                original: try values.decode(Data.self, forKey: .original)
            )
        case .text:
            self = .text(try values.decode(String.self, forKey: .text))
        case .shape:
            self = .shape(try values.decode(EditorShape.self, forKey: .shape))
        case .freehand:
            self = .freehand(try values.decode([EditorPoint].self, forKey: .points))
        }
    }

    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case let .image(data, original):
            try values.encode(Kind.image, forKey: .type)
            try values.encode(data, forKey: .data)
            try values.encode(original, forKey: .original)
        case let .text(text):
            try values.encode(Kind.text, forKey: .type)
            try values.encode(text, forKey: .text)
        case let .shape(shape):
            try values.encode(Kind.shape, forKey: .type)
            try values.encode(shape, forKey: .shape)
        case let .freehand(points):
            try values.encode(Kind.freehand, forKey: .type)
            try values.encode(points, forKey: .points)
        }
    }
}

struct EditorLayer: Identifiable, Codable, Equatable {
    var id: UUID
    var name: String
    var content: EditorLayerContent
    var frame: EditorRect
    var rotation: CGFloat = 0
    var visible = true
    var locked = false
    var opacity: CGFloat = 1
    var color: EditorColor = .signal
    var fill: EditorColor?
    var lineWidth: CGFloat = 6

    init(
        id: UUID = UUID(), name: String, content: EditorLayerContent, frame: EditorRect,
        rotation: CGFloat = 0, visible: Bool = true, locked: Bool = false,
        opacity: CGFloat = 1, color: EditorColor = .signal, fill: EditorColor? = nil,
        lineWidth: CGFloat = 6
    ) {
        self.id = id
        self.name = name
        self.content = content
        self.frame = frame
        self.rotation = rotation
        self.visible = visible
        self.locked = locked
        self.opacity = opacity
        self.color = color
        self.fill = fill
        self.lineWidth = lineWidth
    }
}

struct EditorDocument: Codable, Equatable {
    var width: Int
    var height: Int
    var layers: [EditorLayer]

    init(imageData: Data) throws {
        guard let image = NSImage(data: imageData), image.size.width > 0, image.size.height > 0 else {
            throw EditorError.invalidImage
        }
        let pixels = image.pixelSize
        width = pixels.width
        height = pixels.height
        layers = [EditorLayer(
            name: "Original screenshot",
            content: .image(imageData, original: imageData),
            frame: EditorRect(x: 0, y: 0, width: CGFloat(width), height: CGFloat(height)),
            locked: true
        )]
    }

    init(width: Int, height: Int, layers: [EditorLayer] = []) {
        self.width = max(1, width)
        self.height = max(1, height)
        self.layers = layers
    }
}

enum EditorError: LocalizedError, Equatable {
    case invalidImage
    case cannotRender
    case unsupportedFormat(String)
    case existingFile

    var errorDescription: String? {
        switch self {
        case .invalidImage: return "The image could not be read."
        case .cannotRender: return "The edited image could not be rendered."
        case let .unsupportedFormat(format): return "The native editor cannot export \(format)."
        case .existingFile: return "A file already exists at that location."
        }
    }
}

@MainActor
final class EditorModel: ObservableObject {
    @Published private(set) var document: EditorDocument
    @Published var selectedLayerID: UUID?
    @Published private(set) var dirty = false
    @Published private(set) var restoredDraft = false

    let sourceURL: URL
    private var undoStack: [EditorDocument] = []
    private var redoStack: [EditorDocument] = []
    private var interactiveCheckpoint: EditorDocument?
    private let draftURL: URL

    var canUndo: Bool { !undoStack.isEmpty }
    var canRedo: Bool { !redoStack.isEmpty }
    var selectedLayerIndex: Int? { document.layers.firstIndex { $0.id == selectedLayerID } }

    init(artifact: Artifact) throws {
        sourceURL = artifact.url
        let sourceData = try Data(contentsOf: sourceURL)
        let clean = try EditorDocument(imageData: sourceData)
        let directory = AppStore.dataDirectory.appendingPathComponent("image-editor-drafts", isDirectory: true)
        let identity = sourceURL.standardizedFileURL.resolvingSymlinksInPath().path
        let key = SHA256.hash(data: Data(identity.utf8)).map { String(format: "%02x", $0) }.joined()
        draftURL = directory.appendingPathComponent("\(key).json")
        if let data = try? Data(contentsOf: draftURL), let draft = try? JSONDecoder().decode(EditorDocument.self, from: data) {
            document = draft
            restoredDraft = true
            dirty = true
        } else {
            document = clean
        }
    }

    init(document: EditorDocument, sourceURL: URL) {
        self.document = document
        self.sourceURL = sourceURL
        draftURL = AppStore.dataDirectory
            .appendingPathComponent("image-editor-drafts", isDirectory: true)
            .appendingPathComponent("test-\(UUID().uuidString).json")
    }

    func mutate(_ body: (inout EditorDocument) -> Void) {
        checkpoint()
        body(&document)
        changed()
    }

    func undo() {
        guard let previous = undoStack.popLast() else { return }
        redoStack.append(document)
        document = previous
        selectedLayerID = nil
        changed()
    }

    func redo() {
        guard let next = redoStack.popLast() else { return }
        undoStack.append(document)
        document = next
        selectedLayerID = nil
        changed()
    }

    func addText(_ text: String, at point: CGPoint? = nil) {
        let frame = EditorRect(x: point?.x ?? 48, y: point?.y ?? 48, width: 260, height: 52)
        let layer = EditorLayer(name: "Text", content: .text(text), frame: frame, color: .white)
        mutate { $0.layers.append(layer) }
        selectedLayerID = layer.id
    }

    func addShape(_ shape: EditorShape, at point: CGPoint? = nil) {
        let size: CGSize = shape == .line || shape == .arrow ? CGSize(width: 220, height: 90) : CGSize(width: 180, height: 140)
        let layer = EditorLayer(
            name: shape.rawValue.capitalized, content: .shape(shape),
            frame: EditorRect(x: point?.x ?? 64, y: point?.y ?? 64, width: size.width, height: size.height)
        )
        mutate { $0.layers.append(layer) }
        selectedLayerID = layer.id
    }

    func addStroke(_ points: [CGPoint]) {
        guard points.count > 1 else { return }
        let bounds = points.reduce(CGRect.null) { $0.union(CGRect(origin: $1, size: .zero)) }.insetBy(dx: -8, dy: -8)
        let local = points.map { EditorPoint(x: $0.x - bounds.minX, y: $0.y - bounds.minY) }
        let layer = EditorLayer(name: "Freehand", content: .freehand(local), frame: EditorRect(bounds))
        mutate { $0.layers.append(layer) }
        selectedLayerID = layer.id
    }

    func importImage(from url: URL) throws {
        let data = try Data(contentsOf: url)
        guard let image = NSImage(data: data) else { throw EditorError.invalidImage }
        let pixels = image.pixelSize
        let scale = min(1, CGFloat(document.width) * 0.7 / CGFloat(pixels.width), CGFloat(document.height) * 0.7 / CGFloat(pixels.height))
        let width = CGFloat(pixels.width) * scale
        let height = CGFloat(pixels.height) * scale
        let layer = EditorLayer(
            name: url.deletingPathExtension().lastPathComponent,
            content: .image(data, original: data),
            frame: EditorRect(x: (CGFloat(document.width) - width) / 2, y: (CGFloat(document.height) - height) / 2, width: width, height: height)
        )
        mutate { $0.layers.append(layer) }
        selectedLayerID = layer.id
    }

    func deleteSelected() {
        guard let index = selectedLayerIndex, !document.layers[index].locked else { return }
        mutate { $0.layers.remove(at: index) }
        selectedLayerID = nil
    }

    func duplicateSelected() {
        guard let index = selectedLayerIndex else { return }
        var layer = document.layers[index]
        layer.id = UUID()
        layer.name += " copy"
        layer.locked = false
        layer.frame.x += 12
        layer.frame.y += 12
        mutate { $0.layers.append(layer) }
        selectedLayerID = layer.id
    }

    func moveSelected(by delta: CGSize) {
        guard let index = selectedLayerIndex, !document.layers[index].locked else { return }
        mutate {
            $0.layers[index].frame.x += delta.width
            $0.layers[index].frame.y += delta.height
        }
    }

    func updateSelected(_ body: (inout EditorLayer) -> Void) {
        guard let index = selectedLayerIndex, !document.layers[index].locked else { return }
        mutate { body(&$0.layers[index]) }
    }

    func beginInteractiveEdit() {
        if interactiveCheckpoint == nil { interactiveCheckpoint = document }
    }

    func updateSelectedLive(_ body: (inout EditorLayer) -> Void) {
        guard let index = selectedLayerIndex, !document.layers[index].locked else { return }
        body(&document.layers[index])
        dirty = true
    }

    func endInteractiveEdit() {
        guard let before = interactiveCheckpoint else { return }
        interactiveCheckpoint = nil
        guard before != document else { return }
        undoStack.append(before)
        if undoStack.count > 50 { undoStack.removeFirst() }
        redoStack.removeAll()
        saveDraft()
    }

    func updateLayer(id: UUID, _ body: (inout EditorLayer) -> Void) {
        guard let index = document.layers.firstIndex(where: { $0.id == id }) else { return }
        mutate { body(&$0.layers[index]) }
    }

    func reorderSelected(by offset: Int) {
        guard let index = selectedLayerIndex, !document.layers[index].locked else { return }
        let destination = min(max(0, index + offset), document.layers.count - 1)
        guard destination != index, !document.layers[destination].locked else { return }
        mutate {
            let layer = $0.layers.remove(at: index)
            $0.layers.insert(layer, at: destination)
        }
    }

    func crop(to rect: CGRect) {
        let canvas = CGRect(x: 0, y: 0, width: CGFloat(document.width), height: CGFloat(document.height))
        let crop = rect.standardized.integral.intersection(canvas)
        guard crop.width >= 1, crop.height >= 1 else { return }
        mutate {
            $0.width = Int(crop.width)
            $0.height = Int(crop.height)
            for index in $0.layers.indices {
                $0.layers[index].frame.x -= crop.minX
                $0.layers[index].frame.y -= crop.minY
            }
        }
    }

    func resizeCanvas(width: Int, height: Int) {
        mutate {
            $0.width = min(max(width, 1), 16_384)
            $0.height = min(max(height, 1), 16_384)
        }
    }

    func rotateCanvas(clockwise: Bool) {
        let oldWidth = CGFloat(document.width)
        let oldHeight = CGFloat(document.height)
        mutate {
            let width = $0.width
            $0.width = $0.height
            $0.height = width
            for index in $0.layers.indices {
                let frame = $0.layers[index].frame
                let center = CGPoint(x: frame.cgRect.midX, y: frame.cgRect.midY)
                let rotatedCenter: CGPoint
                if clockwise {
                    rotatedCenter = CGPoint(x: oldHeight - center.y, y: center.x)
                    $0.layers[index].rotation += .pi / 2
                } else {
                    rotatedCenter = CGPoint(x: center.y, y: oldWidth - center.x)
                    $0.layers[index].rotation -= .pi / 2
                }
                $0.layers[index].frame = EditorRect(
                    x: rotatedCenter.x - frame.width / 2,
                    y: rotatedCenter.y - frame.height / 2,
                    width: frame.width,
                    height: frame.height
                )
            }
        }
    }

    func trimTransparentEdges() throws {
        guard let image = try renderedImage().cgImage(forProposedRect: nil, context: nil, hints: nil),
              let data = image.dataProvider?.data, let bytes = CFDataGetBytePtr(data) else { throw EditorError.cannotRender }
        let stride = image.bytesPerRow
        var bounds = CGRect.null
        for y in 0..<image.height {
            for x in 0..<image.width where bytes[y * stride + x * 4 + 3] != 0 {
                bounds = bounds.union(CGRect(x: x, y: y, width: 1, height: 1))
            }
        }
        if !bounds.isNull { crop(to: bounds) }
    }

    func erase(at documentPoint: CGPoint, radius: CGFloat, restore: Bool) throws {
        guard let index = selectedLayerIndex else { return }
        let layer = document.layers[index]
        guard !layer.locked else { return }
        let frame = layer.frame.cgRect
        let center = CGPoint(x: frame.midX, y: frame.midY)
        let dx = documentPoint.x - center.x
        let dy = documentPoint.y - center.y
        let cosine = cos(-layer.rotation)
        let sine = sin(-layer.rotation)
        let localDocumentPoint = CGPoint(
            x: center.x + dx * cosine - dy * sine,
            y: center.y + dx * sine + dy * cosine
        )
        guard frame.contains(localDocumentPoint), case let .image(currentData, originalData) = layer.content,
              let current = NSImage(data: currentData), let original = NSImage(data: originalData),
              let currentCG = current.cgImage(forProposedRect: nil, context: nil, hints: nil),
              let originalCG = original.cgImage(forProposedRect: nil, context: nil, hints: nil) else { return }
        let width = currentCG.width
        let height = currentCG.height
        let colorSpace = CGColorSpaceCreateDeviceRGB()
        var pixels = [UInt8](repeating: 0, count: width * height * 4)
        let context = CGContext(data: &pixels, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4, space: colorSpace, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        context.draw(currentCG, in: CGRect(x: 0, y: 0, width: width, height: height))
        var originalPixels = [UInt8](repeating: 0, count: pixels.count)
        let originalContext = CGContext(data: &originalPixels, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4, space: colorSpace, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        originalContext.draw(originalCG, in: CGRect(x: 0, y: 0, width: width, height: height))
        let centerX = (localDocumentPoint.x - frame.minX) / frame.width * CGFloat(width)
        let centerY = (localDocumentPoint.y - frame.minY) / frame.height * CGFloat(height)
        let pixelRadius = max(1, radius * CGFloat(width) / frame.width)
        for y in max(0, Int(centerY - pixelRadius))..<min(height, Int(centerY + pixelRadius + 1)) {
            for x in max(0, Int(centerX - pixelRadius))..<min(width, Int(centerX + pixelRadius + 1))
                where hypot(CGFloat(x) - centerX, CGFloat(y) - centerY) <= pixelRadius {
                let offset = (y * width + x) * 4
                if restore {
                    pixels[offset..<(offset + 4)] = originalPixels[offset..<(offset + 4)]
                } else {
                    pixels[offset + 3] = 0
                }
            }
        }
        guard let editedCG = context.makeImage() else { throw EditorError.cannotRender }
        let rep = NSBitmapImageRep(cgImage: editedCG)
        guard let png = rep.representation(using: .png, properties: [:]) else { throw EditorError.cannotRender }
        updateSelected { $0.content = .image(png, original: originalData) }
    }

    func renderedImage() throws -> NSImage {
        guard document.width > 0, document.height > 0 else { throw EditorError.cannotRender }
        guard let colorSpace = CGColorSpace(name: CGColorSpace.sRGB),
              let context = CGContext(
                data: nil,
                width: document.width,
                height: document.height,
                bitsPerComponent: 8,
                bytesPerRow: 0,
                space: colorSpace,
                bitmapInfo: CGBitmapInfo.byteOrder32Big.rawValue | CGImageAlphaInfo.premultipliedLast.rawValue
              ) else { throw EditorError.cannotRender }
        context.setBlendMode(.copy)
        context.setFillColor(NSColor.clear.cgColor)
        context.fill(CGRect(x: 0, y: 0, width: CGFloat(document.width), height: CGFloat(document.height)))
        context.setBlendMode(.normal)
        context.translateBy(x: 0, y: CGFloat(document.height))
        context.scaleBy(x: 1, y: -1)
        let graphicsContext = NSGraphicsContext(cgContext: context, flipped: true)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = graphicsContext
        defer { NSGraphicsContext.restoreGraphicsState() }
        for layer in document.layers where layer.visible {
            draw(layer, in: context)
        }
        graphicsContext.flushGraphics()
        guard let image = context.makeImage() else { throw EditorError.cannotRender }
        return NSImage(cgImage: image, size: NSSize(width: CGFloat(document.width), height: CGFloat(document.height)))
    }

    func exportData(format: String, quality: CGFloat = 0.92) throws -> Data {
        let image = try renderedImage()
        guard let cgImage = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else { throw EditorError.cannotRender }
        let rep = NSBitmapImageRep(cgImage: cgImage)
        switch format.lowercased() {
        case "png":
            guard let data = rep.representation(using: .png, properties: [:]) else { throw EditorError.cannotRender }
            return data
        case "jpg", "jpeg":
            guard let data = rep.representation(using: .jpeg, properties: [.compressionFactor: min(max(quality, 0), 1)]) else { throw EditorError.cannotRender }
            return data
        case "tif", "tiff":
            guard let data = rep.representation(using: .tiff, properties: [:]) else { throw EditorError.cannotRender }
            return data
        default:
            throw EditorError.unsupportedFormat(format.uppercased())
        }
    }

    func writeExport(to url: URL, format: String, quality: CGFloat = 0.92) throws {
        if FileManager.default.fileExists(atPath: url.path) { throw EditorError.existingFile }
        try exportData(format: format, quality: quality).write(to: url, options: [.atomic, .withoutOverwriting])
    }

    func clearDraft() {
        try? FileManager.default.removeItem(at: draftURL)
        dirty = false
        restoredDraft = false
    }

    private func checkpoint() {
        undoStack.append(document)
        if undoStack.count > 50 { undoStack.removeFirst() }
        redoStack.removeAll()
    }

    private func changed() {
        dirty = true
        saveDraft()
    }

    private func saveDraft() {
        do {
            try NativeStorage.write(document, to: draftURL)
        } catch {
            AppStore.shared.report(error)
        }
    }

    private func draw(_ layer: EditorLayer, in context: CGContext) {
        let frame = layer.frame.cgRect
        context.saveGState()
        context.setAlpha(layer.opacity)
        context.translateBy(x: frame.midX, y: frame.midY)
        context.rotate(by: layer.rotation)
        context.translateBy(x: -frame.midX, y: -frame.midY)
        switch layer.content {
        case let .image(data, _):
            NSImage(data: data)?.draw(
                in: frame,
                from: .zero,
                operation: .sourceOver,
                fraction: 1,
                respectFlipped: true,
                hints: [.interpolation: NSImageInterpolation.high.rawValue]
            )
        case let .text(text):
            let font = NSFont.systemFont(ofSize: max(12, frame.height * 0.62), weight: .semibold)
            (text as NSString).draw(in: frame, withAttributes: [.font: font, .foregroundColor: layer.color.nsColor])
        case let .shape(shape):
            draw(shape, layer: layer, frame: frame, context: context)
        case let .freehand(points):
            guard let first = points.first else { break }
            let path = CGMutablePath()
            path.move(to: CGPoint(x: frame.minX + first.x, y: frame.minY + first.y))
            for point in points.dropFirst() { path.addLine(to: CGPoint(x: frame.minX + point.x, y: frame.minY + point.y)) }
            context.addPath(path)
            context.setStrokeColor(layer.color.nsColor.cgColor)
            context.setLineWidth(layer.lineWidth)
            context.setLineCap(.round)
            context.setLineJoin(.round)
            context.strokePath()
        }
        context.restoreGState()
    }

    private func draw(_ shape: EditorShape, layer: EditorLayer, frame: CGRect, context: CGContext) {
        let path = CGMutablePath()
        switch shape {
        case .rectangle: path.addRoundedRect(in: frame, cornerWidth: min(16, frame.width / 8), cornerHeight: min(16, frame.height / 8))
        case .ellipse: path.addEllipse(in: frame)
        case .line, .arrow:
            path.move(to: CGPoint(x: frame.minX, y: frame.midY))
            path.addLine(to: CGPoint(x: frame.maxX, y: frame.midY))
            if shape == .arrow {
                let head = max(12, layer.lineWidth * 4)
                path.move(to: CGPoint(x: frame.maxX, y: frame.midY))
                path.addLine(to: CGPoint(x: frame.maxX - head, y: frame.midY - head * 0.55))
                path.move(to: CGPoint(x: frame.maxX, y: frame.midY))
                path.addLine(to: CGPoint(x: frame.maxX - head, y: frame.midY + head * 0.55))
            }
        case .triangle:
            path.move(to: CGPoint(x: frame.midX, y: frame.minY)); path.addLine(to: CGPoint(x: frame.maxX, y: frame.maxY)); path.addLine(to: CGPoint(x: frame.minX, y: frame.maxY)); path.closeSubpath()
        case .diamond:
            path.move(to: CGPoint(x: frame.midX, y: frame.minY)); path.addLine(to: CGPoint(x: frame.maxX, y: frame.midY)); path.addLine(to: CGPoint(x: frame.midX, y: frame.maxY)); path.addLine(to: CGPoint(x: frame.minX, y: frame.midY)); path.closeSubpath()
        case .star:
            for index in 0..<10 {
                let angle = -CGFloat.pi / 2 + CGFloat(index) * .pi / 5
                let radius = (index.isMultiple(of: 2) ? 1 : 0.4) * min(frame.width, frame.height) / 2
                let point = CGPoint(x: frame.midX + cos(angle) * radius, y: frame.midY + sin(angle) * radius)
                index == 0 ? path.move(to: point) : path.addLine(to: point)
            }
            path.closeSubpath()
        }
        if let fill = layer.fill { context.addPath(path); context.setFillColor(fill.nsColor.cgColor); context.fillPath() }
        context.addPath(path)
        context.setStrokeColor(layer.color.nsColor.cgColor)
        context.setLineWidth(layer.lineWidth)
        context.setLineCap(.round)
        context.setLineJoin(.round)
        context.strokePath()
    }
}

private extension NSImage {
    var pixelSize: (width: Int, height: Int) {
        if let representation = representations.first { return (representation.pixelsWide, representation.pixelsHigh) }
        return (max(1, Int(size.width)), max(1, Int(size.height)))
    }
}
