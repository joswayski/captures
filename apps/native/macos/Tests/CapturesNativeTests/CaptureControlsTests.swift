import AppKit
import XCTest
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
        view.dragRegion(NSPoint(x: 432, y: 268))
        view.endRegion()
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
