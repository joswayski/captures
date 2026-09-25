import AppKit
import XCTest
@testable import CapturesNative

private final class OnboardingTransport: SettingsTransport {
    var requests: [[String: Any]] = []
    var failure: Error?
    var granted = false
    var microphone = false
    var screenRequested = false
    var canRequest = true

    func request(_ object: [String: Any]) throws -> [String: Any] {
        requests.append(object)
        if let failure { throw failure }
        let action = object["action"] as? String
        if action == "request_screen" { granted = true; screenRequested = true }
        if action == "request_microphone" { microphone = true }
        return ["ok": true, "state": [
            "platform": "macos", "onboarding_completed": action == "complete" && granted,
            "screen_recording_required": true, "screen_recording_granted": granted,
            "screen_recording_can_request": canRequest,
            "screen_recording_requested_this_launch": screenRequested,
            "microphone_granted": microphone, "microphone_can_request": canRequest,
        ]]
    }
}

final class OnboardingTests: XCTestCase {
    func testRestartPreservesProfileAndQueuedMediaWithoutReplayingLaunchFlags() throws {
        let options = try Options(["--live", "--scene", "idle", "--history-root", "/profile space/history",
            "--settings-file", "/profile %/settings.json", "--appearance", "light",
            "--open-image", "/old.png", "--quit-after", "3"])
        let media = ["/old.png", "/forwarded file.png", "--literal-name.png"]
        let restarted = try Options(permissionRestartArguments(options: options, pendingMedia: media))
        XCTAssertTrue(restarted.live)
        XCTAssertEqual(restarted.historyRoot, options.historyRoot)
        XCTAssertEqual(restarted.settingsFile, options.settingsFile)
        XCTAssertEqual(restarted.openMedia, media)
        XCTAssertEqual(restarted.appearance, "light")
        XCTAssertEqual(restarted.scene, "preferences")
        XCTAssertNil(restarted.quitAfter)
    }

    func testPermissionStatesRenderAndDeniedMicrophoneKeepsSettingsAction() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            for state in ["denied", "ready", "error"] {
                let transport = OnboardingTransport()
                transport.granted = state == "ready"
                transport.canRequest = false
                transport.screenRequested = true
                if state == "error" { transport.failure = SettingsStoreError.backend("Settings are read-only. Fix access and retry.") }
                let controller = OnboardingController(store: try SettingsStore(path: "/fixture.json", transport: transport))
                let loaded = expectation(description: "\(appearance)-\(state)")
                controller.requiresAttention = { loaded.fulfill() }
                controller.check()
                wait(for: [loaded], timeout: 2)
                let view = OnboardingView(frame: NSRect(x: 0, y: 0, width: 1000, height: 720),
                    tokens: Tokens.variants["\(appearance)-mustard"]!, controller: controller)
                view.appearance = NSAppearance(named: appearance == "dark" ? .darkAqua : .aqua)
                view.layoutSubtreeIfNeeded()
                let buttons = view.subviews.compactMap { $0 as? NSButton }
                XCTAssertEqual(buttons.first { $0.title == "Finish setup" }?.isEnabled, state == "ready")
                if state != "error" {
                    XCTAssertEqual(buttons.first { $0.title == "Open Mic Settings" }?.isEnabled, true)
                    XCTAssertEqual(buttons.first { $0.title == "Restart Captures" }?.isHidden, state == "ready")
                }
                if let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] {
                    let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
                    view.cacheDisplay(in: view.bounds, to: bitmap)
                    let path = URL(fileURLWithPath: directory).appendingPathComponent("onboarding-\(appearance)-\(state).png")
                    try FileManager.default.createDirectory(at: path.deletingLastPathComponent(), withIntermediateDirectories: true)
                    try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: path)
                }
            }
        }
    }

    func testFirstRunRequestsScreenAndCompletesWithoutMicrophone() throws {
        let transport = OnboardingTransport()
        let store = try SettingsStore(path: "/fixture.json", transport: transport,
                                      debounceInterval: 0)
        let controller = OnboardingController(store: store)
        let checked = expectation(description: "checked")
        controller.requiresAttention = { checked.fulfill() }
        controller.check()
        wait(for: [checked], timeout: 1)
        XCTAssertFalse(controller.state!.screenGranted)

        let requested = expectation(description: "screen")
        controller.requiresAttention = { requested.fulfill() }
        controller.requestScreen()
        wait(for: [requested], timeout: 1)
        XCTAssertTrue(controller.state!.screenRequestedThisLaunch)

        let completed = expectation(description: "completed")
        controller.completed = { completed.fulfill() }
        controller.complete()
        wait(for: [completed], timeout: 1)
        XCTAssertTrue(controller.state!.completed)
        XCTAssertFalse(controller.state!.microphoneGranted)
        XCTAssertEqual(transport.requests.compactMap { $0["action"] as? String },
                       ["check", "request_screen", "complete"])
    }

    func testErrorsRemainActionableAndBusyDropsDuplicateRequest() throws {
        enum Failure: LocalizedError { case denied; var errorDescription: String? { "Try again" } }
        let transport = OnboardingTransport(); transport.failure = Failure.denied
        let controller = OnboardingController(store: try SettingsStore(path: "/fixture.json",
            transport: transport, debounceInterval: 0))
        let failed = expectation(description: "failed")
        controller.requiresAttention = { failed.fulfill() }
        controller.check()
        controller.requestMicrophone()
        wait(for: [failed], timeout: 1)
        XCTAssertEqual(controller.error, "Try again")
        XCTAssertEqual(transport.requests.count, 1)
        XCTAssertFalse(controller.busy)
    }
}
