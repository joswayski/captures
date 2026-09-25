import XCTest
import CCapturesSettings
@testable import CapturesNative

final class PreviewPolicyTests: XCTestCase {
    func testStackSnapshotAndMirroredLayoutUseRustPolicy() throws {
        let stack = NativePreviewStack()
        XCTAssertFalse(stack.insert(""))
        XCTAssertFalse(stack.insert("bad\0id"))
        for id in ["古い", "middle", "latest"] { XCTAssertTrue(stack.insert(id)) }
        XCTAssertFalse(stack.insert("古い"))
        XCTAssertEqual(stack.ids, ["古い", "middle", "latest"])
        XCTAssertEqual(stack.contentHeight, 608)
        XCTAssertEqual(try XCTUnwrap(stack.cardLayout(index: 0, topAnchor: false)).y, 28)
        XCTAssertEqual(try XCTUnwrap(stack.cardLayout(index: 0, topAnchor: true)).y, 420)
        XCTAssertEqual(try XCTUnwrap(stack.cardLayout(index: 2, topAnchor: true)).y, 52)
        XCTAssertNil(stack.cardLayout(index: -1, topAnchor: false))
        XCTAssertNil(stack.cardLayout(index: 3, topAnchor: false))
        let snapshot = stack.ids
        stack.setCollapsed(true)
        XCTAssertFalse(try XCTUnwrap(stack.cardLayout(index: 0, topAnchor: false)).interactive)
        XCTAssertTrue(try XCTUnwrap(stack.cardLayout(index: 2, topAnchor: false)).interactive)
        XCTAssertTrue(stack.insert("incoming"))
        XCTAssertTrue(stack.isCollapsed)
        XCTAssertEqual(stack.removeAll(snapshot), 3)
        XCTAssertEqual(stack.ids, ["incoming"])
        XCTAssertTrue(stack.isCollapsed)
        XCTAssertTrue(stack.remove("incoming"))
        XCTAssertFalse(stack.isCollapsed)
        XCTAssertFalse(stack.remove("incoming"))
        XCTAssertTrue(stack.insert("fresh"))
        XCTAssertFalse(stack.isCollapsed)
    }

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

    func testDraggedPileStoresClampedVisibleEdgeAndSurvivesArrivalAndExpansion() throws {
        let monitor = CapturesPreviewMonitor(work_x: -2400, work_y: 120,
            work_width: 2400, work_height: 1500, full_x: -2400, full_y: 40,
            full_width: 2400, full_height: 1660, scale_factor: 2)
        for (placement, edge) in [("top_left", 250.0), ("bottom_right", 514.0)] {
            let origin = try XCTUnwrap(NativePreviewLayout.movedOrigin(monitor: monitor, count: 2,
                frameOrigin: NSPoint(x: -700, y: 250), placement: placement))
            XCTAssertEqual(origin.x, -700); XCTAssertEqual(origin.edge, edge)
            let compact = try XCTUnwrap(NativePreviewLayout.geometry(monitor: monitor, count: 3,
                collapsed: true, origin: origin, placement: placement))
            // Count changes the transparent padding, never the front-card top.
            XCTAssertEqual(compact.y + (compact.height - 160) / 2, 302, accuracy: 0.0001)
            let expanded = try XCTUnwrap(NativePreviewLayout.geometry(monitor: monitor, count: 2,
                origin: origin, placement: placement))
            XCTAssertEqual(expanded.x, -700)
            XCTAssertEqual(expanded.y + (origin.anchor == 0 ? expanded.height : 0), edge)
        }
        let bottom = try XCTUnwrap(NativePreviewLayout.movedOrigin(monitor: monitor, count: 2,
            frameOrigin: NSPoint(x: 9000, y: 9000), placement: "bottom_left"))
        XCTAssertEqual(bottom.x, -340); XCTAssertEqual(bottom.edge, 798)
        let top = try XCTUnwrap(NativePreviewLayout.movedOrigin(monitor: monitor, count: 2,
            frameOrigin: NSPoint(x: -9000, y: -9000), placement: "top_right"))
        XCTAssertEqual(top.x, -1200); XCTAssertEqual(top.edge, 72)
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
