import AppKit
import XCTest
@testable import CapturesNative

final class WindowSelectionTests: XCTestCase {
    private let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
    private let targets = [
        WindowSelectionTarget(id: "front", title: "Draft", appName: "Editor",
            rect: NSRect(x: 80, y: 70, width: 520, height: 360), cornerRadius: 25),
        WindowSelectionTarget(id: "back", title: "Reference", appName: "Browser",
            rect: NSRect(x: 420, y: 180, width: 480, height: 390), cornerRadius: 10),
    ]

    func testDescriptorsBecomeDisplayLocalWithoutChangingClippedBoundsOrNames() throws {
        let display = try XCTUnwrap(WindowSelectionDisplay([
            "id": "left", "name": "Left", "x": -1440, "y": -120,
            "width": 1440, "height": 900,
        ]))
        let target = try XCTUnwrap(WindowSelectionTarget([
            "id": "window", "title": "Document", "app_name": "Editor",
            "x": -1460, "y": -100, "width": 400, "height": 300,
            "corner_radius": 12.5,
        ], display: display))
        XCTAssertEqual(display.size, CGSize(width: 1440, height: 900))
        XCTAssertEqual(target.rect, NSRect(x: -20, y: 20, width: 400, height: 300))
        XCTAssertEqual(target.cornerRadius, 12.5)
        XCTAssertEqual(target.name, "Editor — Document")

        var withoutRadius: [String: Any] = [
            "id": "other", "title": "", "app_name": "Browser",
            "x": -1400, "y": -100, "width": 300, "height": 200,
        ]
        let fallback = try XCTUnwrap(WindowSelectionTarget(withoutRadius, display: display,
            fallbackCornerRadius: 25))
        XCTAssertEqual(fallback.cornerRadius, 25)
        XCTAssertEqual(fallback.name, "Browser")
        withoutRadius["corner_radius"] = 0
        XCTAssertEqual(WindowSelectionTarget(withoutRadius, display: display,
            fallbackCornerRadius: 25)?.cornerRadius, 0, "an explicit square edge wins over the OS fallback")
    }

