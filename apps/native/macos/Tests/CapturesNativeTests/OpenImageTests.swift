import AppKit
import XCTest
@testable import CapturesNative

final class OpenImageTests: XCTestCase {
    func testSettingsFailureDoesNotImportBeforeEditorIsReady() throws {
        _ = NSApplication.shared
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame); window.contentView = root
        let transport = OpenImageTransport(image: folder.appendingPathComponent("source.png").path)
        let controller = LiveCaptureController(root: root, window: window,
            tokens: try XCTUnwrap(Tokens.variants["light-mustard"]),
            historyRoot: folder.path, settingsPath: folder.path, transport: transport,
            showPreferences: {})
        defer { withExtendedLifetime(controller) {} }
        controller.openImages(["/first.png", "/second.png"])
        try waitUntil { !controller.externalOpenPending && root.subviews.compactMap {
            ($0 as? NSTextField)?.stringValue
        }.contains { $0.contains("Couldn’t open 2 files") } }
        XCTAssertTrue(transport.requests.isEmpty, "settings failure cannot create History items")
    }

    func testRealBridgeOpensExternalPNGIntoHistoryAndDecodedEditor() throws {
        _ = NSApplication.shared
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let source = folder.appendingPathComponent("outside.png")
        try XCTUnwrap(NSBitmapImageRep(cgImage: PreviewView.fixtureImage(scale: 1))
            .representation(using: .png, properties: [:])).write(to: source)
        let history = folder.appendingPathComponent("history")
        let settingsPath = folder.appendingPathComponent("settings.json").path
        let settingsBridge = SettingsBridge()
        var settings = try XCTUnwrap(settingsBridge.request([
            "operation": "load", "path": settingsPath])["settings"] as? [String: Any])
        settings["output_directory"] = folder.path
        _ = try settingsBridge.request(["operation": "save", "path": settingsPath, "settings": settings])
        let frame = NSRect(x: 0, y: 0, width: 1280, height: 800)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame); window.contentView = root
        let controller = LiveCaptureController(root: root, window: window,
            tokens: try XCTUnwrap(Tokens.variants["dark-mustard"]),
            historyRoot: history.path, settingsPath: settingsPath, showPreferences: {})
        defer { withExtendedLifetime(controller) {} }
        window.makeKeyAndOrderFront(nil)
        controller.openImages([source.path])
        let table = try XCTUnwrap(root.subviews.compactMap { $0 as? NSScrollView }
            .first?.documentView as? NSTableView)
        try waitUntil { table.numberOfRows == 1 && table.selectedRow == 0 }
        try waitUntil { NSApp.windows.contains { $0.title.hasPrefix("Edit screenshot") && $0.isVisible } }
        let editor = try XCTUnwrap(NSApp.windows.first { $0.title.hasPrefix("Edit screenshot") && $0.isVisible })
        defer {
            editor.performClose(nil)
            if let sheet = editor.attachedSheet {
                editor.endSheet(sheet, returnCode: .alertSecondButtonReturn)
                RunLoop.current.run(until: Date().addingTimeInterval(0.05))
            }
            editor.orderOut(nil)
        }
        try waitUntil { editor.contentView.map { descendants($0).compactMap { $0 as? NSImageView }
            .contains { $0.image != nil && $0.accessibilityLabel() == "Edited screenshot preview" } } == true }
        if let output = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"],
           let content = editor.contentView {
            try capture(content, to: URL(fileURLWithPath: output)
                .appendingPathComponent("external-open-dark-editor-normal.png"))
        }
        let controls = try XCTUnwrap(editor.contentView)
        for (label, value) in [("Crop X", "7"), ("Crop Y", "11"),
                               ("Crop width", "120"), ("Crop height", "80")] {
            try XCTUnwrap(descendants(controls).compactMap { $0 as? NSTextField }
                .first { $0.accessibilityLabel() == label }).stringValue = value
        }
        try XCTUnwrap(descendants(controls).compactMap { $0 as? CaptureButton }
            .first { $0.title == "Apply crop" }).performClick(nil)
        try waitUntil { editor.title.contains("Unsaved") }
        let artifacts = try XCTUnwrap(AppBridge().request([
            "operation": "history", "root": history.path])["artifacts"] as? [[String: Any]])
        XCTAssertEqual(artifacts.count, 1)
        XCTAssertEqual((artifacts[0]["entry"] as? [String: Any])?["kind"] as? String, "screenshot")
        XCTAssertNotEqual(artifacts[0]["image_path"] as? String, source.path,
                          "the external source is copied into owned History")
        let entry = try XCTUnwrap(artifacts[0]["entry"] as? [String: Any])
        let id = try XCTUnwrap(entry["id"] as? String)
        let media = try XCTUnwrap(artifacts[0]["image_path"] as? String)
        let before = try Data(contentsOf: URL(fileURLWithPath: media))
        let again = try AppBridge().request(["operation": "open_image", "root": history.path,
                                             "path": source.path, "open_artifact_ids": [id]])
        XCTAssertEqual(again["already_open"] as? Bool, true)
        XCTAssertEqual(((again["artifact"] as? [String: Any])?["entry"] as? [String: Any])?["id"] as? String, id)
        XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: media)), before,
                       "a canonical already-open source cannot replace owned media")
        controller.openImages([source.path])
        try waitUntil { !controller.externalOpenPending }
        XCTAssertTrue(editor.isVisible)
        XCTAssertTrue(editor.title.contains("Unsaved"), "duplicate focus must retain staged edits")
        XCTAssertTrue(descendants(controls).compactMap { ($0 as? NSTextField)?.stringValue }
            .contains { $0.contains("120 × 80") }, "duplicate focus must retain the accepted crop")
        XCTAssertEqual(try XCTUnwrap(AppBridge().request([
            "operation": "history", "root": history.path])["artifacts"] as? [[String: Any]]).count, 1)

        editor.performClose(nil)
        editor.endSheet(try XCTUnwrap(editor.attachedSheet), returnCode: .alertSecondButtonReturn)
        try waitUntil { !editor.isVisible }
        controller.openImages([source.path])
        try waitUntil { !controller.externalOpenPending && editor.isVisible
            && !editor.title.contains("Unsaved") }
        let reopened = try XCTUnwrap(AppBridge().request([
            "operation": "history", "root": history.path])["artifacts"] as? [[String: Any]])
        XCTAssertEqual(reopened.count, 1)
        XCTAssertEqual(((reopened[0]["entry"] as? [String: Any])?["id"] as? String), id,
                       "closing without saving reloads the canonical source under the same History ID")
    }

    func testRealExternalRecordingsOpenWithoutChangingSourceOrAllowingReplace() throws {
        _ = NSApplication.shared
        guard let tools = try? NativeMediaTools.locate() else {
            throw XCTSkip("ffmpeg and ffprobe are required")
        }
        let originalPath = getenv("PATH").map { String(cString: $0) }
        let originalFFmpeg = getenv("CAPTURES_FFMPEG").map { String(cString: $0) }
        let originalFFprobe = getenv("CAPTURES_FFPROBE").map { String(cString: $0) }
        defer {
            for (key, value) in [("PATH", originalPath), ("CAPTURES_FFMPEG", originalFFmpeg),
                                 ("CAPTURES_FFPROBE", originalFFprobe)] {
                if let value { setenv(key, value, 1) } else { unsetenv(key) }
            }
        }
        for (container, suffix, appearance) in [("mp4", "mp4", "dark"),
                                                ("gif", "gif", "light"),
                                                ("webm", "mp4", "dark"),
                                                ("webm", "bin", "light")] {
            let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: folder) }
            let source = folder.appendingPathComponent("outside.\(suffix)")
            let process = Process(); process.executableURL = URL(fileURLWithPath: tools.ffmpeg)
            process.arguments = ["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi",
                "-i", "color=c=red:size=160x90:rate=10:duration=1", "-f", "lavfi",
                "-i", "color=c=blue:size=160x90:rate=10:duration=1", "-filter_complex",
                "[0:v][1:v]concat=n=2:v=1:a=0[v]", "-map", "[v]"]
                + (container == "gif" ? ["-f", "gif"] : ["-c:v", container == "webm" ? "libvpx-vp9" : "mpeg4",
                    "-f", container]) + [source.path]
            try process.run(); process.waitUntilExit()
            XCTAssertEqual(process.terminationStatus, 0)
            if suffix == "bin" {
                setenv("CAPTURES_FFMPEG", tools.ffmpeg, 1)
                setenv("CAPTURES_FFPROBE", tools.ffprobe, 1)
                setenv("PATH", "", 1)
                let configured = try NativeMediaTools.locate(environment: [
                    "CAPTURES_FFMPEG": tools.ffmpeg, "CAPTURES_FFPROBE": tools.ffprobe,
                    "PATH": "",
                ])
                XCTAssertEqual(configured.ffmpeg, tools.ffmpeg)
                XCTAssertEqual(configured.ffprobe, tools.ffprobe)
            }
            let original = try Data(contentsOf: source)
            let history = folder.appendingPathComponent("History")
            let settingsPath = folder.appendingPathComponent("settings.json").path
            let bridge = SettingsBridge()
            var settings = try XCTUnwrap(bridge.request([
                "operation": "load", "path": settingsPath])["settings"] as? [String: Any])
            settings["output_directory"] = folder.path
            _ = try bridge.request(["operation": "save", "path": settingsPath, "settings": settings])
            let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
            let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            defer { window.close() }
            let root = Surface(frame: frame); window.contentView = root
            let tokens = try XCTUnwrap(Tokens.variants["\(appearance)-mustard"])
            root.wantsLayer = true; root.layer?.backgroundColor = tokens.color("surface-canvas").cgColor
            window.appearance = NSAppearance(named: appearance == "light" ? .aqua : .darkAqua)
            let controller = LiveCaptureController(root: root, window: window,
                tokens: tokens,
                historyRoot: history.path, settingsPath: settingsPath, showPreferences: {})
            defer { withExtendedLifetime(controller) {} }
            window.makeKeyAndOrderFront(nil)
            controller.openImages(container == "gif" ? ["/unsupported.tiff", source.path] : [source.path])
            let table = try XCTUnwrap(root.subviews.compactMap { $0 as? NSScrollView }
                .first?.documentView as? NSTableView)
            try waitUntil { table.numberOfRows == 1 && !controller.externalOpenPending }
            if container == "gif" {
                XCTAssertTrue(root.subviews.compactMap { ($0 as? NSTextField)?.stringValue }
                    .contains { $0.contains("/unsupported.tiff") },
                    "a failed file must remain visible while the later recording opens")
                if let output = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] {
                    try capture(root, to: URL(fileURLWithPath: output)
                        .appendingPathComponent("external-media-light-error-minimum.png"))
                }
            }
            let editor = try XCTUnwrap(NSApp.windows.first { $0.title == "Recording editor" && $0.isVisible })
            defer { editor.orderOut(nil) }
            let controls = try XCTUnwrap(editor.contentView)
            XCTAssertNotNil(descendants(controls).compactMap { $0 as? NSImageView }
                .first { $0.accessibilityLabel() == "Decoded recording frame" }?.image,
                "the first source-relative frame must decode in the recording editor")
            let artifacts = try XCTUnwrap(NativeRecordingInfo.request([
                "operation": "history", "root": history.path])["recordings"] as? [[String: Any]])
            let entry = try XCTUnwrap(artifacts.first?["entry"] as? [String: Any])
            let id = try XCTUnwrap(entry["id"] as? String)
            XCTAssertEqual(entry["kind"] as? String, container == "gif" ? "gif" : "video")
            XCTAssertEqual(entry["mime_type"] as? String,
                           container == "gif" ? "image/gif" : "video/\(container)")
            let canonical = try XCTUnwrap(source.path.withCString { realpath($0, nil) })
            defer { free(canonical) }
            XCTAssertEqual(entry["saved_path"] as? String, String(cString: canonical))
            XCTAssertFalse(try XCTUnwrap(descendants(controls).compactMap { $0 as? CaptureButton }
                .first { $0.title == "Replace original…" }).isEnabled,
                "external references must not offer destructive replacement")
            if let output = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"],
               container != "webm" {
                try capture(controls, to: URL(fileURLWithPath: output)
                    .appendingPathComponent("external-media-\(appearance)-\(container)-normal.png"))
                if container == "gif" {
                    editor.setContentSize(NSSize(width: 760, height: 540))
                    try capture(controls, to: URL(fileURLWithPath: output)
                        .appendingPathComponent("external-media-light-gif-minimum.png"))
                }
            }
            let trim = try XCTUnwrap(descendants(controls).compactMap { $0 as? NSTextField }
                .first { $0.accessibilityLabel() == "Trim start milliseconds" })
            let recordingEditor = try XCTUnwrap(editor.delegate as? RecordingEditorController)
            trim.stringValue = "200"
            recordingEditor.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                              object: trim))
            controller.openImages([source.path])
            try waitUntil { !controller.externalOpenPending }
            XCTAssertEqual(trim.stringValue, "200", "canonical focus preserves staged recording edits")
            XCTAssertTrue(editor.isVisible)
            XCTAssertEqual(try XCTUnwrap(NativeRecordingInfo.request([
                "operation": "history", "root": history.path])["recordings"] as? [[String: Any]]).count, 1)
            trim.stringValue = "0"
            recordingEditor.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                              object: trim))
            editor.performClose(nil)
            try waitUntil { !editor.isVisible }
            controller.openImages([source.path])
            try waitUntil { !controller.externalOpenPending && editor.isVisible }
            let reopened = try XCTUnwrap(NativeRecordingInfo.request([
                "operation": "history", "root": history.path])["recordings"] as? [[String: Any]])
            XCTAssertEqual((reopened.first?["entry"] as? [String: Any])?["id"] as? String, id)
            XCTAssertEqual(try Data(contentsOf: source), original, "opening and refocusing must not rewrite source")
            editor.performClose(nil)
        }
    }

    func testRealBatchWaitsForFirstEditorBeforeOpeningDistinctSecondImage() throws {
        _ = NSApplication.shared
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let first = folder.appendingPathComponent("first.png")
        let second = folder.appendingPathComponent("second.png")
        try XCTUnwrap(NSBitmapImageRep(cgImage: PreviewView.fixtureImage(scale: 1))
            .representation(using: .png, properties: [:])).write(to: first)
        let secondImage = PreviewView.fixtureImage(scale: 2)
        try XCTUnwrap(NSBitmapImageRep(cgImage: secondImage)
            .representation(using: .png, properties: [:])).write(to: second)
        let history = folder.appendingPathComponent("history")
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame); window.contentView = root
        let controller = LiveCaptureController(root: root, window: window,
            tokens: try XCTUnwrap(Tokens.variants["light-mustard"]),
            historyRoot: history.path, settingsPath: folder.appendingPathComponent("settings.json").path,
            showPreferences: {})
        defer { withExtendedLifetime(controller) {} }
        window.makeKeyAndOrderFront(nil)
        controller.openImages([first.path, second.path])
        let table = try XCTUnwrap(root.subviews.compactMap { $0 as? NSScrollView }
            .first?.documentView as? NSTableView)
        try waitUntil { table.numberOfRows == 2 && !controller.externalOpenPending }
        let editor = try XCTUnwrap(NSApp.windows.first { $0.title.hasPrefix("Edit screenshot") && $0.isVisible })
        defer { editor.performClose(nil) }
        let dimensions = try XCTUnwrap(descendants(try XCTUnwrap(editor.contentView))
            .compactMap { $0 as? NSTextField }
            .first { $0.accessibilityLabel() == "Edited canvas dimensions" })
        XCTAssertTrue(dimensions.stringValue.contains("\(secondImage.width) × \(secondImage.height)"),
                      "the second distinct source must open after the first editor settles")
        let artifacts = try XCTUnwrap(AppBridge().request([
            "operation": "history", "root": history.path])["artifacts"] as? [[String: Any]])
        XCTAssertEqual(artifacts.count, 2)
        XCTAssertEqual(table.selectedRow, 0, "the last imported source is selected")
    }

    func testQueuedFilesRetainErrorsAndSerializeCanonicalOpens() throws {
        _ = NSApplication.shared
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let png = folder.appendingPathComponent("source.png")
        try XCTUnwrap(NSBitmapImageRep(cgImage: PreviewView.fixtureImage(scale: 1))
            .representation(using: .png, properties: [:])).write(to: png)
        let settingsPath = folder.appendingPathComponent("settings.json").path
        let settingsBridge = SettingsBridge()
        var settings = try XCTUnwrap(settingsBridge.request([
            "operation": "load", "path": settingsPath])["settings"] as? [String: Any])
        settings["output_directory"] = folder.path
        _ = try settingsBridge.request(["operation": "save", "path": settingsPath, "settings": settings])
        let transport = OpenImageTransport(image: png.path, withDisplay: true, realHistory: true)
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame); window.contentView = root
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        root.wantsLayer = true; root.layer?.backgroundColor = tokens.color("surface-canvas").cgColor
        window.appearance = NSAppearance(named: .aqua)
        let controller = LiveCaptureController(root: root, window: window,
            tokens: tokens,
            historyRoot: folder.path, settingsPath: settingsPath, transport: transport,
            showPreferences: {})
        defer { withExtendedLifetime(controller) {} }
        defer { NSApp.windows.filter { $0.title.hasPrefix("Edit screenshot") }.forEach { $0.orderOut(nil) } }
        window.makeKeyAndOrderFront(nil)
        try waitUntil { root.subviews.compactMap { $0 as? CaptureButton }
            .first { $0.title == "Capture display" }?.isEnabled == true }
        controller.setPermissionsVisible(true)
        controller.openImages(["/invalid.tiff", png.path, png.path])
        LiveCaptureController.flush()
        XCTAssertEqual(transport.requests.count, 0, "Permission recovery retains queued media")
        XCTAssertFalse(controller.capture(.display))
        XCTAssertFalse(controller.newCapture())
        controller.setPermissionsVisible(false)
        XCTAssertFalse(controller.prepareEditorForTermination(), "queued startup opens block teardown")
        try waitUntil { transport.firstOpenStarted.wait(timeout: .now()) == .success }
        XCTAssertEqual(transport.requests.count, 1)
        XCTAssertTrue(controller.externalOpenPending)
        XCTAssertFalse(controller.capture(.display), "a global capture shortcut must not race image open")
        XCTAssertFalse(controller.newCapture(), "the global New Capture shortcut shares the gate")
        XCTAssertFalse(try XCTUnwrap(root.subviews.compactMap { $0 as? CaptureButton }
            .first { $0.title == "Edit recording" }).isEnabled,
            "the empty-History editor action stays disabled during the import")
        transport.releaseFirstOpen.signal()
        try waitUntil { transport.requests.count == 3 && !controller.externalOpenPending }
        XCTAssertTrue(controller.prepareEditorForTermination())
        let requests = transport.requests
        XCTAssertEqual(requests.compactMap { $0["path"] as? String },
                       ["/invalid.tiff", png.path, png.path])
        XCTAssertTrue(requests.allSatisfy { ($0["root"] as? String) == folder.path })
        XCTAssertEqual(requests[0]["open_artifact_ids"] as? [String], [])
        XCTAssertEqual(requests[1]["open_artifact_ids"] as? [String], [])
        let table = try XCTUnwrap(root.subviews.compactMap { $0 as? NSScrollView }
            .first?.documentView as? NSTableView)
        XCTAssertEqual(table.numberOfRows, 1)
        XCTAssertEqual(table.selectedRow, 0)
        let artifacts = try XCTUnwrap(AppBridge().request([
            "operation": "history", "root": folder.path])["artifacts"] as? [[String: Any]])
        let entry = try XCTUnwrap(artifacts.first?["entry"] as? [String: Any])
        XCTAssertEqual(requests[2]["open_artifact_ids"] as? [String], [try XCTUnwrap(entry["id"] as? String)])
        XCTAssertTrue(root.subviews.compactMap { ($0 as? NSTextField)?.stringValue }
            .contains { $0.contains("/invalid.tiff") && $0.contains("Unsupported") })
        if let output = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] {
            try capture(root, to: URL(fileURLWithPath: output)
                .appendingPathComponent("external-open-light-error-minimum.png"))
        }
        XCTAssertEqual(transport.operations.filter { $0 == "open_media" }.count, 3)
        XCTAssertFalse(transport.operations.contains("request_permission"))
    }

    func testSelectionChangedDuringOpenDoesNotStealFocus() throws {
        _ = NSApplication.shared
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let png = folder.appendingPathComponent("source.png")
        try XCTUnwrap(NSBitmapImageRep(cgImage: PreviewView.fixtureImage(scale: 1))
            .representation(using: .png, properties: [:])).write(to: png)
        let transport = OpenImageTransport(image: png.path, existing: true)
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame); window.contentView = root
        let controller = LiveCaptureController(root: root, window: window,
            tokens: try XCTUnwrap(Tokens.variants["dark-mustard"]),
            historyRoot: folder.path, settingsPath: nil, transport: transport,
            showPreferences: {})
        defer { withExtendedLifetime(controller) {} }
        window.makeKeyAndOrderFront(nil)
        let table = try XCTUnwrap(root.subviews.compactMap { $0 as? NSScrollView }
            .first?.documentView as? NSTableView)
        try waitUntil { table.numberOfRows == 2 }
        controller.openImages([png.path])
        try waitUntil { transport.firstOpenStarted.wait(timeout: .now()) == .success }
        table.selectRowIndexes([1], byExtendingSelection: false)
        transport.releaseFirstOpen.signal()
        try waitUntil { !controller.externalOpenPending }
        XCTAssertEqual(table.selectedRow, 1, "late History must keep the newer selection")
        XCTAssertFalse(NSApp.windows.contains { $0.title.hasPrefix("Edit screenshot") && $0.isVisible })
    }

    func testSupersedingHistoryDoesNotOpenStaleEditorOrKeepBusyLatch() throws {
        _ = NSApplication.shared
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let source = folder.appendingPathComponent("source.png")
        try XCTUnwrap(NSBitmapImageRep(cgImage: PreviewView.fixtureImage(scale: 1))
            .representation(using: .png, properties: [:])).write(to: source)
        let settingsPath = folder.appendingPathComponent("settings.json").path
        let transport = OpenImageTransport(image: source.path)
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame); window.contentView = root
        let controller = LiveCaptureController(root: root, window: window,
            tokens: try XCTUnwrap(Tokens.variants["dark-mustard"]),
            historyRoot: folder.path, settingsPath: settingsPath, transport: transport,
            showPreferences: {})
        defer { withExtendedLifetime(controller) {} }
        let table = try XCTUnwrap(root.subviews.compactMap { $0 as? NSScrollView }
            .first?.documentView as? NSTableView)
        try waitUntil { transport.operations.contains("history") }
        // Wait for the first History callback, not just its worker response.
        try waitUntil { !controller.externalOpenPending && table.numberOfRows == 0 }
        let gate = transport.blockNextHistory()
        controller.openImages([source.path])
        try waitUntil { transport.firstOpenStarted.wait(timeout: .now()) == .success }
        transport.releaseFirstOpen.signal()
        try waitUntil { transport.blockedHistoryStarted.wait(timeout: .now()) == .success }
        controller.refreshHistory()
        gate.signal()
        try waitUntil { !controller.externalOpenPending && table.numberOfRows == 1 }
        XCTAssertTrue(controller.prepareEditorForTermination())
        XCTAssertFalse(NSApp.windows.contains { $0.title.hasPrefix("Edit screenshot") && $0.isVisible })
    }

    private func waitUntil(_ condition: () -> Bool) throws {
        let deadline = Date().addingTimeInterval(5)
        while Date() < deadline {
            if condition() { return }
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        XCTFail("external open did not settle")
        throw AppBridgeError.invalidResponse
    }

    private func descendants(_ view: NSView) -> [NSView] {
        view.subviews + view.subviews.flatMap(descendants)
    }

    private func capture(_ view: NSView, to path: URL) throws {
        view.window?.display(); view.layoutSubtreeIfNeeded()
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        try FileManager.default.createDirectory(at: path.deletingLastPathComponent(),
                                                withIntermediateDirectories: true)
        try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: path)
    }
}

