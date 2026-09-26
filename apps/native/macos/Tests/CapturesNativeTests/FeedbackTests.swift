import AppKit
import XCTest
@testable import CapturesNative

final class FeedbackTests: XCTestCase {
    private final class Transport {
        var context: Result<[String: Any], Error> = .success([
            "app_version": "0.1.0-native", "os": "macos", "os_version": "26.0", "arch": "aarch64",
            "system_label": "macos · 26.0 · aarch64"])
        var holdContext = false
        var finishContext: (() -> Void)?
        var drafts: [[String: Any]] = []
        var finish: ((Result<[String: Any], Error>) -> Void)?
        func request(_ object: [String: Any], completion: @escaping (Result<[String: Any], Error>) -> Void) {
            if object.string("operation") == "context" {
                let context = self.context
                if holdContext { finishContext = { completion(context) } } else { completion(context) }
                return
            }
            drafts.append(object["draft"] as! [String: Any]); finish = completion
        }
    }

    /// Keep every test window within a 600 pt tall screen.
    private func makeForm(_ tokens: Tokens, live: Bool, transport: Transport) -> FeedbackController {
        let form = FeedbackController(tokens: tokens, live: live, request: transport.request)
        form.window.setContentSize(NSSize(width: 640, height: 560))
        form.layoutForm()
        return form
    }

    func testExplicitSendBusyFailureRetryAndRetainedDraft() throws {
        _ = NSApplication.shared
        let transport = Transport()
        let form = makeForm(try XCTUnwrap(Tokens.variants["dark-mustard"]), live: true, transport: transport)
        defer { form.dismiss() }
        XCTAssertTrue(transport.drafts.isEmpty)
        XCTAssertFalse(form.sendButton.isEnabled)
        form.message.string = "  selector overlaps the toolbar  "
        form.contact.stringValue = "   "; form.categoryButtons[1].performClick(nil)
        XCTAssertEqual(form.selectedCategory, 1)
        form.submit(); form.submit()
        XCTAssertEqual(transport.drafts.count, 1)
        XCTAssertEqual(transport.drafts[0]["message"] as? String, "selector overlaps the toolbar")
        XCTAssertEqual(transport.drafts[0]["category"] as? String, "idea")
        XCTAssertTrue(transport.drafts[0]["contact"] is NSNull)
        XCTAssertEqual(transport.drafts[0].count, 3)
        XCTAssertFalse(form.message.isEditable)
        XCTAssertFalse(form.categoryButtons.contains { $0.isEnabled })
        XCTAssertFalse(form.contact.isEnabled); XCTAssertFalse(form.sendButton.isEnabled)
        XCTAssertEqual(form.sendButton.title, "Sending…")
        form.dismiss()
        transport.finish?(.failure(AppBridgeError.backend("Offline — try again")))
        XCTAssertEqual(form.message.string, "  selector overlaps the toolbar  ")
        XCTAssertEqual(form.status.stringValue, "Offline — try again")
        XCTAssertFalse(form.statusBox.isHidden)
        XCTAssertTrue(form.sendButton.isEnabled)
        form.contact.stringValue = " @example "; form.submit()
        XCTAssertEqual(transport.drafts[1]["contact"] as? String, "@example")
        transport.finish?(.success([:]))
        XCTAssertEqual(form.message.string, "")
        XCTAssertEqual(form.contact.stringValue, " @example ")
        XCTAssertEqual(form.status.stringValue, "Thanks — feedback sent.")
        XCTAssertFalse(form.sendButton.isEnabled)
        XCTAssertFalse(form.messagePlaceholder.isHidden, "an emptied message shows its placeholder again")
    }

