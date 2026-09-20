import AppKit
import CCapturesSettings

enum FeedbackBridge {
    private static let queue = DispatchQueue(label: "es.captures.native.feedback", qos: .utility)

    static func request(_ object: [String: Any], completion: @escaping (Result<[String: Any], Error>) -> Void) {
        queue.async {
            let result = Result { () throws -> [String: Any] in
                let data = try JSONSerialization.data(withJSONObject: object)
                let pointer = String(decoding: data, as: UTF8.self).withCString { captures_feedback_request_v1($0) }
                guard let pointer else { throw AppBridgeError.invalidResponse }
                defer { captures_settings_free_v1(pointer) }
                let data = Data(bytes: pointer, count: strlen(pointer))
                guard let response = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                      let ok = response["ok"] as? Bool else { throw AppBridgeError.invalidResponse }
                guard ok else { throw AppBridgeError.backend(response.string("error", "Feedback could not be sent.")) }
                guard let value = response["result"] as? [String: Any] else { throw AppBridgeError.invalidResponse }
                return value
            }
            DispatchQueue.main.async { completion(result) }
        }
    }
}

/// Retained by the workbench, not the sheet: closing/reopening preserves the
/// draft and any in-flight request. No network call happens until explicit Send.
final class FeedbackController: NSObject, NSTextViewDelegate, NSTextFieldDelegate, NSWindowDelegate {
    typealias Request = ([String: Any], @escaping (Result<[String: Any], Error>) -> Void) -> Void
    let window: NSWindow
    let message = NSTextView()
    let contact = NSTextField(string: "")
    let category = NSPopUpButton(frame: .zero, pullsDown: false)
    private(set) var sendButton: CaptureButton!
    private(set) var sending = false
    private(set) var status = NSTextField(wrappingLabelWithString: "")
    private let details = NSTextField(wrappingLabelWithString: "Loading local app and system details…")
    private let counter = NSTextField(labelWithString: "0 / 8000")
    private let root: Surface
    private let request: Request
    private let live: Bool
    private var contextReady = false
    private var contextError = false
    private var sent = false
    private var tokens: Tokens

