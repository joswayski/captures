#if canImport(AppKit)
  import AppKit
  import CoreImage
  import CoreVideo
  import Darwin
  import Metal
  import QuartzCore

  struct BenchmarkConfig: Decodable {
    let schema: Int
    let scenario, mode: String
    let checkpointMs, durationMs, cycleMs: Double
    let width, height: Int
    let scale: Double
    let fixturePath: String
    let particles: [ThumbnailDustParticle]
    let seed: Int
    let startGatePath: String?
  }

  private struct Ready: Encodable {
    let schema = 1
    let measurementProtocol = 2
    let pid: Int32
    let window_id: Int
    let scale: Double
    let scenario, mode, renderer: String
    let gpuDevice: String
    let checkpointMs: Double
  }

  private struct Result: Encodable {
    let schema: Int, pid: Int32, window_id: Int
    let scale: Double
    let scenario, mode, renderer: String
    let gpuDevice: String
    let checkpointMs, elapsedMs: Double
    let callbackIntervalsMs, setupMs: [Double]
    let cycles: Int
    let complete: Bool
    let metric: String
  }

  private final class FlippedView: NSView {
    override var isFlipped: Bool { true }
  }

  final class NativeRenderer {
    private let config: BenchmarkConfig
    private let output: URL
    private let image: CIImage
    private let ci: CIContext
    private let gpuName: String
    private let root = CALayer()
    private let survivor = CALayer()
    private let exitingShell = CALayer()
    private let exiting = CALayer()
    private let dust = CALayer()
    private let dustMask = CAShapeLayer()
    private var chips: [CALayer] = []
    private var window: NSWindow!
    private var displayLink: CVDisplayLink?
    private var started = 0.0
    private var lastTimestamp: Double?
    private var callbackIntervals: [Double] = []
    private var setupTimes: [Double] = []
    private var cycle = -1
    private var completed = false
    private let callbackLock = NSLock()
    private var callbackPending = false

    init(config: BenchmarkConfig, output: URL) throws {
      self.config = config
      self.output = output
      guard let device = MTLCreateSystemDefaultDevice() else {
        throw Failure("No Metal device; refusing a software-only graphical benchmark")
      }
      gpuName = device.name
      // Explicit Metal device: preparation cannot silently choose a CPU context.
      // CSS brightness multiplies sRGB components, not linear-light components.
      ci = CIContext(
        mtlDevice: device,
        options: [
          .cacheIntermediates: true,
          .workingColorSpace: CGColorSpace(name: CGColorSpace.sRGB)!,
        ])
      guard config.schema == 1, config.width == 640, config.height == 720,
        config.scale > 0, config.cycleMs == 3200,
        config.checkpointMs.isFinite, config.checkpointMs >= 0,
        config.durationMs.isFinite, config.durationMs > 0,
        ["checkpoint", "run"].contains(config.mode),
        ["dust-bottom-left", "dust-top-right", "settle-bottom", "settle-top"].contains(
          config.scenario)
      else { throw Failure("invalid benchmark config") }
      guard config.fixturePath.hasPrefix("/"),
        FileManager.default.fileExists(atPath: config.fixturePath),
        let decoded = CIImage(
          contentsOf: URL(fileURLWithPath: config.fixturePath),
          options: [.applyOrientationProperty: true])
      else { throw Failure("failed to decode fixture PNG") }
      image = decoded
    }

    func start() throws {
      NSApplication.shared.setActivationPolicy(.accessory)
      let view = FlippedView(frame: NSRect(x: 0, y: 0, width: config.width, height: config.height))
      view.wantsLayer = true
      root.frame = view.bounds
      root.isGeometryFlipped = true
      root.backgroundColor = CGColor(
        srgbRed: 0x20 / 255, green: 0x24 / 255, blue: 0x2b / 255, alpha: 1)
      view.layer = root
      window = NSWindow(
        contentRect: view.bounds, styleMask: [.borderless], backing: .buffered, defer: false)
      window.contentView = view
      window.isOpaque = true
      window.backgroundColor = NSColor(
        srgbRed: 0x20 / 255, green: 0x24 / 255, blue: 0x2b / 255, alpha: 1)
      window.hasShadow = false
      window.level = .normal
      window.center()
      guard let screen = window.screen, screen.visibleFrame.contains(window.frame) else {
        throw Failure("640x720 fixture does not fit the display work area")
      }
      guard window.backingScaleFactor == config.scale else {
        throw Failure(
          "backing scale \(window.backingScaleFactor) does not match config scale \(config.scale)")
      }
      root.contentsScale = config.scale
      try rebuildTextures()
      setPose(config.mode == "checkpoint" ? config.checkpointMs : 0)
      window.makeKeyAndOrderFront(nil)
      NSApplication.shared.activate(ignoringOtherApps: true)
      CATransaction.flush()
      if config.mode == "checkpoint" {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.08) { [self] in
          tryOrTerminate { try writeReady() }
        }
      } else {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.08) { [self] in
          tryOrTerminate {
            try writeReady()
            waitForStart(deadline: CACurrentMediaTime() + 90)
          }
        }
      }
      // CVDisplayLink holds an unretained callback pointer. Keep its owner alive
      // for the complete application run loop, including checkpoint mode.
      withExtendedLifetime(self) { NSApplication.shared.run() }
    }

    private func cover(rounded: Bool, snapToPixels: Bool) throws -> CGImage {
      let scale = CGFloat(config.scale)
      let target = CGRect(x: 0, y: 0, width: 284 * scale, height: 160 * scale)
      let fitted = thumbnailCoverRect(
        source: image.extent.size, target: target.size, snapToPixels: snapToPixels)
      var cover =
        image
        .transformed(by: CGAffineTransform(translationX: -image.extent.minX, y: -image.extent.minY))
        .transformed(
          by: CGAffineTransform(
            scaleX: fitted.width / image.extent.width, y: fitted.height / image.extent.height)
        )
        // Geometry above uses the browser's top-left origin; Core Image uses bottom-left.
        .transformed(
          by: CGAffineTransform(translationX: fitted.minX, y: target.maxY - fitted.maxY)
        )
        .cropped(to: target)
      if rounded {
        let mask = CIFilter(
          name: "CIRoundedRectangleGenerator",
          parameters: [
            "inputExtent": CIVector(cgRect: target), "inputRadius": 12 * scale,
            "inputColor": CIColor.white,
          ])!.outputImage!.cropped(to: target)
        let clear = CIImage(color: .clear).cropped(to: target)
        cover = cover.applyingFilter(
          "CIBlendWithAlphaMask",
          parameters: [kCIInputBackgroundImageKey: clear, kCIInputMaskImageKey: mask])
      }
      guard
        let result = ci.createCGImage(
          cover, from: target, format: .RGBA8, colorSpace: CGColorSpace(name: CGColorSpace.sRGB))
      else { throw Failure("failed to render fixture") }
      return result
    }

    private func filtered(_ source: CIImage) -> CIImage {
      let scale = config.scale
      return source.applyingFilter("CIGaussianBlur", parameters: [kCIInputRadiusKey: 2 * scale])
        .applyingFilter(
          "CIColorMatrix",
          parameters: [
            "inputRVector": CIVector(x: 0.5, y: 0, z: 0, w: 0),
            "inputGVector": CIVector(x: 0, y: 0.5, z: 0, w: 0),
            "inputBVector": CIVector(x: 0, y: 0, z: 0.5, w: 0),
            "inputAVector": CIVector(x: 0, y: 0, z: 0, w: 1),
          ])
    }

    private func rebuildTextures() throws {
      CATransaction.begin()
      CATransaction.setDisableActions(true)
      defer { CATransaction.commit() }
      let media = try cover(rounded: true, snapToPixels: true)
      let scale = CGFloat(config.scale)
      root.sublayers?.forEach { $0.removeFromSuperlayer() }
      survivor.transform = CATransform3DIdentity
      exiting.transform = CATransform3DIdentity
      survivor.frame = CGRect(
        x: 178, y: config.scenario.contains("bottom") ? 136 : 504, width: 284, height: 160)
      survivor.contents = media
      survivor.contentsScale = scale
      survivor.masksToBounds = true
      survivor.cornerRadius = 12
      root.addSublayer(survivor)
      chips.removeAll(keepingCapacity: true)
      guard config.scenario.hasPrefix("dust") else { return }
      dust.sublayers = nil
      exitingShell.sublayers = nil
      dust.frame = CGRect(x: 58, y: 200, width: 524, height: 400)
      dust.mask = dustMask
      root.addSublayer(dust)
      let sharp = try cover(rounded: true, snapToPixels: false)
      let sharpImage = CIImage(cgImage: sharp)
      for particle in config.particles {
        let pad = 8.0
        let pixelRect = CGRect(
          x: (particle.sourceLeft - pad) * scale,
          y: (160 - particle.sourceTop - particle.height - pad) * scale,
          width: (particle.width + 2 * pad) * scale, height: (particle.height + 2 * pad) * scale)
        let clear = CIImage(color: .clear).cropped(to: pixelRect)
        // Slice first and filter second, matching CSS filter on each clipped
        // chip. The transparent 8pt margin retains the blur fringe.
        let sliceRect = pixelRect.insetBy(dx: pad * scale, dy: pad * scale)
        let crop = filtered(sharpImage.cropped(to: sliceRect).composited(over: clear)).cropped(
          to: pixelRect)
        guard
          let texture = ci.createCGImage(
            crop, from: pixelRect, format: .RGBA8, colorSpace: CGColorSpace(name: CGColorSpace.sRGB)
          )
        else { throw Failure("failed to render dust chip \(particle.id)") }
        let layer = CALayer()
        layer.anchorPoint = CGPoint(x: 0.5, y: 0.5)
        layer.bounds = CGRect(
          x: 0, y: 0, width: particle.width + 2 * pad, height: particle.height + 2 * pad)
        layer.position = CGPoint(
          x: particle.left + particle.width / 2, y: particle.top + particle.height / 2)
        layer.contents = texture
        layer.contentsScale = scale
        dust.addSublayer(layer)
        chips.append(layer)
      }
      exitingShell.frame = CGRect(x: 178, y: 320, width: 284, height: 160)
      exitingShell.masksToBounds = true
      exitingShell.cornerRadius = 12
      exiting.frame = exitingShell.bounds
      exiting.contentsScale = scale
      let extent = CGRect(x: 0, y: 0, width: sharp.width, height: sharp.height)
      let unrounded = CIImage(cgImage: try cover(rounded: false, snapToPixels: true))
      guard
        let sourceTexture = ci.createCGImage(
          filtered(unrounded).cropped(to: extent), from: extent, format: .RGBA8,
          colorSpace: CGColorSpace(name: CGColorSpace.sRGB))
      else { throw Failure("failed to render source fade") }
      exiting.contents = sourceTexture
      exitingShell.addSublayer(exiting)
      root.addSublayer(exitingShell)
    }

    private func setPose(_ elapsed: Double) {
      CATransaction.begin()
      CATransaction.setDisableActions(true)
      let settleStart = config.scenario.hasPrefix("dust") ? 1800.0 : 0
      let settle = cubicBezierProgress(
        0.4, 0, 0.2, 1, min(1, max(0, (elapsed - settleStart) / 580)))
      survivor.setAffineTransform(
        CGAffineTransform(
          translationX: 0, y: (config.scenario.contains("bottom") ? 184 : -184) * settle))
      if config.scenario.hasPrefix("dust") {
        let fadeLinear = min(1, max(0, elapsed / 550))
        let fadeT =
          fadeLinear <= 0.2 ? 0 : cubicBezierProgress(0.22, 0.1, 0.25, 1, (fadeLinear - 0.2) / 0.8)
        exiting.opacity = Float(1 - fadeT)
        exiting.setAffineTransform(CGAffineTransform(scaleX: 1.015, y: 1.015))
        // CSS animation timing functions apply independently between each
        // pair of keyframes, rather than easing one global progress value.
        let clipLinear = min(1, max(0, elapsed / 2550))
        let opening =
          clipLinear <= 0.08
          ? 0
          : clipLinear <= 0.70
            ? cubicBezierProgress(0.33, 0, 0.2, 1, (clipLinear - 0.08) / 0.62) : 1
        let inset = 120 * (1 - opening)
        let radius = 12 * (1 - opening)
        dustMask.path = CGPath(
          roundedRect: dust.bounds.insetBy(dx: inset, dy: inset), cornerWidth: radius,
          cornerHeight: radius, transform: nil)
        let disappearing =
          clipLinear <= 0.70
          ? 0
          : clipLinear <= 0.90
            ? cubicBezierProgress(0.33, 0, 0.2, 1, (clipLinear - 0.70) / 0.20) : 1
        dust.opacity = Float(1 - disappearing)
        for (index, particle) in config.particles.enumerated() where index < chips.count {
          let visual = thumbnailDustVisualAt(particle, elapsedMs: elapsed)
          let layer = chips[index]
          layer.opacity = Float(visual.opacity)
          var transform = CATransform3DMakeTranslation(visual.dx, visual.dy, 0)
          transform = CATransform3DRotate(transform, visual.rotate * .pi / 180, 0, 0, 1)
          layer.transform = CATransform3DScale(transform, visual.scale, visual.scale, 1)
        }
      }
      CATransaction.commit()
    }

    private func readiness() -> Ready {
      Ready(
        pid: getpid(), window_id: window.windowNumber, scale: config.scale,
        scenario: config.scenario, mode: config.mode, renderer: "appkit-core-animation",
        gpuDevice: gpuName, checkpointMs: config.checkpointMs)
    }
    private func writeReady() throws {
      try encode(readiness(), to: URL(fileURLWithPath: output.path + ".ready.json"))
    }

    private func waitForStart(deadline: Double) {
      if let path = config.startGatePath {
        guard path.hasPrefix("/") else { fail(Failure("startGatePath must be absolute")) }
        if !FileManager.default.fileExists(atPath: path) {
          guard CACurrentMediaTime() < deadline else { fail(Failure("start gate timed out")) }
          DispatchQueue.main.asyncAfter(deadline: .now() + 0.01) { [self] in
            waitForStart(deadline: deadline)
          }
          return
        }
      }
      tryOrTerminate { try beginDisplayLink() }
    }

    private func beginDisplayLink() throws {
      if config.startGatePath != nil {
        var timebase = mach_timebase_info_data_t()
        mach_timebase_info(&timebase)
        let product = mach_absolute_time().multipliedFullWidth(by: UInt64(timebase.numer))
        let ns = UInt64(timebase.denom).dividingFullWidth(product).quotient
        try encode(
          ["schema": UInt64(1), "pid": UInt64(getpid()), "startHostTimeNs": ns],
          to: URL(fileURLWithPath: output.path + ".started.json"))
      }
      started = CACurrentMediaTime()
      var link: CVDisplayLink?
      guard CVDisplayLinkCreateWithActiveCGDisplays(&link) == kCVReturnSuccess, let link else {
        throw Failure("failed to create CVDisplayLink")
      }
      displayLink = link
      CVDisplayLinkSetOutputCallback(
        link,
        { _, _, _, _, _, context in
          let owner = Unmanaged<NativeRenderer>.fromOpaque(context!).takeUnretainedValue()
          // Coalesce main-thread work. Scheduled display timestamps would hide
          // stalls and an unbounded dispatch queue would replay obsolete frames.
          owner.callbackLock.lock()
          if owner.callbackPending {
            owner.callbackLock.unlock()
            return kCVReturnSuccess
          }
          owner.callbackPending = true
          owner.callbackLock.unlock()
          DispatchQueue.main.async {
            owner.tick(CACurrentMediaTime())
            owner.callbackLock.lock()
            owner.callbackPending = false
            owner.callbackLock.unlock()
          }
          return kCVReturnSuccess
        }, Unmanaged.passUnretained(self).toOpaque())
      guard CVDisplayLinkStart(link) == kCVReturnSuccess else {
        throw Failure("failed to start CVDisplayLink")
      }
    }

    private func tick(_ timestamp: Double) {
      guard !completed else { return }
      if let lastTimestamp { callbackIntervals.append((timestamp - lastTimestamp) * 1000) }
      lastTimestamp = timestamp
      let elapsed = (CACurrentMediaTime() - started) * 1000
      if elapsed < config.durationMs {
        let nextCycle = Int(elapsed / config.cycleMs)
        if nextCycle != cycle {
          cycle = nextCycle
          let began = CACurrentMediaTime()
          do { try rebuildTextures() } catch { fail(error) }
          setupTimes.append((CACurrentMediaTime() - began) * 1000)
        }
        setPose(elapsed.truncatingRemainder(dividingBy: config.cycleMs))
      }
      if elapsed >= config.durationMs {
        completed = true
        if let displayLink { CVDisplayLinkStop(displayLink) }
        let ready = readiness()
        let result = Result(
          schema: ready.schema, pid: ready.pid, window_id: ready.window_id, scale: ready.scale,
          scenario: ready.scenario, mode: ready.mode, renderer: ready.renderer,
          gpuDevice: ready.gpuDevice, checkpointMs: ready.checkpointMs, elapsedMs: elapsed,
          callbackIntervalsMs: callbackIntervals, setupMs: setupTimes, cycles: setupTimes.count,
          complete: true,
          metric: "main-thread animation callback intervals; not GPU presented frames")
        do { try encode(result, to: output) } catch { fail(error) }
      }
    }
  }

  struct Failure: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
  }
  func encode<T: Encodable>(_ value: T, to url: URL) throws {
    try JSONEncoder().encode(value).write(to: url, options: .atomic)
  }
  func fail(_ error: Error) -> Never {
    FileHandle.standardError.write(Data("native benchmark: \(error)\n".utf8))
    exit(1)
  }
  func tryOrTerminate(_ body: () throws -> Void) { do { try body() } catch { fail(error) } }
#endif
