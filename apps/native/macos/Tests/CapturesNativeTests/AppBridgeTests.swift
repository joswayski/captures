import XCTest
@testable import CapturesNative

final class AppBridgeTests: XCTestCase {
    func testCapturePreferencesRespectCopyOffAndExportChoice() throws {
        let off = try CapturePreferences(["auto_copy_to_clipboard": false,
            "output_directory": "/exports/custom", "screenshot_format": "webp"])
        XCTAssertFalse(off.autoCopy)
        XCTAssertEqual(off.directory, "/exports/custom")
        XCTAssertEqual(off.format, "webp")
        let on = try CapturePreferences(["auto_copy_to_clipboard": true,
            "output_directory": "/exports/jpeg", "screenshot_format": "jpeg"])
        XCTAssertTrue(on.autoCopy)
        XCTAssertEqual(on.format, "jpeg")
        XCTAssertThrowsError(try CapturePreferences([:]))
        XCTAssertThrowsError(try CapturePreferences(["auto_copy_to_clipboard": false,
            "output_directory": "/exports", "screenshot_format": "gif"]))
    }

    func testDecodesResultEnvelope() throws {
        let response = try AppBridge.decode(Data(#"{"ok":true,"result":{"kind":"history_root","path":"/native/history"}}"#.utf8))
        XCTAssertEqual(response["path"] as? String, "/native/history")
    }

    func testBackendErrorIsPreserved() {
        XCTAssertThrowsError(try AppBridge.decode(Data(#"{"ok":false,"error":"screen access denied"}"#.utf8))) { error in
            XCTAssertEqual(error as? AppBridgeError, .backend("screen access denied"))
        }
    }

    func testMalformedSuccessIsRejected() {
        XCTAssertThrowsError(try AppBridge.decode(Data(#"{"ok":true}"#.utf8))) { error in
            XCTAssertEqual(error as? AppBridgeError, .invalidResponse)
        }
    }

    func testArtifactRequiresPathsAndHistoryMetadata() {
        XCTAssertNil(CaptureArtifact(["entry": ["id": "missing"]]))
        let artifact = CaptureArtifact(["entry": ["id": "one", "width": 800, "height": 600,
            "created_at": "2026-09-18T00:00:00Z"], "image_path": "/image.png", "preview_path": "/preview.png"])
        XCTAssertEqual(artifact?.id, "one")
        XCTAssertEqual(artifact?.width, 800)
    }

    func testLiveOptionValidationAndExplicitRoot() throws {
        let options = try Options(["--live", "--history-root", "/tmp/native-history"])
        XCTAssertTrue(options.live); XCTAssertEqual(options.historyRoot, "/tmp/native-history")
        XCTAssertThrowsError(try Options(["--live", "--exercise"]))
        XCTAssertThrowsError(try Options(["--live", "--reference-chips"]))
        XCTAssertThrowsError(try Options(["--history-root", "/tmp/not-live"]))
    }
}
