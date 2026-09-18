import XCTest
import CCapturesSettings

final class WindowSessionBridgeTests: XCTestCase {
    func testCancelledPreparationReturnsOwnedErrorWithoutScreenAccess() throws {
        var response: UnsafeMutablePointer<CChar>?
        let handle = "not-a-display".withCString {
            captures_window_prepare_v1($0, 0, true, true, 0, &response)
        }
        defer {
            captures_settings_free_v1(response)
            captures_window_free_v1(handle)
        }
        XCTAssertNil(handle)
        let pointer = try XCTUnwrap(response)
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8)) as? [String: Any])
        XCTAssertEqual(json["ok"] as? Bool, false)
        XCTAssertEqual(json["error"] as? String, "Capture cancelled")
    }

    func testWindowPixelLayoutMatchesRegionAndFailedBorrowPreservesOutput() {
        XCTAssertEqual(MemoryLayout<CapturesWindowPixels>.size, 32)
        XCTAssertEqual(MemoryLayout<CapturesWindowPixels>.offset(of: \.bytes_per_row), 24)
        var pixels = CapturesWindowPixels(data: nil, length: 77, width: 11, height: 7, bytes_per_row: 44)
        XCTAssertFalse(captures_window_pixels_v1(nil, &pixels))
        XCTAssertEqual(pixels.length, 77)
        XCTAssertEqual(pixels.width, 11)
        XCTAssertEqual(pixels.height, 7)
        XCTAssertEqual(pixels.bytes_per_row, 44)
        XCTAssertNil(pixels.data)
    }
}
