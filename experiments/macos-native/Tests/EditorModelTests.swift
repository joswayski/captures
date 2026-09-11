import AppKit
import ImageIO
import XCTest

@MainActor
final class EditorModelTests: XCTestCase {
    func testCropTranslatesLayersAndSupportsUndo() {
        let source = EditorLayer(
            name: "Source",
            content: .shape(.rectangle),
            frame: EditorRect(x: 10, y: 20, width: 80, height: 60),
            locked: true
        )
        let annotation = EditorLayer(
            name: "Arrow",
            content: .shape(.arrow),
            frame: EditorRect(x: 40, y: 50, width: 40, height: 20)
        )
        let model = EditorModel(
            document: EditorDocument(width: 200, height: 120, layers: [source, annotation]),
            sourceURL: URL(fileURLWithPath: "/tmp/source.png")
        )

        model.crop(to: CGRect(x: 25, y: 30, width: 100, height: 70))

        XCTAssertEqual(model.document.width, 100)
        XCTAssertEqual(model.document.height, 70)
        XCTAssertEqual(model.document.layers[0].frame.x, -15)
        XCTAssertEqual(model.document.layers[0].frame.y, -10)
        XCTAssertEqual(model.document.layers[1].frame.x, 15)
        XCTAssertEqual(model.document.layers[1].frame.y, 20)

        model.undo()
        XCTAssertEqual(model.document.width, 200)
        XCTAssertEqual(model.document.layers[1].frame, annotation.frame)
        XCTAssertTrue(model.canRedo)
    }

    func testLockedLayerCannotBeDeletedOrReordered() {
        let locked = EditorLayer(
            name: "Original", content: .shape(.rectangle),
            frame: EditorRect(x: 0, y: 0, width: 100, height: 100), locked: true
        )
        let editable = EditorLayer(
            name: "Text", content: .text("hello"),
            frame: EditorRect(x: 10, y: 10, width: 30, height: 20)
        )
        let model = EditorModel(
            document: EditorDocument(width: 100, height: 100, layers: [locked, editable]),
            sourceURL: URL(fileURLWithPath: "/tmp/source.png")
        )

        model.selectedLayerID = locked.id
        model.deleteSelected()
        model.reorderSelected(by: 1)
        model.updateSelected { $0.rotation = .pi }
        model.beginInteractiveEdit()
        model.updateSelectedLive { $0.frame.width = 31 }
        model.endInteractiveEdit()

        XCTAssertEqual(model.document.layers.map(\.id), [locked.id, editable.id])
        XCTAssertEqual(model.document.layers[0], locked)
        XCTAssertFalse(model.canUndo)
    }

    func testRotateClockwisePreservesLocalFrameAndTransformsWorldBounds() {
        let layer = EditorLayer(
            name: "Layer", content: .shape(.ellipse),
            frame: EditorRect(x: 10, y: 20, width: 30, height: 40)
        )
        let model = EditorModel(
            document: EditorDocument(width: 200, height: 100, layers: [layer]),
            sourceURL: URL(fileURLWithPath: "/tmp/source.png")
        )

        model.rotateCanvas(clockwise: true)

        XCTAssertEqual(model.document.width, 100)
        XCTAssertEqual(model.document.height, 200)
        let clockwise = model.document.layers[0]
        XCTAssertEqual(clockwise.frame, EditorRect(x: 45, y: 5, width: 30, height: 40))
        XCTAssertEqual(clockwise.rotation, .pi / 2, accuracy: 0.0001)
        assertRect(worldBounds(of: clockwise), equals: CGRect(x: 40, y: 10, width: 40, height: 30))

        model.rotateCanvas(clockwise: false)
        XCTAssertEqual(model.document.width, 200)
        XCTAssertEqual(model.document.height, 100)
        XCTAssertEqual(model.document.layers[0].frame, layer.frame)
        XCTAssertEqual(model.document.layers[0].rotation, 0, accuracy: 0.0001)

        model.undo()
        XCTAssertEqual(model.document.layers[0], clockwise)
        model.undo()
        XCTAssertEqual(model.document.layers[0], layer)
    }

