import AppKit
import XCTest
@testable import CapturesNative

private final class OnboardingTransport: SettingsTransport {
    private let lock = NSLock()
    private var log: [[String: Any]] = []
    /// Requests run on the settings worker; read them from the main thread.
    var requests: [[String: Any]] { lock.lock(); defer { lock.unlock() }; return log }
    var failure: Error?
    var granted = false
    var microphone = false
    var screenRequested = false
    var microphoneRequested = false
    var canRequest = true
    /// Grant immediately on request; false models a denied/unchanged switch.
    var grantsOnRequest = true

    func request(_ object: [String: Any]) throws -> [String: Any] {
        lock.lock(); log.append(object); lock.unlock()
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

/// Records scheduled permission polls; tests fire them by hand.
private final class ManualTimer: OnboardingTimer {
    let interval: TimeInterval
    private let action: () -> Void
    private(set) var invalidated = false
    init(interval: TimeInterval, action: @escaping () -> Void) { self.interval = interval; self.action = action }
    func fire() { if !invalidated { action() } }
    func invalidate() { invalidated = true }
}

private final class ManualTimers {
    private(set) var all: [ManualTimer] = []
    var active: [ManualTimer] { all.filter { !$0.invalidated } }
    func schedule(_ interval: TimeInterval, _ action: @escaping () -> Void) -> OnboardingTimer {
        let timer = ManualTimer(interval: interval, action: action)
        all.append(timer)
        return timer
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
                XCTAssertNil(view.refreshButton.superview,
                             "First-run setup polls like shipping and has no Refresh button")
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
                    XCTAssertFalse(allButtons(view).contains { !$0.isHidden && $0.title == "Refresh status" })
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
        XCTAssertEqual(view.microphoneRow.status.accessibilityLabel(), "Granted",
                       "Shipping hides the check mark from assistive technology")
        XCTAssertEqual(view.microphoneRow.accessibilityLabel(), "Microphone")
        XCTAssertEqual(view.microphoneRow.accessibilityRole(), .group)
    }

    func testSharedCopyMatchesTheShippingSetupWindow() throws {
        let copy = OnboardingCopy.current
        XCTAssertEqual(copy.eyebrow, "Welcome to Captures")
        XCTAssertEqual(copy.title, "Required permissions")
        XCTAssertEqual(copy.lede, "Captures only reads the pixels you choose to capture. Nothing is uploaded, and nothing leaves this computer unless you send it somewhere.")
        XCTAssertEqual(copy.refresh, "Refresh status")
        XCTAssertEqual(copy.start, "Start capturing")
        XCTAssertEqual(copy.pollInterval, 1.5)
        XCTAssertEqual(copy.settingsAway, 2.5)
    }

    func testFirstRunPollsWhileWaitingAndStopsOnceGranted() throws {
        _ = NSApplication.shared
        let transport = OnboardingTransport()
        transport.grantsOnRequest = false
        let timers = ManualTimers()
        let controller = OnboardingController(store: try SettingsStore(path: "/fixture.json",
            transport: transport, debounceInterval: 0), now: { 0 }, schedule: timers.schedule)
        let view = OnboardingView(frame: NSRect(x: 0, y: 0, width: 700, height: 560),
            tokens: Tokens.variants["light-mustard"]!, controller: controller)
        XCTAssertTrue(controller.pollsWhileWaiting)
        var changes = 0
        let redraw = controller.changed
        controller.changed = { redraw(); changes += 1 }
        controller.check()
        try settle { !controller.busy }
        XCTAssertTrue(timers.active.isEmpty, "Nothing to wait for before a request")

        controller.requestScreen()
        try settle { !controller.busy }
        let timer = try XCTUnwrap(timers.active.first)
        XCTAssertEqual(timers.active.count, 1)
        XCTAssertEqual(timer.interval, 1.5, "Shipping PERMISSION_POLL_MS")
        XCTAssertEqual(view.primaryButton.title, "Restart Captures")

        // A poll is a quiet check: it never disables the window's controls.
        var seen = changes
        timer.fire()
        XCTAssertFalse(controller.busy)
        XCTAssertTrue(view.screenRow.button.isEnabled)
        XCTAssertTrue(view.primaryButton.isEnabled)
        try settle { changes == seen + 1 }
        XCTAssertEqual(transport.requests.count, 3)
        XCTAssertEqual(transport.requests.last?["action"] as? String, "check")
        XCTAssertFalse(timer.invalidated)

        transport.granted = true
        seen = changes
        timer.fire()
        try settle { changes == seen + 1 }
        XCTAssertEqual(controller.state?.screenGranted, true)
        XCTAssertTrue(timer.invalidated, "Polling stops once access is granted")
        XCTAssertFalse(controller.isPolling)
        XCTAssertEqual(view.screenRow.status.status?.label, "Granted")

        // Permission recovery keeps Refresh status and never polls.
        let recovery = OnboardingController(store: try SettingsStore(path: "/fixture.json",
            transport: transport, debounceInterval: 0), now: { 0 }, schedule: timers.schedule)
        let sheet = OnboardingView(frame: NSRect(x: 0, y: 0, width: 700, height: 560),
            tokens: Tokens.variants["light-mustard"]!, controller: recovery, done: {})
        XCTAssertFalse(recovery.pollsWhileWaiting)
        XCTAssertNotNil(sheet.refreshButton.superview)
        XCTAssertFalse(sheet.refreshButton.isHidden)
    }

    func testReturningFromSettingsRestartsOnlyAfterTheAwayThreshold() throws {
        _ = NSApplication.shared
        let transport = OnboardingTransport()
        transport.canRequest = false
        transport.grantsOnRequest = false
        var clock: TimeInterval = 100
        let timers = ManualTimers()
        let controller = OnboardingController(store: try SettingsStore(path: "/fixture.json",
            transport: transport, debounceInterval: 0), now: { clock }, schedule: timers.schedule)
        let view = OnboardingView(frame: NSRect(x: 0, y: 0, width: 700, height: 560),
            tokens: Tokens.variants["light-mustard"]!, controller: controller)
        var restarts = 0
        view.restartRequested = { restarts += 1 }
        controller.check()
        try settle { !controller.busy }
        XCTAssertFalse(controller.leftForSettings)
        controller.requestScreen()
        try settle { !controller.busy }
        XCTAssertTrue(controller.leftForSettings, "Open Settings sent the user away")

        // Back after 2.25 s: re-check without restarting.
        clock = 200
        controller.resignedActive()
        clock = 202.25
        controller.becameActive()
        XCTAssertTrue(controller.busy)
        try settle { !controller.busy }
        XCTAssertEqual(transport.requests.count, 3)
        XCTAssertEqual(restarts, 0)
        XCTAssertEqual(view.primaryButton.title, "Restart Captures")

        // Focus without a preceding blur never restarts.
        clock = 300
        controller.becameActive()
        try settle { !controller.busy }
        XCTAssertEqual(transport.requests.count, 4)
        XCTAssertEqual(restarts, 0)

        // Back after 2.5 s with access still unreported: restart once.
        clock = 400
        controller.resignedActive()
        clock = 402.5
        controller.becameActive()
        try settle { restarts == 1 }
        XCTAssertEqual(view.primaryButton.title, "Restarting…")
        XCTAssertFalse(view.primaryButton.isEnabled)
        clock = 500
        controller.resignedActive()
        clock = 510
        controller.becameActive()
        try settle { !controller.busy }
        XCTAssertEqual(transport.requests.count, 6)
        XCTAssertEqual(restarts, 1, "A pending restart is not repeated")

        // Access reported on return: no restart, even after a long visit.
        let granted = OnboardingTransport()
        granted.canRequest = false
        granted.grantsOnRequest = false
        let other = OnboardingController(store: try SettingsStore(path: "/fixture.json",
            transport: granted, debounceInterval: 0), now: { clock }, schedule: timers.schedule)
        let otherView = OnboardingView(frame: NSRect(x: 0, y: 0, width: 700, height: 560),
            tokens: Tokens.variants["light-mustard"]!, controller: other)
        var otherRestarts = 0
        otherView.restartRequested = { otherRestarts += 1 }
        other.requestScreen()
        try settle { !other.busy }
        XCTAssertTrue(other.leftForSettings)
        clock = 600
        other.resignedActive()
        granted.granted = true
        clock = 630
        other.becameActive()
        try settle { !other.busy }
        XCTAssertEqual(granted.requests.count, 2)
        XCTAssertEqual(otherRestarts, 0)
        XCTAssertEqual(otherView.primaryButton.title, "Start capturing")
    }

    func testSetupTabOrderFollowsShippingCardsThenActions() throws {
        _ = NSApplication.shared
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let transport = OnboardingTransport()
        let controller = OnboardingController(store: try SettingsStore(path: "/fixture.json",
            transport: transport, debounceInterval: 0), now: { 0 }, schedule: ManualTimers().schedule)
        let frame = NSRect(x: 0, y: 0, width: 700, height: 560)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let view = OnboardingView(frame: frame, tokens: tokens, controller: controller)
        window.contentView = view
        XCTAssertTrue(window.initialFirstResponder === view.primaryButton,
                      "While checking, only the primary action exists")
        controller.check()
        try settle { !controller.busy }
        XCTAssertTrue(window.initialFirstResponder === view.screenRow.button,
                      "Allow access is the first control, like Tab from the top of the page")
        XCTAssertTrue(KeyViewLoop.order(from: view.screenRow.button).elementsEqual(
            [view.screenRow.button, view.microphoneRow.button, view.primaryButton] as [NSView], by: ===))

        let recovery = OnboardingView(frame: frame, tokens: tokens, controller: OnboardingController(
            store: try SettingsStore(path: "/fixture.json", transport: transport, debounceInterval: 0),
            now: { 0 }, schedule: ManualTimers().schedule), done: {})
        XCTAssertTrue(KeyViewLoop.order(from: recovery.screenRow.button).elementsEqual(
            [recovery.screenRow.button, recovery.microphoneRow.button, recovery.refreshButton,
             recovery.primaryButton] as [NSView], by: ===), "Recovery's Refresh precedes Done")
    }

    /// Runs the main loop until `condition` holds, failing after a deadline.
    private func settle(_ condition: () -> Bool) throws {
        let deadline = Date().addingTimeInterval(3)
        while !condition() && Date() < deadline { RunLoop.current.run(until: Date().addingTimeInterval(0.01)) }
        XCTAssertTrue(condition())
        guard condition() else { throw SettingsStoreError.invalidResponse }
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
