import AppKit
import XCTest
import CCapturesSettings
@testable import CapturesNative

final class RegionSelectionTests: XCTestCase {
    func testDrawShiftReleaseMoveAndCornerResizeUseRustGeometry() {
        var model = RegionSelection(bounds: CapturesSelectionBounds(width: 1000, height: 720))
        XCTAssertFalse(model.capturable)
        model.setAspect(16.0 / 9.0)
        model.begin(NSPoint(x: 100, y: 80), handleRadius: 8, shift: false)
        model.update(NSPoint(x: 420, y: 260), shift: false)
        XCTAssertEqual(model.rect.width, 320); XCTAssertEqual(model.rect.height, 180)
        model.recompute(shift: true)
        XCTAssertEqual(model.rect.width, 320); XCTAssertEqual(model.rect.height, 320)
        model.recompute(shift: false)
        XCTAssertEqual(model.rect.height, 180)
        model.end()
        model.begin(NSPoint(x: 200, y: 160), handleRadius: 8, shift: false)
        XCTAssertEqual(model.mode, 1)
        model.update(NSPoint(x: 240, y: 185), shift: false); model.end()
        XCTAssertEqual(model.rect.x, 140); XCTAssertEqual(model.rect.y, 105)
        XCTAssertEqual(model.rect.width, 320); XCTAssertEqual(model.rect.height, 180)
        model.begin(NSPoint(x: 460, y: 105), handleRadius: 8, shift: false)
        XCTAssertEqual(model.mode, 3, "NE handle, not move or NW")
        model.update(NSPoint(x: 520, y: 75), shift: false); model.end()
        XCTAssertEqual(model.rect.x, 140)
        XCTAssertEqual(model.rect.y + model.rect.height, 285, accuracy: 0.0001)
        XCTAssertEqual(model.rect.width / model.rect.height, 16.0 / 9.0, accuracy: 0.0001)
    }