    init(tokens: Tokens, live: Bool, request: @escaping Request = FeedbackBridge.request) {
        self.tokens = tokens; self.live = live; self.request = request
        let bounds = NSRect(x: 0, y: 0, width: 620, height: 656)
        root = Surface(frame: bounds)
        window = NSWindow(contentRect: bounds, styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        super.init()
        window.title = "Send feedback"; window.delegate = self
        window.contentView = root
        let inset = tokens.number("s-6")
        let width = root.bounds.width - inset * 2
        label("Send feedback", y: 24, height: 30, size: tokens.number("text-xl"))
        label("Tell us what broke, what is missing, or what you wish worked better.", y: 64, height: 40)
        label("Only what you type and the details below are sent to captur.es. No captures, files, or diagnostics are attached.", y: 104, height: 44, muted: true)
        label("Category", y: 164, height: 20)
        category.frame = NSRect(x: inset, y: 188, width: 180, height: 30)
        category.addItems(withTitles: ["Bug", "Idea", "Other"])
        category.setAccessibilityLabel("Feedback category"); root.addSubview(category)
        label("Message", y: 234, height: 20)
        let scroll = NSScrollView(frame: NSRect(x: inset, y: 260, width: width, height: 128))
        scroll.hasVerticalScroller = true; scroll.borderType = .lineBorder
        message.frame = NSRect(x: 0, y: 0, width: width - 18, height: 128)
        message.isRichText = false; message.isVerticallyResizable = true
        message.isHorizontallyResizable = false
        message.textContainer?.widthTracksTextView = true
        message.textContainerInset = NSSize(width: tokens.number("s-2"), height: tokens.number("s-2"))
        message.delegate = self; message.setAccessibilityLabel("Feedback message")
        scroll.documentView = message; root.addSubview(scroll)
        counter.frame = NSRect(x: inset, y: 392, width: width, height: 18); root.addSubview(counter)
        label("Contact (optional, up to 200 characters)", y: 418, height: 20)
        contact.frame = NSRect(x: inset, y: 442, width: width, height: 30)
        contact.placeholderString = "X handle, GitHub username, email…"
        contact.delegate = self; contact.setAccessibilityLabel("Contact (optional)"); root.addSubview(contact)
        details.frame = NSRect(x: inset, y: 488, width: width, height: 48)
        details.maximumNumberOfLines = 3; root.addSubview(details)
        status.frame = NSRect(x: inset, y: 544, width: width, height: 54)
        status.maximumNumberOfLines = 3; root.addSubview(status)
        let close = CaptureButton("Close", frame: NSRect(x: inset, y: 606, width: 86, height: 30), tokens: tokens) { [weak self] in self?.dismiss() }
        close.keyEquivalent = "\u{1b}"; root.addSubview(close)
        sendButton = CaptureButton("Send feedback", frame: NSRect(x: 426, y: 606, width: 170, height: 30), tokens: tokens) { [weak self] in
            guard let self else { return }
            if self.contextError { self.loadContext() } else { self.submit() }
        }
        root.addSubview(sendButton)
        restyle(tokens)
        loadContext()
    }

    func present(on parent: NSWindow) {
        guard window.sheetParent == nil, parent.attachedSheet == nil else { return }
        parent.beginSheet(window)
        window.makeFirstResponder(message)
    }

    func dismiss() {
        window.sheetParent?.endSheet(window)
        window.orderOut(nil)
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool { dismiss(); return false }

    func restyle(_ tokens: Tokens) {
        self.tokens = tokens
        root.wantsLayer = true; root.layer?.backgroundColor = tokens.color("surface-canvas").cgColor
        window.appearance = NSAppearance(named: tokens.color("text").brightnessComponent > 0.5 ? .darkAqua : .aqua)
        for view in root.subviews {
            if let label = view as? NSTextField {
                label.textColor = tokens.color(label === status ? (sent ? "positive-text" : "danger-text")
                    : label.identifier?.rawValue == "feedback-muted" ? "text-muted" : "text")
            }
            if let button = view as? CaptureButton { button.tokens = tokens; button.needsDisplay = true }
        }
        message.backgroundColor = tokens.color("surface-sunken")
        message.textColor = tokens.color("text")
        message.insertionPointColor = tokens.color("text")
        message.font = .systemFont(ofSize: tokens.number("text-md"))
        contact.backgroundColor = tokens.color("surface-sunken")
        details.textColor = tokens.color("text-muted"); counter.textColor = tokens.color("text-muted")
    }

    private func loadContext() {
        contextError = false; status.stringValue = ""; updateControls()
        request(["operation": "context"]) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let value):
                guard let version = value["app_version"] as? String,
                      let os = value["os"] as? String, let osVersion = value["os_version"] as? String,
                      let arch = value["arch"] as? String else {
                    contextFailed(AppBridgeError.invalidResponse); return
                }
                details.stringValue = "Included automatically\nApp version: \(version) · System: \(os) · \(osVersion) · \(arch)"
                contextReady = true
            case .failure(let error): contextFailed(error)
            }
            updateControls()
        }
    }

    private func contextFailed(_ error: Error) {
        contextError = true; contextReady = false
        details.stringValue = "App and system details could not be loaded."
        status.stringValue = error.localizedDescription; updateControls()
    }

    func updateControls() {
        let count = message.string.unicodeScalars.count
        counter.stringValue = "\(count) / 8000"
        counter.textColor = tokens.color(count > 8000 ? "danger-text" : "text-muted")
        let valid = !message.string.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && count <= 8000 && contact.stringValue.unicodeScalars.count <= 200
        message.isEditable = !sending; contact.isEnabled = !sending; category.isEnabled = !sending
        sendButton.title = contextError ? "Retry details" : sending ? "Sending…" : "Send feedback"
        sendButton.setAccessibilityLabel(sendButton.title)
        sendButton.isEnabled = contextError || (live && contextReady && valid && !sending)
        sendButton.needsDisplay = true
        if !live { status.stringValue = "Fixture mode — sending feedback is disabled." }
    }

    func textDidChange(_ notification: Notification) { status.stringValue = ""; updateControls() }
    func controlTextDidChange(_ notification: Notification) { updateControls() }

    func submit() {
        updateControls()
        guard live, contextReady, !sending, sendButton.isEnabled else { return }
        let contactText = contact.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let draft: [String: Any] = ["message": message.string.trimmingCharacters(in: .whitespacesAndNewlines),
            "contact": contactText.isEmpty ? NSNull() : contactText as Any,
            "category": ["bug", "idea", "other"][category.indexOfSelectedItem]]
        sending = true; sent = false; status.stringValue = ""; updateControls()
        request(["operation": "submit", "draft": draft]) { [weak self] result in
            guard let self else { return }
            sending = false
            switch result {
            case .success:
                sent = true
                message.string = ""; status.stringValue = "Thanks — feedback sent."
                status.textColor = tokens.color("positive-text")
            case .failure(let error):
                status.stringValue = error.localizedDescription
                status.textColor = tokens.color("danger-text")
            }
            updateControls()
        }
    }

    private func label(_ value: String, y: CGFloat, height: CGFloat, size: CGFloat? = nil, muted: Bool = false) {
        let label = NSTextField(wrappingLabelWithString: value)
        label.frame = NSRect(x: tokens.number("s-6"), y: y, width: 572, height: height)
        label.font = .systemFont(ofSize: size ?? tokens.number("text-sm"))
        label.textColor = tokens.color(muted ? "text-muted" : "text")
        if muted { label.identifier = NSUserInterfaceItemIdentifier("feedback-muted") }
        root.addSubview(label)
    }
}