    func testPixelRendererUsesExactDimensionsAndRotatesAsymmetricColorsClockwise() throws {
        let red = try solidPNG(.red, rgba: [255, 0, 0, 255])
        let blue = try solidPNG(.blue, rgba: [0, 0, 255, 255])
        let layers = [
            EditorLayer(name: "top-left", content: .image(red, original: red), frame: EditorRect(x: 0, y: 0, width: 1, height: 1)),
            EditorLayer(name: "bottom-right", content: .image(blue, original: blue), frame: EditorRect(x: 3, y: 2, width: 1, height: 1)),
        ]
        let model = EditorModel(
            document: EditorDocument(width: 4, height: 3, layers: layers),
            sourceURL: URL(fileURLWithPath: "/tmp/source.png")
        )

        model.rotateCanvas(clockwise: true)
        let rendered = try XCTUnwrap(model.renderedImage().cgImage(forProposedRect: nil, context: nil, hints: nil))
        XCTAssertEqual(try pixel(rendered, x: 2, y: 0), [255, 0, 0, 255], "Rendered red before encoding")
        XCTAssertEqual(try pixel(rendered, x: 0, y: 3), [0, 0, 255, 255], "Rendered blue before encoding")
        let png = try model.exportData(format: "png")
        let image = try decodePNG(png)

        XCTAssertEqual(image.width, 3)
        XCTAssertEqual(image.height, 4)
        // Bitmap pixel coordinates start at the top left: clockwise maps
        // top-left to top-right and bottom-right to bottom-left.
        XCTAssertEqual(try pixel(image, x: 2, y: 0), [255, 0, 0, 255], "Exported red")
        XCTAssertEqual(try pixel(image, x: 0, y: 3), [0, 0, 255, 255], "Exported blue")
    }

    func testEraseInverseTransformsRotatedLayerAndRejectsLockedLayer() throws {
        let opaque = try solidPNG(.white, rgba: [255, 255, 255, 255], width: 5, height: 3)
        let frame = EditorRect(x: 10, y: 20, width: 50, height: 30)
        let layer = EditorLayer(name: "Rotated", content: .image(opaque, original: opaque), frame: frame, rotation: .pi / 2)
        let model = EditorModel(
            document: EditorDocument(width: 80, height: 80, layers: [layer]),
            sourceURL: URL(fileURLWithPath: "/tmp/source.png")
        )
        model.selectedLayerID = layer.id

        // Local point (15, 35) rotates to world point (35, 15), outside the
        // unrotated frame. Erasing it therefore proves inverse hit testing.
        try model.erase(at: CGPoint(x: 35, y: 15), radius: 5, restore: false)
        guard case let .image(edited, _) = model.document.layers[0].content else {
            return XCTFail("Expected image layer")
        }
        let editedImage = try decodePNG(edited)
        XCTAssertEqual(try pixel(editedImage, x: 0, y: 1), [0, 0, 0, 0])
        XCTAssertEqual(try pixel(editedImage, x: 4, y: 0), [255, 255, 255, 255])

        try model.erase(at: CGPoint(x: 35, y: 15), radius: 5, restore: true)
        guard case let .image(restored, _) = model.document.layers[0].content else {
            return XCTFail("Expected restored image layer")
        }
        XCTAssertEqual(try pixel(decodePNG(restored), x: 0, y: 1), [255, 255, 255, 255])

        var locked = layer
        locked.locked = true
        let lockedModel = EditorModel(
            document: EditorDocument(width: 80, height: 80, layers: [locked]),
            sourceURL: URL(fileURLWithPath: "/tmp/source.png")
        )
        lockedModel.selectedLayerID = locked.id
        try lockedModel.erase(at: CGPoint(x: 35, y: 15), radius: 5, restore: false)
        XCTAssertEqual(lockedModel.document.layers[0], locked)
        XCTAssertFalse(lockedModel.canUndo)
    }

