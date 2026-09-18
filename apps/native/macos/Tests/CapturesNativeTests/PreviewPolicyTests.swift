import XCTest
import CCapturesSettings
@testable import CapturesNative

final class PreviewPolicyTests: XCTestCase {
    func testRetinaNegativeOriginAndAllCornerMappingsThroughCABI() throws {
        let monitor = CapturesPreviewMonitor(work_x: -2400, work_y: 120,
            work_width: 2400, work_height: 1500, full_x: -2400, full_y: 40,
            full_width: 2400, full_height: 1660, scale_factor: 2)
        for (placement, x, y, anchor) in [
            ("bottom_left", -1200.0, 558.0, UInt32(0)),
            ("bottom_right", -340.0, 558.0, UInt32(0)),
            ("top_left", -1200.0, 72.0, UInt32(1)),
            ("top_right", -340.0, 72.0, UInt32(1))
        ] {
            let geometry = try XCTUnwrap(NativePreviewLayout.geometry(monitor: monitor, count: 1, placement: placement))
            XCTAssertEqual(geometry.x, x); XCTAssertEqual(geometry.y, y)
            XCTAssertEqual(geometry.anchor, anchor)
            XCTAssertEqual(geometry.width, 340); XCTAssertEqual(geometry.height, 240)
            XCTAssertEqual(geometry.padding, 28); XCTAssertEqual(geometry.card_height, 160)
            XCTAssertEqual(geometry.control_gutter, 52)
        }
        XCTAssertNil(NativePreviewLayout.geometry(monitor: monitor, count: -1, placement: "top_left"))
        XCTAssertNil(NativePreviewLayout.geometry(monitor: monitor, count: 1, placement: "invalid"))
    }

    func testVisibilityGenerationsDecodeCancellationAndSettingsThroughCABI() throws {
        let policy = NativePreviewPolicy()
        let first = try XCTUnwrap(policy.beginCapture())
        XCTAssertNil(policy.beginCapture())
        XCTAssertFalse(policy.visible(count: 1, enabled: true, includeInCaptures: false))
        XCTAssertTrue(policy.visible(count: 1, enabled: true, includeInCaptures: true))
        XCTAssertFalse(policy.visible(count: 1, enabled: false, includeInCaptures: true))
        XCTAssertFalse(policy.visible(count: 0, enabled: true, includeInCaptures: true))
        XCTAssertFalse(policy.wait(generation: first, artifact: "old\0truncated"))
        XCTAssertTrue(policy.wait(generation: first, artifact: "old"))
        let second = try XCTUnwrap(policy.beginCapture())
        XCTAssertFalse(policy.restore(generation: first))
        XCTAssertFalse(policy.ready(artifact: "old"))
        XCTAssertTrue(policy.wait(generation: second, artifact: "new"))
        policy.suppressCaptureUI(true)
        XCTAssertFalse(policy.ready(artifact: "new\0truncated"))
        XCTAssertTrue(policy.ready(artifact: "new"))
        XCTAssertFalse(policy.visible(count: 1, enabled: true, includeInCaptures: false))
        policy.suppressCaptureUI(false)
        XCTAssertTrue(policy.visible(count: 1, enabled: true, includeInCaptures: false))
        let third = try XCTUnwrap(policy.beginCapture())
        XCTAssertTrue(policy.restore(generation: third))
        XCTAssertTrue(policy.visible(count: 1, enabled: true, includeInCaptures: false))
        let fourth = try XCTUnwrap(policy.beginCapture())
        XCTAssertTrue(policy.wait(generation: fourth, artifact: "decode-error"))
        XCTAssertTrue(policy.stopWaiting())
        XCTAssertFalse(policy.ready(artifact: "decode-error"))
        XCTAssertTrue(policy.visible(count: 1, enabled: true, includeInCaptures: false))
    }
}
