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
        view.mouseDown(with: try event(.leftMouseDown, 100, 80))
        view.mouseUp(with: try event(.leftMouseUp, 101, 81))
        XCTAssertTrue(confirmed.isEmpty)
        view.mouseDown(with: try event(.leftMouseDown, 100, 80))
        view.mouseDragged(with: try event(.leftMouseDragged, 420, 260))
        XCTAssertTrue(confirmed.isEmpty, "do not auto-start before release")
        view.mouseUp(with: try event(.leftMouseUp, 420, 260))
        XCTAssertEqual(confirmed.count, 1)
        let region = try XCTUnwrap(confirmed.first)
        XCTAssertEqual(region.x, 100); XCTAssertEqual(region.y, 80)
        XCTAssertEqual(region.width, 320); XCTAssertEqual(region.height, 180)
        view.end(); XCTAssertEqual(confirmed.count, 1, "no duplicate pointer-up commit")
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
