import AppKit
import XCTest
@testable import CapturesNative

final class RecordingHUDTests: XCTestCase {
    func testEveryHUDColorTokenExistsInEveryTheme() {
        for (theme, tokens) in Tokens.variants {
            let missing = RecordingHUDColorToken.allCases
                .map(\.rawValue)
                .filter { tokens.colors[$0] == nil }
            XCTAssertEqual(missing, [], "\(theme) is missing Recording HUD color tokens")
        }
    }

    func testCompactHUDRunningPausedAndUnavailableControlsFit() throws {
        _ = NSApplication.shared
        for appearance in ["dark", "light"] {
            let tokens = try XCTUnwrap(Tokens.variants["\(appearance)-mustard"])
            let hud = RecordingHUDView(frame: NSRect(x: 0, y: 0, width: 430, height: 102),
                tokens: tokens)
            let window = NSWindow(contentRect: hud.bounds, styleMask: [.borderless],
                backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            window.contentView = hud; defer { window.close() }

            XCTAssertEqual(hud.frame.size, NSSize(width: 430, height: 102))
            XCTAssertTrue(hud.subviews.allSatisfy { $0.frame.maxX <= 430 && $0.frame.maxY <= 102 },
                "compact controls must not clip")
            XCTAssertEqual(hud.subviews.compactMap { $0 as? CaptureButton }.filter(\.isEnabled).count, 5,
                "mic-less sessions keep mute unavailable")
            let microphone = try XCTUnwrap(hud.subviews.compactMap { $0 as? CaptureButton }
                .first { $0.accessibilityLabel()?.contains("Microphone unavailable") == true })
            XCTAssertFalse(microphone.isEnabled)
            XCTAssertEqual(microphone.toolTip,
                "Microphone unavailable because no microphone was selected")
            let buttons = hud.subviews.compactMap { $0 as? CaptureButton }
            XCTAssertEqual(buttons.filter(\.signal).compactMap { $0.accessibilityLabel() },
                ["Stop recording"])
            XCTAssertTrue(buttons.filter { $0.accessibilityLabel() != "Stop recording" }
                .allSatisfy { !$0.signal }, "non-destructive HUD actions stay neutral")
            hud.setLifecycleActionsEnabled(false)
            XCTAssertEqual(hud.subviews.compactMap { $0 as? CaptureButton }.filter(\.isEnabled).count, 0)
            if appearance == "dark" {
                try render(hud, window: window, name: "recording-hud-dark-busy")
            }
            hud.setLifecycleActionsEnabled(true)
            XCTAssertEqual(hud.subviews.compactMap { $0 as? CaptureButton }.filter(\.isEnabled).count, 5)
            hud.setMicrophone(muted: false, available: true)
            XCTAssertTrue(microphone.isEnabled)
            XCTAssertEqual(microphone.accessibilityLabel(), "Mute microphone")
            XCTAssertEqual(hud.subviews.compactMap { $0 as? CaptureButton }.filter(\.isEnabled).count, 6)
            try render(hud, window: window, name: "recording-hud-\(appearance)-unmuted")
            let bitmap = try XCTUnwrap(microphone.bitmapImageRepForCachingDisplay(in: microphone.bounds))
            microphone.cacheDisplay(in: microphone.bounds, to: bitmap)
            let scale = CGFloat(bitmap.pixelsHigh) / microphone.bounds.height
            let centerX = Int(microphone.bounds.midX * scale)
            let iconTop = (microphone.bounds.height - 14) / 2
            let capsuleInterior = try XCTUnwrap(bitmap.colorAt(x: centerX,
                y: Int((iconTop + 3) * scale))?.usingColorSpace(.deviceRGB))
            let stand = try XCTUnwrap(bitmap.colorAt(x: centerX,
                y: Int((iconTop + 11) * scale))?.usingColorSpace(.deviceRGB))
            XCTAssertGreaterThan(stand.redComponent, capsuleInterior.redComponent + 0.2,
                "the microphone stand must be below its hollow capsule, not upside down")
            hud.setLifecycleActionsEnabled(false)
            // A snapshot queued before the mutation can arrive while it is busy.
            hud.setMicrophone(muted: true, available: true)
            XCTAssertFalse(microphone.isEnabled, "a late snapshot must not unlock lifecycle actions")
            XCTAssertTrue(microphone.selected)
            XCTAssertEqual(microphone.accessibilityLabel(), "Unmute microphone")
            XCTAssertEqual((microphone.accessibilityValue() as? NSNumber)?.intValue, 1)
            try render(hud, window: window, name: "recording-hud-\(appearance)-mic-busy")
            hud.setLifecycleActionsEnabled(true)
            XCTAssertTrue(microphone.isEnabled)
            var restarted = false
            hud.restart = { restarted = true }
            try XCTUnwrap(buttons.first { $0.accessibilityLabel() == "Restart recording" })
                .performClick(nil)
            XCTAssertTrue(restarted)
            hud.setPaused(false, elapsedMilliseconds: 94_000)
            XCTAssertFalse(hud.paused)
            try render(hud, window: window, name: "recording-hud-\(appearance)-running")
            hud.setPaused(true, elapsedMilliseconds: 94_000)
            XCTAssertTrue(hud.paused)
            try render(hud, window: window, name: "recording-hud-\(appearance)-paused")
        }
    }

    func testHUDChromeAppearsOnlyOnInteractionAndStop() throws {
        _ = NSApplication.shared
        let tokens = try XCTUnwrap(Tokens.variants["dark-mustard"])
        let hud = RecordingHUDView(frame: NSRect(x: 0, y: 0, width: 430, height: 102),
            tokens: tokens)
        let window = NSWindow(contentRect: hud.bounds, styleMask: [.borderless],
            backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = hud; defer { window.close() }
        hud.setMicrophone(muted: false, available: true)
        let buttons = hud.subviews.compactMap { $0 as? CaptureButton }
        XCTAssertTrue(buttons.allSatisfy { $0.hudControl && $0.frame.height == 34 })
        XCTAssertEqual(hud.layer?.cornerRadius, tokens.number("r-xl"))
        let microphone = try XCTUnwrap(buttons.first { $0.accessibilityLabel() == "Mute microphone" })

        func background(_ x: CGFloat, _ y: CGFloat) throws -> NSColor {
            window.display(); hud.layoutSubtreeIfNeeded()
            let bitmap = try XCTUnwrap(hud.bitmapImageRepForCachingDisplay(in: hud.bounds))
            hud.cacheDisplay(in: hud.bounds, to: bitmap)
            let scale = CGFloat(bitmap.pixelsWide) / hud.bounds.width
            return try XCTUnwrap(bitmap.colorAt(x: Int(x * scale), y: Int(y * scale))?
                .usingColorSpace(.deviceRGB))
        }
        let x = microphone.frame.minX + 5
        let y = microphone.frame.minY + 5
        let bare = try background(microphone.frame.minX - 1, y)
        let idle = try background(x, y)
        XCTAssertEqual(idle.redComponent, bare.redComponent, accuracy: 0.01,
            "an ordinary idle button must not paint a background")
        let edge = try background(microphone.frame.minX + 1, microphone.frame.midY)
        XCTAssertEqual(edge.redComponent, bare.redComponent, accuracy: 0.01,
            "an ordinary idle button must not paint a border")
        let event = try XCTUnwrap(NSEvent.enterExitEvent(with: .mouseEntered, location: .zero,
            modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil,
            eventNumber: 0, trackingNumber: 0, userData: nil))
        microphone.mouseEntered(with: event)
        XCTAssertGreaterThan(try background(x, y).redComponent, bare.redComponent + 0.03)
        try render(hud, window: window, name: "recording-hud-dark-hover")
        microphone.mouseExited(with: event)
        hud.setMicrophone(muted: true, available: true)
        XCTAssertGreaterThan(try background(x, y).redComponent, bare.redComponent + 0.03)
        try render(hud, window: window, name: "recording-hud-dark-selected")
        hud.setLifecycleActionsEnabled(false)
        microphone.mouseEntered(with: event)
        XCTAssertEqual(try background(x, y).redComponent, bare.redComponent, accuracy: 0.01,
            "a disabled button must not paint selected or hover chrome")
        hud.setLifecycleActionsEnabled(true)
        let stop = try XCTUnwrap(buttons.first { $0.accessibilityLabel() == "Stop recording" })
        let stopColor = try background(stop.frame.minX + 5, stop.frame.minY + 5)
        XCTAssertGreaterThan(stopColor.redComponent, stopColor.greenComponent,
            "Stop retains its signal surface without a hover")
    }

    func testPrivacyNoticeReflectsCaptureInclusionSetting() throws {
        _ = NSApplication.shared
        let tokens = try XCTUnwrap(Tokens.variants["dark-mustard"])
        let included = RecordingHUDView(frame: NSRect(x: 0, y: 0, width: 430, height: 102),
            tokens: tokens, excludedFromCapture: false)
        XCTAssertTrue(included.subviews.compactMap { ($0 as? NSTextField)?.stringValue }
            .contains("These controls will appear in recordings"))
    }

    func testHiddenNoticeIsFixedGlassNoninteractiveAndExplainsRestoration() throws {
        _ = NSApplication.shared
        let tokens = try XCTUnwrap(Tokens.variants["dark-mustard"])
        let notice = RecordingControlsHiddenNoticeView(
            frame: NSRect(x: 0, y: 0, width: 360, height: 96), tokens: tokens)
        let labels = notice.subviews.compactMap { ($0 as? NSTextField)?.stringValue }
        XCTAssertTrue(labels.contains("Recording controls hidden"))
        XCTAssertTrue(labels.contains { $0.contains("menu bar") && $0.contains("New Capture") })
        let background = try XCTUnwrap(notice.layer?.backgroundColor.flatMap(NSColor.init(cgColor:))?
            .usingColorSpace(.deviceRGB))
        let expected = try XCTUnwrap(tokens.color(RecordingHUDColorToken.glassStrong.rawValue)
            .usingColorSpace(.deviceRGB))
        XCTAssertEqual(background.redComponent, expected.redComponent, accuracy: 0.001)
        XCTAssertEqual(background.greenComponent, expected.greenComponent, accuracy: 0.001)
        XCTAssertEqual(background.blueComponent, expected.blueComponent, accuracy: 0.001)
        XCTAssertEqual(background.alphaComponent, expected.alphaComponent, accuracy: 0.001)
        let window = NSWindow(contentRect: notice.bounds, styleMask: [.borderless],
            backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = notice
        defer { window.close() }
        try render(notice, window: window, name: "recording-controls-hidden-notice")
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