    func testImportedFileDraftSurvivesANewArtifactIdentity() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let source = directory.appendingPathComponent("imported.png")
        let original = try solidPNG(.blue, rgba: [0, 0, 255, 255], width: 7, height: 3)
        try original.write(to: source)
        let first = try EditorModel(artifact: Artifact(path: source.path, kind: "image"))
        defer { first.clearDraft() }
        first.resizeCanvas(width: 11, height: 5)
        let reopened = try EditorModel(artifact: Artifact(path: source.path, kind: "image"))
        XCTAssertTrue(reopened.restoredDraft)
        XCTAssertEqual(reopened.document.width, 11)
        XCTAssertEqual(reopened.document.height, 5)
        XCTAssertEqual(try Data(contentsOf: source), original)
    }

    func testExportRefusesExistingFileWithoutChangingIt() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let output = directory.appendingPathComponent("existing.png")
        let sentinel = Data("do not replace".utf8)
        try sentinel.write(to: output)
        let layer = EditorLayer(
            name: "Layer", content: .shape(.rectangle),
            frame: EditorRect(x: 0, y: 0, width: 16, height: 16), fill: .signal
        )
        let model = EditorModel(
            document: EditorDocument(width: 16, height: 16, layers: [layer]),
            sourceURL: directory.appendingPathComponent("source.png")
        )

        XCTAssertThrowsError(try model.writeExport(to: output, format: "png")) { error in
            XCTAssertEqual(error as? EditorError, .existingFile)
        }
        XCTAssertEqual(try Data(contentsOf: output), sentinel)
    }

    private func worldBounds(of layer: EditorLayer) -> CGRect {
        let frame = layer.frame.cgRect
        let center = CGPoint(x: frame.midX, y: frame.midY)
        let cosine = cos(layer.rotation)
        let sine = sin(layer.rotation)
        let corners = [
            CGPoint(x: frame.minX, y: frame.minY), CGPoint(x: frame.maxX, y: frame.minY),
            CGPoint(x: frame.maxX, y: frame.maxY), CGPoint(x: frame.minX, y: frame.maxY),
        ].map { point -> CGPoint in
            let dx = point.x - center.x
            let dy = point.y - center.y
            return CGPoint(x: center.x + dx * cosine - dy * sine, y: center.y + dx * sine + dy * cosine)
        }
        let xs = corners.map(\.x)
        let ys = corners.map(\.y)
        return CGRect(x: xs.min()!, y: ys.min()!, width: xs.max()! - xs.min()!, height: ys.max()! - ys.min()!)
    }

    private func assertRect(_ actual: CGRect, equals expected: CGRect, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(actual.minX, expected.minX, accuracy: 0.0001, file: file, line: line)
        XCTAssertEqual(actual.minY, expected.minY, accuracy: 0.0001, file: file, line: line)
        XCTAssertEqual(actual.width, expected.width, accuracy: 0.0001, file: file, line: line)
        XCTAssertEqual(actual.height, expected.height, accuracy: 0.0001, file: file, line: line)
    }

    private func solidPNG(_ color: NSColor, rgba: [UInt8], width: Int = 1, height: Int = 1) throws -> Data {
        let space = try XCTUnwrap(CGColorSpace(name: CGColorSpace.sRGB))
        let context = try XCTUnwrap(CGContext(
            data: nil, width: width, height: height, bitsPerComponent: 8,
            bytesPerRow: width * 4, space: space,
            bitmapInfo: CGBitmapInfo.byteOrder32Big.rawValue | CGImageAlphaInfo.premultipliedLast.rawValue
        ))
        // Use an explicit Core Graphics color space rather than AppKit's legacy
        // device-RGB bitmap initializer/setColor path. Verify the encoded input.
        context.setFillColor(try XCTUnwrap(color.usingColorSpace(.sRGB)).cgColor)
        context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        let original = try XCTUnwrap(context.makeImage())
        XCTAssertEqual(try pixel(original, x: 0, y: 0), rgba, "Fixture before PNG encoding")
        let rep = NSBitmapImageRep(cgImage: original)
        let png = try XCTUnwrap(rep.representation(using: .png, properties: [:]))
        let decoded = try decodePNG(png)
        XCTAssertEqual(try pixel(decoded, x: 0, y: 0), rgba, "Fixture after PNG encoding")
        return png
    }

    private func decodePNG(_ data: Data) throws -> CGImage {
        let source = try XCTUnwrap(CGImageSourceCreateWithData(data as CFData, nil))
        return try XCTUnwrap(CGImageSourceCreateImageAtIndex(source, 0, nil))
    }

    /// Honor the decoded profile, then inspect literal sRGB RGBA8 bytes without
    /// NSBitmapImageRep.colorAt's additional NSColor color-space interpretation.
    private func pixel(_ image: CGImage, x: Int, y: Int) throws -> [UInt8] {
        guard (0..<image.width).contains(x), (0..<image.height).contains(y) else {
            XCTFail("Pixel (\(x), \(y)) outside \(image.width) × \(image.height)")
            return []
        }
        let space = try XCTUnwrap(CGColorSpace(name: CGColorSpace.sRGB))
        let context = try XCTUnwrap(CGContext(
            data: nil, width: image.width, height: image.height, bitsPerComponent: 8,
            bytesPerRow: image.width * 4, space: space,
            bitmapInfo: CGBitmapInfo.byteOrder32Big.rawValue | CGImageAlphaInfo.premultipliedLast.rawValue
        ))
        context.setBlendMode(.copy)
        context.interpolationQuality = .none
        context.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
        let bytes = try XCTUnwrap(context.data).assumingMemoryBound(to: UInt8.self)
        let offset = y * context.bytesPerRow + x * 4
        return withExtendedLifetime(context) { (0..<4).map { bytes[offset + $0] } }
    }
}

/// Direct swiftc entry point. Compile this file together with EditorModel.swift
/// and the files that define Artifact/AppStore, excluding the application's @main.
@main
@MainActor
struct EditorModelTestRunner {
    static func main() {
        let reporter = FailureReporter()
        XCTestObservationCenter.shared.addTestObserver(reporter)
        let suite = XCTestSuite(forTestCaseClass: EditorModelTests.self)
        suite.run()
        guard let run = suite.testRun, run.executionCount > 0 else {
            fputs("EditorModelTests: no tests executed\n", stderr)
            exit(1)
        }
        print("EditorModelTests: \(run.executionCount) executed, \(run.totalFailureCount) failures")
        exit(run.hasSucceeded && run.executionCount == suite.testCaseCount ? 0 : 1)
    }

    private final class FailureReporter: NSObject, XCTestObservation {
        func testCase(_ testCase: XCTestCase, didFailWithDescription description: String,
                      inFile filePath: String?, atLine lineNumber: Int) {
            fputs("\(testCase.name): \(filePath ?? "unknown"):\(lineNumber): \(description)\n", stderr)
        }
    }
}
