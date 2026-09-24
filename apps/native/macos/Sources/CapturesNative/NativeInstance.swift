import Foundation
import CCapturesSettings

protocol NativeInstanceTransport {
    func request(_ object: [String: Any]) throws -> [String: Any]
}

private func wakeNativeInstance() {
    // Rust may invoke this on a worker. Never reenter the instance FFI from the
    // callback; the AppKit thread drains after receiving the notification.
    DispatchQueue.main.async {
        NotificationCenter.default.post(name: NativeInstance.wakeNotification, object: nil)
    }
}

final class NativeInstanceBridge: NativeInstanceTransport {
    func request(_ object: [String: Any]) throws -> [String: Any] {
        precondition(Thread.isMainThread)
        let data = try JSONSerialization.data(withJSONObject: object)
        let pointer = String(decoding: data, as: UTF8.self).withCString {
            captures_instance_request_v1($0, wakeNativeInstance)
        }
        guard let pointer else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(pointer) }
        return try AppBridge.decode(Data(bytes: pointer, count: strlen(pointer)))
    }
}

final class NativeInstance {
    static let wakeNotification = Notification.Name("CapturesNativeInstanceWake")

    private let transport: NativeInstanceTransport
    private var closed = false

    private init(transport: NativeInstanceTransport) {
        self.transport = transport
    }

    static func start(historyRoot: String?, paths: [String],
                      transport: NativeInstanceTransport = NativeInstanceBridge()) throws
        -> (primary: Bool, owner: NativeInstance?) {
        precondition(Thread.isMainThread)
        let result = try transport.request([
            "operation": "start",
            "history_root": historyRoot.map { $0 as Any } ?? NSNull(),
            "paths": paths,
        ])
        guard let primary = result["primary"] as? Bool else { throw AppBridgeError.invalidResponse }
        return (primary, primary ? NativeInstance(transport: transport) : nil)
    }

    func nextRequest() throws -> [String]? {
        precondition(Thread.isMainThread)
        guard !closed else { return nil }
        let result = try transport.request(["operation": "next"])
        if result["request"] is NSNull { return nil }
        guard let request = result["request"] as? [String: Any],
              let paths = request["paths"] as? [String]
        else { throw AppBridgeError.invalidResponse }
        return paths
    }

    func stopAccepting() {
        precondition(Thread.isMainThread)
        guard !closed else { return }
        _ = try? transport.request(["operation": "stop"])
    }

    func close() {
        precondition(Thread.isMainThread)
        guard !closed else { return }
        _ = try? transport.request(["operation": "close"])
        closed = true
    }

    deinit {
        assert(closed, "NativeInstance must be explicitly closed after termination is accepted")
    }
}