    func testFixtureInvalidLimitsAndMissingContextNeverSend() throws {
        _ = NSApplication.shared
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        for live in [false, true] {
            let transport = Transport()
            let form = makeForm(tokens, live: live, transport: transport)
            defer { form.dismiss() }
            form.message.string = String(repeating: "🦀", count: 8000)
            form.contact.stringValue = String(repeating: "é", count: 200)
            form.updateControls(); XCTAssertEqual(form.sendButton.isEnabled, live)
            XCTAssertEqual(form.status.stringValue, live ? "" : "Fixture mode — sending feedback is disabled.")
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
        let form = makeForm(tokens, live: true, transport: transport)
        defer { form.dismiss() }
        form.message.string = "A valid draft"; form.submit()
        XCTAssertTrue(transport.drafts.isEmpty)
        XCTAssertEqual(form.sendButton.title, "Retry details")
        XCTAssertEqual(form.status.stringValue, "Unavailable")
    }

    func testShippingCopyPlaceholdersAndLoadingDetails() throws {
        _ = NSApplication.shared
        let transport = Transport()
        transport.holdContext = true
        let form = makeForm(try XCTUnwrap(Tokens.variants["dark-mustard"]), live: true, transport: transport)
        defer { form.dismiss() }
        XCTAssertEqual(form.window.title, "Send Feedback")
        XCTAssertNil(form.window.sheetParent)
        XCTAssertTrue(form.window.styleMask.contains(.resizable))
        XCTAssertEqual(form.window.contentMinSize, NSSize(width: 460, height: 460))
        XCTAssertTrue(FeedbackCopy.text("intro").hasSuffix(
            "Captures sends what you type here plus the app and system details listed below."))
        XCTAssertEqual(FeedbackCopy.text("contact_help"),
                       "Optional — we may use this if we need to ask a follow-up question.")
        XCTAssertEqual(form.categoryButtons.map(\.optionTitle), ["Bug", "Idea", "Other"])
        XCTAssertEqual(form.categoryButtons[0].optionDetail, "Something is broken or unexpected")
        XCTAssertEqual(form.categoryButtons[0].accessibilityRole(), .radioButton)
        XCTAssertEqual(form.messagePlaceholder.stringValue, "What happened? What did you expect?")
        form.categoryButtons[1].performClick(nil)
        XCTAssertEqual(form.messagePlaceholder.stringValue, "What's the idea? What problem would it solve?")
        form.categoryButtons[2].performClick(nil)
        XCTAssertEqual(form.messagePlaceholder.stringValue, "What would you like us to know?")
        XCTAssertTrue(form.categoryButtons[2].active); XCTAssertFalse(form.categoryButtons[0].active)
        // Shipping shows "…" for each detail until the local context arrives.
        XCTAssertEqual(form.versionValue.stringValue, "…")
        XCTAssertEqual(form.systemValue.stringValue, "…")
        let deadline = Date().addingTimeInterval(2)
        while transport.finishContext == nil, Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        try XCTUnwrap(transport.finishContext)()
        XCTAssertEqual(form.versionValue.stringValue, "0.1.0-native")
        XCTAssertEqual(form.systemValue.stringValue, "macos · 26.0 · aarch64")
        form.message.string = "Something"; form.updateControls()
        XCTAssertTrue(form.messagePlaceholder.isHidden)
    }

    func testFeedbackStatesUseLiveComponentsAndFitInBothAppearances() throws {
        _ = NSApplication.shared
        for appearance in ["dark", "light"] {
            let transport = Transport()
            let form = makeForm(try XCTUnwrap(Tokens.variants["\(appearance)-mustard"]), live: true, transport: transport)
            defer { form.dismiss() }
            XCTAssertLessThanOrEqual(form.window.frame.height, 600)
            let document = form.document
            for child in document.subviews where !child.isHidden {
                XCTAssertTrue(document.bounds.contains(child.frame), "\(child)")
            }
            // Three equal radio cards in one row; right-aligned footer with the
            // status to its left.
            let cards = form.categoryButtons.map { $0.convert($0.bounds, to: document) }
            XCTAssertEqual(Set(cards.map(\.minY)).count, 1)
            XCTAssertLessThan(cards[0].maxX, cards[1].minX); XCTAssertLessThan(cards[1].maxX, cards[2].minX)
            XCTAssertGreaterThanOrEqual(cards[0].height, 62)
            let send = form.sendButton.frame
            XCTAssertEqual(send.maxX, document.bounds.width - 24, accuracy: 1)
            XCTAssertEqual(form.message.accessibilityLabel(), "Feedback message")
            XCTAssertEqual(form.contact.accessibilityLabel(), "Contact (optional)")
            XCTAssertFalse(form.window.isReleasedWhenClosed)
            try capture(document, "\(appearance)-empty")
            form.message.string = "The recording selector overlaps the toolbar on my second display."
            form.submit()
            try capture(document, "\(appearance)-sending")
            transport.finish?(.failure(AppBridgeError.backend("Could not reach the feedback service. Check your connection and try again.")))
            XCTAssertLessThan(form.statusBox.frame.maxX, form.sendButton.frame.minX)
            XCTAssertGreaterThan(form.statusBox.frame.height, 0)
            try capture(document, "\(appearance)-error")
            form.submit(); transport.finish?(.success([:]))
            try capture(document, "\(appearance)-sent")
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
