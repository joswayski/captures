import Foundation

@_silgen_name("captures_native_request")
private func nativeRequest(_ request: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>?
@_silgen_name("captures_native_free")
private func nativeFree(_ response: UnsafeMutablePointer<CChar>)

final class Backend {
    static let shared = Backend()
    private let queue = DispatchQueue(label: "es.captur.native.engine", qos: .userInitiated)
    private let mediaQueue = DispatchQueue(label: "es.captur.native.media", qos: .userInitiated)

    // Never block AppKit on permission prompts, capture, encoding, or Rust's
    // serialized recording state machine. All view callbacks return on main.
    func call(_ op: String, _ fields: [String: Any] = [:],
              completion: @escaping (Result<[String: Any], Error>) -> Void) {
        // Long stateless exports must not delay pause/stop/status or overlay
        // session checks. Rust likewise dispatches these outside its engine.
        let destination = ["media_probe", "media_export", "image_encode"].contains(op) ? mediaQueue : queue
        destination.async {
            let result = Result<[String: Any], Error> {
                var request = fields
                request["op"] = op
                let bytes = try JSONSerialization.data(withJSONObject: request)
                guard let json = String(data: bytes, encoding: .utf8) else {
                    throw NativeFailure("Could not encode the engine request.")
                }
                return try json.withCString { pointer in
                    guard let response = nativeRequest(pointer) else {
                        throw NativeFailure("The capture engine could not allocate a response.")
                    }
                    defer { nativeFree(response) }
                    let data = Data(String(cString: response).utf8)
                    guard let envelope = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
                        throw NativeFailure("The capture engine returned invalid JSON.")
                    }
                    guard envelope["ok"] as? Bool == true else {
                        throw NativeFailure(envelope["error"] as? String ?? "The capture operation failed.")
                    }
                    guard let value = envelope["value"] as? [String: Any] else {
                        throw NativeFailure("The capture engine response has no value.")
                    }
                    return value
                }
            }
            DispatchQueue.main.async { completion(result) }
        }
    }
}
