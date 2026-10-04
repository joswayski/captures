import AppKit
import CCapturesSettings

struct CrashPreview: Equatable {
    let title: String
    let explanation: String
    let summary: String
    let uncleanExit: Bool
    let hasExceptionEvidence: Bool

    init?(_ value: [String: Any]) {
        guard let title = value["title"] as? String,
              let explanation = value["explanation"] as? String,
              let summary = value["summary"] as? String,
              let uncleanExit = value["unclean_exit"] as? Bool,
              let hasExceptionEvidence = value["has_exception_evidence"] as? Bool else { return nil }
        self.title = title; self.explanation = explanation; self.summary = summary
        self.uncleanExit = uncleanExit; self.hasExceptionEvidence = hasExceptionEvidence
    }
}

protocol CrashDiagnosticsTransport {
    func request(_ object: [String: Any]) throws -> [String: Any]
}

final class CrashDiagnosticsBridge: CrashDiagnosticsTransport {
    func request(_ object: [String: Any]) throws -> [String: Any] {
        precondition(Thread.isMainThread)
        let data = try JSONSerialization.data(withJSONObject: object)
        let pointer = String(decoding: data, as: UTF8.self).withCString { captures_crash_request_v1($0) }
        guard let pointer else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(pointer) }
        return try AppBridge.decode(Data(bytes: pointer, count: strlen(pointer)))
    }
}

/// Owns the marker only while this process owns the live-instance lock.
final class CrashDiagnostics {
    private let transport: CrashDiagnosticsTransport
    private(set) var preview: CrashPreview?
    private var armed = true

    private init(transport: CrashDiagnosticsTransport, preview: CrashPreview?) {
        self.transport = transport; self.preview = preview
    }

    static func start(historyRoot: String?, transport: CrashDiagnosticsTransport = CrashDiagnosticsBridge()) throws -> CrashDiagnostics {
        precondition(Thread.isMainThread)
        let value = try transport.request(["operation": "start",
            "history_root": historyRoot.map { $0 as Any } ?? NSNull()])
        return CrashDiagnostics(transport: transport, preview: try decodePreview(value))
    }

    private static func decodePreview(_ value: [String: Any]) throws -> CrashPreview? {
        guard let raw = value["preview"] else { throw AppBridgeError.invalidResponse }
        if raw is NSNull { return nil }
        guard let object = raw as? [String: Any], let preview = CrashPreview(object) else {
            throw AppBridgeError.invalidResponse
        }
        return preview
    }

    func markClean() throws {
        precondition(Thread.isMainThread)
        guard armed else { return }
        _ = try transport.request(["operation": "clean_exit"])
        armed = false
    }

    func resume() throws {
        precondition(Thread.isMainThread)
        guard !armed else { return }
        _ = try transport.request(["operation": "resume"])
        armed = true
    }

    func dismiss() throws {
        precondition(Thread.isMainThread)
        _ = try transport.request(["operation": "dismiss"])
        preview = nil
    }
}

enum CrashReviewChoice: Equatable { case copy, addToFeedback, dismiss, later }

enum CrashReview {
    static func present(_ preview: CrashPreview, tokens: Tokens,
                        choose: ((NSAlert) -> CrashReviewChoice)? = nil) -> CrashReviewChoice {
        let alert = NSAlert()
        alert.alertStyle = preview.hasExceptionEvidence ? .warning : .informational
        alert.messageText = preview.title
        alert.informativeText = preview.explanation
        alert.window.appearance = NSAppearance(named:
            tokens.color("text").brightnessComponent > 0.5 ? .darkAqua : .aqua)
        for title in ["Copy Summary", "Add to Feedback", "Dismiss", "Later"] { alert.addButton(withTitle: title) }
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 460, height: 150))
        scroll.hasVerticalScroller = true; scroll.borderType = .bezelBorder
        scroll.useTokenScrollers(tokens)
        let text = NSTextView(frame: scroll.bounds)
        text.string = preview.summary; text.isEditable = false; text.isSelectable = true
        text.isVerticallyResizable = true; text.isHorizontallyResizable = false
        text.autoresizingMask = [.width]
        text.textContainer?.widthTracksTextView = true
        text.font = .monospacedSystemFont(ofSize: tokens.number("text-sm"), weight: .regular)
        text.textColor = tokens.color("text"); text.backgroundColor = tokens.color("surface-field")
        scroll.documentView = text; alert.accessoryView = scroll
        if let choose { return choose(alert) }
        switch alert.runModal() {
        case .alertFirstButtonReturn: return .copy
        case .alertSecondButtonReturn: return .addToFeedback
        case .alertThirdButtonReturn: return .dismiss
        default: return .later
        }
    }
}
