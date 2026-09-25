import AppKit

/// Preserve the live profile and all queued Apple-event/instance media, without
/// replaying fixture deadlines, screenshot flags or the original media twice.
func permissionRestartArguments(options: Options, pendingMedia: [String]) -> [String] {
    var arguments = ["--live", "--scene", "preferences"]
    if let path = options.historyRoot { arguments += ["--history-root", path] }
    if let path = options.settingsFile { arguments += ["--settings-file", path] }
    if options.appearanceOverride { arguments += ["--appearance", options.appearance] }
    if options.themeOverride { arguments += ["--theme", options.theme] }
    if !pendingMedia.isEmpty { arguments += ["--"] + pendingMedia }
    return arguments
}

struct OnboardingState: Equatable {
    let platform: String
    let completed: Bool
    let screenRequired: Bool
    let screenGranted: Bool
    let screenCanRequest: Bool
    let screenRequestedThisLaunch: Bool
    let microphoneGranted: Bool
    let microphoneCanRequest: Bool

    init(_ value: [String: Any]) throws {
        guard let platform = value["platform"] as? String,
              let completed = value["onboarding_completed"] as? Bool,
              let screenRequired = value["screen_recording_required"] as? Bool,
              let screenGranted = value["screen_recording_granted"] as? Bool,
              let screenCanRequest = value["screen_recording_can_request"] as? Bool,
              let screenRequested = value["screen_recording_requested_this_launch"] as? Bool,
              let microphoneGranted = value["microphone_granted"] as? Bool,
              let microphoneCanRequest = value["microphone_can_request"] as? Bool else {
            throw SettingsStoreError.invalidResponse
        }
        self.platform = platform; self.completed = completed
        self.screenRequired = screenRequired; self.screenGranted = screenGranted
        self.screenCanRequest = screenCanRequest
        self.screenRequestedThisLaunch = screenRequested
        self.microphoneGranted = microphoneGranted
        self.microphoneCanRequest = microphoneCanRequest
    }
}

final class OnboardingController {
    private let store: SettingsStore
    private(set) var state: OnboardingState?
    private(set) var busy = false
    private(set) var error: String?
    var changed: () -> Void = {}
    var completed: () -> Void = {}
    var requiresAttention: () -> Void = {}

    init(store: SettingsStore) { self.store = store }

    func check() { request("check") }
    func requestScreen() { request("request_screen") }
    func requestMicrophone() { request("request_microphone") }
    func complete() { request("complete") }
    func flush() { store.flush() }

    private func request(_ action: String) {
        guard !busy else { return }
        busy = true; error = nil; changed()
        store.onboarding(action) { [weak self] result in
            guard let self else { return }
            self.busy = false
            switch result {
            case .success(let state):
                self.state = state
                if state.completed { self.completed() }
                else { self.requiresAttention() }
            case .failure(let error):
                self.error = error.localizedDescription
                self.requiresAttention()
            }
            self.changed()
        }
    }
}

