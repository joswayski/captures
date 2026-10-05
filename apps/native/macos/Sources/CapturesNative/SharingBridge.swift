import Foundation
import CCapturesSettings

protocol SharingTransport: AnyObject {
    func request(_ object: [String: Any]) throws -> [String: Any]
    func shutdown(completion: @escaping () -> Void)
}

/// Main-thread command/event transport; the opaque Rust worker alone owns
/// credentials, HTTP and original bytes. No handle exists before explicit Open.
final class NativeSharingTransport: SharingTransport {
    private let root: String
    private var handle: OpaquePointer?
    private var closed = false
    private var draining = false
    private var shutdownWaiters: [() -> Void] = []

    init(root: String) { self.root = root }

    func request(_ object: [String: Any]) throws -> [String: Any] {
        precondition(Thread.isMainThread)
        guard !closed else { throw AppBridgeError.backend("Sharing worker is closed.") }
        if handle == nil {
            guard object.string("operation") == "open" else { throw AppBridgeError.backend("Open a capture to start sharing.") }
            handle = root.withCString { captures_sharing_create_v1($0) }
        }
        guard let handle else { throw AppBridgeError.backend("Sharing worker could not start.") }
        let data = try JSONSerialization.data(withJSONObject: object)
        let pointer = String(decoding: data, as: UTF8.self).withCString {
            captures_sharing_request_v1(handle, $0)
        }
        guard let pointer else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(pointer) }
        guard let response = try JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8)) as? [String: Any],
              let ok = response["ok"] as? Bool else { throw AppBridgeError.invalidResponse }
        guard ok else { throw AppBridgeError.backend(response.string("error", "Sharing command failed.")) }
        guard let result = response["result"] as? [String: Any] else { throw AppBridgeError.invalidResponse }
        return result
    }

    func shutdown(completion: @escaping () -> Void) {
        precondition(Thread.isMainThread)
        if draining { shutdownWaiters.append(completion); return }
        closed = true
        let owned = handle; handle = nil
        guard let owned else { completion(); return }
        draining = true; shutdownWaiters.append(completion)
        DispatchQueue.global(qos: .utility).async {
            captures_sharing_free_v1(owned)
            DispatchQueue.main.async {
                self.draining = false
                let waiters = self.shutdownWaiters; self.shutdownWaiters.removeAll()
                waiters.forEach { $0() }
            }
        }
    }

    deinit {
        if let owned = handle {
            DispatchQueue.global(qos: .utility).async { captures_sharing_free_v1(owned) }
        }
    }
}

extension CaptureArtifact {
    /// Always the private original media, never a thumbnail or saved export.
    var sharingSelection: [String: Any]? {
        guard let path = isRecording ? mediaPath : imagePath else { return nil }
        let ext = URL(fileURLWithPath: path).pathExtension.lowercased()
        let types = ["png": "image/png", "jpg": "image/jpeg", "jpeg": "image/jpeg",
                     "webp": "image/webp", "gif": "image/gif", "mp4": "video/mp4", "webm": "video/webm"]
        guard let type = types[ext] else { return nil }
        return ["operation": "open", "artifact_id": id, "path": path,
                "name": "Capture-\(id).\(ext)", "content_type": type]
    }
}

/// Presentation state survives window closure. Poll only during accepted work;
/// static/hidden idle has no repeating timer and never consults the vault.
final class SharingModel {
    private let transport: SharingTransport?
    private var timer: Timer?
    private var applyingSettings = false
    private var closing = false
    private(set) var artifact: CaptureArtifact?
    private(set) var state: [String: Any] = [:]
    private(set) var busy = false
    private(set) var uploading = false
    private(set) var progress: (UInt64, UInt64)?
    private(set) var status = "Ready"
    private(set) var failure: String?
    var email = ""
    var code = ""
    var password = ""
    var expiry = ""
    var removePassword = false
    var removeExpiry = false
    var didChange: (() -> Void)?

    init(transport: SharingTransport?) { self.transport = transport }
    deinit { timer?.invalidate() }
    var isLive: Bool { transport != nil && !closing }
    var auth: String { (state["auth"] as? [String: Any])?.string("status", "unavailable") ?? "unavailable" }
    var userEmail: String { (state["auth"] as? [String: Any])?.string("email") ?? "" }
    var asset: [String: Any]? { (state["opened"] as? [String: Any])?["asset"] as? [String: Any] }
    var share: [String: Any]? { asset?["share"] as? [String: Any] }
    var trashed: Bool { asset?["deletedAt"] as? String != nil }
    var link: String? {
        guard !busy, failure == nil, auth == "signed_in", !trashed else { return nil }
        return state["link"] as? String
    }
    var patch: [String: Any] {
        var value: [String: Any] = [:]
        if removePassword { value["password"] = NSNull() }
        else if !password.isEmpty { value["password"] = password }
        let date = expiry.trimmingCharacters(in: .whitespacesAndNewlines)
        if removeExpiry { value["expiresAt"] = NSNull() }
        else if !date.isEmpty { value["expiresAt"] = date }
        return value
    }

