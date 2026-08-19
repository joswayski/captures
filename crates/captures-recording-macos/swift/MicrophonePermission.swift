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
    let request = {
        AVCaptureDevice.requestAccess(for: .audio) { allowed in
            granted = allowed
            gate.signal()
        }
    }

    if Thread.isMainThread {
        request()
        // The system prompt needs the main run loop. Waiting on a semaphore
        // here would freeze the dialog, so pump until the user answers.
        let deadline = Date().addingTimeInterval(120)
        while gate.wait(timeout: .now()) == .timedOut, Date() < deadline {
            _ = RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.05))
        }
    } else {
        DispatchQueue.main.async(execute: request)
        _ = gate.wait(timeout: .now() + 120)
    }
    return granted
}
