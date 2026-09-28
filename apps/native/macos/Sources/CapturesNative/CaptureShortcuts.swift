import Foundation
import Darwin
import CCapturesSettings

/// Shipping `captures_macos_window::disable_symbolic_hotkeys`. Screenshot.app
/// keeps listening after `com.apple.symbolichotkeys` is written (shared Rust
/// does that write), so the overlapping ⌘⇧3 / ⌘⇧4 / ⌘⇧5 ids are also disabled
/// in WindowServer for this login session before Captures claims them.
enum SymbolicHotKeys {
    private typealias SetEnabled = @convention(c) (Int32, Bool) -> Int32

    private static let setEnabled: SetEnabled? = {
        let symbol = "CGSSetSymbolicHotKeyEnabled"
        // RTLD_DEFAULT is `(void *)-2` on Darwin; Swift cannot import that macro.
        var pointer = dlsym(UnsafeMutableRawPointer(bitPattern: -2), symbol)
        for framework in ["/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",
                          "/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices"]
        where pointer == nil {
            // The handle is intentionally never closed, keeping the symbol valid.
            if let handle = dlopen(framework, RTLD_NOW | RTLD_GLOBAL) { pointer = dlsym(handle, symbol) }
        }
        guard let pointer else { return nil }
        return unsafeBitCast(pointer, to: SetEnabled.self)
    }()

    /// Ids the shared takeover asks the host to disable live.
    static func ids(in configureResult: [String: Any]) -> [Int32] {
        let raw = configureResult["disable_symbolic_hotkeys"] as? [Any] ?? []
        return raw.compactMap { ($0 as? NSNumber)?.int32Value }
    }

    /// Returns failures to log; shipping keeps registering shortcuts anyway.
    static func disable(_ ids: [Int32]) -> [String] {
        guard !ids.isEmpty else { return [] }
        guard let setEnabled else { return ["CGSSetSymbolicHotKeyEnabled is unavailable"] }
        return ids.compactMap { id in
            let status = setEnabled(id, false)
            return status == 0 ? nil : "CGSSetSymbolicHotKeyEnabled(\(id)) returned \(status)"
        }
    }
}

enum CaptureShortcut: String {
    case newCapture = "new_capture"
    case region, window, display
    case recordRegion = "record_region"
    case recordWindow = "record_window"
    case recordDisplay = "record_display"

    var mode: UnifiedCaptureMode {
        switch self {
        case .recordRegion, .recordWindow, .recordDisplay: return .record
        default: return .screenshot
        }
    }

    var target: UnifiedCaptureTarget? {
        switch self {
        case .newCapture: return nil
        case .region, .recordRegion: return .region
        case .window, .recordWindow: return .window
        case .display, .recordDisplay: return .display
        }
    }
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

    // Pure policy requests are also safe for fixture Preferences; no live owner
    // is constructed and no global shortcut is registered by these methods.
    static func record(code: String, control: Bool, shift: Bool, alt: Bool, meta: Bool) throws -> [String: Any] {
        try request(["operation": "record", "platform": "macos", "event": [
            "code": code, "ctrlKey": control, "shiftKey": shift, "altKey": alt, "metaKey": meta,
        ]])
    }

    static func display(_ shortcut: String) throws -> [String] {
        let result = try request(["operation": "display", "platform": "macos", "shortcut": shortcut])
        guard let keys = result["keys"] as? [String] else { throw AppBridgeError.invalidResponse }
        return keys
    }

    /// Live WindowServer disable for the shared takeover's macOS ids.
    /// Tests replace it so they never change the login session's hotkeys.
    private let disableSymbolicHotKeys: ([Int32]) -> [String]

    init(settings: [String: Any],
         disableSymbolicHotKeys: @escaping ([Int32]) -> [String] = SymbolicHotKeys.disable) throws {
        precondition(Thread.isMainThread)
        self.disableSymbolicHotKeys = disableSymbolicHotKeys
        let result = try Self.request(["operation": "configure", "settings": settings])
        closed = false
        applyTakeover(result)
    }
    deinit { close() }

    func update(settings: [String: Any]) throws {
        precondition(!closed)
        applyTakeover(try Self.request(["operation": "configure", "settings": settings]))
    }

    /// Shipping disables the overlapping system keys, then registers its own;
    /// the OS-registered Captures chords already exist here and only receive
    /// the keys once WindowServer stops consuming them.
    private func applyTakeover(_ result: [String: Any]) {
        let errors = (result["takeover_errors"] as? [String] ?? [])
            + disableSymbolicHotKeys(SymbolicHotKeys.ids(in: result))
        for error in errors { Metrics.write(["event": "system-shortcut-takeover", "detail": error]) }
    }

    func setEnabled(_ enabled: Bool) {
        guard !closed else { return }
        _ = try? Self.request(["operation": "enabled", "enabled": enabled])
    }

    func setRestoreOnly(_ restoreOnly: Bool) {
        guard !closed else { return }
        _ = try? Self.request(["operation": "restore_only", "restore_only": restoreOnly])
    }

    /// Route the display shortcut past a running recording's capture flow.
    func setRecordingScreenshot(_ allowed: Bool) {
        guard !closed else { return }
        _ = try? Self.request(["operation": "recording_screenshot", "allowed": allowed])
    }

    func setSuspended(_ suspended: Bool) throws {
        guard !closed else { return }
        _ = try Self.request(["operation": "suspended", "suspended": suspended])
    }

    func setSelectorGeneration(_ generation: UInt64?) throws {
        guard !closed else { return }
        _ = try Self.request(["operation": "selector",
                              "generation": generation.map { $0 as Any } ?? NSNull()])
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
