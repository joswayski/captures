import AppKit
import XCTest
import CCapturesSettings
@testable import CapturesNative

final class CaptureControlsTests: XCTestCase {
    private let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
    private let targets = [
        WindowSelectionTarget(id: "front", title: "Draft", appName: "Editor",
            rect: NSRect(x: 72, y: 64, width: 492, height: 332), cornerRadius: 24),
        WindowSelectionTarget(id: "back", title: "Reference", appName: "Browser",
            rect: NSRect(x: 406, y: 176, width: 478, height: 372), cornerRadius: 10),
    ]

    func testTargetSwitchesRetainSettledRegionAndWindowIndependently() throws {
        _ = NSApplication.shared
        var confirmed: [WindowSelectionChoice] = []
        let view = makeView(confirm: { confirmed.append($0) })

        XCTAssertEqual(view.target, .region)
        XCTAssertNil(view.choice, "an empty region must keep Capture disabled")
        let capture = try XCTUnwrap(buttons(in: view).first { $0.title == "Capture" })
        XCTAssertFalse(capture.isEnabled)

        view.beginRegion(NSPoint(x: 91, y: 77))
        XCTAssertFalse(view.isGuidanceVisible,
            "shipping guidance hides while a region gesture is active")
        view.dragRegion(NSPoint(x: 432, y: 268))
        view.endRegion()
        XCTAssertTrue(view.isGuidanceVisible)
        let region = try XCTUnwrap(view.choice)
        guard case .region(let rect) = region else { return XCTFail("expected region") }
        XCTAssertEqual(rect.x, 91); XCTAssertEqual(rect.y, 77)
        XCTAssertEqual(rect.width, 341); XCTAssertEqual(rect.height, 191)
        XCTAssertTrue(capture.isEnabled)

        view.setTarget(.window)
        XCTAssertNil(view.choice, "a region is not a window selection")
        view.selectWindow(NSPoint(x: 24, y: 40))
        XCTAssertEqual(view.choice, .window(index: 1, id: "back"))
        view.setTarget(.region)
        XCTAssertEqual(view.choice, region, "explicit target toggles retain the settled region")
        view.setTarget(.window)
        XCTAssertEqual(view.choice, .window(index: 1, id: "back"),
            "explicit target toggles retain the selected window")
        view.confirmSelection()
        XCTAssertEqual(confirmed, [.window(index: 1, id: "back")])
        view.setTarget(.display)
        XCTAssertEqual(view.choice, .display)
    }