final class OnboardingView: NSView {
    override var isFlipped: Bool { true }
    private let controller: OnboardingController
    private let tokens: Tokens
    private let done: (() -> Void)?
    private let title = NSTextField(labelWithString: "Set up Captures")
    private let detail = NSTextField(wrappingLabelWithString: "Captures needs Screen Recording access to capture screenshots, GIFs, and video. Processing stays on your Mac unless you choose to share or save elsewhere.")
    private let screen = NSTextField(labelWithString: "Screen Recording — Checking…")
    private let microphone = NSTextField(labelWithString: "Microphone — Optional")
    private let errorLabel = NSTextField(wrappingLabelWithString: "")
    private lazy var screenButton = button("Allow Screen Recording", #selector(requestScreen))
    private lazy var microphoneButton = button("Allow Microphone", #selector(requestMicrophone))
    private lazy var refreshButton = button("Refresh status", #selector(refresh))
    private lazy var continueButton = button("Finish setup", #selector(finish))
    private lazy var restartButton = button("Restart Captures", #selector(restart))
    var restartRequested: () -> Void = {}

    init(frame: NSRect, tokens: Tokens, controller: OnboardingController,
         done: (() -> Void)? = nil) {
        self.tokens = tokens; self.controller = controller; self.done = done
        super.init(frame: frame)
        if done != nil {
            title.stringValue = "Capture permissions"
            continueButton.title = "Done"
        }
        wantsLayer = true; layer?.backgroundColor = tokens.color("surface-canvas").cgColor
        title.font = .systemFont(ofSize: tokens.number("text-3xl"), weight: .semibold)
        detail.font = .systemFont(ofSize: tokens.number("text-md"))
        detail.textColor = tokens.color("text-muted")
        [title, screen, microphone].forEach { $0.textColor = tokens.color("text") }
        errorLabel.textColor = tokens.color("danger-text")
        [title, detail, screen, microphone, screenButton, microphoneButton, refreshButton,
         continueButton, restartButton, errorLabel].forEach(addSubview)
        controller.changed = { [weak self] in self?.update() }
        needsLayout = true
        update()
    }
    required init?(coder: NSCoder) { nil }

    private func button(_ label: String, _ action: Selector) -> NSButton {
        let value = NSButton(title: label, target: self, action: action)
        value.bezelStyle = .rounded
        return value
    }
    override func layout() {
        super.layout()
        let x = max(40, (bounds.width - 600) / 2), w: CGFloat = 600
        title.frame = NSRect(x: x, y: 90, width: w, height: 38)
        detail.frame = NSRect(x: x, y: 140, width: w, height: 64)
        screen.frame = NSRect(x: x, y: 230, width: w, height: 24)
        screenButton.frame = NSRect(x: x, y: 262, width: 190, height: 32)
        microphone.frame = NSRect(x: x, y: 320, width: w, height: 24)
        microphoneButton.frame = NSRect(x: x, y: 352, width: 160, height: 32)
        refreshButton.frame = NSRect(x: x, y: 410, width: 125, height: 32)
        restartButton.frame = NSRect(x: x + 140, y: 410, width: 145, height: 32)
        continueButton.frame = NSRect(x: x, y: 466, width: 145, height: 34)
        errorLabel.frame = NSRect(x: x, y: 520, width: w, height: 70)
    }
    func update() {
        let state = controller.state
        screen.stringValue = "Screen Recording — " + (state == nil ? "Checking…" : state?.screenGranted == true ? "Allowed" : "Not allowed")
        microphone.stringValue = "Microphone — " + (state?.microphoneGranted == true ? "Allowed" : "Optional, not allowed")
        screenButton.title = state?.screenCanRequest == false ? "Open Screen Settings" : "Allow Screen Recording"
        microphoneButton.title = state?.microphoneCanRequest == false ? "Open Mic Settings" : "Allow Microphone"
        detail.stringValue = state?.screenRequestedThisLaunch == true && state?.screenGranted != true
            ? "Turn on this copy of Captures in Screen Recording settings, then restart Captures. A local build may have a different row from a downloaded app. Microphone access is optional."
            : "Captures needs Screen Recording access for screenshots, GIFs, and video. Nothing is uploaded unless you choose to share it. Microphone access is optional."
        if done != nil {
            detail.stringValue = "Your captures and editors stay open. Grant access, refresh status, then retry your capture. If macOS requires a restart, save your work before quitting and reopening Captures. Microphone access is optional."
        }
        errorLabel.stringValue = controller.error ?? ""
        screenButton.isEnabled = !controller.busy && state != nil && state?.screenGranted != true
        microphoneButton.isEnabled = !controller.busy && state != nil && state?.microphoneGranted != true
        refreshButton.isEnabled = !controller.busy
        continueButton.isEnabled = !controller.busy && (done != nil || state?.screenRequired == false || state?.screenGranted == true)
        restartButton.isHidden = done != nil || state?.screenRequestedThisLaunch != true || state?.screenGranted == true
        restartButton.isEnabled = !controller.busy
    }
    @objc private func requestScreen() { controller.requestScreen() }
    @objc private func requestMicrophone() { controller.requestMicrophone() }
    @objc private func refresh() { controller.check() }
    @objc private func finish() {
        if let done { done() }
        else { controller.complete() }
    }
    @objc private func restart() { restartRequested() }
}
