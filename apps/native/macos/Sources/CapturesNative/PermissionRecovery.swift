import Foundation

/// Shipping Screen Recording recovery dialog (`captures_app::permission_recovery`).
struct CapturePermissionPrompt: Equatable {
    enum Recovery: String {
        case restart
        case resetAndRestart = "reset_and_restart"
    }

    let recovery: Recovery
    let title: String
    let message: String
    let confirm: String
    let cancel: String
    /// Shipping shows the reset dialog as an error and the restart one as info.
    let critical: Bool

    init(_ value: [String: Any]) throws {
        guard let raw = value["recovery"] as? String, let recovery = Recovery(rawValue: raw),
              let title = value["title"] as? String, let message = value["message"] as? String,
              let confirm = value["confirm"] as? String, let cancel = value["cancel"] as? String,
              let critical = value["critical"] as? Bool
        else { throw SettingsStoreError.invalidResponse }
        self.recovery = recovery; self.title = title; self.message = message
        self.confirm = confirm; self.cancel = cancel; self.critical = critical
    }
}

/// A denied capture offers "Restart & Retry" (or "Reset, Restart & Retry"),
/// persists the capture mode, relaunches, and the next launch runs it again.
/// Copy, classification, persistence and the TCC reset stay in shared Rust.
final class CapturePermissionRecovery {
    private let transport: SettingsTransport
    let settingsPath: String

    init(settingsPath: String?, transport: SettingsTransport = SettingsBridge()) throws {
        self.transport = transport
        if let settingsPath { self.settingsPath = settingsPath }
        else {
            guard let path = try transport.request(["operation": "default_path"])["path"] as? String
            else { throw SettingsStoreError.invalidResponse }
            self.settingsPath = path
        }
    }

    static func mode(for kind: StillCaptureKind) -> String {
        switch kind {
        case .region: return "region"
        case .window: return "window"
        case .display: return "display"
        }
    }

    static func kind(for mode: String) -> StillCaptureKind? {
        switch mode {
        case "region": return .region
        case "window": return .window
        case "display": return .display
        default: return nil
        }
    }

    /// Nil where shipping never offers recovery (every platform but macOS).
    func prompt() throws -> CapturePermissionPrompt? {
        let response = try transport.request(["operation": "permission_recovery_prompt"])
        guard let supported = response["supported"] as? Bool,
              let value = response["prompt"] as? [String: Any]
        else { throw SettingsStoreError.invalidResponse }
        return supported ? try CapturePermissionPrompt(value) : nil
    }

    func isPermissionDenied(_ message: String) -> Bool {
        let response = try? transport.request(["operation": "permission_recovery_classify", "message": message])
        return response?["denied"] as? Bool ?? false
    }

    func failureMessage(_ error: String) -> String {
        let response = try? transport.request(["operation": "permission_recovery_classify", "message": error])
        return response?["failure"] as? String ?? error
    }

    func scheduleRetry(_ kind: StillCaptureKind) throws {
        _ = try transport.request(["operation": "permission_recovery_schedule",
                                   "path": settingsPath, "mode": Self.mode(for: kind)])
    }

    /// Takes and clears the retry, so a failed retry cannot relaunch in a loop.
    func takePendingCapture() throws -> StillCaptureKind? {
        let response = try transport.request(["operation": "permission_recovery_take", "path": settingsPath])
        if response["mode"] is NSNull || response["mode"] == nil { return nil }
        guard let mode = response["mode"] as? String, let kind = Self.kind(for: mode)
        else { throw SettingsStoreError.invalidResponse }
        return kind
    }

    /// Resets only this bundle's Screen Recording record (`tccutil`).
    func resetScreenPermission(bundleIdentifier: String?) throws {
        guard let bundleIdentifier, !bundleIdentifier.isEmpty else {
            throw SettingsStoreError.backend("Captures has no bundle identifier to reset.")
        }
        _ = try transport.request(["operation": "permission_recovery_reset",
                                   "path": settingsPath, "bundle_id": bundleIdentifier])
    }
}

/// Shipping `report_capture_error` dialog copy (`captures_app::capture_error`):
/// a failed tray, shortcut or menu capture shows a "Captures" alert, never the
/// History error card.
struct CaptureErrorCopy: Equatable {
    let title: String
    let button: String

    static let current: CaptureErrorCopy = {
        do { return try CaptureErrorCopy(transport: SettingsBridge()) }
        catch { preconditionFailure("Capture error copy is unavailable: \(error.localizedDescription)") }
    }()

    init(transport: SettingsTransport) throws {
        let response = try transport.request(["operation": "capture_error_copy"])
        guard let copy = response["copy"] as? [String: Any],
              let title = copy["title"] as? String, let button = copy["button"] as? String
        else { throw SettingsStoreError.invalidResponse }
        self.title = title; self.button = button
    }
}
