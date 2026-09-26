import AppKit
import XCTest
@testable import CapturesNative

private final class OnboardingTransport: SettingsTransport {
    var requests: [[String: Any]] = []
    var failure: Error?
    var granted = false
    var microphone = false
    var screenRequested = false
    var microphoneRequested = false
    var canRequest = true
    /// Grant immediately on request; false models a denied/unchanged switch.
    var grantsOnRequest = true

    func request(_ object: [String: Any]) throws -> [String: Any] {
        requests.append(object)
        if let failure { throw failure }
        let action = object["action"] as? String
        if action == "request_screen" { granted = granted || grantsOnRequest; screenRequested = true }
        if action == "request_microphone" { microphone = grantsOnRequest; microphoneRequested = true }
        var state: [String: Any] = [
            "platform": "macos", "onboarding_completed": action == "complete" && granted,
            "screen_recording_required": true, "screen_recording_granted": granted,
            "screen_recording_can_request": canRequest && !granted,
            "screen_recording_requested_this_launch": screenRequested,
            "microphone_granted": microphone, "microphone_can_request": canRequest && !microphone,
            "microphone_requested_this_launch": microphoneRequested,
        ]
        // Presentation is always derived by the shared Rust service.
        let derived = try SettingsBridge().request(["operation": "onboarding_presentation", "state": state])
        state["presentation"] = derived["presentation"]
        return ["ok": true, "state": state]
    }
}

private func allButtons(_ view: NSView) -> [NSButton] {
    view.subviews.flatMap { child -> [NSButton] in
        if let button = child as? NSButton { return [button] }
        return allButtons(child)
    }
}

