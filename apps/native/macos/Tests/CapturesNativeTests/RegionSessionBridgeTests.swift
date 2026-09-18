import XCTest
import CCapturesSettings

final class RegionSessionBridgeTests: XCTestCase {
    func testCancelledPreparationReturnsOwnedErrorWithoutScreenAccess() throws {
        var response: UnsafeMutablePointer<CChar>?
        let handle = "not-a-display".withCString {
            captures_region_prepare_v1($0, 0, true, true, &response)
        }
        defer {
            captures_settings_free_v1(response)
            captures_region_free_v1(handle)
        }
        XCTAssertNil(handle)
        let pointer = try XCTUnwrap(response)
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8)) as? [String: Any])
        XCTAssertEqual(json["ok"] as? Bool, false)
        XCTAssertEqual(json["error"] as? String, "Capture cancelled")
    }

    func testPixelLayoutAndFailedBorrowDoNotOverwriteStorage() {
        XCTAssertEqual(MemoryLayout<CapturesRegionPixels>.size, 32)
        XCTAssertEqual(MemoryLayout<CapturesRegionPixels>.offset(of: \.length), 8)
        XCTAssertEqual(MemoryLayout<CapturesRegionPixels>.offset(of: \.width), 16)
        XCTAssertEqual(MemoryLayout<CapturesRegionPixels>.offset(of: \.height), 20)
        XCTAssertEqual(MemoryLayout<CapturesRegionPixels>.offset(of: \.bytes_per_row), 24)
        var pixels = CapturesRegionPixels(data: nil, length: 48, width: 4, height: 3, bytes_per_row: 16)
        XCTAssertFalse(captures_region_pixels_v1(nil, &pixels))
        XCTAssertEqual(pixels.length, 48)
        XCTAssertEqual(pixels.width, 4)
        XCTAssertNil(pixels.data)
    }
}
