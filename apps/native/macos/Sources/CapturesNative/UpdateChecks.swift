import Foundation
import CCapturesSettings

/// Bounded shared development-profile validation/publication, with no updater
/// handle, network request or package execution. Publish only after host drain.
@discardableResult func updateShutdownIntent(_ object: [String: Any]) throws -> Bool {
    let data = try JSONSerialization.data(withJSONObject: object)
    let pointer = String(decoding: data, as: UTF8.self).withCString {
        captures_update_shutdown_intent_v1($0)
    }
    guard let pointer else { throw AppBridgeError.invalidResponse }
    defer { captures_settings_free_v1(pointer) }
    guard let response = try JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8)) as? [String: Any],
          let ok = response["ok"] as? Bool else { throw AppBridgeError.invalidResponse }
    guard ok else { throw AppBridgeError.backend(response.string("error", "Shutdown intent was not published.")) }
    guard let result = response["result"] as? [String: Any], let written = result["written"] as? Bool
    else { throw AppBridgeError.invalidResponse }
    return written
}

protocol UpdateCheckTransport: AnyObject {
    func request(_ object: [String: Any]) throws -> [String: Any]
    func shutdown(completion: @escaping () -> Void)
}

/// Main-thread commands. HTTP and signed package verification stay in Rust.
final class NativeUpdateCheckTransport: UpdateCheckTransport {
    private var handle: OpaquePointer?
    private var draining = false
    private var shutdownWaiters: [() -> Void] = []

    init(configuration: [String: Any]) throws {
        let data = try JSONSerialization.data(withJSONObject: configuration)
        handle = String(decoding: data, as: UTF8.self).withCString {
            captures_update_checks_create_v1($0)
        }
        guard handle != nil else {
            throw AppBridgeError.backend("Native update-check configuration is invalid. Check the endpoint, public key file, current version and optional staging directory.")
        }
    }

    func request(_ object: [String: Any]) throws -> [String: Any] {
        precondition(Thread.isMainThread)
        guard let handle else { throw AppBridgeError.backend("Update checker is closed.") }
        let data = try JSONSerialization.data(withJSONObject: object)
        let pointer = String(decoding: data, as: UTF8.self).withCString {
            captures_update_checks_request_v1(handle, $0)
        }
        guard let pointer else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(pointer) }
        guard let response = try JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8)) as? [String: Any],
              let ok = response["ok"] as? Bool else { throw AppBridgeError.invalidResponse }
        guard ok else { throw AppBridgeError.backend(response.string("error", "Update-check command failed.")) }
        guard let result = response["result"] as? [String: Any] else { throw AppBridgeError.invalidResponse }
        return result
    }

    func shutdown(completion: @escaping () -> Void) {
        precondition(Thread.isMainThread)
        if draining { shutdownWaiters.append(completion); return }
        let owned = handle; handle = nil
        guard let owned else { completion(); return }
        draining = true; shutdownWaiters.append(completion)
        DispatchQueue.global(qos: .utility).async {
            captures_update_checks_free_v1(owned)
            DispatchQueue.main.async {
                self.draining = false
                let waiters = self.shutdownWaiters; self.shutdownWaiters.removeAll()
                waiters.forEach { $0() }
            }
        }
    }

    deinit {
        if let owned = handle {
            DispatchQueue.global(qos: .utility).async { captures_update_checks_free_v1(owned) }
        }
    }
}

/// Retained across Preferences closure. Only installed Previews start periodic checks.
final class UpdateCheckModel {
    private let transport: UpdateCheckTransport
    private var timer: Timer?
    private var previewTimer: Timer?
    private var closed = false
    private(set) var presentation: [String: Any] = [:]
    private(set) var notice: [String: Any]?
    private(set) var generation = 0
    private(set) var showChangelog = true
    /// A capture or editor is open, so the notice warns that installing closes it.
    private(set) var openCaptures = false
    /// The host accepted an install and is quitting to run it.
    private(set) var installing = false
    private(set) var busy = false
    var didChange: (() -> Void)?
    var requestInstallation: (() -> Bool)?

    init(transport: UpdateCheckTransport) throws {
        self.transport = transport
        try apply(transport.request(["operation": "poll"]))
    }

    deinit { timer?.invalidate(); previewTimer?.invalidate() }
    var enabled: Bool { !closed && !busy && presentation["enabled"] as? Bool == true }

    func startPreviewChecks() {
        guard previewTimer == nil else { return }
        check(quietly: true)
        // The worker decides what a quiet check does: behind an available or
        // verified update it only looks for a newer release, so an offer left
        // open while releases land does not install an old one.
        let timer = Timer(timeInterval: 300, repeats: true) { [weak self] _ in
            self?.check(quietly: true)
        }
        previewTimer = timer
        RunLoop.main.add(timer, forMode: .common)
    }

    /// A quiet check (background, or one Preferences reports in its Updates
    /// card) raises the notice only for an available update, as on Windows;
    /// the notice's own Check again reports every result in the notice.
    @discardableResult func check(quietly: Bool = false) -> Bool {
        guard enabled else { return false }
        return command("check", automatic: quietly)
    }

