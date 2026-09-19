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
            XCTAssertEqual(hud.subviews.compactMap { $0 as? CaptureButton }.filter(\.isEnabled).count, 4,
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
            XCTAssertEqual(hud.subviews.compactMap { $0 as? CaptureButton }.filter(\.isEnabled).count, 4)
            hud.setMicrophone(muted: false, available: true)
            XCTAssertTrue(microphone.isEnabled)
            XCTAssertEqual(microphone.accessibilityLabel(), "Mute microphone")
            XCTAssertEqual(hud.subviews.compactMap { $0 as? CaptureButton }.filter(\.isEnabled).count, 5)
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

    func testPrivacyNoticeReflectsCaptureInclusionSetting() throws {
        _ = NSApplication.shared
        let tokens = try XCTUnwrap(Tokens.variants["dark-mustard"])
        let included = RecordingHUDView(frame: NSRect(x: 0, y: 0, width: 430, height: 102),
            tokens: tokens, excludedFromCapture: false)
        XCTAssertTrue(included.subviews.compactMap { ($0 as? NSTextField)?.stringValue }
            .contains("These controls will appear in recordings"))
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
