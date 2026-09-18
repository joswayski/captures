import XCTest
@testable import CapturesNative

final class AppBridgeTests: XCTestCase {
    func testCapturePreferencesRespectCopyOffAndExportChoice() throws {
        let off = try CapturePreferences(["auto_copy_to_clipboard": false,
            "show_cursor_in_screenshots": true, "freeze_screen": false, "auto_start_on_selection": true,
            "show_mini_previews": false, "mini_preview_placement": "top_left",
            "include_mini_previews_in_captures": true,
            "output_directory": "/exports/custom", "screenshot_format": "webp", "screenshot_countdown_seconds": 0])
        XCTAssertFalse(off.autoCopy)
        XCTAssertTrue(off.includeCursor)
        XCTAssertFalse(off.freezeScreen)
        XCTAssertTrue(off.autoStart)
        XCTAssertEqual(off.directory, "/exports/custom")
        XCTAssertEqual(off.format, "webp")
        XCTAssertEqual(off.miniPreviewSettings,
            MiniPreviewSettings(enabled: false, placement: "top_left", includeInCaptures: true))
        let on = try CapturePreferences(["auto_copy_to_clipboard": true,
            "show_cursor_in_screenshots": false, "freeze_screen": true, "auto_start_on_selection": false,
            "show_mini_previews": true, "mini_preview_placement": "bottom_right",
            "include_mini_previews_in_captures": false,
            "output_directory": "/exports/jpeg", "screenshot_format": "jpeg", "screenshot_countdown_seconds": 7])
        XCTAssertTrue(on.autoCopy)
        XCTAssertFalse(on.includeCursor)
        XCTAssertTrue(on.freezeScreen)
        XCTAssertFalse(on.autoStart)
        XCTAssertEqual(on.format, "jpeg")
        XCTAssertEqual(on.countdown, 7)
        XCTAssertThrowsError(try CapturePreferences([:]))
        XCTAssertThrowsError(try CapturePreferences(["auto_copy_to_clipboard": false,
            "show_cursor_in_screenshots": false, "freeze_screen": true, "auto_start_on_selection": false,
            "show_mini_previews": true, "mini_preview_placement": "bottom_right",
            "include_mini_previews_in_captures": false,
            "output_directory": "/exports", "screenshot_format": "gif", "screenshot_countdown_seconds": 0]))
        XCTAssertThrowsError(try CapturePreferences(["auto_copy_to_clipboard": false,
            "show_cursor_in_screenshots": false, "freeze_screen": true, "auto_start_on_selection": false,
            "show_mini_previews": true, "mini_preview_placement": "bottom_right",
            "include_mini_previews_in_captures": false,
            "output_directory": "/exports", "screenshot_format": "png", "screenshot_countdown_seconds": 11]))
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
        XCTAssertEqual(try Options(["--scene", "window"]).scene, "window")
        XCTAssertThrowsError(try Options(["--live", "--exercise"]))
        XCTAssertThrowsError(try Options(["--live", "--reference-chips"]))
        XCTAssertThrowsError(try Options(["--history-root", "/tmp/not-live"]))
    }
}
