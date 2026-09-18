import Foundation
import CCapturesSettings

enum SettingsStoreError: LocalizedError {
    case invalidResponse
    case backend(String)
    var errorDescription: String? {
        switch self {
        case .invalidResponse: return "The settings service returned an invalid response."
        case .backend(let message): return message
        }
    }
}

/// Thin transport only. Validation, defaults and migrations remain in shared Rust.
protocol SettingsTransport {
    func request(_ object: [String: Any]) throws -> [String: Any]
}

final class SettingsBridge: SettingsTransport {
    func request(_ object: [String: Any]) throws -> [String: Any] {
        let data = try JSONSerialization.data(withJSONObject: object)
        let result: UnsafeMutablePointer<CChar>? = String(decoding: data, as: UTF8.self).withCString {
            captures_settings_request_v1($0)
        }
        guard let result else { throw SettingsStoreError.invalidResponse }
        defer { captures_settings_free_v1(result) }
        let responseData = Data(bytes: result, count: strlen(result))
        guard let response = try JSONSerialization.jsonObject(with: responseData) as? [String: Any],
              let ok = response["ok"] as? Bool else { throw SettingsStoreError.invalidResponse }
        if !ok { throw SettingsStoreError.backend(response["error"] as? String ?? "Settings could not be saved.") }
        return response
    }

    func defaultPath() throws -> String {
        guard let path = try request(["operation": "default_path"])["path"] as? String else {
            throw SettingsStoreError.invalidResponse
        }
        return path
    }
}

/// Serial, coalescing persistence. Completion from an older revision never updates newer UI.
final class SettingsStore {
    typealias Completion = (Int, Result<[String: Any], Error>) -> Void
    private let transport: SettingsTransport
    private let queue = DispatchQueue(label: "es.captures.native.settings")
    private let path: String
    private var pending: (Int, [String: Any], Completion)?
    private var debounce: DispatchWorkItem?
    private var revision = 0
    private let revisionLock = NSLock()
    private let debounceInterval: TimeInterval

    init(path: String?, transport: SettingsTransport = SettingsBridge(), debounceInterval: TimeInterval = 0.18) throws {
        self.transport = transport
        self.debounceInterval = debounceInterval
        if let path { self.path = path }
        else {
            guard let path = try transport.request(["operation": "default_path"])["path"] as? String else {
                throw SettingsStoreError.invalidResponse
            }
            self.path = path
        }
    }

    func load(_ completion: @escaping (Result<[String: Any], Error>) -> Void) {
        queue.async {
            let result = Result { () throws -> [String: Any] in
                guard let settings = try self.transport.request(["operation": "load", "path": self.path])["settings"] as? [String: Any] else {
                    throw SettingsStoreError.invalidResponse
                }
                return settings
            }
            DispatchQueue.main.async { completion(result) }
        }
    }

    @discardableResult func save(_ settings: [String: Any], completion: @escaping Completion) -> Int {
        revisionLock.lock()
        revision += 1
        let current = revision
        revisionLock.unlock()
        queue.async {
            self.pending = (current, settings, completion)
            self.debounce?.cancel()
            let work = DispatchWorkItem { [weak self] in self?.writePending() }
            self.debounce = work
            self.queue.asyncAfter(deadline: .now() + self.debounceInterval, execute: work)
        }
        return current
    }

    private func writePending() {
        guard let item = pending else { return }
        pending = nil
        let result = Result { () throws -> [String: Any] in
            guard let saved = try transport.request([
                "operation": "save", "path": path, "settings": item.1,
            ])["settings"] as? [String: Any] else { throw SettingsStoreError.invalidResponse }
            return saved
        }
        DispatchQueue.main.async { item.2(item.0, result) }
    }

    func flush() {
        queue.sync {
            debounce?.cancel()
            debounce = nil
            writePending()
        }
    }
}

extension Dictionary where Key == String, Value == Any {
    func string(_ key: String, _ fallback: String = "") -> String { self[key] as? String ?? fallback }
    func bool(_ key: String, _ fallback: Bool = false) -> Bool { self[key] as? Bool ?? fallback }
    func int(_ key: String, _ fallback: Int = 0) -> Int { self[key] as? Int ?? fallback }
}
