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

    func testRecordingPreferencesBuildVideoOptionsWithoutDroppingStoredSettings() throws {
        let preferences = try RecordingPreferences([
            "video_fps": 30, "video_max_resolution": "p1080", "countdown_seconds": 7,
            "show_cursor": false, "highlight_clicks": true, "show_keystrokes": true,
            "capture_system_audio": true, "microphone_device_id": "studio-mic",
            "mono_audio": true, "gif_max_width": 640, "gif_max_colors": 128,
        ])
        let options = preferences.options(target: ["type": "display", "display_id": "7"],
            capabilities: NativeRecordingCapabilities([
                "system_audio": true, "microphone": true, "cursor_control": true,
                "click_highlights": true, "controls_excluded": true,
            ])!)
        XCTAssertEqual(options["kind"] as? String, "video")
        XCTAssertEqual(options["frames_per_second"] as? Int, 30)
        XCTAssertEqual(options["max_resolution"] as? String, "p1080")
        XCTAssertEqual(options["show_keystrokes"] as? Bool, true)
        let audio = try XCTUnwrap(options["audio"] as? [String: Any])
        XCTAssertEqual(audio["capture_system_audio"] as? Bool, true)
        XCTAssertEqual(audio["microphone_device_id"] as? String, "studio-mic")
        XCTAssertEqual(audio["mono_output"] as? Bool, true)

        var controls = RecordingControlState(framesPerSecond: 60, maxResolution: "original",
            showCursor: true, highlightClicks: false, systemAudio: false,
            microphoneDeviceID: nil)
        let offOptions = nativeRecordingOptions(preferences: preferences,
            target: ["type": "display", "display_id": "7"],
            capabilities: NativeRecordingCapabilities([
                "system_audio": true, "microphone": true, "cursor_control": true,
                "click_highlights": true, "controls_excluded": true,
            ])!, controls: controls)
        let offAudio = try XCTUnwrap(offOptions["audio"] as? [String: Any])
        XCTAssertTrue(offAudio["microphone_device_id"] is NSNull,
            "choosing Off must override the stored microphone")

        controls.microphoneDeviceID = "built-in-mic"
        let selectedOptions = nativeRecordingOptions(preferences: preferences,
            target: ["type": "display", "display_id": "7"],
            capabilities: NativeRecordingCapabilities([
                "system_audio": true, "microphone": true, "cursor_control": true,
                "click_highlights": true, "controls_excluded": true,
            ])!, controls: controls)
        let selectedAudio = try XCTUnwrap(selectedOptions["audio"] as? [String: Any])
        XCTAssertEqual(selectedAudio["microphone_device_id"] as? String, "built-in-mic",
            "the picker must pass the actual selected device ID")
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
            "created_at": "2026-09-18T00:00:00Z", "mode": "window"],
            "image_path": "/image.png", "preview_path": "/preview.png"])
        XCTAssertEqual(artifact?.id, "one")
        XCTAssertEqual(artifact?.width, 800)
        XCTAssertEqual(artifact?.mode, "window")
        let legacy = CaptureArtifact(["entry": ["id": "legacy", "width": 1, "height": 1,
            "created_at": "2026-09-18T00:00:00Z"], "image_path": "/image.png",
            "preview_path": "/preview.png"])
        XCTAssertEqual(legacy?.mode, "region")
    }

    func testLiveOptionValidationAndExplicitRoot() throws {
        let options = try Options(["--live", "--history-root", "/tmp/native-history"])
        XCTAssertTrue(options.live); XCTAssertEqual(options.historyRoot, "/tmp/native-history")
        let images = try Options(["--open-image", "/tmp/red.png", "--live",
                                  "--open-image", "/tmp/blue.webp"])
        XCTAssertEqual(images.openImages, ["/tmp/red.png", "/tmp/blue.webp"])
        XCTAssertEqual(try Options(["--scene", "window"]).scene, "window")
        XCTAssertThrowsError(try Options(["--live", "--exercise"]))
        XCTAssertThrowsError(try Options(["--live", "--reference-chips"]))
        XCTAssertThrowsError(try Options(["--history-root", "/tmp/not-live"]))
        XCTAssertThrowsError(try Options(["--open-image", "/tmp/red.png"]))
        XCTAssertThrowsError(try Options(["--open-image"]))
        XCTAssertThrowsError(try Options(["--live", "--open-image", ""]))
    }
}