    func testEnterEscapeAndAutoStartRespectSelectionBoundaries() throws {
        _ = NSApplication.shared
        let window = NSWindow(contentRect: frame, styleMask: [.borderless],
            backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        var confirmed: [WindowSelectionChoice] = []
        var cancelled = 0
        let manual = makeView(confirm: { confirmed.append($0) }, cancel: { cancelled += 1 })
        window.contentView = manual
        manual.keyDown(with: try keyEvent(window: window, keyCode: 36, characters: "\r"))
        XCTAssertTrue(confirmed.isEmpty)
        manual.beginRegion(NSPoint(x: 120, y: 92)); manual.dragRegion(NSPoint(x: 380, y: 246))
        manual.endRegion()
        manual.keyDown(with: try keyEvent(window: window, keyCode: 36, characters: "\r"))
        XCTAssertEqual(confirmed.count, 1)
        manual.keyDown(with: try keyEvent(window: window, keyCode: 53, characters: "\u{1b}"))
        XCTAssertEqual(cancelled, 1)

        confirmed.removeAll()
        let automatic = makeView(autoStart: true, confirm: { confirmed.append($0) })
        automatic.beginRegion(NSPoint(x: 80, y: 60))
        automatic.dragRegion(NSPoint(x: 81, y: 61)); automatic.endRegion()
        XCTAssertTrue(confirmed.isEmpty, "a sub-minimum drag cannot auto-capture")
        automatic.beginRegion(NSPoint(x: 80, y: 60))
        automatic.dragRegion(NSPoint(x: 320, y: 210))
        XCTAssertTrue(confirmed.isEmpty, "auto-start waits for pointer release")
        automatic.endRegion()
        XCTAssertEqual(confirmed.count, 1)

        confirmed.removeAll(); automatic.setTarget(.window)
        automatic.selectWindow(NSPoint(x: 24, y: 40))
        XCTAssertEqual(confirmed, [.window(index: 1, id: "back")])
        confirmed.removeAll(); automatic.setTarget(.display)
        XCTAssertEqual(confirmed, [.display])
    }

    func testPreparationGateRejectsCancelledAndSupersededResults() {
        var gate = CapturePreparationGate()
        let first = gate.begin()
        XCTAssertTrue(gate.accepts(first))
        let replacement = gate.begin()
        XCTAssertFalse(gate.accepts(first), "a display switch must reject its stale preparation")
        XCTAssertTrue(gate.accepts(replacement))
        gate.invalidate()
        XCTAssertFalse(gate.accepts(replacement), "cancel must prevent a late preparation from reopening controls")
    }

    func testBlankControlsSpaceDragsAndClampsWithoutStartingARegion() throws {
        _ = NSApplication.shared
        let window = NSWindow(contentRect: frame, styleMask: [.borderless],
            backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let view = makeView()
        window.contentView = view
        let original = view.controls.frame

        let controlsPoint = NSPoint(x: view.controls.frame.midX, y: view.controls.frame.midY)
        view.mouseDown(with: try mouseEvent(.leftMouseDown, window: window,
            x: controlsPoint.x, y: controlsPoint.y))
        view.mouseDragged(with: try mouseEvent(.leftMouseDragged, window: window,
            x: 120, y: 100))
        view.mouseUp(with: try mouseEvent(.leftMouseUp, window: window, x: 120, y: 100))
        XCTAssertNil(view.choice)
        XCTAssertNil(view.region.mode)
        XCTAssertEqual(view.region.rect.width, 0,
            "a controls-origin gesture must never leak into region selection")

        view.controls.beginPanelDrag(at: NSPoint(x: original.midX, y: original.midY))
        view.controls.dragPanel(to: NSPoint(x: -500, y: -500))
        XCTAssertEqual(view.controls.frame.origin, NSPoint(x: 16, y: 16))
        view.controls.endPanelDrag()

        view.controls.beginPanelDrag(at: NSPoint(x: 26, y: 26))
        view.controls.dragPanel(to: NSPoint(x: 5_000, y: 5_000))
        XCTAssertEqual(view.controls.frame.maxX, frame.width - 16)
        XCTAssertEqual(view.controls.frame.maxY, frame.height - 16)
        view.controls.endPanelDrag()

        XCTAssertTrue(view.controls.hitTest(NSPoint(x: 20, y: 70)) === view.controls,
            "the blank footer is the drag target")
        let capture = try XCTUnwrap(buttons(in: view.controls).first { $0.title == "Capture" })
        XCTAssertTrue(view.controls.hitTest(NSPoint(x: capture.frame.midX,
            y: capture.frame.midY)) === capture, "interactive controls do not begin a panel drag")
    }

    func testNarrowMonitorKeepsPickerAndPrimaryActionVisibleWithoutOverlap() throws {
        _ = NSApplication.shared
        let narrowFrame = NSRect(x: 0, y: 0, width: 768, height: 600)
        let window = NSWindow(contentRect: narrowFrame, styleMask: [.borderless],
            backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let view = UnifiedCaptureSelectionView(frame: narrowFrame, image: nil, targets: targets,
            tokens: Tokens.variants["dark-mustard"]!, autoStart: false,
            hitTest: { _ in 0 }, displayTitles: ["Main display · 768 × 600"],
            selectedDisplay: 0, confirm: { _ in }, cancel: {}, changeDisplay: { _ in })
        window.contentView = view
        XCTAssertEqual(view.controls.frame.width, 736)
        try render(view, window: window, name: "capture-controls-dark-narrow-region")
        view.setTarget(.display)
        try render(view, window: window, name: "capture-controls-dark-narrow-display")

        let capture = try XCTUnwrap(buttons(in: view.controls).first { $0.title == "Capture" })
        let picker = try XCTUnwrap(descendant(in: view.controls, accessibilityLabel: "Display"))
        XCTAssertTrue(view.controls.bounds.contains(capture.frame))
        XCTAssertTrue(view.controls.bounds.contains(picker.frame))
        XCTAssertFalse(capture.frame.intersects(picker.frame))
        for subview in view.controls.subviews where !subview.isHidden {
            XCTAssertTrue(view.controls.bounds.contains(subview.frame),
                "\(subview) clips on a 768-point monitor")
        }
    }

    func testPanelIsExcludedFromCapturedPixels() throws {
        _ = NSApplication.shared
        let screen = try XCTUnwrap(NSScreen.main)
        var cancelled = 0
        let panel = UnifiedCapturePanel(screen: screen, image: nil, targets: targets,
            tokens: Tokens.variants["dark-mustard"]!, autoStart: false,
            hitTest: { _ in -1 }, displayTitles: ["Main display"], selectedDisplay: 0,
            confirm: { _ in }, cancel: { cancelled += 1 }, changeDisplay: { _ in })
        defer { panel.close() }
        XCTAssertEqual(panel.title, "Captures Capture Controls")
        XCTAssertEqual(panel.sharingType, .none)
        XCTAssertEqual(panel.selector.target, .region)
        panel.cancelOperation(nil)
        XCTAssertEqual(cancelled, 1, "Escape command routing cancels even when a control owns focus")
    }

    func testFixedGlassEmptySelectedAndAutoStatesRenderInBothAppearances() throws {
        _ = NSApplication.shared
        for appearance in ["dark", "light"] {
            let window = NSWindow(contentRect: frame, styleMask: [.borderless],
                backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            defer { window.close() }
            let empty = makeView(appearance: appearance)
            window.contentView = empty
            try render(empty, window: window, name: "capture-controls-\(appearance)-empty")

            empty.beginRegion(NSPoint(x: 114, y: 78))
            empty.dragRegion(NSPoint(x: 478, y: 283)); empty.endRegion()
            empty.setAspect(4)
            try render(empty, window: window, name: "capture-controls-\(appearance)-selected")

            let automatic = makeView(appearance: appearance, autoStart: true)
            window.contentView = automatic
            automatic.setTarget(.window)
            _ = automatic.hoverWindow(NSPoint(x: 24, y: 40))
            try render(automatic, window: window, name: "capture-controls-\(appearance)-auto")

            let controls = buttons(in: automatic.controls)
            XCTAssertEqual(controls.filter(\.selected).map(\.title).sorted(),
                ["Screenshot", "Window"])
            XCTAssertTrue(controls.allSatisfy(\.glass))
            XCTAssertTrue(controls.allSatisfy { $0.accessibilityLabel() != nil })
            XCTAssertTrue(controls.first { $0.title == "Capture" }?.isHidden == true)
            XCTAssertTrue(controls.first { $0.title == "Record" }?.isEnabled == false)
            XCTAssertEqual(automatic.controls.frame.width, 854)
            XCTAssertEqual(automatic.controls.frame.height, 86)
            XCTAssertEqual(automatic.controls.frame.minY, frame.height - 112)
        }
    }

    private func makeView(appearance: String = "light", autoStart: Bool = false,
                          confirm: @escaping (WindowSelectionChoice) -> Void = { _ in },
                          cancel: @escaping () -> Void = {}) -> UnifiedCaptureSelectionView {
        UnifiedCaptureSelectionView(frame: frame,
            image: PreviewView.fixtureImage(scale: 2048.0 / 284.0), targets: targets,
            tokens: Tokens.variants["\(appearance)-mustard"]!, autoStart: autoStart,
            hitTest: { point in point.x < 50 ? 1 : (point.x < 400 ? 0 : -1) },
            displayTitles: ["Main display · 1000 × 720", "External · 1920 × 1080"],
            selectedDisplay: 0, confirm: confirm, cancel: cancel, changeDisplay: { _ in })
    }

    private func buttons(in view: NSView) -> [CaptureButton] {
        view.subviews.flatMap { subview in
            (subview as? CaptureButton).map { [$0] } ?? buttons(in: subview)
        }
    }

    private func descendant(in view: NSView, accessibilityLabel: String) -> NSView? {
        if view.accessibilityLabel() == accessibilityLabel { return view }
        return view.subviews.lazy.compactMap {
            descendant(in: $0, accessibilityLabel: accessibilityLabel)
        }.first
    }

    private func mouseEvent(_ type: NSEvent.EventType, window: NSWindow,
                            x: CGFloat, y: CGFloat) throws -> NSEvent {
        try XCTUnwrap(NSEvent.mouseEvent(with: type,
            location: NSPoint(x: x, y: frame.height - y), modifierFlags: [],
            timestamp: 0, windowNumber: window.windowNumber, context: nil,
            eventNumber: 1, clickCount: 1, pressure: 1))
    }

    private func keyEvent(window: NSWindow, keyCode: UInt16, characters: String) throws -> NSEvent {
        try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
            timestamp: 0, windowNumber: window.windowNumber, context: nil,
            characters: characters, charactersIgnoringModifiers: characters,
            isARepeat: false, keyCode: keyCode))
    }

    private func render(_ view: NSView, window: NSWindow, name: String) throws {
        window.display(); view.layoutSubtreeIfNeeded()
        guard let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] else { return }
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        let url = URL(fileURLWithPath: directory).appendingPathComponent("\(name).png")
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
            withIntermediateDirectories: true)
        try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: url)
    }
}
