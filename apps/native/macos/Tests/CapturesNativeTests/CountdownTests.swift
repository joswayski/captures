import AppKit
import XCTest
@testable import CapturesNative

final class CountdownTests: XCTestCase {
    func testCountdownKeepsViewsAndMediaPaletteAcrossAppearance() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let tokens = Tokens.variants["\(appearance)-mustard"]!
            let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
            let window = NSWindow(contentRect: frame, styleMask: [.borderless], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            let view = ScreenshotCountdownContent(frame: frame, tokens: tokens, remaining: 3)
            window.contentView = view
            view.layoutSubtreeIfNeeded()
            let identities = view.subviews.map(ObjectIdentifier.init)
            view.setRemaining(7)
            XCTAssertEqual(view.subviews.map(ObjectIdentifier.init), identities)
            let labels = view.subviews.compactMap { $0 as? NSTextField }
            XCTAssertEqual(labels.map(\.stringValue), ["SCREENSHOT IN", "7", "Press Esc to cancel"])
            XCTAssertEqual(labels[0].accessibilityLabel(), "Screenshot in")
            XCTAssertEqual(labels[1].accessibilityLabel(), "Screenshot in 7 seconds")
            XCTAssertEqual(labels[1].textColor, tokens.color("glass-text"))
            for label in labels { XCTAssertTrue(view.bounds.contains(label.frame)) }
            XCTAssertLessThanOrEqual(labels[0].frame.maxY, labels[1].frame.minY)
            XCTAssertLessThanOrEqual(labels[1].frame.maxY, labels[2].frame.minY)

            if let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] {
                window.display()
                let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
                view.cacheDisplay(in: view.bounds, to: bitmap)
                let url = URL(fileURLWithPath: directory).appendingPathComponent("countdown-\(appearance).png")
                try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
                try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: url)
            }
            window.close()
        }
    }

    func testRecordingCountdownUsesShippingHeadingAndCancellingCopy() throws {
        _ = NSApplication.shared
        let tokens = try XCTUnwrap(Tokens.variants["dark-mustard"])
        let view = ScreenshotCountdownContent(frame: NSRect(x: 0, y: 0, width: 1000, height: 720),
            tokens: tokens, remaining: 3, kind: .recording)
        let identities = view.subviews.map(ObjectIdentifier.init)
        var labels = view.subviews.compactMap { $0 as? NSTextField }
        XCTAssertEqual(labels.map(\.stringValue), ["RECORDING STARTS IN", "3", "Press Esc to cancel"])
        XCTAssertEqual(labels[0].accessibilityLabel(), "Recording starts in")
        XCTAssertEqual(labels[1].accessibilityLabel(), "Recording starts in 3 seconds")
        XCTAssertFalse(view.cancelling)
        view.setCancelling()
        XCTAssertTrue(view.cancelling)
        XCTAssertEqual(view.subviews.map(ObjectIdentifier.init), identities)
        labels = view.subviews.compactMap { $0 as? NSTextField }
        XCTAssertEqual(labels.map(\.stringValue), ["RECORDING STARTS IN", "3", "Cancelling…"])
    }
}
