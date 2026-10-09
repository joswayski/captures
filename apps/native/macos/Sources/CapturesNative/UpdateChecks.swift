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

/// Retained across Preferences closure. No automatic check or idle polling.
final class UpdateCheckModel {
    private let transport: UpdateCheckTransport
    private var timer: Timer?
    private var closed = false
    private(set) var presentation: [String: Any] = [:]
    private(set) var notice: [String: Any]?
    private(set) var generation = 0
    private(set) var showChangelog = true
    private(set) var busy = false
    var didChange: (() -> Void)?

    init(transport: UpdateCheckTransport) throws {
        self.transport = transport
        try apply(transport.request(["operation": "poll"]))
    }

    deinit { timer?.invalidate() }
    var enabled: Bool { !closed && !busy && presentation["enabled"] as? Bool == true }

    @discardableResult func check() -> Bool {
        guard enabled else { return false }
        return command("check")
    }

    var acquisition: UpdateNoticeButton? { UpdateNoticeButton(presentation["acquisition"]) }

    @discardableResult func acquire(_ action: UpdateNoticeAction) -> Bool {
        guard !closed, let button = acquisition, button.enabled, button.action == action else { return false }
        guard action != .downloadVerify || !busy else { return false }
        switch action {
        case .downloadVerify: return command("download_verify")
        case .cancelDownload: return command("cancel_download")
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

    private func command(_ operation: String) -> Bool {
        // An unread reply is not evidence that an accepted request stopped.
        busy = true
        var accepted = false
        do {
            let reply = try transport.request(["operation": operation, "show_changelog": showChangelog])
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
        do { try apply(transport.request(["operation": "poll", "show_changelog": showChangelog])) }
        catch { presentation["status"] = error.localizedDescription; presentation["failed"] = true; didChange?() }
    }

    func setShowChangelog(_ show: Bool) {
        guard !closed, show != showChangelog else { return }
        showChangelog = show
        do { try apply(transport.request(["operation": "poll", "show_changelog": show])) }
        catch { presentation["status"] = error.localizedDescription; presentation["failed"] = true; didChange?() }
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
    }

    func shutdown(completion: @escaping () -> Void) {
        closed = true; timer?.invalidate(); timer = nil
        transport.shutdown(completion: completion)
    }
}
