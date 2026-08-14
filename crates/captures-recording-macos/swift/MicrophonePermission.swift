import AVFoundation
import Foundation

@_cdecl("captures_microphone_authorized")
public func capturesMicrophoneAuthorized() -> Bool {
    AVCaptureDevice.authorizationStatus(for: .audio) == .authorized
}

@_cdecl("captures_microphone_can_request")
public func capturesMicrophoneCanRequest() -> Bool {
    AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined
}

/// Shows the system microphone prompt when status is still undetermined.
/// Returns whether the process is authorized after the call.
@_cdecl("captures_microphone_request")
public func capturesMicrophoneRequest() -> Bool {
    let status = AVCaptureDevice.authorizationStatus(for: .audio)
    if status == .authorized {
        return true
    }
    if status != .notDetermined {
        return false
    }

    let gate = DispatchSemaphore(value: 0)
    var granted = false
    AVCaptureDevice.requestAccess(for: .audio) { allowed in
        granted = allowed
        gate.signal()
    }
    _ = gate.wait(timeout: .now() + 120)
    return granted
}
