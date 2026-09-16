#if os(macOS)
  import AppKit
  import CoreGraphics
  import CoreMedia
  import CoreVideo
  import CryptoKit
  import Darwin
  import Foundation
  import ScreenCaptureKit

  private let measurementDescription =
    "ScreenCaptureKit WindowServer display timestamps; not physical-panel scanout"

  private struct ReadyReport: Encodable {
    let schema = 1
    let pid: Int32
    let targetPid: Int32
    let window_id: UInt32
    let width: Int
    let height: Int
    let requestedHz: Int
    let measurement = measurementDescription
  }

  private struct DisplayReport: Encodable {
    let scale: Double
    let width: Double
    let height: Double
    let screenCaptureAllowed: Bool
  }

  private struct ObservedFrame: Encodable {
    let status: Int
    let displayTimeNs: UInt64?
    let arrivalHostTimeNs: UInt64
    let pixelHash: String?
    let processingNs: UInt64
  }

  private struct FinalReport: Encodable {
    let schema = 1
    let window_id: UInt32
    let targetPid: Int32
    let requestedHz: Int
    let width: Int
    let height: Int
    let startHostTimeNs: UInt64
    let endHostTimeNs: UInt64
    let frames: [ObservedFrame]
    let complete: Bool
    let metric: String
  }

  private struct ErrorReport: Encodable {
    let schema = 1
    let error: String
  }

  private enum ObserverError: Error, CustomStringConvertible {
    case usage(String)
    case invalidArgument(String)
    case operation(String)

    var description: String {
      switch self {
      case .usage(let message), .invalidArgument(let message), .operation(let message):
        return message
      }
    }
  }

  private func hostTimeNs(_ ticks: UInt64 = mach_absolute_time()) -> UInt64 {
    struct Timebase {
      static let value: mach_timebase_info_data_t = {
        var info = mach_timebase_info_data_t()
        mach_timebase_info(&info)
        return info
      }()
    }
    let info = Timebase.value
    let product = ticks.multipliedFullWidth(by: UInt64(info.numer))
    let quotient = UInt64(info.denom).dividingFullWidth(product).quotient
    return quotient
  }

  private func writeJSON<T: Encodable>(_ value: T, to url: URL) throws {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    let data = try encoder.encode(value)
    try data.write(to: url, options: .atomic)
  }

  private func stderr(_ message: String) {
    FileHandle.standardError.write(Data((message + "\n").utf8))
  }

  private func runDisplay(arguments: ArraySlice<String>) throws {
    guard arguments.isEmpty else {
      throw ObserverError.usage("usage: observer display")
    }
    guard let screen = NSScreen.main else {
      throw ObserverError.operation("AppKit did not provide a main display")
    }
    let report = DisplayReport(
      scale: Double(screen.backingScaleFactor), width: Double(screen.visibleFrame.width),
      height: Double(screen.visibleFrame.height),
      screenCaptureAllowed: CGPreflightScreenCaptureAccess())
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    FileHandle.standardOutput.write(try encoder.encode(report))
    FileHandle.standardOutput.write(Data("\n".utf8))
  }

  private func runLaunch(arguments: ArraySlice<String>) throws {
    guard arguments.count == 3 else {
      throw ObserverError.usage("usage: observer launch APP_BUNDLE CONFIG_JSON OUTPUT_JSON")
    }
    let appURL = URL(fileURLWithPath: arguments[arguments.startIndex])
    let configPath = arguments[arguments.index(after: arguments.startIndex)]
    let outputPath = arguments[arguments.index(arguments.startIndex, offsetBy: 2)]
    let configuration = NSWorkspace.OpenConfiguration()
    configuration.arguments = [configPath, outputPath]
    configuration.createsNewApplicationInstance = true
    configuration.activates = true

    var result: Result<Int32, Error>?
    NSWorkspace.shared.openApplication(at: appURL, configuration: configuration) {
      application, error in
      DispatchQueue.main.async {
        if let error = error {
          result = .failure(error)
        } else if let application = application {
          result = .success(application.processIdentifier)
        } else {
          result = .failure(
            ObserverError.operation("LaunchServices returned neither an application nor an error"))
        }
      }
    }
    let deadline = Date(timeIntervalSinceNow: 30)
    while result == nil {
      guard Date() < deadline else {
        throw ObserverError.operation("LaunchServices launch timed out")
      }
      RunLoop.current.run(mode: .default, before: Date(timeIntervalSinceNow: 0.05))
    }
    switch result! {
    case .success(let pid):
      let data = try JSONSerialization.data(
        withJSONObject: ["pid": Int(pid)], options: [.sortedKeys])
      FileHandle.standardOutput.write(data)
      FileHandle.standardOutput.write(Data("\n".utf8))
    case .failure(let error):
      throw ObserverError.operation(
        "could not launch \(appURL.path) through LaunchServices: \(error.localizedDescription)")
    }
  }

  private func terminate(arguments: ArraySlice<String>) throws {
    let values = Array(arguments)
    guard values.count == 2, let pid = Int32(values[0]), pid > 0 else {
      throw ObserverError.usage("usage: observer terminate PID UNIQUE_BUNDLE_ID")
    }
    guard let app = NSRunningApplication(processIdentifier: pid) else { return }
    guard app.bundleIdentifier == values[1] else {
      throw ObserverError.operation("refusing to terminate a PID with a different bundle identity")
    }
    if !app.terminate() && !app.forceTerminate() {
      throw ObserverError.operation("could not request termination of PID \(pid)")
    }
    let deadline = Date(timeIntervalSinceNow: 10)
    while !app.isTerminated && Date() < deadline {
      RunLoop.current.run(mode: .default, before: Date(timeIntervalSinceNow: 0.05))
    }
    if !app.isTerminated {
      guard app.forceTerminate() else {
        throw ObserverError.operation("could not force termination of PID \(pid)")
      }
      let forceDeadline = Date(timeIntervalSinceNow: 5)
      while !app.isTerminated && Date() < forceDeadline {
        RunLoop.current.run(mode: .default, before: Date(timeIntervalSinceNow: 0.05))
      }
    }
    guard app.isTerminated else {
      throw ObserverError.operation("PID \(pid) remained alive after forced termination")
    }
  }

  private final class CaptureObserver: NSObject, SCStreamOutput, SCStreamDelegate {
    private let windowID: CGWindowID
    private let targetPID: pid_t
    private let width: Int
    private let height: Int
    private let requestedHz: Int
    private let outputURL: URL
    private let readyURL: URL
    private let stopURL: URL
    private let errorURL: URL
    private let callbackQueue = DispatchQueue(
      label: "es.captur.observer.frames", qos: .userInteractive)
    private var stream: SCStream?
    private var frames: [ObservedFrame] = []
    private var startHostTimeNs: UInt64 = 0
    // Scheduled only on callbackQueue; readyWritten belongs to the main thread.
    private var readinessScheduled = false
    private var readyWritten = false
    private var finishing = false
    private var exitCode: Int32?
    private var deadline: Date!

    init(
      windowID: CGWindowID, targetPID: pid_t, width: Int, height: Int, requestedHz: Int,
      outputURL: URL
    ) {
      self.windowID = windowID
      self.targetPID = targetPID
      self.width = width
      self.height = height
      self.requestedHz = requestedHz
      self.outputURL = outputURL
      self.readyURL = URL(fileURLWithPath: outputURL.path + ".ready.json")
      self.stopURL = URL(fileURLWithPath: outputURL.path + ".stop")
      self.errorURL = URL(fileURLWithPath: outputURL.path + ".error.json")
    }

    func run() -> Int32 {
      deadline = Date(timeIntervalSinceNow: 120)
      begin()
      while exitCode == nil {
        RunLoop.current.run(mode: .default, before: Date(timeIntervalSinceNow: 0.05))
        poll()
      }
      return exitCode!
    }

    private func begin() {
      stderr("observer: enumerating window \(windowID) for PID \(targetPID)")
      guard CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess() else {
        fail(
          "Screen Recording permission was denied. Enable this observer in System Settings > Privacy & Security > Screen Recording, then run it again."
        )
        return
      }
      SCShareableContent.getExcludingDesktopWindows(true, onScreenWindowsOnly: false) {
        [weak self] content, error in
        DispatchQueue.main.async {
          guard let self = self, !self.finishing else { return }
          if let error = error {
            self.fail(
              "could not enumerate shareable windows (check Screen Recording permission): \(error.localizedDescription)"
            )
            return
          }
          guard let window = content?.windows.first(where: { $0.windowID == self.windowID }) else {
            self.fail("window \(self.windowID) was not present in SCShareableContent")
            return
          }
          guard let owner = window.owningApplication else {
            self.fail("window \(self.windowID) has no owning application")
            return
          }
          guard owner.processID == self.targetPID else {
            self.fail(
              "window \(self.windowID) belongs to PID \(owner.processID), not requested PID \(self.targetPID)"
            )
            return
          }
          self.start(window: window)
        }
      }
    }

    private func start(window: SCWindow) {
      stderr("observer: starting \(width)x\(height) capture")
      let configuration = SCStreamConfiguration()
      configuration.width = width
      configuration.height = height
      configuration.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(requestedHz))
      configuration.queueDepth = 8
      configuration.showsCursor = false
      configuration.capturesAudio = false
      configuration.pixelFormat = kCVPixelFormatType_32BGRA
      if #available(macOS 14.0, *) {
        configuration.ignoreShadowsSingleWindow = true
      }
      let filter = SCContentFilter(desktopIndependentWindow: window)
      let newStream = SCStream(filter: filter, configuration: configuration, delegate: self)
      stream = newStream
      do {
        try newStream.addStreamOutput(self, type: .screen, sampleHandlerQueue: callbackQueue)
      } catch {
        fail("could not attach ScreenCaptureKit frame output: \(error.localizedDescription)")
        return
      }
      // Define the raw observation interval from the request to start capture. This
      // is deliberately independent of the renderer's separately-recorded marker.
      startHostTimeNs = hostTimeNs()
      newStream.startCapture { [weak self] error in
        DispatchQueue.main.async {
          guard let self = self, !self.finishing else { return }
          if let error = error {
            self.fail("ScreenCaptureKit could not start capture: \(error.localizedDescription)")
          } else {
            stderr("observer: capture started")
          }
        }
      }
    }

    private func poll() {
      guard !finishing else { return }
      if FileManager.default.fileExists(atPath: stopURL.path) {
        finishSuccessfully()
      } else if Date() >= deadline {
        fail("capture exceeded its 120-second deadline before \(stopURL.path) appeared")
      }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
      DispatchQueue.main.async { [weak self] in
        guard let self = self, !self.finishing else { return }
        self.fail(
          "ScreenCaptureKit stopped unexpectedly: \(error.localizedDescription)",
          streamAlreadyStopped: true)
      }
    }

    func stream(
      _ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer,
      of outputType: SCStreamOutputType
    ) {
      guard outputType == .screen else { return }
      let arrivalTicks = mach_absolute_time()
      let arrivalNs = hostTimeNs(arrivalTicks)
      guard
        let attachmentArray = CMSampleBufferGetSampleAttachmentsArray(
          sampleBuffer, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
        let attachment = attachmentArray.first,
        let statusRaw = attachment[.status] as? Int
      else {
        DispatchQueue.main.async { [weak self] in
          self?.fail("ScreenCaptureKit delivered a frame without SCStream frame status metadata")
        }
        return
      }

      let displayNs = (attachment[.displayTime] as? UInt64).map(hostTimeNs)
      if frames.isEmpty { stderr("observer: first frame status \(statusRaw)") }
      // Started is metadata, not documented image data. Wait for a complete
      // frame before releasing the workload so pixel changes have a baseline.
      if statusRaw == 4 {
        appendFrame(
          status: statusRaw, displayTimeNs: displayNs, arrivalNs: arrivalNs,
          pixelHash: nil, processingStartTicks: arrivalTicks)
        return
      }
      guard isPopulated(statusRaw) else {
        appendFrame(
          status: statusRaw, displayTimeNs: displayNs, arrivalNs: arrivalNs,
          pixelHash: nil, processingStartTicks: arrivalTicks)
        return
      }
      guard let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer) else {
        appendFrame(
          status: statusRaw, displayTimeNs: displayNs, arrivalNs: arrivalNs,
          pixelHash: nil, processingStartTicks: arrivalTicks)
        DispatchQueue.main.async { [weak self] in
          self?.fail("a populated frame had no pixel buffer")
        }
        return
      }
      guard CVPixelBufferGetWidth(pixelBuffer) == width,
        CVPixelBufferGetHeight(pixelBuffer) == height
      else {
        appendFrame(
          status: statusRaw, displayTimeNs: displayNs, arrivalNs: arrivalNs,
          pixelHash: nil, processingStartTicks: arrivalTicks)
        let actual = "\(CVPixelBufferGetWidth(pixelBuffer))x\(CVPixelBufferGetHeight(pixelBuffer))"
        DispatchQueue.main.async { [weak self] in
          self?.fail(
            "ScreenCaptureKit produced \(actual), expected \(self?.width ?? 0)x\(self?.height ?? 0)"
          )
        }
        return
      }
      guard CVPixelBufferGetPixelFormatType(pixelBuffer) == kCVPixelFormatType_32BGRA else {
        appendFrame(
          status: statusRaw, displayTimeNs: displayNs, arrivalNs: arrivalNs,
          pixelHash: nil, processingStartTicks: arrivalTicks)
        DispatchQueue.main.async { [weak self] in self?.fail("populated frame was not BGRA") }
        return
      }
      do {
        let hash = try hashVisibleBGRA(pixelBuffer)
        appendFrame(
          status: statusRaw, displayTimeNs: displayNs, arrivalNs: arrivalNs,
          pixelHash: hash, processingStartTicks: arrivalTicks)
        scheduleReadiness()
      } catch {
        appendFrame(
          status: statusRaw, displayTimeNs: displayNs, arrivalNs: arrivalNs,
          pixelHash: nil, processingStartTicks: arrivalTicks)
        DispatchQueue.main.async { [weak self] in self?.fail(error.localizedDescription) }
      }
    }

    private func isPopulated(_ statusRaw: Int) -> Bool {
      statusRaw == SCFrameStatus.complete.rawValue
    }

    private func scheduleReadiness() {
      guard !readinessScheduled else { return }
      readinessScheduled = true
      let report = ReadyReport(
        pid: ProcessInfo.processInfo.processIdentifier,
        targetPid: targetPID, window_id: windowID, width: width,
        height: height, requestedHz: requestedHz)
      DispatchQueue.main.async { [self] in
        guard !finishing else { return }
        do {
          try writeJSON(report, to: readyURL)
          readyWritten = true
        } catch {
          fail("could not write readiness file atomically: \(error.localizedDescription)")
        }
      }
    }

    private func hashVisibleBGRA(_ pixelBuffer: CVPixelBuffer) throws -> String {
      let lockResult = CVPixelBufferLockBaseAddress(pixelBuffer, .readOnly)
      guard lockResult == kCVReturnSuccess else {
        throw ObserverError.operation(
          "could not lock a ScreenCaptureKit pixel buffer (CVReturn \(lockResult))")
      }
      defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, .readOnly) }
      guard let base = CVPixelBufferGetBaseAddress(pixelBuffer) else {
        throw ObserverError.operation("ScreenCaptureKit pixel buffer had no base address")
      }
      let rowBytes = width * 4
      let stride = CVPixelBufferGetBytesPerRow(pixelBuffer)
      guard stride >= rowBytes else {
        throw ObserverError.operation(
          "BGRA pixel-buffer stride \(stride) is shorter than visible row \(rowBytes)")
      }
      var sha = SHA256()
      for row in 0..<height {
        sha.update(
          bufferPointer: UnsafeRawBufferPointer(
            start: base.advanced(by: row * stride), count: rowBytes))
      }
      return sha.finalize().map { String(format: "%02x", $0) }.joined()
    }

    private func appendFrame(
      status: Int, displayTimeNs: UInt64?, arrivalNs: UInt64,
      pixelHash: String?, processingStartTicks: UInt64
    ) {
      frames.append(
        ObservedFrame(
          status: status, displayTimeNs: displayTimeNs,
          arrivalHostTimeNs: arrivalNs, pixelHash: pixelHash,
          processingNs: hostTimeNs() - hostTimeNs(processingStartTicks)))
    }

    private func finishSuccessfully() {
      guard readyWritten else {
        fail(
          "stop was requested before ScreenCaptureKit delivered a valid started or complete frame")
        return
      }
      finishing = true
      stopStream { [weak self] stopError in
        guard let self = self else { return }
        if let stopError = stopError {
          self.writeFailure(
            "ScreenCaptureKit could not stop cleanly: \(stopError.localizedDescription)")
          return
        }
        self.callbackQueue.async {
          let report = FinalReport(
            window_id: self.windowID, targetPid: self.targetPID,
            requestedHz: self.requestedHz, width: self.width, height: self.height,
            startHostTimeNs: self.startHostTimeNs, endHostTimeNs: hostTimeNs(),
            frames: self.frames, complete: true, metric: measurementDescription)
          do {
            try writeJSON(report, to: self.outputURL)
            DispatchQueue.main.async { self.exitCode = 0 }
          } catch {
            DispatchQueue.main.async {
              self.writeFailure(
                "could not write final report atomically: \(error.localizedDescription)")
            }
          }
        }
      }
    }

    private func fail(_ message: String, streamAlreadyStopped: Bool = false) {
      guard !finishing else { return }
      finishing = true
      if streamAlreadyStopped {
        writeFailure(message)
      } else {
        stopStream { [weak self] stopError in
          let detail =
            stopError.map { "\(message); stopping capture also failed: \($0.localizedDescription)" }
            ?? message
          self?.writeFailure(detail)
        }
      }
    }

    private func stopStream(completion: @escaping (Error?) -> Void) {
      guard let stream = stream else {
        completion(nil)
        return
      }
      stream.stopCapture { error in
        DispatchQueue.main.async {
          completion(error)
        }
      }
    }

    private func writeFailure(_ message: String) {
      stderr("observer: \(message)")
      callbackQueue.async { [self] in
        var detail = message
        let report = FinalReport(
          window_id: windowID, targetPid: targetPID,
          requestedHz: requestedHz, width: width, height: height,
          startHostTimeNs: startHostTimeNs, endHostTimeNs: hostTimeNs(),
          frames: frames, complete: false, metric: measurementDescription)
        do {
          try writeJSON(report, to: outputURL)
        } catch {
          detail += "; could not preserve incomplete frame report: \(error.localizedDescription)"
        }
        do {
          try writeJSON(ErrorReport(error: detail), to: errorURL)
        } catch {
          stderr("observer: could not write error report: \(error.localizedDescription)")
        }
        DispatchQueue.main.async { self.exitCode = 1 }
      }
    }
  }

  private func runCapture(arguments: ArraySlice<String>) throws -> Int32 {
    guard arguments.count == 6 else {
      throw ObserverError.usage("usage: observer capture WINDOW_ID PID WIDTH HEIGHT HZ OUTPUT_JSON")
    }
    // A command-line process must establish AppKit's WindowServer connection
    // before SCContentFilter/SCStream use it (CGS_REQUIRE_INIT otherwise aborts).
    // No observer window or Dock icon should compete with the measured app.
    NSApplication.shared.setActivationPolicy(.prohibited)
    NSApplication.shared.finishLaunching()
    let values = Array(arguments)
    guard let windowID = UInt32(values[0]) else {
      throw ObserverError.invalidArgument("invalid WINDOW_ID")
    }
    guard let targetPID = Int32(values[1]), targetPID > 0 else {
      throw ObserverError.invalidArgument("invalid PID")
    }
    guard let width = Int(values[2]), width > 0 else {
      throw ObserverError.invalidArgument("invalid WIDTH")
    }
    guard let height = Int(values[3]), height > 0 else {
      throw ObserverError.invalidArgument("invalid HEIGHT")
    }
    guard let hz = Int(values[4]), hz > 0, hz <= Int(Int32.max) else {
      throw ObserverError.invalidArgument("invalid HZ")
    }
    return CaptureObserver(
      windowID: windowID, targetPID: targetPID, width: width,
      height: height, requestedHz: hz,
      outputURL: URL(fileURLWithPath: values[5])
    ).run()
  }

  @main
  private enum ObserverMain {
    static func main() {
      do {
        let arguments = CommandLine.arguments.dropFirst()
        guard let mode = arguments.first else {
          throw ObserverError.usage(
            "usage: observer (display|preflight|launch|capture|terminate) ...")
        }
        switch mode {
        case "display":
          try runDisplay(arguments: arguments.dropFirst())
        case "preflight":
          guard CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess() else {
            throw ObserverError.operation(
              "Enable Screen Recording for Terminal/parity-observer in System Settings, then restart the terminal and retry. No measurement has started."
            )
          }
        case "launch":
          try runLaunch(arguments: arguments.dropFirst())
        case "capture":
          Darwin.exit(try runCapture(arguments: arguments.dropFirst()))
        case "terminate":
          try terminate(arguments: arguments.dropFirst())
        default:
          throw ObserverError.usage(
            "unknown mode '\(mode)'; expected display, preflight, launch, capture or terminate")
        }
      } catch {
        stderr("observer: \(error)")
        Darwin.exit(2)
      }
    }
  }
#else
  import Foundation
  import Glibc

  @main
  private enum ObserverMain {
    static func main() {
      FileHandle.standardError.write(Data("observer: this helper requires macOS\n".utf8))
      exit(2)
    }
  }
#endif
