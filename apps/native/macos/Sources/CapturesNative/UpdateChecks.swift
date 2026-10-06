import Foundation
import CCapturesSettings

protocol UpdateCheckTransport: AnyObject {
    func request(_ object: [String: Any]) throws -> [String: Any]
    func shutdown(completion: @escaping () -> Void)
}

/// Main-thread, read-only commands. HTTP and signed metadata stay in Rust.
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
            throw AppBridgeError.backend("Native update-check configuration is invalid. Check the endpoint, public key file and current version.")
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
        // An unread reply is not evidence that an accepted request stopped.
        busy = true
        var accepted = false
        do {
            let reply = try transport.request(["operation": "check"])
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
        do { try apply(transport.request(["operation": "poll"])) }
        catch { presentation["status"] = error.localizedDescription; presentation["failed"] = true; didChange?() }
    }

    private func apply(_ reply: [String: Any]) throws {
        guard let checking = reply["checking"] as? Bool,
              let copy = reply["presentation"] as? [String: Any],
              copy["status"] is String, copy["enabled"] is Bool else { throw AppBridgeError.invalidResponse }
        let changed = !NSDictionary(dictionary: presentation).isEqual(to: copy)
        busy = checking; presentation = copy
        if !busy { timer?.invalidate(); timer = nil }
        if changed { didChange?() }
    }

    func shutdown(completion: @escaping () -> Void) {
        closed = true; timer?.invalidate(); timer = nil
        transport.shutdown(completion: completion)
    }
}
