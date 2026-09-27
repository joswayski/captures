import XCTest
@testable import CapturesNative

/// Records requests and answers the reset without touching the TCC database.
private final class RecoveryResetTransport: SettingsTransport {
    private(set) var requests: [[String: Any]] = []

    func request(_ object: [String: Any]) throws -> [String: Any] {
        requests.append(object)
        if object["operation"] as? String == "permission_recovery_reset" { return ["ok": true] }
        return try SettingsBridge().request(object)
    }
}

final class PermissionRecoveryTests: XCTestCase {
    private var directory: URL!
    private var settingsPath: String!

    override func setUpWithError() throws {
        directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        // A missing file loads as defaults; the first retry writes it.
        settingsPath = directory.appendingPathComponent("settings.json").path
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: directory)
    }

    func testSharedPromptMatchesTheShippingDialogs() throws {
        let recovery = try CapturePermissionRecovery(settingsPath: settingsPath)
        let prompt = try XCTUnwrap(try recovery.prompt())
        XCTAssertEqual(prompt.title, "Captures Setup")
        XCTAssertEqual(prompt.cancel, "Not Now")
        switch prompt.recovery {
        case .restart:
            XCTAssertEqual(prompt.confirm, "Restart & Retry")
            XCTAssertFalse(prompt.critical)
            XCTAssertTrue(prompt.message.hasPrefix("macOS requires Captures to restart"))
        case .resetAndRestart:
            XCTAssertEqual(prompt.confirm, "Reset, Restart & Retry")
            XCTAssertTrue(prompt.critical)
            XCTAssertTrue(prompt.message.contains("reset only its own record"))
        }
    }

    func testOnlyDeniedCapturesOfferRecovery() throws {
        let recovery = try CapturePermissionRecovery(settingsPath: settingsPath)
        XCTAssertTrue(recovery.isPermissionDenied("Couldn’t start capture: screen capture permission was denied"))
        XCTAssertFalse(recovery.isPermissionDenied("Couldn’t start capture: screen capture permission was requested"))
        XCTAssertFalse(recovery.isPermissionDenied("Couldn’t start capture: Capture cancelled"))
        XCTAssertEqual(recovery.failureMessage("tccutil exited with status 1"),
            "Captures could not reset or restart its Screen Recording setup: tccutil exited with status 1")
    }

    func testScheduledRetryIsTakenOnce() throws {
        let recovery = try CapturePermissionRecovery(settingsPath: settingsPath)
        XCTAssertNil(try recovery.takePendingCapture())
        for kind in [StillCaptureKind.region, .window, .display] {
            try recovery.scheduleRetry(kind)
            XCTAssertEqual(try recovery.takePendingCapture(), kind)
            XCTAssertNil(try recovery.takePendingCapture())
        }
        XCTAssertEqual(CapturePermissionRecovery.kind(for: "window"), .window)
        XCTAssertNil(CapturePermissionRecovery.kind(for: "everything"))
        XCTAssertEqual(CapturePermissionRecovery.mode(for: .display), "display")
    }

    func testResetNeedsABundleIdentifierAndTargetsOnlyThisProfile() throws {
        let transport = RecoveryResetTransport()
        let recovery = try CapturePermissionRecovery(settingsPath: settingsPath, transport: transport)
        XCTAssertThrowsError(try recovery.resetScreenPermission(bundleIdentifier: nil))
        XCTAssertThrowsError(try recovery.resetScreenPermission(bundleIdentifier: ""))
        XCTAssertTrue(transport.requests.isEmpty)
        try recovery.resetScreenPermission(bundleIdentifier: "es.captures.native")
        let request = try XCTUnwrap(transport.requests.last)
        XCTAssertEqual(request["operation"] as? String, "permission_recovery_reset")
        XCTAssertEqual(request["bundle_id"] as? String, "es.captures.native")
        XCTAssertEqual(request["path"] as? String, settingsPath)
    }

    func testRelaunchKeepsTheProfileArguments() throws {
        let options = try Options(["--live", "--settings-file", "/tmp/s.json", "--history-root", "/tmp/h"])
        let arguments = permissionRestartArguments(options: options, pendingMedia: [])
        XCTAssertEqual(arguments, ["--live", "--scene", "preferences", "--history-root", "/tmp/h",
                                   "--settings-file", "/tmp/s.json"])
    }
}
