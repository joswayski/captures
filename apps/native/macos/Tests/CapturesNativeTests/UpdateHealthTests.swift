import XCTest
@testable import CapturesNative

final class UpdateHealthTests: XCTestCase {
    private let token = "123e4567-e89b-42d3-a456-426614174000"

    func testOptionsRequireCompleteExplicitLiveInvocation() throws {
        let base = ["--live", "--history-root", "/profile/history", "--settings-file", "/profile/settings.json",
                    "--native-update-ready-file", "/private/ready", "--native-update-ready-token", token]
        let options = try Options(base)
        XCTAssertEqual(options.nativeUpdateReadyFile, "/private/ready")
        XCTAssertEqual(options.nativeUpdateReadyToken, token)

        for invalid in [
            Array(base.dropFirst()),
            base.filter { $0 != "--native-update-ready-token" && $0 != token },
            ["--live", "--history-root", "relative", "--settings-file", "/settings"] + Array(base.suffix(4)),
            ["--live", "--history-root", "/history", "--settings-file", "relative"] + Array(base.suffix(4)),
            Array(base.dropLast()) + [token.uppercased()],
            Array(base.dropLast()) + ["123e4567-e89b-12d3-a456-426614174000"],
            Array(base.dropLast()) + [token + "\n"],
            base + ["--open-media", "/tmp/capture.png"],
            base + ["--exercise"],
            base + ["--scene", "idle"],
            base + ["--screenshot", "/tmp/screenshot.png"],
            base + ["--native-update-ready-token", token],
            base + ["--update-state", "available"],
        ] { XCTAssertThrowsError(try Options(invalid), "accepted \(invalid)") }
        XCTAssertThrowsError(try Options(Array(base.dropFirst()), bundled: true),
                             "a bundled launch must still include explicit --live")
    }

    func testAcknowledgementWritesExactBytesOnlyOnce() throws {
        let fixture = try makeEmptyFile()
        defer { try? FileManager.default.removeItem(at: fixture.directory) }
        let acknowledgement = try UpdateHealthAcknowledgement(file: fixture.file.path, token: token)
        try acknowledgement.acknowledge()
        try acknowledgement.acknowledge()
        XCTAssertEqual(try Data(contentsOf: fixture.file), Data("\(token)\n".utf8))
    }

    func testAcknowledgementRejectsUnsafeOrOccupiedTargets() throws {
        let fixture = try makeEmptyFile()
        defer { try? FileManager.default.removeItem(at: fixture.directory) }
        try Data("occupied".utf8).write(to: fixture.file)
        XCTAssertThrowsError(try UpdateHealthAcknowledgement(file: fixture.file.path, token: token))
        let directoryTarget = fixture.directory.appendingPathComponent("target-directory")
        try FileManager.default.createDirectory(at: directoryTarget, withIntermediateDirectories: false)
        XCTAssertThrowsError(try UpdateHealthAcknowledgement(file: directoryTarget.path, token: token))
        let link = fixture.directory.appendingPathComponent("ready-link")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: fixture.file)
        XCTAssertThrowsError(try UpdateHealthAcknowledgement(file: link.path, token: token))
        XCTAssertThrowsError(try UpdateHealthAcknowledgement(
            file: fixture.directory.appendingPathComponent("missing").path, token: token))
    }

    func testOrderingFailuresAndTerminationSuppressAcknowledgement() throws {
        try assertCoordinator(events: [.workspace], verifies: false, writes: false)
        try assertCoordinator(events: [.settings], verifies: false, writes: false)
        try assertCoordinator(events: [.workspace, .settings], verificationSucceeds: false,
                              verifies: true, writes: false)
        try assertCoordinator(events: [.workspace, .cancel, .settings], verifies: false, writes: false)
        try assertCoordinator(events: [.settings, .workspace], verifies: true, writes: true)
    }

    func testTerminationDuringToolVerificationSuppressesAcknowledgement() throws {
        let fixture = try makeEmptyFile()
        defer { try? FileManager.default.removeItem(at: fixture.directory) }
        let acknowledgement = try UpdateHealthAcknowledgement(file: fixture.file.path, token: token)
        let verificationStarted = expectation(description: "verification started")
        let releaseVerification = DispatchSemaphore(value: 0)
        let queue = DispatchQueue(label: "test.update-health.termination")
        let coordinator = UpdateHealthCoordinator(acknowledgement: acknowledgement, queue: queue) {
            XCTAssertFalse(Thread.isMainThread)
            verificationStarted.fulfill()
            releaseVerification.wait()
        }
        coordinator.workspaceDidRender()
        coordinator.settingsDidLoad()
        wait(for: [verificationStarted], timeout: 1)
        coordinator.cancel()
        releaseVerification.signal()
        queue.sync {}
        XCTAssertEqual(try Data(contentsOf: fixture.file), Data())
    }

    func testPackagedToolsUseOnlyExecutableSiblingBinaries() throws {
        let fixture = try makeEmptyFile()
        defer { try? FileManager.default.removeItem(at: fixture.directory) }
        let executable = fixture.directory.appendingPathComponent("CapturesNative")
        let binaries = fixture.directory.appendingPathComponent("binaries")
        try FileManager.default.createDirectory(at: binaries, withIntermediateDirectories: false)
        #if arch(arm64)
        let target = "aarch64-apple-darwin"
        #else
        let target = "x86_64-apple-darwin"
        #endif
        for name in ["ffmpeg", "ffprobe"] {
            let tool = binaries.appendingPathComponent("\(name)-\(target)")
            try Data("#!/bin/sh\nexit 0\n".utf8).write(to: tool)
            try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: tool.path)
        }
        let tools = try NativeMediaTools.packaged(executable: executable)
        XCTAssertEqual(tools.ffmpeg, binaries.appendingPathComponent("ffmpeg-\(target)").path)
        XCTAssertEqual(tools.ffprobe, binaries.appendingPathComponent("ffprobe-\(target)").path)
    }

    private enum Event { case workspace, settings, cancel }

    private func assertCoordinator(events: [Event], verificationSucceeds: Bool = true,
                                   verifies expectedVerification: Bool, writes: Bool) throws {
        let fixture = try makeEmptyFile()
        defer { try? FileManager.default.removeItem(at: fixture.directory) }
        let acknowledgement = try UpdateHealthAcknowledgement(file: fixture.file.path, token: token)
        let completed = expectation(description: "verification")
        completed.isInverted = !expectedVerification
        let queue = DispatchQueue(label: "test.update-health.ordering")
        let coordinator = UpdateHealthCoordinator(acknowledgement: acknowledgement, queue: queue) {
            completed.fulfill()
            if !verificationSucceeds { throw UpdateHealthError.invalidFile }
        }
        for event in events {
            switch event {
            case .workspace: coordinator.workspaceDidRender()
            case .settings: coordinator.settingsDidLoad()
            case .cancel: coordinator.cancel()
            }
        }
        wait(for: [completed], timeout: expectedVerification ? 1 : 0.05)
        queue.sync {}
        XCTAssertEqual((try Data(contentsOf: fixture.file)) == Data("\(token)\n".utf8), writes)
    }

    private func makeEmptyFile() throws -> (directory: URL, file: URL) {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("captures-update-health-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
        let file = directory.appendingPathComponent("ready")
        XCTAssertTrue(FileManager.default.createFile(atPath: file.path, contents: Data()))
        return (directory, file)
    }
}
