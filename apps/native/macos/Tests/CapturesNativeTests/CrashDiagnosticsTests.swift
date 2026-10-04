import AppKit
import XCTest
@testable import CapturesNative

private final class CrashTransport: CrashDiagnosticsTransport {
    var responses: [[String: Any]]
    private(set) var operations: [String] = []
    init(_ responses: [[String: Any]]) { self.responses = responses }
    func request(_ object: [String: Any]) throws -> [String: Any] {
        operations.append(object.string("operation"))
        return responses.removeFirst()
    }
}

final class CrashDiagnosticsTests: XCTestCase {
    private let retained: [String: Any] = [
        "title": "Previous session did not close normally",
        "explanation": "A forced stop or power loss is not proof of a crash. Nothing has been sent.",
        "summary": "Visible local summary",
        "unclean_exit": true,
        "has_exception_evidence": false,
    ]

    func testStartCleanAndFailedRestartResumeOrdering() throws {
        let transport = CrashTransport([["preview": retained], [:], [:], [:]])
        let diagnostics = try CrashDiagnostics.start(historyRoot: "/private/profile", transport: transport)
        XCTAssertEqual(diagnostics.preview?.summary, "Visible local summary")
        try diagnostics.markClean()
        try diagnostics.markClean() // replacement-process markers must not be removed later
        try diagnostics.resume()
        try diagnostics.dismiss()
        XCTAssertEqual(transport.operations, ["start", "clean_exit", "resume", "dismiss"])
        XCTAssertNil(diagnostics.preview)
    }

    func testReviewCopyIsLocalAndDistinguishesUnconfirmedEvidence() throws {
        _ = NSApplication.shared
        let preview = try XCTUnwrap(CrashPreview(retained))
        let tokens = try XCTUnwrap(Tokens.variants["dark-mustard"])
        var message = ""
        let choice = CrashReview.present(preview, tokens: tokens) { alert in
            message = alert.messageText + "\n" + alert.informativeText
            XCTAssertEqual(alert.buttons.map(\.title), ["Copy Summary", "Add to Feedback", "Dismiss", "Later"])
            return .copy
        }
        XCTAssertEqual(choice, .copy)
        XCTAssertTrue(message.contains("Previous session did not close normally"))
        XCTAssertTrue(message.contains("power loss is not proof"))
        XCTAssertTrue(message.contains("Nothing has been sent"))
    }
}
