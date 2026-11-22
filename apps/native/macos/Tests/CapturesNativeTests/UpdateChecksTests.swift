import AppKit
import XCTest
@testable import CapturesNative

final class UpdateChecksTests: XCTestCase {
    private final class Transport: UpdateCheckTransport {
        var operations: [String] = []
        var checking = false
        var status = "Not checked"
        var pollError = false
        var shutdowns = 0
        var acquisition: [String: Any]?
        var rawStatus: [String: Any] = ["state": "idle"]
        var installRestart: [String: Any]?
        var card: [String: Any]?
        func request(_ object: [String: Any]) throws -> [String: Any] {
            let operation = object.string("operation")
            operations.append(operation)
            if operation == "install_preview" { installRestart = object["restart"] as? [String: Any] }
            if operation == "poll" && pollError { throw AppBridgeError.invalidResponse }
            if operation == "check" { checking = true; status = "Checking signed metadata…" }
            if operation == "download_verify" {
                checking = true; status = "Downloading…"
                acquisition = ["label": "Cancel download", "enabled": true, "action": ["action": "cancel_download"]]
            }
            if operation == "cancel_download" {
                acquisition?["enabled"] = false
            }
            return ["accepted": operation != "poll", "checking": checking, "status": rawStatus,
                "presentation": ["version": "Native development 2026.9.99",
                    "channel": "Explicit development endpoint · Check only", "status": status,
                    "action": checking ? "Checking…" : "Check Now", "enabled": !checking,
                    "detail": "No installation or update channel is enabled.", "acquisition": acquisition ?? [:],
                    "card": card ?? NSNull()]]
        }
        func shutdown(completion: @escaping () -> Void) { shutdowns += 1; completion() }
    }

    func testPreviewInstallPassesCompleteRestartWithoutLosingOwnerSourceIdentity() throws {
        let transport = Transport(), model = try UpdateCheckModel(transport: transport)
        let restart: [String: Any] = ["restore_preferences": true,
            "preview_artifact_ids": ["older", "newer"],
            "capture_windows": [["kind": "screenshot_editor", "artifact_id": "owner", "source_artifact_id": "copy"]]]
        try model.launchPreviewInstall(restart: restart)
        XCTAssertTrue(NSDictionary(dictionary: try XCTUnwrap(transport.installRestart)).isEqual(to: restart))
    }

    func testShutdownSnapshotRequiresAStagedPackageAndSurvivesWorkerCleanup() throws {
        let transport = Transport(), model = try UpdateCheckModel(transport: transport)
        for state in ["idle", "available", "downloading", "verifying", "cancelling", "download_error"] {
            transport.rawStatus = ["state": state]
            XCTAssertNil(model.stagedShutdownStatus())
        }
        transport.rawStatus = ["state": "staged", "sha256": "exact selected artifact",
            "release": ["version": "2026.10.50"]]
        let snapshot = try XCTUnwrap(model.stagedShutdownStatus())
        model.shutdown {}
        transport.rawStatus = ["state": "idle"]
        XCTAssertEqual(snapshot.string("sha256"), "exact selected artifact")
        XCTAssertNil(model.stagedShutdownStatus(), "a closed checker cannot prepare another handoff")
    }

    func testAcquisitionRoutesExplicitInputAndKeepsCancellationBusyUntilAcknowledged() throws {
        let transport = Transport(), model = try UpdateCheckModel(transport: transport)
        XCTAssertFalse(model.acquire(.downloadVerify))
        transport.acquisition = ["label": "Download and verify", "enabled": true, "action": ["action": "download_verify"]]
        XCTAssertTrue(model.check())
        transport.checking = false; model.poll()
        XCTAssertTrue(model.acquire(.downloadVerify))
        XCTAssertFalse(model.acquire(.downloadVerify))
        XCTAssertFalse(model.check())
        XCTAssertTrue(model.acquire(.cancelDownload))
        XCTAssertFalse(model.acquire(.cancelDownload))
        XCTAssertTrue(model.busy, "cancellation is not a cleanup acknowledgement")
        XCTAssertFalse(model.check())
        transport.checking = false; transport.acquisition = nil
        model.poll()
        XCTAssertFalse(model.busy)
        XCTAssertEqual(transport.operations.filter { $0 == "download_verify" }.count, 1)
        XCTAssertEqual(transport.operations.filter { $0 == "cancel_download" }.count, 1)
        transport.acquisition = ["label": "Restart and install", "enabled": true, "action": ["action": "install"]]
        XCTAssertTrue(model.check())
        transport.checking = false; model.poll()
        var requested = false
        model.requestInstallation = { requested = true; return true }
        XCTAssertFalse(model.acquire(.install), "a visible action alone is not a verified stage")
        XCTAssertFalse(requested)
        transport.rawStatus = ["state": "staged"]
        model.requestInstallation = { false }
        XCTAssertFalse(model.acquire(.install), "host admission can veto installation")
        model.requestInstallation = { requested = true; return true }
        XCTAssertTrue(model.acquire(.install))
        XCTAssertTrue(requested)
        model.shutdown {}
        XCTAssertFalse(model.acquire(.downloadVerify))
        XCTAssertFalse(model.acquire(.install))
    }