    func testRustHitResultsDriveHoverClickEnterAndEscapeWithoutSwiftGeometry() throws {
        _ = NSApplication.shared
        let window = NSWindow(contentRect: frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false; defer { window.close() }
        var hits: [NSPoint] = []
        var confirmed: [WindowSelectionChoice] = []
        var cancelled = 0
        let view = WindowSelectionView(frame: frame, image: nil, targets: targets,
            tokens: Tokens.variants["light-mustard"]!, autoStart: false,
            hitTest: { point in
                hits.append(NSPoint(x: point.x, y: point.y))
                if point.x == 10 { return -1 }
                if point.x == 20 { return 1 }
                return nil
            }, confirm: { confirmed.append($0) }, cancel: { cancelled += 1 })
        window.contentView = view

        view.mouseMoved(with: try mouseEvent(.mouseMoved, window: window, x: 20, y: 44))
        XCTAssertEqual(view.hoveredIndex, 1)
        XCTAssertNil(view.choice, "hover alone cannot become the confirmed target")
        XCTAssertEqual(view.activeChoice, .window(index: 1, id: "back"))
        view.hover(NSPoint(x: 999, y: 44))
        XCTAssertEqual(view.hoveredIndex, 1, "invalid hit-test results leave the visible target unchanged")
        view.mouseDown(with: try mouseEvent(.leftMouseDown, window: window, x: 20, y: 50))
        XCTAssertEqual(view.choice, .window(index: 1, id: "back"))
        XCTAssertTrue(confirmed.isEmpty, "manual mode selects on click but waits for confirmation")
        view.mouseMoved(with: try mouseEvent(.mouseMoved, window: window, x: 10, y: 54))
        XCTAssertEqual(view.activeChoice, .display, "hover remains an independent visual preview")
        XCTAssertEqual(view.choice, .window(index: 1, id: "back"), "moving cannot replace the clicked target")

        view.keyDown(with: try keyEvent(window: window, keyCode: 36, characters: "\r"))
        XCTAssertEqual(confirmed, [.window(index: 1, id: "back")])
        view.keyDown(with: try keyEvent(window: window, keyCode: 53, characters: "\u{1b}"))
        XCTAssertEqual(cancelled, 1)
        XCTAssertEqual(hits, [NSPoint(x: 20, y: 44), NSPoint(x: 999, y: 44),
            NSPoint(x: 20, y: 50), NSPoint(x: 10, y: 54)])
    }

    func testAutomaticStartCommitsOnlyAfterAValidClick() {
        _ = NSApplication.shared
        var confirmed: [WindowSelectionChoice] = []
        let view = WindowSelectionView(frame: frame, image: nil, targets: targets,
            tokens: Tokens.variants["dark-mustard"]!, autoStart: true,
            hitTest: { $0.x == 40 ? 0 : nil }, confirm: { confirmed.append($0) }, cancel: {})
        view.select(NSPoint(x: 30, y: 40))
        XCTAssertTrue(confirmed.isEmpty, "a failed shared hit test cannot confirm a stale target")
        view.select(NSPoint(x: 40, y: 40))
        XCTAssertEqual(view.choice, .window(index: 0, id: "front"))
        XCTAssertEqual(confirmed, [.window(index: 0, id: "front")])
    }

    func testRepresentativeWindowAndDisplayStatesRenderInBothAppearances() throws {
        _ = NSApplication.shared
        for appearance in ["dark", "light"] {
            let window = NSWindow(contentRect: frame, styleMask: [.borderless], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false; defer { window.close() }
            let view = WindowSelectionView(frame: frame,
                image: PreviewView.fixtureImage(scale: 2048.0 / 284.0), targets: targets,
                tokens: Tokens.variants["\(appearance)-mustard"]!, autoStart: false,
                hitTest: { $0.x < 300 ? 0 : -1 }, confirm: { _ in }, cancel: {})
            window.contentView = view
            view.hover(NSPoint(x: 100, y: 100))
            XCTAssertNil(view.choice)
            XCTAssertEqual(view.activeChoice, .window(index: 0, id: "front"))
            try render(view, window: window, name: "window-\(appearance)-target")
            view.hover(NSPoint(x: 900, y: 650))
            XCTAssertEqual(view.activeChoice, .display)
            XCTAssertEqual(view.accessibilityValue() as? String, "Entire display")
            XCTAssertTrue(view.subviews.compactMap { ($0 as? NSTextField)?.stringValue }
                .contains("Select a window to continue · Esc to cancel · Press Enter to confirm"))
            try render(view, window: window, name: "window-\(appearance)-display")

            let controls = view.subviews.flatMap(\.subviews).compactMap { $0 as? CaptureButton }
            XCTAssertEqual(controls.map(\.title).sorted(), ["Cancel", "Capture"])
            XCTAssertTrue(controls.allSatisfy(\.glass))
            XCTAssertTrue(controls.allSatisfy { $0.accessibilityLabel() != nil })
        }

        let window = NSWindow(contentRect: frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false; defer { window.close() }
        let view = WindowSelectionView(frame: frame,
            image: PreviewView.fixtureImage(scale: 2048.0 / 284.0), targets: targets,
            tokens: Tokens.variants["dark-mustard"]!, autoStart: true,
            hitTest: { _ in 0 }, confirm: { _ in }, cancel: {})
        window.contentView = view; view.hover(NSPoint(x: 100, y: 100))
        XCTAssertTrue(view.subviews.compactMap { ($0 as? NSTextField)?.stringValue }
            .contains("Select a window to continue · Esc to cancel"))
        try render(view, window: window, name: "window-dark-auto-start")
    }

    private func keyEvent(window: NSWindow, keyCode: UInt16, characters: String) throws -> NSEvent {
        try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
            timestamp: 0, windowNumber: window.windowNumber, context: nil,
            characters: characters, charactersIgnoringModifiers: characters,
            isARepeat: false, keyCode: keyCode))
    }

    private func mouseEvent(_ type: NSEvent.EventType, window: NSWindow,
                            x: CGFloat, y: CGFloat) throws -> NSEvent {
        try XCTUnwrap(NSEvent.mouseEvent(with: type,
            location: NSPoint(x: x, y: frame.height - y), modifierFlags: [],
            timestamp: 0, windowNumber: window.windowNumber, context: nil,
            eventNumber: 1, clickCount: 1, pressure: 1))
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
