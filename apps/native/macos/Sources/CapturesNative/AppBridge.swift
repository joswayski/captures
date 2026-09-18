import Foundation
import CoreGraphics
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
    let includeCursor: Bool
    let directory: String
    let format: String
    let countdown: Int
    let freezeScreen: Bool
    let autoStart: Bool

    init(_ settings: [String: Any]) throws {
        guard let autoCopy = settings["auto_copy_to_clipboard"] as? Bool,
              let includeCursor = settings["show_cursor_in_screenshots"] as? Bool,
              let directory = settings["output_directory"] as? String,
              let format = settings["screenshot_format"] as? String,
              ["png", "jpeg", "webp"].contains(format),
              let freezeScreen = settings["freeze_screen"] as? Bool,
              let autoStart = settings["auto_start_on_selection"] as? Bool,
              let countdown = settings["screenshot_countdown_seconds"] as? Int,
              (0...10).contains(countdown)
        else { throw SettingsStoreError.invalidResponse }
        self.autoCopy = autoCopy; self.directory = directory; self.format = format
        self.includeCursor = includeCursor
        self.countdown = countdown
        self.freezeScreen = freezeScreen; self.autoStart = autoStart
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

/// Immutable Rust ownership. Image providers retain this object, but this object
/// never caches an image/provider (which would create a retain cycle).
final class NativeRegionSession {
    private let handle: OpaquePointer
    let logicalSize: CGSize

    private init(handle: OpaquePointer, logicalSize: CGSize) {
        self.handle = handle; self.logicalSize = logicalSize
    }
    deinit { captures_region_free_v1(handle) }

    static func prepare(display: String, generation: UInt64, preferences: CapturePreferences) throws -> NativeRegionSession {
        var response: UnsafeMutablePointer<CChar>?
        let handle = display.withCString { captures_region_prepare_v1($0, generation,
            preferences.freezeScreen, preferences.includeCursor, &response) }
        defer { captures_settings_free_v1(response) }
        do {
            guard let response else { throw AppBridgeError.invalidResponse }
            let result = try AppBridge.decode(Data(bytes: response, count: strlen(response)))
            guard let handle, let descriptor = result["display"] as? [String: Any],
                  let width = descriptor["width"] as? NSNumber, let height = descriptor["height"] as? NSNumber
            else { throw AppBridgeError.invalidResponse }
            return NativeRegionSession(handle: handle, logicalSize: CGSize(width: width.doubleValue, height: height.doubleValue))
        } catch { captures_region_free_v1(handle); throw error }
    }

    func image() throws -> CGImage? {
        var pixels = CapturesRegionPixels()
        guard captures_region_pixels_v1(handle, &pixels) else { return nil }
        guard let data = pixels.data else { throw AppBridgeError.invalidResponse }
        let retained = Unmanaged.passRetained(self)
        guard let provider = CGDataProvider(dataInfo: retained.toOpaque(), data: data, size: pixels.length,
            releaseData: { info, _, _ in
                if let info { Unmanaged<NativeRegionSession>.fromOpaque(info).release() }
            }) else { retained.release(); throw AppBridgeError.invalidResponse }
        guard let image = CGImage(width: Int(pixels.width), height: Int(pixels.height),
            bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: pixels.bytes_per_row,
            space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
            provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
        else { throw AppBridgeError.invalidResponse }
        return image
    }

    func capture(root: String, rect: CapturesSelectionRect, afterCountdown: Bool) throws -> CaptureArtifact {
        let response = root.withCString { captures_region_capture_v1(handle, $0, rect, afterCountdown) }
        guard let response else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(response) }
        let result = try AppBridge.decode(Data(bytes: response, count: strlen(response)))
        guard let value = result["artifact"] as? [String: Any], let artifact = CaptureArtifact(value)
        else { throw AppBridgeError.invalidResponse }
        return artifact
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
