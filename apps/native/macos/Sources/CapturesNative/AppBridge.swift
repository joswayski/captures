import Foundation
import CCapturesSettings

enum AppBridgeError: LocalizedError, Equatable {
    case invalidResponse
    case backend(String)

    var errorDescription: String? {
        switch self {
        case .invalidResponse: return "The capture service returned an invalid response."
        case .backend(let message): return message
        }
    }
}

protocol AppTransport {
    func request(_ object: [String: Any]) throws -> [String: Any]
}

struct CapturePreferences {
    let autoCopy: Bool
    let directory: String
    let format: String
    let countdown: Int

    init(_ settings: [String: Any]) throws {
        guard let autoCopy = settings["auto_copy_to_clipboard"] as? Bool,
              let directory = settings["output_directory"] as? String,
              let format = settings["screenshot_format"] as? String,
              ["png", "jpeg", "webp"].contains(format),
              let countdown = settings["screenshot_countdown_seconds"] as? Int,
              (0...10).contains(countdown)
        else { throw SettingsStoreError.invalidResponse }
        self.autoCopy = autoCopy; self.directory = directory; self.format = format
        self.countdown = countdown
    }

    static func load(path: String?, transport: SettingsTransport = SettingsBridge()) throws -> Self {
        let resolvedPath: String
        if let path { resolvedPath = path }
        else {
            guard let value = try transport.request(["operation": "default_path"])["path"] as? String
            else { throw SettingsStoreError.invalidResponse }
            resolvedPath = value
        }
        guard let settings = try transport.request(["operation": "load", "path": resolvedPath])["settings"] as? [String: Any]
        else { throw SettingsStoreError.invalidResponse }
        return try Self(settings)
    }
}

final class AppBridge: AppTransport {
    static func flow(_ object: [String: Any]) throws -> [String: Any] {
        precondition(Thread.isMainThread)
        let data = try JSONSerialization.data(withJSONObject: object)
        let pointer = String(decoding: data, as: UTF8.self).withCString { captures_flow_request_v1($0) }
        guard let pointer else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(pointer) }
        return try decode(Data(bytes: pointer, count: strlen(pointer)))
    }

    func request(_ object: [String: Any]) throws -> [String: Any] {
        let data = try JSONSerialization.data(withJSONObject: object)
        let pointer: UnsafeMutablePointer<CChar>? = String(decoding: data, as: UTF8.self).withCString {
            captures_app_request_v1($0)
        }
        guard let pointer else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(pointer) }
        return try Self.decode(Data(bytes: pointer, count: strlen(pointer)))
    }

    static func decode(_ data: Data) throws -> [String: Any] {
        guard let envelope = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let ok = envelope["ok"] as? Bool else { throw AppBridgeError.invalidResponse }
        guard ok else { throw AppBridgeError.backend(envelope["error"] as? String ?? "Capture operation failed.") }
        guard let result = envelope["result"] as? [String: Any] else { throw AppBridgeError.invalidResponse }
        return result
    }
}

struct DisplayItem {
    let id: String
    let title: String
    init?(_ value: [String: Any]) {
        guard let id = value["id"] as? String, let name = value["name"] as? String,
              let width = value["width"] as? NSNumber, let height = value["height"] as? NSNumber else { return nil }
        self.id = id
        let primary = value["is_primary"] as? Bool == true ? " · Main" : ""
        title = "\(name) · \(width.intValue) × \(height.intValue)\(primary)"
    }
}

struct CaptureArtifact {
    let id: String
    let imagePath: String
    let previewPath: String
    let width: Int
    let height: Int
    let createdAt: String
    var savedPath: String?

    init?(_ value: [String: Any]) {
        guard let entry = value["entry"] as? [String: Any], let id = entry["id"] as? String,
              let image = value["image_path"] as? String, let preview = value["preview_path"] as? String,
              let width = entry["width"] as? NSNumber, let height = entry["height"] as? NSNumber,
              let created = entry["created_at"] as? String else { return nil }
        self.id = id; imagePath = image; previewPath = preview
        self.width = width.intValue; self.height = height.intValue; createdAt = created
        savedPath = entry["saved_path"] as? String
    }
}
