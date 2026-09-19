import Foundation
import XCTest
@testable import CapturesNative

final class RecordingBridgeTests: XCTestCase {
    func testGenerationGateChangesWithoutReentrantBridgeWork() {
        let gate = RecordingGenerationGate()
        XCTAssertFalse(gate.accepts(41))
        gate.set(41)
        XCTAssertTrue(gate.accepts(41))
        XCTAssertFalse(gate.accepts(42))
        gate.set(nil)
        XCTAssertFalse(gate.accepts(41))
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