    var acquisition: UpdateNoticeButton? { UpdateNoticeButton(presentation["acquisition"]) }

    /// An installable release build's Preferences card (shipping `UpdatePreferences`).
    var card: [String: Any]? { presentation["card"] as? [String: Any] }
    private var installAfterDownload = false

    /// The card's single button. Shipping's Update now downloads and
    /// installs in one step, so a verified download goes on to install.
    @discardableResult func performCardStep() -> Bool {
        switch card?["step"] as? String {
        case "check": return check(quietly: true)
        case "download":
            // Set first: the download request already belongs to an install.
            installAfterDownload = true
            let accepted = acquire(.downloadVerify)
            if !accepted { installAfterDownload = false }
            return accepted
        case "install": return acquire(.install)
        default: return false
        }
    }

    @discardableResult func acquire(_ action: UpdateNoticeAction) -> Bool {
        guard !closed, let button = acquisition, button.enabled, button.action == action else { return false }
        guard action != .downloadVerify || !busy else { return false }
        switch action {
        case .downloadVerify: return command("download_verify")
        case .cancelDownload: return command("cancel_download")
        case .install:
            guard !busy, stagedShutdownStatus() != nil else { return false }
            return requestInstallation?() ?? false
        default: return false
        }
    }

    /// Snapshot while the handle still owns the verified stage, before shutdown
    /// drops it. The external helper authenticates these bytes again later.
    func stagedShutdownStatus() -> [String: Any]? {
        guard !closed, let reply = try? transport.request(["operation": "poll"]),
              let status = reply["status"] as? [String: Any], status["state"] as? String == "staged"
        else { return nil }
        return status
    }

    func launchPreviewInstall(restart: [String: Any]) throws {
        let reply = try transport.request(["operation": "install_preview", "restart": restart])
        guard reply["accepted"] as? Bool == true else {
            throw AppBridgeError.backend("The update installer could not start.")
        }
    }

    private func command(_ operation: String, automatic: Bool = false) -> Bool {
        // An unread reply is not evidence that an accepted request stopped.
        busy = true
        var accepted = false
        do {
            var request: [String: Any] = ["operation": operation, "show_changelog": showChangelog,
                                          "open_captures": openCaptures, "installing": installRequested]
            if automatic { request["automatic"] = true }
            let reply = try transport.request(request)
            try apply(reply)
            accepted = reply["accepted"] as? Bool == true
        } catch {
            presentation["status"] = error.localizedDescription; presentation["failed"] = true; didChange?()
        }
        if busy && timer == nil {
            let timer = Timer(timeInterval: 0.1, repeats: true) { [weak self] _ in self?.poll() }
            self.timer = timer
            RunLoop.main.add(timer, forMode: .common)
        }
        return accepted
    }

    func poll() {
        guard !closed, busy else { return }
        refresh()
    }

    func setShowChangelog(_ show: Bool) {
        guard !closed, show != showChangelog else { return }
        showChangelog = show
        refresh()
    }

    func setOpenCaptures(_ open: Bool) {
        guard !closed, open != openCaptures else { return }
        openCaptures = open
        refresh()
    }

    /// Update now's verified package shows its finished download, not the
    /// offer again, until Captures quits to install it (as on Linux).
    func setInstalling(_ installing: Bool) {
        guard !closed, installing != self.installing else { return }
        self.installing = installing
        refresh()
    }

    private var installRequested: Bool { installing || installAfterDownload }

    private func refresh() {
        do {
            try apply(transport.request(["operation": "poll", "show_changelog": showChangelog,
                                         "open_captures": openCaptures, "installing": installRequested]))
        } catch { presentation["status"] = error.localizedDescription; presentation["failed"] = true; didChange?() }
    }

    private func apply(_ reply: [String: Any]) throws {
        guard let checking = reply["checking"] as? Bool,
              let copy = reply["presentation"] as? [String: Any],
              copy["status"] is String, copy["enabled"] is Bool else { throw AppBridgeError.invalidResponse }
        let nextNotice = reply["notice"] as? [String: Any]
        let nextGeneration = reply["generation"] as? Int ?? 0
        let changed = !NSDictionary(dictionary: presentation).isEqual(to: copy)
            || generation != nextGeneration
            || !NSDictionary(dictionary: notice ?? [:]).isEqual(to: nextNotice ?? [:])
        busy = checking; presentation = copy
        notice = nextNotice; generation = nextGeneration
        if !busy { timer?.invalidate(); timer = nil }
        if changed { didChange?() }
        if installAfterDownload, !busy {
            installAfterDownload = false
            if card?["step"] as? String == "install" { _ = acquire(.install) }
            // A refused install, failed download or cancel offers the update again.
            if !installing { refresh() }
        }
    }

    func shutdown(completion: @escaping () -> Void) {
        closed = true; timer?.invalidate(); timer = nil
        previewTimer?.invalidate(); previewTimer = nil
        transport.shutdown(completion: completion)
    }
}
