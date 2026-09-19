import Foundation
import CCapturesSettings

enum CaptureShortcut: String {
    case region, window, display
}

private func wakeCaptureShortcuts() {
    // global-hotkey may call from an OS worker. This callback has no captured
    // owner or action payload; a late wake only drains the current Rust queue.
    DispatchQueue.main.async {
        NotificationCenter.default.post(name: NativeCaptureShortcuts.wakeNotification, object: nil)
    }
}

/// Construct and release on the AppKit thread, only for the live host. Rust owns
/// registrations, Escape routing, press/release arming and stale-event rejection.
final class NativeCaptureShortcuts {
    static let wakeNotification = Notification.Name("CapturesNativeShortcutWake")
    private var closed = true

    init(settings: [String: Any]) throws {
        precondition(Thread.isMainThread)
        _ = try Self.request(["operation": "configure", "settings": settings])
        closed = false
    }
    deinit { close() }

    func update(settings: [String: Any]) throws {
        precondition(!closed)
        _ = try Self.request(["operation": "configure", "settings": settings])
    }

    func setEnabled(_ enabled: Bool) {
        guard !closed else { return }
        _ = try? Self.request(["operation": "enabled", "enabled": enabled])
    }

    func nextAction() throws -> CaptureShortcut? {
        guard !closed else { return nil }
        let result = try Self.request(["operation": "next"])
        if result["action"] is NSNull { return nil }
        guard let raw = result["action"] as? String, let action = CaptureShortcut(rawValue: raw)
        else { throw AppBridgeError.invalidResponse }
        return action
    }

    func close() {
        guard !closed else { return }
        _ = try? Self.request(["operation": "close"])
        closed = true
    }

    private static func request(_ object: [String: Any]) throws -> [String: Any] {
        precondition(Thread.isMainThread)
        let data = try JSONSerialization.data(withJSONObject: object)
        let pointer = String(decoding: data, as: UTF8.self).withCString {
            captures_shortcuts_request_v1($0, wakeCaptureShortcuts)
        }
        guard let pointer else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(pointer) }
        return try AppBridge.decode(Data(bytes: pointer, count: strlen(pointer)))
    }
}