    func testIdleBusyRetryClosedAndHiddenPresentationNeverStartAutomaticChecks() throws {
        let transport = Transport(), model = try UpdateCheckModel(transport: transport)
        model.poll()
        XCTAssertEqual(transport.operations, ["poll"])
        XCTAssertEqual(model.presentation.string("status"), "Not checked")
        XCTAssertTrue(model.check())
        XCTAssertTrue(model.busy)
        XCTAssertFalse(model.enabled)
        XCTAssertFalse(model.check())
        XCTAssertEqual(transport.operations.filter { $0 == "check" }.count, 1)
        transport.pollError = true
        model.poll()
        XCTAssertTrue(model.busy, "an unread response cannot unpin pending work")
        XCTAssertFalse(model.check())
        transport.pollError = false
        // No Preferences observer: closing its window must not destroy state.
        model.didChange = nil
        transport.checking = false; transport.status = "Development update 2026.10.50 available"
        model.poll()
        XCTAssertEqual(model.presentation.string("status"), transport.status)
        XCTAssertTrue(model.enabled)
        let count = transport.operations.count
        model.poll(); model.poll()
        XCTAssertEqual(transport.operations.count, count, "completed checks have no idle polling")
        XCTAssertTrue(model.check())
        model.shutdown {}
        let closedCount = transport.operations.count
        model.poll()
        XCTAssertFalse(model.check())
        XCTAssertFalse(model.enabled)
        XCTAssertEqual(transport.operations.count, closedCount)
        XCTAssertEqual(transport.shutdowns, 1)
    }

    func testReleaseCardUpdateNowDownloadsThenInstallsLikeShipping() throws {
        _ = NSApplication.shared
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
        defer { try? FileManager.default.removeItem(at: directory) }
        let transport = Transport()
        let fallback = "If this copy cannot update itself, download from captur.es and install over it. Settings and captures stay."
        func card(_ action: String, _ step: String?, heading: String, detail: String = "",
                  progress: Double? = nil, bar: Bool = false, error: String? = nil) -> [String: Any] {
            ["description": "Captures checks for a new version automatically. Update now installs it in place.",
             "action": action, "step": step ?? NSNull(), "heading": heading, "detail": detail,
             "current": false, "progress_bar": bar, "progress": progress ?? NSNull(),
             "error": error ?? NSNull(), "fallback": fallback]
        }
        transport.card = card("Update now", "download", heading: "Version 2026.11.13.2 is available",
                              detail: "104 MB download")
        transport.acquisition = ["label": "Download", "enabled": true, "action": ["action": "download_verify"]]
        let model = try UpdateCheckModel(transport: transport)
        defer { model.shutdown {} }
        var installs = 0
        model.requestInstallation = { installs += 1; return true }
        for appearance in ["light", "dark"] {
            let root = Surface(frame: NSRect(x: 0, y: 0, width: 980, height: 720))
            let loaded = expectation(description: "settings loaded")
            let controller = PreferencesController(root: root,
                store: try SettingsStore(path: directory.appendingPathComponent("\(appearance).json").path),
                tokens: { Tokens.variants["\(appearance)-mustard"]! }, appearanceChanged: { _, _, _ in },
                settingsPersisted: { _ in loaded.fulfill() }, showHistory: {}, updateChecks: model)
            controller.revealUpdates()
            wait(for: [loaded], timeout: 5)
            XCTAssertFalse(descendants(in: root).contains { ($0 as? NSTextField)?.stringValue == "Package verification" },
                           "release builds never show the development row")
            try renderCard(root, "preferences-updates-available-\(appearance)")
        }
        let root = Surface(frame: NSRect(x: 0, y: 0, width: 980, height: 720))
        let loaded = expectation(description: "settings loaded")
        let controller = PreferencesController(root: root,
            store: try SettingsStore(path: directory.appendingPathComponent("settings.json").path),
            tokens: { Tokens.variants["dark-mustard"]! }, appearanceChanged: { _, _, _ in },
            settingsPersisted: { _ in loaded.fulfill() }, showHistory: {}, updateChecks: model)
        model.didChange = { [weak controller] in controller?.refreshUpdateChecks() }
        wait(for: [loaded], timeout: 5)
        let button = try XCTUnwrap(find("updates.check", in: root) as? CaptureButton)
        XCTAssertEqual(button.title, "Update now")
        button.performClick(nil)
        XCTAssertEqual(transport.operations.filter { $0 == "download_verify" }.count, 1)
        transport.card = card("Installing…", nil, heading: "Updating to version 2026.11.13.2",
                              detail: "26.0 MB / 104 MB · 25%", progress: 25, bar: true)
        model.poll()
        let installing = try XCTUnwrap(find("updates.check", in: root) as? CaptureButton)
        XCTAssertEqual(installing.title, "Installing…")
        XCTAssertFalse(installing.isEnabled)
        try renderCard(root, "preferences-updates-installing-dark")
        // The verified package goes straight on to install, as Update now did.
        transport.checking = false; transport.rawStatus = ["state": "staged"]
        transport.acquisition = ["label": "Restart and install", "enabled": true, "action": ["action": "install"]]
        transport.card = card("Update now", "install", heading: "Version 2026.11.13.2 is available")
        model.poll()
        XCTAssertEqual(installs, 1)
        model.poll()
        XCTAssertEqual(installs, 1, "one Update now installs once")
        transport.card = card("Try again", "download", heading: "Update failed",
                              error: "Could not install the update: Native update signature verification failed.")
        model.setShowChangelog(false) // An idle model refreshes through an explicit poll.
        XCTAssertEqual(try XCTUnwrap(find("updates.check", in: root) as? CaptureButton).title, "Try again")
        try renderCard(root, "preferences-updates-failed-dark")
        controller.flush()
    }

