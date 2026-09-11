import AppKit
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

        XCTAssertEqual(model.document.layers.map(\.id), [locked.id, editable.id])
        XCTAssertFalse(model.canUndo)
    }

    func testRotateClockwiseSwapsCanvasAndTransformsAsymmetricFrame() {
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
        XCTAssertEqual(model.document.layers[0].frame, EditorRect(x: 40, y: 10, width: 40, height: 30))
        XCTAssertEqual(model.document.layers[0].rotation, .pi / 2, accuracy: 0.0001)
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

    nonisolated static let allTests: [(String, (EditorModelTests) -> () throws -> Void)] = [
        ("testCropTranslatesLayersAndSupportsUndo", { test in { MainActor.assumeIsolated { test.testCropTranslatesLayersAndSupportsUndo() } } }),
        ("testLockedLayerCannotBeDeletedOrReordered", { test in { MainActor.assumeIsolated { test.testLockedLayerCannotBeDeletedOrReordered() } } }),
        ("testRotateClockwiseSwapsCanvasAndTransformsAsymmetricFrame", { test in { MainActor.assumeIsolated { test.testRotateClockwiseSwapsCanvasAndTransformsAsymmetricFrame() } } }),
        ("testExportRefusesExistingFileWithoutChangingIt", { test in { try MainActor.assumeIsolated { try test.testExportRefusesExistingFileWithoutChangingIt() } } }),
    ]
}

/// Direct swiftc entry point. Compile this file together with EditorModel.swift
/// and the files that define Artifact/AppStore, excluding the application's @main.
@main
@MainActor
struct EditorModelTestRunner {
    static func main() {
        XCTMain([testCase(EditorModelTests.allTests)])
    }
}