private final class OpenImageTransport: AppTransport {
    let firstOpenStarted = DispatchSemaphore(value: 0)
    let releaseFirstOpen = DispatchSemaphore(value: 0)
    let blockedHistoryStarted = DispatchSemaphore(value: 0)
    private let lock = NSLock()
    private let image: String
    private let existing: Bool
    private let withDisplay: Bool
    private let realHistory: Bool
    private var opened = false
    private var seen: [[String: Any]] = []
    private var calls: [String] = []
    private var blockedHistory: DispatchSemaphore?
    var requests: [[String: Any]] { lock.lock(); defer { lock.unlock() }; return seen }
    var operations: [String] { lock.lock(); defer { lock.unlock() }; return calls }

    init(image: String, existing: Bool = false, withDisplay: Bool = false,
         realHistory: Bool = false) {
        self.image = image; self.existing = existing; self.withDisplay = withDisplay
        self.realHistory = realHistory
    }

    func blockNextHistory() -> DispatchSemaphore {
        let gate = DispatchSemaphore(value: 0)
        lock.lock(); blockedHistory = gate; lock.unlock()
        return gate
    }

    func request(_ object: [String: Any]) throws -> [String: Any] {
        let operation = object["operation"] as? String ?? ""
        lock.lock(); calls.append(operation); lock.unlock()
        switch operation {
        case "history":
            lock.lock()
            let gate = blockedHistory
            blockedHistory = nil
            lock.unlock()
            if let gate {
                blockedHistoryStarted.signal()
                _ = gate.wait(timeout: .now() + 5)
            }
            if realHistory { return try AppBridge().request(object) }
            lock.lock(); let isOpened = opened; lock.unlock()
            let artifact = entry("opened-id")
            return ["artifacts": existing
                ? [artifact, entry("original-id")]
                : (isOpened ? [artifact] : [])]
        case "displays": return ["displays": withDisplay ? [[
            "id": "fixture", "name": "Fixture", "width": 1000, "height": 720,
            "x": 0, "y": 0, "scale_factor": 1, "is_primary": true,
        ]] : []]
        case "open_media":
            lock.lock(); seen.append(object); let count = seen.count; lock.unlock()
            if count == 1 {
                firstOpenStarted.signal()
                _ = releaseFirstOpen.wait(timeout: .now() + 5)
            }
            if object["path"] as? String == "/invalid.tiff" {
                throw AppBridgeError.backend("Unsupported image format")
            }
            if realHistory { return try AppBridge().request(object) }
            lock.lock(); opened = true; lock.unlock()
            return ["artifact": entry("opened-id"), "already_open": count > 2 || existing]
        default: throw AppBridgeError.invalidResponse
        }
    }

    private func entry(_ id: String) -> [String: Any] {
        ["entry": ["id": id, "kind": "screenshot", "width": 284, "height": 160,
                   "created_at": id == "opened-id" ? "2026-09-24T00:00:00Z" : "2026-09-23T00:00:00Z"],
         "image_path": image, "preview_path": image]
    }
}