    private func renderCard(_ root: NSView, _ name: String) throws {
        guard let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"],
              let card = find("preferences-card.updates", in: root) else { return }
        root.layoutSubtreeIfNeeded()
        let bitmap = try XCTUnwrap(card.bitmapImageRepForCachingDisplay(in: card.bounds))
        card.cacheDisplay(in: card.bounds, to: bitmap)
        let url = URL(fileURLWithPath: directory).appendingPathComponent("\(name).png")
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: url)
    }

    private func descendants(in view: NSView) -> [NSView] {
        view.subviews + view.subviews.flatMap { descendants(in: $0) }
    }

    func testPreferencesStatusRefreshPreservesARecorderAndReflowsAbout() throws {
        _ = NSApplication.shared
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
        defer { try? FileManager.default.removeItem(at: directory) }
        let transport = Transport(), model = try UpdateCheckModel(transport: transport)
        defer { model.shutdown {} }
        let root = Surface(frame: NSRect(x: 0, y: 0, width: 980, height: 720))
        let loaded = expectation(description: "settings loaded")
        let controller = PreferencesController(root: root,
            store: try SettingsStore(path: directory.appendingPathComponent("settings.json").path),
            tokens: { Tokens.variants["dark-mustard"]! }, appearanceChanged: { _, _, _ in },
            settingsPersisted: { _ in loaded.fulfill() }, showHistory: {}, updateChecks: model)
        model.didChange = { [weak controller] in controller?.refreshUpdateChecks() }
        controller.revealUpdates() // Also exercises the not-yet-loaded navigation.
        wait(for: [loaded], timeout: 5)
        let recorder = try XCTUnwrap(controller.shortcutRecorder(identifier: "new_capture_shortcut"))
        recorder.performClick(nil)
        XCTAssertTrue(controller.isRecordingShortcut)
        XCTAssertTrue(model.check())
        XCTAssertTrue(controller.shortcutRecorder(identifier: "new_capture_shortcut") === recorder)
        XCTAssertTrue(controller.isRecordingShortcut)
        transport.checking = false
        transport.status = "Native update signature verification failed."
        model.poll()
        XCTAssertTrue(controller.isRecordingShortcut, "a result must not cancel shortcut input")
        XCTAssertEqual(try XCTUnwrap(find("updates.status", in: root) as? NSTextField).stringValue, transport.status)
        XCTAssertTrue(try XCTUnwrap(find("updates.check", in: root) as? CaptureButton).isEnabled)
        let updates = try XCTUnwrap(find("preferences-card.updates", in: root))
        let about = try XCTUnwrap(find("preferences-card.about", in: root))
        XCTAssertGreaterThanOrEqual(about.frame.minY, updates.frame.maxY)
        controller.handleShortcutInput(code: "Escape", control: false, shift: false, alt: false, meta: false)
        controller.flush()
    }

    func testOptionsRequireAllFlagsExplicitLivePathsAndNoHealthOrFixtures() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
        defer { try? FileManager.default.removeItem(at: directory) }
        let key = directory.appendingPathComponent("public.key")
        try "untrusted comment: minisign public key E7620F1842B4E81F\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3".write(to: key, atomically: false, encoding: .utf8)
        let flags = ["--native-update-manifest-url", "http://127.0.0.1:9/native.json",
                     "--native-update-public-key-file", key.path, "--native-update-current-version", "2026.9.99"]
        let base = ["--live", "--history-root", directory.appendingPathComponent("history").path,
                    "--settings-file", directory.appendingPathComponent("settings.json").path]
        let options = try Options(base + flags)
        defer { options.nativeUpdateChecks?.shutdown {} }
        XCTAssertNotNil(options.nativeUpdateChecks)
        XCTAssertNil(try Options(["--live"]).nativeUpdateChecks)
        let staged = try Options(base + flags + ["--native-update-staging-directory", directory.path])
        defer { staged.nativeUpdateChecks?.shutdown {} }
        XCTAssertNotNil(staged.nativeUpdateChecks)
        let retained = directory.appendingPathComponent("unread-missing-base").path
        let deltaFlags = ["--native-update-staging-directory", directory.path,
                          "--native-update-base-archive", retained]
        let delta = try Options(base + flags + deltaFlags)
        defer { delta.nativeUpdateChecks?.shutdown {} }
        XCTAssertNotNil(delta.nativeUpdateChecks)
        XCTAssertFalse(FileManager.default.fileExists(atPath: retained))
        XCTAssertThrowsError(try Options(base + flags + ["--native-update-base-archive", retained]))
        let shutdownFlags = ["--native-update-shutdown-intent-file", directory.appendingPathComponent("intent.json").path]
        XCTAssertThrowsError(try Options(base + flags + shutdownFlags), "requires staging")
        XCTAssertThrowsError(try Options(base + flags + deltaFlags + shutdownFlags), "must not enroll an arbitrary profile")
        XCTAssertThrowsError(try Options(base + flags + ["--native-update-staging-directory", directory.path,
            "--native-update-base-archive", "relative"]))
        XCTAssertThrowsError(try Options(base + flags + deltaFlags + ["--native-update-base-archive", retained]))
        for path in ["relative", directory.appendingPathComponent("missing").path, key.path] {
            XCTAssertThrowsError(try Options(base + flags + ["--native-update-staging-directory", path]))
        }
        XCTAssertThrowsError(try Options(base + ["--native-update-staging-directory", directory.path]))
        XCTAssertThrowsError(try Options(base + flags + ["--native-update-staging-directory", directory.path,
            "--native-update-staging-directory", directory.path]))
        for invalid in [Array(base.dropFirst()) + flags, base + Array(flags.dropLast(2)),
            ["--live", "--history-root", "relative", "--settings-file", "/settings"] + flags,
            ["--live", "--history-root", "/history", "--settings-file", "relative"] + flags,
            base + flags + ["--scene", "idle"], base + flags + ["--exercise"],
            base + flags + ["--open-media", "/tmp/capture.png"],
            base + flags + ["--native-update-current-version", "2026.9.99"],
            base + flags + ["--native-update-ready-file", "/ready", "--native-update-ready-token", "123e4567-e89b-42d3-a456-426614174000"],
        ] as [[String]] { XCTAssertThrowsError(try Options(invalid)) }
        XCTAssertThrowsError(try Options(Array(base.dropFirst()) + flags, bundled: true))
        XCTAssertFalse(FileManager.default.fileExists(atPath: directory.appendingPathComponent("settings.json").path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: directory.appendingPathComponent("history").path))
    }

    func testTrayChecksAreDisabledByDefaultAndRouteOnlyWhenConfigured() {
        _ = NSApplication.shared
        var checks = 0
        let target = LiveStatusActions(newCapture: {}, capture: { _ in }, history: {}, preferences: {},
            checkUpdates: { checks += 1 }, outputFolder: {}, quit: {})
        for _ in 0..<2 {
            let menu = target.makeMenu()
            XCTAssertTrue(menu.items[12].isEnabled)
            menu.performActionForItem(at: 12)
        }
        XCTAssertEqual(checks, 2, "menu rebuilds must keep the explicit check action")
        let disabled = LiveStatusActions(newCapture: {}, capture: { _ in }, history: {}, preferences: {}, outputFolder: {}, quit: {})
        XCTAssertFalse(disabled.makeMenu().items[12].isEnabled)
    }

    private func find(_ identifier: String, in view: NSView) -> NSView? {
        if view.identifier?.rawValue == identifier { return view }
        for child in view.subviews { if let result = find(identifier, in: child) { return result } }
        return nil
    }
}
