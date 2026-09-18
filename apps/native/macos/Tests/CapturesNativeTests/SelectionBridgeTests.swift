import XCTest
import CCapturesSettings

/// Real C/Rust calls, not a Swift reimplementation of the region algorithm.
final class SelectionBridgeTests: XCTestCase {
    func testValueLayoutsReverseDragAndShiftAcrossTheRustABI() {
        XCTAssertEqual(MemoryLayout<CapturesSelectionPoint>.size, 16)
        XCTAssertEqual(MemoryLayout<CapturesSelectionBounds>.size, 16)
        XCTAssertEqual(MemoryLayout<CapturesSelectionRect>.size, 32)
        let origin = CapturesSelectionPoint(x: 303.5, y: 207.25)
        let current = CapturesSelectionPoint(x: 103.25, y: 57.75)
        let initial = CapturesSelectionRect(x: 0, y: 0, width: 0, height: 0)
        let bounds = CapturesSelectionBounds(width: 800, height: 600)
        var output = initial
        XCTAssertTrue(captures_selection_drag_v1(0, origin, current, initial, bounds, 0, false, &output))
        XCTAssertEqual(output.x, 103.25); XCTAssertEqual(output.y, 57.75)
        XCTAssertEqual(output.width, 200.25); XCTAssertEqual(output.height, 149.5)
        XCTAssertTrue(captures_selection_drag_v1(0, origin, current, initial, bounds, 16.0 / 9.0, true, &output))
        XCTAssertEqual(output.x, 103.25); XCTAssertEqual(output.y, 7)
        XCTAssertEqual(output.width, 200.25); XCTAssertEqual(output.height, 200.25)
        XCTAssertFalse(captures_selection_drag_v1(99, origin, current, initial, bounds, 0, false, &output))
        XCTAssertEqual(output.width, 200.25)
    }

    func testSettledAspectKeepsCenterAndFreeformDoesNotExpand() {
        let rect = CapturesSelectionRect(x: 21, y: 37, width: 320, height: 180)
        let bounds = CapturesSelectionBounds(width: 900, height: 700)
        var output = CapturesSelectionRect()
        XCTAssertTrue(captures_selection_constrain_v1(rect, bounds, 1, &output))
        XCTAssertEqual(output.x, 91); XCTAssertEqual(output.y, 37)
        XCTAssertEqual(output.width, 180); XCTAssertEqual(output.height, 180)
        let square = output
        XCTAssertTrue(captures_selection_constrain_v1(square, bounds, 0, &output))
        XCTAssertEqual(output.x, 91); XCTAssertEqual(output.width, 180)
    }
}