final class OnboardingTests: XCTestCase {
    func testRecoveryDonePreservesSetupAndNeverRestartsOrCompletes() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            for state in ["denied", "ready", "error"] {
                let transport = OnboardingTransport()
                transport.granted = state == "ready"
                transport.screenRequested = true
                transport.canRequest = false
                if state == "error" { transport.failure = SettingsStoreError.backend("Permission status unavailable. Refresh to retry.") }
                let controller = OnboardingController(store: try SettingsStore(path: "/fixture.json", transport: transport))
                var closed = false
                let view = OnboardingView(frame: NSRect(x: 0, y: 0, width: 700, height: 620),
                    tokens: Tokens.variants["\(appearance)-mustard"]!, controller: controller,
                    done: { closed = true })
                view.appearance = NSAppearance(named: appearance == "dark" ? .darkAqua : .aqua)
                let loaded = expectation(description: "recovery-\(appearance)-\(state)")
                controller.requiresAttention = { loaded.fulfill() }
                controller.check()
                let done = view.primaryButton
                XCTAssertEqual(done.title, "Done")
                XCTAssertFalse(done.isEnabled, "In-flight status requests retain the sheet")
                wait(for: [loaded], timeout: 2)
                let buttons = allButtons(view)
                XCTAssertTrue(done.isEnabled, "Denied access and check failures cannot trap the user")
                XCTAssertEqual(done.title, "Done", "Recovery never offers a restart or completion")
                XCTAssertFalse(buttons.contains { !$0.isHidden && ["Restart Captures", "Start capturing"].contains($0.title) })
                XCTAssertEqual(view.refreshButton.title, "Refresh status")
                XCTAssertTrue(view.refreshButton.isEnabled)
                view.layoutSubtreeIfNeeded()
                if let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] {
                    let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
                    view.cacheDisplay(in: view.bounds, to: bitmap)
                    let path = URL(fileURLWithPath: directory).appendingPathComponent("permission-recovery-\(appearance)-\(state).png")
                    try FileManager.default.createDirectory(at: path.deletingLastPathComponent(), withIntermediateDirectories: true)
                    try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: path)
                }
                done.performClick(nil)
                XCTAssertTrue(closed)
                XCTAssertEqual(transport.requests.compactMap { $0["action"] as? String }, ["check"])
            }
        }
    }

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

    func testPermissionStatesRenderLikeTheShippingSetupWindow() throws {
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
                let primary = view.primaryButton
                XCTAssertTrue(view.subviews.contains { ($0 as? NSTextField)?.stringValue == "WELCOME TO CAPTURES" })
                XCTAssertTrue(view.subviews.contains {
                    ($0 as? NSTextField)?.stringValue.hasPrefix("Captures only reads the pixels you choose to capture.") == true
                })
                XCTAssertEqual(view.refreshButton.title, "Refresh status", "Refresh stays as a secondary action")
                XCTAssertLessThan(view.refreshButton.frame.maxX, primary.frame.minX)
                switch state {
                case "denied":
                    XCTAssertEqual(primary.title, "Restart Captures")
                    XCTAssertTrue(primary.isEnabled)
                    XCTAssertEqual(view.screenRow.status.status?.label, "Restart required")
                    XCTAssertEqual(view.screenRow.button.title, "Open Settings")
                    XCTAssertFalse(view.screenRow.button.isHidden)
                    XCTAssertFalse(view.microphoneRow.isHidden)
                    XCTAssertEqual(view.microphoneRow.button.title, "Allow microphone")
                case "ready":
                    XCTAssertEqual(primary.title, "Start capturing")
                    XCTAssertTrue(primary.isEnabled)
                    XCTAssertEqual(view.screenRow.status.status, try OnboardingStatus(["label": "Granted", "ready": true] as [String: Any]))
                    XCTAssertTrue(view.screenRow.button.isHidden)
                default:
                    XCTAssertEqual(primary.title, "Start capturing")
                    XCTAssertFalse(primary.isEnabled)
                    XCTAssertEqual(view.screenRow.detail.stringValue, "Checking the access available on this computer…")
                    XCTAssertTrue(view.subviews.contains { box in !box.isHidden && box.subviews.contains { label in
                        (label as? NSTextField)?.stringValue == "Settings are read-only. Fix access and retry." } })
                    XCTAssertTrue(view.refreshButton.isEnabled, "Errors stay actionable")
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

    func testMicrophoneOffersSettingsOnlyAfterAskingAndShowsGrantedPill() throws {
        _ = NSApplication.shared
        let transport = OnboardingTransport()
        transport.granted = true
        transport.canRequest = false
        transport.grantsOnRequest = false
        let controller = OnboardingController(store: try SettingsStore(path: "/fixture.json", transport: transport))
        let view = OnboardingView(frame: NSRect(x: 0, y: 0, width: 1000, height: 720),
            tokens: Tokens.variants["light-mustard"]!, controller: controller)
        let steps: [(() -> Void, String)] = [({ controller.check() }, "Allow microphone"),
                                             ({ controller.requestMicrophone() }, "Open Settings")]
        for (action, label) in steps {
            let loaded = expectation(description: label)
            controller.requiresAttention = { loaded.fulfill() }
            action()
            wait(for: [loaded], timeout: 2)
            XCTAssertEqual(view.microphoneRow.button.title, label)
            XCTAssertNil(view.microphoneRow.status.status)
        }
        XCTAssertTrue(view.microphoneRow.detail.stringValue.hasPrefix("Turn Captures on in Microphone settings."))
        transport.grantsOnRequest = true
        let granted = expectation(description: "granted")
        controller.requiresAttention = { granted.fulfill() }
        controller.requestMicrophone()
        wait(for: [granted], timeout: 2)
        XCTAssertTrue(view.microphoneRow.button.isHidden)
        XCTAssertEqual(view.microphoneRow.status.status?.label, "Granted")
        XCTAssertEqual(view.microphoneRow.status.accessibilityLabel(), "Granted ✓")
    }

    func testSharedCopyMatchesTheShippingSetupWindow() throws {
        let copy = OnboardingCopy.current
        XCTAssertEqual(copy.eyebrow, "Welcome to Captures")
        XCTAssertEqual(copy.title, "Required permissions")
        XCTAssertEqual(copy.lede, "Captures only reads the pixels you choose to capture. Nothing is uploaded, and nothing leaves this computer unless you send it somewhere.")
        XCTAssertEqual(copy.refresh, "Refresh status")
        XCTAssertEqual(copy.start, "Start capturing")
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