    @discardableResult func open(_ artifact: CaptureArtifact) -> Bool {
        poll()
        guard !busy else { return false }
        guard let command = artifact.sharingSelection else {
            failure = "Original capture media is unavailable. No thumbnail is uploaded in its place."
            status = failure!; didChange?(); return false
        }
        let changed = self.artifact?.id != artifact.id
        guard send(command, status: "Opening selected capture…") else { return false }
        if changed {
            password = ""; expiry = ""; removePassword = false; removeExpiry = false
            state = [:]
        }
        self.artifact = artifact
        didChange?()
        return true
    }

    @discardableResult func send(_ command: [String: Any], status: String) -> Bool {
        guard isLive, !busy, let transport else { return false }
        do {
            guard try transport.request(command)["accepted"] as? Bool == true else { throw AppBridgeError.invalidResponse }
            self.status = status; failure = nil; busy = true; progress = nil
            uploading = command.string("operation") == "upload"
            applyingSettings = uploading || (command.string("operation") == "configure" && command["enabled"] as? Bool == true)
            timer?.invalidate()
            let timer = Timer(timeInterval: 0.1, repeats: true) { [weak self] _ in self?.poll() }
            self.timer = timer
            RunLoop.main.add(timer, forMode: .common)
            didChange?()
            return true
        } catch {
            failure = error.localizedDescription; self.status = failure!; didChange?(); return false
        }
    }

    func cancel() {
        guard isLive, busy, let transport else { return }
        do {
            _ = try transport.request(["operation": "cancel"])
            status = "Cancelling… Current HTTP request may take up to two minutes."
        } catch { failure = error.localizedDescription; status = failure! }
        didChange?()
    }

    func poll() {
        guard busy, let transport else { return }
        do {
            let reply = try transport.request(["operation": "poll"])
            guard let events = reply["events"] as? [[String: Any]] else { throw AppBridgeError.invalidResponse }
            for event in events {
                switch event.string("event") {
                case "progress":
                    if let read = event["read"] as? NSNumber, let total = event["total"] as? NSNumber {
                        progress = (read.uint64Value, total.uint64Value)
                    }
                case "finished":
                    state = event; failure = event["error"] as? String
                    status = failure ?? "Ready"; busy = false; uploading = false
                    timer?.invalidate(); timer = nil
                    if failure == nil && (applyingSettings || (password.isEmpty && expiry.isEmpty && !removePassword && !removeExpiry)) {
                        expiry = share?["expiresAt"] as? String ?? ""
                        password = ""; removePassword = false; removeExpiry = false
                    }
                    if auth == "signed_in" { code = "" }
                    applyingSettings = false
                default: throw AppBridgeError.invalidResponse
                }
            }
            if !events.isEmpty { didChange?() }
        } catch {
            failure = error.localizedDescription; status = failure!; didChange?()
            // An unread reply is not proof the accepted worker has finished.
            // Keep it pinned and retry Poll; never silently accept another job.
        }
    }

    func shutdown(completion: @escaping () -> Void) {
        closing = true; timer?.invalidate(); timer = nil
        guard let transport else { completion(); return }
        transport.shutdown(completion: completion)
    }

    /// Render-only states, unavailable to a live transport. Never creates a
    /// worker or changes the canonical API/vault, even for Copy/Open/Cancel.
    func loadFixture(_ name: String) {
        guard transport == nil else { return }
        email = ""; code = ""; password = ""; expiry = ""
        removePassword = false; removeExpiry = false
        let signedIn = ["shared", "uploading", "trash", "error"].contains(name)
        let auth = name == "otp" ? "code_sent" : name == "vault" ? "save_required" : signedIn ? "signed_in" : "signed_out"
        state = ["auth": ["status": auth, "email": "you@example.com"], "opened": ["status": "unassociated"]]
        if ["shared", "trash", "error"].contains(name) {
            var asset: [String: Any] = ["name": "Capture.png", "deletedAt": NSNull(), "share": NSNull()]
            if name == "shared" {
                asset["share"] = ["passwordProtected": true, "expiresAt": "2027-01-31T18:00:00Z", "sharedAt": "2026-10-05T12:01:00Z"]
                state["link"] = "https://captur.es/s/fixture-link"
                expiry = "2027-01-31T18:00:00Z"
            }
            if name == "trash" { asset["deletedAt"] = "2026-10-05T13:00:00Z" }
            state["opened"] = ["status": "asset", "asset": asset]
        }
        busy = name == "uploading"; uploading = busy
        progress = busy ? (3_145_728, 8_388_608) : nil
        failure = name == "error" ? "Sharing service is unavailable. Nothing uploads automatically; retry later." : nil
        status = failure ?? (busy ? "Uploading original capture…" : "Ready")
        didChange?()
    }
}
