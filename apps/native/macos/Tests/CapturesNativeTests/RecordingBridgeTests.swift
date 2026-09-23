import Foundation
import XCTest
@testable import CapturesNative

final class RecordingBridgeTests: XCTestCase {
    func testLifecycleGateRejectsRapidAndConflictingActionsUntilCompletion() {
        var gate = RecordingLifecycleGate()
        XCTAssertTrue(gate.begin())
        XCTAssertTrue(gate.busy)
        XCTAssertFalse(gate.begin(), "a second Pause, Stop, or Discard cannot queue")
        gate.end()
        XCTAssertFalse(gate.busy)
        XCTAssertTrue(gate.begin(), "the next lifecycle action is allowed after completion")
    }

    func testGenerationGateChangesWithoutReentrantBridgeWork() {
        let gate = RecordingGenerationGate()
        XCTAssertFalse(gate.accepts(41))
        gate.set(41)
        XCTAssertTrue(gate.accepts(41))
        XCTAssertFalse(gate.accepts(42))
        gate.set(nil)
        XCTAssertFalse(gate.accepts(41))
    }

    func testMicrophoneSamplerBoundsRequestsAndRejectsStaleCompletion() throws {
        let queue = DispatchQueue(label: "fixture.microphone")
        let started = DispatchSemaphore(value: 0)
        let release = DispatchSemaphore(value: 0)
        var reads = 0
        var levels: [Double] = []
        var useful = true
        let sampler = RecordingMicrophoneSampler(queue: queue, read: {
            reads += 1
            if reads == 1 { started.signal(); _ = release.wait(timeout: .now() + 3) }
            return reads == 1 ? 0.81 : 0.17
        }, useful: { useful }, interval: 60, deliver: { levels.append($0) })
        sampler.setActive(true)
        sampler.sample()
        XCTAssertEqual(started.wait(timeout: .now() + 3), .success)
        sampler.sample()
        XCTAssertEqual(reads, 1, "a slow read cannot enqueue another read")
        sampler.setActive(false) // hide / pause / mute / new take
        sampler.setActive(true)
        release.signal()
        var drained = false
        queue.async { DispatchQueue.main.async { drained = true } }
        try waitUntil { drained }
        XCTAssertEqual(levels, [0], "old visible take cannot publish after restoration")
        sampler.sample()
        try waitUntil { levels.count == 2 }
        XCTAssertEqual(levels, [0, 0.17], "fresh sampling resumes without overlapping the old request")
        useful = false // lifecycle work is in progress on the same serialized queue
        sampler.sample()
        XCTAssertEqual(reads, 2, "no meter request may queue behind lifecycle work")
        sampler.setActive(false)
        XCTAssertEqual(levels, [0, 0.17, 0])

        let blockedQueue = DispatchQueue(label: "fixture.microphone.lifecycle")
        let unblock = DispatchSemaphore(value: 0)
        blockedQueue.async { _ = unblock.wait(timeout: .now() + 3) }
        var staleReads = 0
        let pending = RecordingMicrophoneSampler(queue: blockedQueue, read: {
            staleReads += 1; return 0.92
        }, useful: { true }, interval: 60, deliver: { _ in })
        pending.setActive(true); pending.sample(); pending.setActive(false)
        unblock.signal()
        var settled = false
        blockedQueue.async { DispatchQueue.main.async { settled = true } }
        try waitUntil { settled }
        XCTAssertEqual(staleReads, 0, "queued work invalidated before execution must not touch the old session")
    }

    func testPreparedSessionMicrophoneReadDoesNotMutateSnapshot() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let options: [String: Any] = ["kind": "video", "target": ["type": "display", "display_id": "fixture"],
            "frames_per_second": 15, "max_resolution": "original", "countdown_seconds": 0,
            "show_cursor": false]
        let display: [String: Any] = ["id": "fixture", "name": "Fixture", "x": 0, "y": 0,
            "width": 640, "height": 360, "scale_factor": 1.0, "is_primary": true]
        let (session, initial) = try NativeRecordingSession.prepare(
            recoveryRoot: root.path, options: options, display: display)
        XCTAssertEqual(try session.microphoneLevel(), 0)
        XCTAssertEqual(try session.snapshot(), initial)
        _ = try session.discard()
    }

    private func waitUntil(_ condition: () -> Bool) throws {
        let deadline = Date().addingTimeInterval(3)
        while !condition() && Date() < deadline {
            RunLoop.main.run(until: Date().addingTimeInterval(0.01))
        }
        XCTAssertTrue(condition())
        guard condition() else { throw AppBridgeError.invalidResponse }
    }

    func testExplicitMediaToolsAreVerifiedBeforeRecording() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("captures-tools-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let tool = directory.appendingPathComponent("media-tool")
        try Data("#!/bin/sh\nexit 0\n".utf8).write(to: tool)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: tool.path)

        let tools = try NativeMediaTools.locate(environment: [
            "CAPTURES_FFMPEG": tool.path, "CAPTURES_FFPROBE": tool.path,
        ], executable: nil)
        XCTAssertEqual(tools.ffmpeg, tool.path)
        XCTAssertEqual(tools.ffprobe, tool.path)
        XCTAssertNoThrow(try tools.verify())
    }
}
