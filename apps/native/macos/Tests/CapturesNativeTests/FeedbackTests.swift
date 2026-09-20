import AppKit
import XCTest
@testable import CapturesNative

final class FeedbackTests: XCTestCase {
    private final class Transport {
        var context: Result<[String: Any], Error> = .success([
            "app_version": "0.1.0-native", "os": "macos", "os_version": "26.0", "arch": "aarch64"])
        var drafts: [[String: Any]] = []
        var finish: ((Result<[String: Any], Error>) -> Void)?
        func request(_ object: [String: Any], completion: @escaping (Result<[String: Any], Error>) -> Void) {
            if object.string("operation") == "context" { completion(context); return }
            drafts.append(object["draft"] as! [String: Any]); finish = completion
        }
    }

    func testExplicitSendBusyFailureRetryAndRetainedDraft() throws {
        _ = NSApplication.shared
        let transport = Transport()
        let form = FeedbackController(tokens: try XCTUnwrap(Tokens.variants["dark-mustard"]), live: true, request: transport.request)
        defer { form.dismiss() }
        XCTAssertTrue(transport.drafts.isEmpty)
        XCTAssertFalse(form.sendButton.isEnabled)
        form.message.string = "  selector overlaps the toolbar  "
        form.contact.stringValue = "   "; form.category.selectItem(at: 1)
        form.submit(); form.submit()
        XCTAssertEqual(transport.drafts.count, 1)
        XCTAssertEqual(transport.drafts[0]["message"] as? String, "selector overlaps the toolbar")
        XCTAssertEqual(transport.drafts[0]["category"] as? String, "idea")
        XCTAssertTrue(transport.drafts[0]["contact"] is NSNull)
        XCTAssertEqual(transport.drafts[0].count, 3)
        XCTAssertFalse(form.message.isEditable); XCTAssertFalse(form.category.isEnabled)
        XCTAssertFalse(form.contact.isEnabled); XCTAssertFalse(form.sendButton.isEnabled)
        form.dismiss()
        transport.finish?(.failure(AppBridgeError.backend("Offline — try again")))
        XCTAssertEqual(form.message.string, "  selector overlaps the toolbar  ")
        XCTAssertEqual(form.status.stringValue, "Offline — try again")
        XCTAssertTrue(form.sendButton.isEnabled)
        form.contact.stringValue = " @example "; form.submit()
        XCTAssertEqual(transport.drafts[1]["contact"] as? String, "@example")
        transport.finish?(.success([:]))
        XCTAssertEqual(form.message.string, "")
        XCTAssertEqual(form.contact.stringValue, " @example ")
        XCTAssertEqual(form.status.stringValue, "Thanks — feedback sent.")
        XCTAssertFalse(form.sendButton.isEnabled)
    }

    func testFixtureInvalidLimitsAndMissingContextNeverSend() throws {
        _ = NSApplication.shared
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        for live in [false, true] {
            let transport = Transport()
            let form = FeedbackController(tokens: tokens, live: live, request: transport.request)
            defer { form.dismiss() }
            form.message.string = String(repeating: "🦀", count: 8000)
            form.contact.stringValue = String(repeating: "é", count: 200)
            form.updateControls(); XCTAssertEqual(form.sendButton.isEnabled, live)
            form.message.string += "x"; form.submit()
            XCTAssertTrue(transport.drafts.isEmpty)
            form.message.string = "Valid message"; form.contact.stringValue += "x"; form.submit()
            XCTAssertTrue(transport.drafts.isEmpty)
            if !live {
                form.contact.stringValue = ""; form.submit()
                XCTAssertTrue(transport.drafts.isEmpty)
            }
        }
        let transport = Transport()
        transport.context = .failure(AppBridgeError.backend("Unavailable"))
        let form = FeedbackController(tokens: tokens, live: true, request: transport.request)
        defer { form.dismiss() }
        form.message.string = "A valid draft"; form.submit()
        XCTAssertTrue(transport.drafts.isEmpty)
        XCTAssertEqual(form.sendButton.title, "Retry details")
    }

    func testFeedbackStatesUseLiveComponentsAndFitInBothAppearances() throws {
        _ = NSApplication.shared
        for appearance in ["dark", "light"] {
            let transport = Transport()
            let form = FeedbackController(tokens: try XCTUnwrap(Tokens.variants["\(appearance)-mustard"]), live: true, request: transport.request)
            defer { form.dismiss() }
            let view = try XCTUnwrap(form.window.contentView)
            for child in view.subviews { XCTAssertTrue(view.bounds.contains(child.frame), "\(child)") }
            XCTAssertEqual(form.message.accessibilityLabel(), "Feedback message")
            XCTAssertEqual(form.contact.accessibilityLabel(), "Contact (optional)")
            XCTAssertFalse(form.window.isReleasedWhenClosed)
            try capture(view, "\(appearance)-empty")
            form.message.string = "The recording selector overlaps the toolbar on my second display."
            form.submit()
            try capture(view, "\(appearance)-sending")
            transport.finish?(.failure(AppBridgeError.backend("Could not reach the feedback service. Check your connection and try again.")))
            try capture(view, "\(appearance)-error")
            form.submit(); transport.finish?(.success([:]))
            try capture(view, "\(appearance)-sent")
        }
    }

    private func capture(_ view: NSView, _ state: String) throws {
        guard let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] else { return }
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        let url = URL(fileURLWithPath: directory).appendingPathComponent("feedback-\(state).png")
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: url)
    }
}
