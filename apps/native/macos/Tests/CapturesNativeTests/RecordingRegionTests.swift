import AppKit
import XCTest
@testable import CapturesNative

final class RecordingRegionTests: XCTestCase {
    func testGuidePixelsStayOutsideRecordedRegion() throws {
        _ = NSApplication.shared
        let tokens = try XCTUnwrap(Tokens.variants["dark-mustard"])
        let bounds = NSRect(x: 0, y: 0, width: 640, height: 360)
        for region in [NSRect(x: 101, y: 53, width: 317, height: 179), bounds] {
            let view = RecordingRegionView(frame: bounds, region: region, tokens: tokens)
            let window = NSWindow(contentRect: bounds, styleMask: [.borderless],
                backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false; window.isOpaque = false
            window.backgroundColor = .clear
            window.contentView = view; defer { window.close() }
            let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: bounds))
            view.cacheDisplay(in: bounds, to: bitmap)
            let scale = CGFloat(bitmap.pixelsWide) / bounds.width
            func color(_ x: CGFloat, _ y: CGFloat) throws -> NSColor {
                try XCTUnwrap(bitmap.colorAt(x: Int(x * scale), y: Int(y * scale))?
                    .usingColorSpace(.deviceRGB))
            }
            // All four inner edges plus the center must be truly transparent,
            // not merely look dark against a black fixture background.
            for x in [region.minX, region.midX, region.maxX - 1] {
                for y in [region.minY, region.midY, region.maxY - 1] {
                    XCTAssertEqual(try color(x, y).alphaComponent, 0, accuracy: 0.001)
                }
            }
            if region != bounds {
                XCTAssertGreaterThan(try color(100, 53).alphaComponent, 0.99)
                XCTAssertGreaterThan(try color(418, 100).alphaComponent, 0.99)
                XCTAssertGreaterThan(try color(200, 52).alphaComponent, 0.99)
                XCTAssertGreaterThan(try color(200, 232).alphaComponent, 0.99)
                let veil = try color(20, 20).alphaComponent
                XCTAssertGreaterThan(veil, 0); XCTAssertLessThan(veil, 1)
                if let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] {
                    let url = URL(fileURLWithPath: directory).appendingPathComponent("recording-region.png")
                    try FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                        withIntermediateDirectories: true)
                    try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: url)
                }
            }
        }
    }

    func testPanelIsPassiveAndUsesTheWholeDisplay() throws {
        _ = NSApplication.shared
        let screen = try XCTUnwrap(NSScreen.main)
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let panel = RecordingRegionPanel(screen: screen,
            region: NSRect(x: 101, y: 53, width: 317, height: 179), tokens: tokens)
        defer { panel.close() }
        XCTAssertEqual(panel.frame, screen.frame)
        XCTAssertTrue(panel.ignoresMouseEvents)
        XCTAssertFalse(panel.hidesOnDeactivate)
        XCTAssertFalse(panel.canBecomeKey); XCTAssertFalse(panel.canBecomeMain)
        XCTAssertEqual(panel.sharingType, .none)
        XCTAssertFalse(try XCTUnwrap(panel.contentView).isAccessibilityElement())
    }

    func testOnlyCanonicalRegionTargetsProduceGuideRect() throws {
        let rect = ["x": 101, "y": 53, "width": 317, "height": 179]
        for type in ["region", "display", "window"] {
            let snapshot = try XCTUnwrap(NativeRecordingSnapshot([
                "id": "take", "state": "paused", "elapsed_ms": 37000,
                "options": ["audio": ["microphone_muted": false],
                    "target": ["type": type, "display_id": "display", "rect": rect]],
            ]))
            XCTAssertEqual(snapshot.region, type == "region"
                ? NSRect(x: 101, y: 53, width: 317, height: 179) : nil)
        }
    }
}
