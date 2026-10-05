import Foundation
import Darwin

enum UpdateHealthError: Error { case invalidFile }

/// A one-use acknowledgement for the updater-owned, already-open readiness file.
final class UpdateHealthAcknowledgement {
    private var descriptor: Int32
    private let bytes: Data
    private let lock = NSLock()

    init(file: String, token: String) throws {
        var pathStatus = stat()
        guard lstat(file, &pathStatus) == 0, (pathStatus.st_mode & S_IFMT) == S_IFREG else {
            throw UpdateHealthError.invalidFile
        }
        let descriptor = open(file, O_WRONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)
        guard descriptor >= 0 else { throw UpdateHealthError.invalidFile }
        var status = stat()
        guard fstat(descriptor, &status) == 0, (status.st_mode & S_IFMT) == S_IFREG,
              status.st_size == 0 else {
            close(descriptor)
            throw UpdateHealthError.invalidFile
        }
        self.descriptor = descriptor
        bytes = Data("\(token)\n".utf8)
    }

    deinit { closeIfNeeded() }

    func acknowledge() throws {
        lock.lock()
        defer { lock.unlock() }
        guard descriptor >= 0 else { return }
        let result = bytes.withUnsafeBytes { write(descriptor, $0.baseAddress, $0.count) }
        guard result == bytes.count, fsync(descriptor) == 0 else {
            close(descriptor); descriptor = -1
            throw UpdateHealthError.invalidFile
        }
        close(descriptor); descriptor = -1
    }

    func cancel() {
        lock.lock(); defer { lock.unlock() }
        closeIfNeeded()
    }

    private func closeIfNeeded() {
        if descriptor >= 0 { close(descriptor); descriptor = -1 }
    }
}

/// Gates acknowledgement on the rendered workspace, a successful settings load,
/// and packaged media-tool verification. Verification never occupies AppKit's queue.
final class UpdateHealthCoordinator {
    private let acknowledgement: UpdateHealthAcknowledgement
    private let verifyTools: () throws -> Void
    private let queue: DispatchQueue
    private let lock = NSLock()
    private var workspaceRendered = false
    private var settingsLoaded = false
    private var verificationStarted = false
    private var cancelled = false

    init(acknowledgement: UpdateHealthAcknowledgement,
         queue: DispatchQueue = DispatchQueue(label: "com.captures.update-health", qos: .userInitiated),
         verifyTools: @escaping () throws -> Void) {
        self.acknowledgement = acknowledgement
        self.queue = queue
        self.verifyTools = verifyTools
    }

    func workspaceDidRender() { advance(workspace: true, settings: false) }
    func settingsDidLoad() { advance(workspace: false, settings: true) }

    func cancel() {
        lock.lock(); cancelled = true; lock.unlock()
        acknowledgement.cancel()
    }

    private func advance(workspace: Bool, settings: Bool) {
        lock.lock()
        workspaceRendered = workspaceRendered || workspace
        settingsLoaded = settingsLoaded || settings
        let start = workspaceRendered && settingsLoaded && !verificationStarted && !cancelled
        if start { verificationStarted = true }
        lock.unlock()
        guard start else { return }
        queue.async { [self] in
            guard (try? verifyTools()) != nil else { return }
            lock.lock()
            if !cancelled { try? acknowledgement.acknowledge() }
            lock.unlock()
        }
    }
}

extension NativeMediaTools {
    static func packaged(executable: URL? = Bundle.main.executableURL) throws -> Self {
        let suffix = ProcessInfo.processInfo.machineHardwareName == "arm64"
            ? "aarch64-apple-darwin" : "x86_64-apple-darwin"
        guard let directory = executable?.deletingLastPathComponent() else {
            throw UpdateHealthError.invalidFile
        }
        let ffmpeg = directory.appendingPathComponent("binaries/ffmpeg-\(suffix)").path
        let ffprobe = directory.appendingPathComponent("binaries/ffprobe-\(suffix)").path
        guard FileManager.default.isExecutableFile(atPath: ffmpeg),
              FileManager.default.isExecutableFile(atPath: ffprobe) else {
            throw UpdateHealthError.invalidFile
        }
        return Self(ffmpeg: ffmpeg, ffprobe: ffprobe)
    }
}