    func testNativePointerEventsAutoStartMinimumAndRetainedControls() throws {
        _ = NSApplication.shared
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
        let window = NSWindow(contentRect: frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        var confirmed: [CapturesSelectionRect] = []
        var cancelled = 0
        let view = RegionSelectionView(frame: frame, image: nil, tokens: Tokens.variants["light-mustard"]!,
            autoStart: true, confirm: { confirmed.append($0) }, cancel: { cancelled += 1 })
        window.contentView = view
        let identities = view.subviews.map(ObjectIdentifier.init)
        func event(_ type: NSEvent.EventType, _ x: CGFloat, _ y: CGFloat) throws -> NSEvent {
            try XCTUnwrap(NSEvent.mouseEvent(with: type, location: NSPoint(x: x, y: 720-y),
                modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil,
                eventNumber: 1, clickCount: 1, pressure: 1))
        }
        XCTAssertFalse(view.subviews.flatMap(\.subviews).contains { $0 is CaptureButton && !$0.isHidden && !($0.superview?.isHidden ?? false) },
            "the direct overlay has no toolbar")
        XCTAssertEqual(view.guidanceText, "Drag to select a region · Shift for square · Esc to cancel")
        view.mouseDown(with: try event(.leftMouseDown, 100, 80))
        view.mouseUp(with: try event(.leftMouseUp, 101, 81))
        XCTAssertTrue(confirmed.isEmpty)
        XCTAssertEqual(view.guidanceText, "Click and drag to select a region · Shift for square · Esc to cancel",
            "a click without a region shows the shipping feedback")
        view.mouseDown(with: try event(.leftMouseDown, 100, 80))
        view.mouseDragged(with: try event(.leftMouseDragged, 420, 260))
        XCTAssertTrue(confirmed.isEmpty, "do not auto-start before release")
        view.mouseUp(with: try event(.leftMouseUp, 420, 260))
        XCTAssertEqual(confirmed.count, 1)
        let region = try XCTUnwrap(confirmed.first)
        XCTAssertEqual(region.x, 100); XCTAssertEqual(region.y, 80)
        XCTAssertEqual(region.width, 320); XCTAssertEqual(region.height, 180)
        view.end(); XCTAssertEqual(confirmed.count, 1, "no duplicate pointer-up commit")
        view.mouseDown(with: try event(.leftMouseDown, 200, 160))
        view.mouseUp(with: try event(.leftMouseUp, 240, 185))
        XCTAssertEqual(confirmed.count, 1, "moving is adjust-only even with automatic start")
        view.mouseDown(with: try event(.leftMouseDown, 460, 285))
        view.mouseUp(with: try event(.leftMouseUp, 520, 325))
        XCTAssertEqual(confirmed.count, 1, "resizing is adjust-only even with automatic start")
        view.keyDown(with: try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero,
            modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil,
            characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}", isARepeat: false, keyCode: 53)))
        XCTAssertEqual(cancelled, 1)
        XCTAssertEqual(view.subviews.map(ObjectIdentifier.init), identities)
    }

    func testManualConfirmAndRenderRepresentativeStatesInBothAppearances() throws {
        _ = NSApplication.shared
        for appearance in ["dark", "light"] {
            let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
            let window = NSWindow(contentRect: frame, styleMask: [.borderless], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            defer { window.close() }
            var count = 0
            let view = RegionSelectionView(frame: frame, image: PreviewView.fixtureImage(scale: 2048.0 / 284.0),
                tokens: Tokens.variants["\(appearance)-mustard"]!, autoStart: false,
                confirm: { _ in count += 1 }, cancel: {})
            window.contentView = view
            view.confirmSelection(); XCTAssertEqual(count, 0)
            try render(view, window: window, name: "region-\(appearance)-empty")
            view.begin(NSPoint(x: 100, y: 80)); view.drag(NSPoint(x: 420, y: 260)); view.end()
            XCTAssertEqual(count, 0)
            try render(view, window: window, name: "region-\(appearance)-drawn")
            view.setAspect(1)
            XCTAssertEqual(view.selection.rect.x, 170)
            XCTAssertEqual(view.selection.rect.width, 180)
            try render(view, window: window, name: "region-\(appearance)-square")
            view.confirmSelection(); XCTAssertEqual(count, 1)
            let controls = view.subviews.flatMap(\.subviews).compactMap { $0 as? CaptureButton }
            XCTAssertEqual(controls.count, 8)
            XCTAssertEqual(controls.filter(\.selected).map(\.title).sorted(), ["1:1", "Capture"])
            for button in controls {
                XCTAssertTrue(button.glass)
                XCTAssertTrue(try XCTUnwrap(button.superview).bounds.contains(button.frame))
                XCTAssertNotNil(button.accessibilityLabel())
            }
        }
    }

    func testDirectMarqueeUsesShippingShadeHairlinesAndBadgeWithoutHandles() throws {
        _ = NSApplication.shared
        let frame = NSRect(x: 0, y: 0, width: 800, height: 560)
        let window = NSWindow(contentRect: frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let view = RegionSelectionView(frame: frame, image: nil, tokens: Tokens.variants["dark-mustard"]!,
            autoStart: true, confirm: { _ in }, cancel: {})
        window.contentView = view
        view.begin(NSPoint(x: 100, y: 80)); view.drag(NSPoint(x: 420, y: 260))
        window.display(); view.layoutSubtreeIfNeeded()
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        try render(view, window: window, name: "region-dark-direct-drag")
        func alpha(_ x: CGFloat, _ y: CGFloat) throws -> CGFloat {
            let px = Int(x * CGFloat(bitmap.pixelsWide) / view.bounds.width)
            let py = Int(y * CGFloat(bitmap.pixelsHigh) / view.bounds.height)
            return try XCTUnwrap(bitmap.colorAt(x: px, y: py)).alphaComponent
        }
        XCTAssertEqual(try alpha(50, 300), 0.2, accuracy: 0.03, "the lighter shipping region shade")
        XCTAssertEqual(try alpha(250, 170), 0, accuracy: 0.01, "the selection stays clear")
        XCTAssertEqual(try alpha(104, 77.5), 0.2, accuracy: 0.03, "no corner handle outside the box")
        XCTAssertEqual(try alpha(103, 83), 0, accuracy: 0.05, "no corner handle inside the box")
        XCTAssertEqual(try alpha(99.5, 170), 1 - 0.8 * 0.55, accuracy: 0.05, "dark outer hairline over the shade")
        XCTAssertGreaterThan(try alpha(100.75, 170), 0.95, "1.5 pt accent border")
        // The 1 pt inner hairline spans 101.5–102.5: a whole 2x pixel or half a 1x pixel.
        let inner = try alpha(102, 170)
        XCTAssertTrue(inner > 0.1 && inner < 0.35, "light 28% inner hairline, got \(inner)")

        let badge = try XCTUnwrap(view.subviews.compactMap { $0 as? NSTextField }
            .first { $0.stringValue == "320 × 180" })
        XCTAssertFalse(badge.isHidden)
        XCTAssertEqual(badge.frame.minX, 101.5, accuracy: 0.01, "left-aligned inside the border")
        XCTAssertEqual(badge.frame.minY, 51.5, accuracy: 0.01, "30 pt above the box")
        view.end()
        view.begin(NSPoint(x: 500, y: 10)); view.drag(NSPoint(x: 700, y: 200))
        XCTAssertEqual(badge.frame.minY, 11.5 + Tokens.variants["dark-mustard"]!.number("s-3"), accuracy: 0.01,
            "near the top of the screen the badge moves inside the box")
    }

    private func render(_ view: NSView, window: NSWindow, name: String) throws {
        window.display(); view.layoutSubtreeIfNeeded()
        guard let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] else { return }
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        let url = URL(fileURLWithPath: directory).appendingPathComponent("\(name).png")
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: url)
    }
}
