import Foundation
import CCapturesSettings

/// The helper has no AppKit windows or tray; the original process stays the
/// LaunchServices application and owns every document/open-file event.
enum StartupRecovery {
    static func start(_ options: inout Options) {
        guard options.live else { return }
        do {
            var object: [String: Any] = ["operation": "start"]
            if let file = options.nativeUpdateReadyFile, let token = options.nativeUpdateReadyToken {
                object["health"] = ["file": file, "token": token]
            }
            if let health = try request(object)["health"] as? [String: String] {
                options.nativeUpdateReadyFile = health["file"]
                options.nativeUpdateReadyToken = health["token"]
            }
        } catch {
            FileHandle.standardError.write(Data("Startup recovery unavailable. Continuing launch.\n".utf8))
        }
    }

    static func finish() { _ = try? request(["operation": "finish"]) }

    private static func request(_ object: [String: Any]) throws -> [String: Any] {
        precondition(Thread.isMainThread)
        let data = try JSONSerialization.data(withJSONObject: object)
        let pointer = String(decoding: data, as: UTF8.self).withCString {
            captures_preview_recovery_request_v1($0)
        }
        guard let pointer else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(pointer) }
        return try AppBridge.decode(Data(bytes: pointer, count: strlen(pointer)))
    }
}
