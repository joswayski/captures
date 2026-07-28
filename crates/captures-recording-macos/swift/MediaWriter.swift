import AppKit
import AVFoundation
import CoreGraphics
import CoreMedia
import CoreVideo
import Foundation

private struct CapturesClickRipple {
    let point: CGPoint
    let startedAt: TimeInterval
}

private final class CapturesMediaWriter {
    private let writer: AVAssetWriter
    private let videoInput: AVAssetWriterInput
    private let videoAdaptor: AVAssetWriterInputPixelBufferAdaptor
    private let audioInput: AVAssetWriterInput?
    private let outputWidth: Int
    private let outputHeight: Int
    private let sourceFrame: CGRect
    private let highlightClicks: Bool
    private let videoFrameIntervalSeconds: TimeInterval
    private let lock = NSLock()
    private let clickLock = NSLock()
    private var started = false
    private var finished = false
    private var firstTimestamp = CMTime.invalid
    private var lastTimestamp = CMTime.invalid
    private var firstFrameUptime: TimeInterval?
    private var latestVideoBuffer: CVPixelBuffer?
    private var latestVideoTimestamp = CMTime.invalid
    private var latestVideoGeneration: UInt64 = 0
    private var appendedVideoGeneration: UInt64 = 0
    private var videoFramesWritten: UInt64 = 0
    private var videoDrainTimer: DispatchSourceTimer?
    private let videoDrainInterval: DispatchTimeInterval
    private var clickMonitor: Any?
    private var clickRipples: [CapturesClickRipple] = []
    private let activity: NSObjectProtocol
    private(set) var droppedFrames: UInt64 = 0
    private(set) var failure: String?

    private static let clickRippleDuration: TimeInterval = 0.62

    init?(
        path: String,
        width: Int,
        height: Int,
        framesPerSecond: Int,
        capturesAudio: Bool,
        mono: Bool,
        highlightClicks: Bool,
        sourceX: Double,
        sourceY: Double,
        sourceWidth: Double,
        sourceHeight: Double
    ) {
        outputWidth = width
        outputHeight = height
        sourceFrame = CGRect(
            x: sourceX,
            y: sourceY,
            width: sourceWidth,
            height: sourceHeight
        )
        self.highlightClicks = highlightClicks
        videoFrameIntervalSeconds = 1.0 / Double(max(1, framesPerSecond))
        do {
            writer = try AVAssetWriter(outputURL: URL(fileURLWithPath: path), fileType: .mp4)
        } catch {
            return nil
        }
        // Fragmented MP4 writes playable indexing data throughout the segment.
        // If Captures is interrupted, recovery can salvage the latest fragment
        // instead of requiring AVAssetWriter to close the whole file cleanly.
        writer.movieFragmentInterval = CMTime(seconds: 1, preferredTimescale: 600)

        let pixelsPerSecond = Double(max(2, width)) * Double(max(2, height)) * Double(max(1, framesPerSecond))
        // Screen recordings contain sharp text and UI edges that degrade much
        // sooner than camera footage. Keep the H.264 master comfortably above
        // a delivery encode so Preserve quality has a strong source to copy.
        let averageBitRate = Int(min(60_000_000, max(4_000_000, pixelsPerSecond * 0.20)))
        let videoSettings: [String: Any] = [
            AVVideoCodecKey: AVVideoCodecType.h264,
            AVVideoWidthKey: width,
            AVVideoHeightKey: height,
            AVVideoCompressionPropertiesKey: [
                AVVideoAverageBitRateKey: averageBitRate,
                AVVideoExpectedSourceFrameRateKey: framesPerSecond,
                AVVideoMaxKeyFrameIntervalKey: max(1, framesPerSecond * 2),
                AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
                AVVideoAllowFrameReorderingKey: false,
            ],
        ]
        videoInput = AVAssetWriterInput(mediaType: .video, outputSettings: videoSettings)
        videoInput.expectsMediaDataInRealTime = true
        videoDrainInterval = .milliseconds(max(8, 1_000 / max(1, framesPerSecond)))
        let pixelBufferAttributes: [String: Any] = [
            kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
            kCVPixelBufferWidthKey as String: width,
            kCVPixelBufferHeightKey as String: height,
            kCVPixelBufferIOSurfacePropertiesKey as String: [:] as [String: Any],
        ]
        videoAdaptor = AVAssetWriterInputPixelBufferAdaptor(
            assetWriterInput: videoInput,
            sourcePixelBufferAttributes: pixelBufferAttributes
        )
        guard writer.canAdd(videoInput) else { return nil }
        writer.add(videoInput)

        if capturesAudio {
            let audioSettings: [String: Any] = [
                AVFormatIDKey: kAudioFormatMPEG4AAC,
                AVSampleRateKey: 48_000,
                AVNumberOfChannelsKey: mono ? 1 : 2,
                AVEncoderBitRateKey: mono ? 96_000 : 128_000,
            ]
            let input = AVAssetWriterInput(mediaType: .audio, outputSettings: audioSettings)
            input.expectsMediaDataInRealTime = true
            guard writer.canAdd(input) else { return nil }
            writer.add(input)
            audioInput = input
        } else {
            audioInput = nil
        }
        activity = ProcessInfo.processInfo.beginActivity(
            options: [.userInitiated, .idleSystemSleepDisabled],
            reason: "Captures is recording the screen"
        )
        installClickMonitorIfNeeded()
    }

    deinit {
        videoDrainTimer?.cancel()
        if let clickMonitor {
            if Thread.isMainThread {
                NSEvent.removeMonitor(clickMonitor)
            } else {
                DispatchQueue.main.sync {
                    NSEvent.removeMonitor(clickMonitor)
                }
            }
        }
        ProcessInfo.processInfo.endActivity(activity)
    }

    private func installClickMonitorIfNeeded() {
        guard highlightClicks else { return }
        let install = { [weak self] in
            guard let self else { return }
            self.clickMonitor = NSEvent.addGlobalMonitorForEvents(
                matching: [.leftMouseDown, .rightMouseDown, .otherMouseDown]
            ) { [weak self] event in
                guard
                    let self,
                    let point = event.cgEvent?.location,
                    self.sourceFrame.contains(point)
                else { return }
                self.clickLock.lock()
                self.clickRipples.removeAll {
                    event.timestamp - $0.startedAt >= Self.clickRippleDuration
                }
                self.clickRipples.append(CapturesClickRipple(
                    point: point,
                    startedAt: event.timestamp
                ))
                self.clickLock.unlock()
            }
        }
        if Thread.isMainThread {
            install()
        } else {
            DispatchQueue.main.sync(execute: install)
        }
    }

    private func activeClickRipples(at uptime: TimeInterval) -> [CapturesClickRipple] {
        guard highlightClicks else { return [] }
        clickLock.lock()
        clickRipples.removeAll {
            uptime - $0.startedAt >= Self.clickRippleDuration
                || uptime + 0.05 < $0.startedAt
        }
        let active = clickRipples
        clickLock.unlock()
        return active
    }

    private func startVideoDrainIfNeeded() {
        guard videoDrainTimer == nil else { return }
        // AVAssetWriter can briefly apply backpressure while ScreenCaptureKit
        // continues delivering frames. Drain the newest pending real frame as
        // soon as the encoder is ready. Only repeat a frame when there has
        // genuinely been no newer content for 250 ms.
        let timer = DispatchSource.makeTimerSource(
            queue: DispatchQueue.global(qos: .userInitiated)
        )
        timer.schedule(
            deadline: .now() + videoDrainInterval,
            repeating: videoDrainInterval,
            leeway: .milliseconds(2)
        )
        timer.setEventHandler { [weak self] in
            self?.drainVideoFrame()
        }
        videoDrainTimer = timer
        timer.resume()
    }

    private func currentPresentationTimestamp() -> CMTime? {
        guard let firstFrameUptime, firstTimestamp.isValid else { return nil }
        return CMTimeAdd(
            firstTimestamp,
            CMTime(
                seconds: max(0, ProcessInfo.processInfo.systemUptime - firstFrameUptime),
                preferredTimescale: 600_000
            )
        )
    }

    private func drainVideoFrame() {
        lock.lock()
        defer { lock.unlock() }
        guard
            !finished,
            failure == nil,
            let timestamp = currentPresentationTimestamp()
        else { return }

        let appended: Bool
        if latestVideoGeneration > appendedVideoGeneration {
            appended = appendLatestVideoFrame(
                preferredTimestamp: latestVideoTimestamp,
                generation: latestVideoGeneration,
                waitUntilReady: false
            )
        } else if shouldAppendHeartbeat(at: timestamp) {
            appended = appendLatestVideoFrame(
                preferredTimestamp: timestamp,
                generation: nil,
                waitUntilReady: false
            )
        } else {
            return
        }
        if !appended {
            failure = writer.error?.localizedDescription
                ?? "AVAssetWriter could not append a recording frame"
        }
    }

    private func shouldAppendHeartbeat(at timestamp: CMTime) -> Bool {
        guard lastTimestamp.isValid else { return false }
        let elapsed = CMTimeGetSeconds(CMTimeSubtract(timestamp, lastTimestamp))
        guard elapsed.isFinite else { return false }
        let hasActiveRipple = !activeClickRipples(
            at: ProcessInfo.processInfo.systemUptime
        ).isEmpty
        return elapsed >= (hasActiveRipple ? videoFrameIntervalSeconds : 0.25)
    }

    private func copyVideoBuffer(
        _ source: CVPixelBuffer,
        ripples: [CapturesClickRipple] = [],
        uptime: TimeInterval = 0
    ) -> CVPixelBuffer? {
        guard
            CVPixelBufferGetPixelFormatType(source) == kCVPixelFormatType_32BGRA,
            CVPixelBufferGetWidth(source) == outputWidth,
            CVPixelBufferGetHeight(source) == outputHeight
        else {
            failure = "ScreenCaptureKit delivered an unexpected video frame format"
            return nil
        }

        var destination: CVPixelBuffer?
        if let pool = videoAdaptor.pixelBufferPool {
            guard
                CVPixelBufferPoolCreatePixelBuffer(nil, pool, &destination) == kCVReturnSuccess
            else {
                failure = "Apple's H.264 writer could not allocate a video frame"
                return nil
            }
        } else {
            let attributes: [String: Any] = [
                kCVPixelBufferIOSurfacePropertiesKey as String: [:] as [String: Any],
            ]
            guard
                CVPixelBufferCreate(
                    nil,
                    outputWidth,
                    outputHeight,
                    kCVPixelFormatType_32BGRA,
                    attributes as CFDictionary,
                    &destination
                ) == kCVReturnSuccess
            else {
                failure = "Apple's H.264 writer could not allocate a video frame"
                return nil
            }
        }
        guard let destination else { return nil }

        guard CVPixelBufferLockBaseAddress(source, .readOnly) == kCVReturnSuccess else {
            failure = "ScreenCaptureKit's video frame could not be read"
            return nil
        }
        defer { CVPixelBufferUnlockBaseAddress(source, .readOnly) }
        guard CVPixelBufferLockBaseAddress(destination, []) == kCVReturnSuccess else {
            failure = "Apple's H.264 video frame could not be written"
            return nil
        }
        defer { CVPixelBufferUnlockBaseAddress(destination, []) }
        guard
            let sourceBase = CVPixelBufferGetBaseAddress(source),
            let destinationBase = CVPixelBufferGetBaseAddress(destination)
        else {
            failure = "A recording video frame did not expose pixel data"
            return nil
        }

        let sourceBytesPerRow = CVPixelBufferGetBytesPerRow(source)
        let destinationBytesPerRow = CVPixelBufferGetBytesPerRow(destination)
        let bytesPerRow = min(outputWidth * 4, sourceBytesPerRow, destinationBytesPerRow)
        for row in 0..<outputHeight {
            memcpy(
                destinationBase.advanced(by: row * destinationBytesPerRow),
                sourceBase.advanced(by: row * sourceBytesPerRow),
                bytesPerRow
            )
        }
        if !ripples.isEmpty {
            drawClickRipples(
                ripples,
                into: destinationBase,
                bytesPerRow: destinationBytesPerRow,
                uptime: uptime
            )
        }
        return destination
    }

    private func drawClickRipples(
        _ ripples: [CapturesClickRipple],
        into baseAddress: UnsafeMutableRawPointer,
        bytesPerRow: Int,
        uptime: TimeInterval
    ) {
        guard sourceFrame.width > 0, sourceFrame.height > 0 else { return }
        let scaleX = Double(outputWidth) / sourceFrame.width
        let scaleY = Double(outputHeight) / sourceFrame.height
        let visualScale = max(0.5, sqrt(scaleX * scaleY))

        for ripple in ripples {
            let elapsed = uptime - ripple.startedAt
            guard elapsed >= 0, elapsed < Self.clickRippleDuration else { continue }
            let progress = min(1, max(0, elapsed / Self.clickRippleDuration))
            let eased = 1 - pow(1 - progress, 3)
            let centerX = (ripple.point.x - sourceFrame.minX) * scaleX
            let centerY = (ripple.point.y - sourceFrame.minY) * scaleY
            guard
                centerX >= 0,
                centerY >= 0,
                centerX < Double(outputWidth),
                centerY < Double(outputHeight)
            else { continue }

            let radius = (8 + 31 * eased) * visualScale
            let thickness = (3.8 - 1.3 * progress) * visualScale
            let haloWidth = 5.5 * visualScale
            let extent = radius + thickness + haloWidth
            let minimumX = max(0, Int(floor(centerX - extent)))
            let maximumX = min(outputWidth - 1, Int(ceil(centerX + extent)))
            let minimumY = max(0, Int(floor(centerY - extent)))
            let maximumY = min(outputHeight - 1, Int(ceil(centerY + extent)))
            let fade = pow(1 - progress, 0.62)
            let centerFade = max(0, 1 - progress / 0.24)
            let antialiasWidth = max(1, 0.8 * visualScale)

            for y in minimumY...maximumY {
                let deltaY = (Double(y) + 0.5) - centerY
                let row = baseAddress
                    .advanced(by: y * bytesPerRow)
                    .assumingMemoryBound(to: UInt8.self)
                for x in minimumX...maximumX {
                    let deltaX = (Double(x) + 0.5) - centerX
                    let distance = hypot(deltaX, deltaY)
                    let distanceFromRing = abs(distance - radius)
                    let ringCoverage = min(
                        1,
                        max(0, (thickness / 2 + antialiasWidth - distanceFromRing)
                            / antialiasWidth)
                    )
                    let haloCoverage = max(
                        0,
                        1 - distanceFromRing / (thickness / 2 + haloWidth)
                    )
                    let centerRadius = 9 * visualScale
                    let centerCoverage = centerFade * max(
                        0,
                        1 - distance / max(1, centerRadius)
                    )
                    let alpha = min(
                        0.96,
                        ringCoverage * 0.92 * fade
                            + haloCoverage * 0.24 * fade
                            + centerCoverage * 0.28
                    )
                    guard alpha > 0.002 else { continue }

                    let pixel = row.advanced(by: x * 4)
                    let inverse = 1 - alpha
                    // kCVPixelFormatType_32BGRA: lavender #A997FF.
                    pixel[0] = UInt8(min(255, Double(pixel[0]) * inverse + 255 * alpha))
                    pixel[1] = UInt8(min(255, Double(pixel[1]) * inverse + 151 * alpha))
                    pixel[2] = UInt8(min(255, Double(pixel[2]) * inverse + 169 * alpha))
                }
            }
        }
    }

    func append(_ sample: CMSampleBuffer, kind: Int32) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard !finished, failure == nil else { return false }
        let timestamp = CMSampleBufferGetPresentationTimeStamp(sample)
        if !started {
            guard kind == 0 else { return true }
            guard writer.startWriting() else {
                failure = writer.error?.localizedDescription ?? "AVAssetWriter could not start"
                return false
            }
            writer.startSession(atSourceTime: timestamp)
            firstTimestamp = timestamp
            firstFrameUptime = ProcessInfo.processInfo.systemUptime
            started = true
        }

        if kind == 0 || kind == 2 {
            guard
                let imageBuffer = CMSampleBufferGetImageBuffer(sample)
            else {
                failure = "ScreenCaptureKit delivered a video frame without pixels"
                return false
            }

            let isIdleFrame = kind == 2
            if isIdleFrame && latestVideoGeneration > appendedVideoGeneration {
                // Never let a cursor-only/idle surface replace pending screen
                // content while the high-resolution encoder is catching up.
                return true
            }
            if !isIdleFrame {
                if latestVideoGeneration > appendedVideoGeneration {
                    droppedFrames += 1
                }
                latestVideoGeneration &+= 1
            }
            // ScreenCaptureKit owns a small pool of IOSurfaces. Passing those
            // surfaces directly to AVAssetWriter lets the encoder retain the
            // entire capture pool, after which ScreenCaptureKit can no longer
            // deliver changing frames. Copy into the writer's pool immediately
            // so the capture surface is released when this callback returns.
            guard let copiedBuffer = copyVideoBuffer(imageBuffer) else {
                return false
            }
            latestVideoBuffer = copiedBuffer
            latestVideoTimestamp = timestamp
            startVideoDrainIfNeeded()

            guard appendLatestVideoFrame(
                preferredTimestamp: timestamp,
                generation: isIdleFrame ? nil : latestVideoGeneration,
                waitUntilReady: videoFramesWritten == 0
            ) else {
                failure = writer.error?.localizedDescription
                    ?? "AVAssetWriter rejected a video frame"
                return false
            }
            return true
        }

        guard let audioInput else { return true }
        guard audioInput.isReadyForMoreMediaData else { return true }
        if !audioInput.append(sample) {
            failure = writer.error?.localizedDescription ?? "AVAssetWriter rejected an audio sample"
            return false
        }
        return true
    }

    private func appendLatestVideoFrame(
        preferredTimestamp: CMTime,
        generation: UInt64?,
        waitUntilReady: Bool
    ) -> Bool {
        guard
            let imageBuffer = latestVideoBuffer,
            preferredTimestamp.isValid
        else { return true }
        if waitUntilReady {
            for _ in 0..<100 where !videoInput.isReadyForMoreMediaData {
                Thread.sleep(forTimeInterval: 0.002)
            }
        }
        guard videoInput.isReadyForMoreMediaData else {
            return !waitUntilReady
        }

        let presentationTimestamp: CMTime
        if lastTimestamp.isValid && CMTimeCompare(preferredTimestamp, lastTimestamp) <= 0 {
            let minimumTimestamp = CMTimeAdd(
                lastTimestamp,
                CMTime(value: 1, timescale: 600_000)
            )
            if let currentTimestamp = currentPresentationTimestamp(),
               CMTimeCompare(currentTimestamp, minimumTimestamp) > 0 {
                presentationTimestamp = currentTimestamp
            } else {
                presentationTimestamp = minimumTimestamp
            }
        } else {
            presentationTimestamp = preferredTimestamp
        }

        let uptime = ProcessInfo.processInfo.systemUptime
        let ripples = activeClickRipples(at: uptime)
        let outputBuffer: CVPixelBuffer
        if ripples.isEmpty {
            outputBuffer = imageBuffer
        } else {
            guard let rendered = copyVideoBuffer(
                imageBuffer,
                ripples: ripples,
                uptime: uptime
            ) else { return false }
            outputBuffer = rendered
        }

        guard videoAdaptor.append(outputBuffer, withPresentationTime: presentationTimestamp) else {
            return false
        }

        lastTimestamp = presentationTimestamp
        if let generation {
            appendedVideoGeneration = max(appendedVideoGeneration, generation)
        }
        videoFramesWritten += 1
        return true
    }

    func finish() -> Bool {
        lock.lock()
        guard !finished else {
            let successful = failure == nil
            lock.unlock()
            return successful
        }
        finished = true
        videoDrainTimer?.cancel()
        videoDrainTimer = nil
        guard started, failure == nil else {
            if failure == nil { failure = "the recording did not contain a complete video frame" }
            writer.cancelWriting()
            lock.unlock()
            return false
        }
        if latestVideoGeneration > appendedVideoGeneration {
            guard appendLatestVideoFrame(
                preferredTimestamp: latestVideoTimestamp,
                generation: latestVideoGeneration,
                waitUntilReady: true
            ) else {
                failure = writer.error?.localizedDescription
                    ?? "AVAssetWriter could not append the recording's latest frame"
                writer.cancelWriting()
                lock.unlock()
                return false
            }
        }
        if let endTimestamp = currentPresentationTimestamp() {
            guard appendLatestVideoFrame(
                preferredTimestamp: endTimestamp,
                generation: nil,
                waitUntilReady: true
            ) else {
                failure = writer.error?.localizedDescription
                    ?? "AVAssetWriter could not append the recording's final frame"
                writer.cancelWriting()
                lock.unlock()
                return false
            }
        }
        videoInput.markAsFinished()
        audioInput?.markAsFinished()
        lock.unlock()

        let completion = DispatchSemaphore(value: 0)
        writer.finishWriting { completion.signal() }
        completion.wait()

        lock.lock()
        defer { lock.unlock() }
        if writer.status != .completed {
            failure = writer.error?.localizedDescription ?? "AVAssetWriter did not finish the recording"
            return false
        }
        return true
    }

    var durationMilliseconds: UInt64 {
        lock.lock()
        defer { lock.unlock() }
        guard firstTimestamp.isValid, lastTimestamp.isValid else { return 0 }
        let duration = CMTimeSubtract(lastTimestamp, firstTimestamp)
        let seconds = CMTimeGetSeconds(duration)
        guard seconds.isFinite, seconds > 0 else { return 0 }
        return UInt64((seconds * 1_000).rounded())
    }

    var hasVideoFrame: Bool {
        lock.lock()
        defer { lock.unlock() }
        return videoFramesWritten > 0
    }

    func copyFailure(into buffer: UnsafeMutablePointer<CChar>?, capacity: Int) -> Int {
        lock.lock()
        let bytes = Array((failure ?? writer.error?.localizedDescription ?? "").utf8CString)
        lock.unlock()
        guard let buffer, capacity > 0 else { return max(0, bytes.count - 1) }
        let count = min(capacity - 1, max(0, bytes.count - 1))
        for index in 0..<count { buffer[index] = bytes[index] }
        buffer[count] = 0
        return count
    }
}

@_cdecl("captures_media_writer_create")
public func capturesMediaWriterCreate(
    _ path: UnsafePointer<CChar>,
    _ width: UInt32,
    _ height: UInt32,
    _ framesPerSecond: UInt32,
    _ capturesAudio: Bool,
    _ mono: Bool,
    _ highlightClicks: Bool,
    _ sourceX: Double,
    _ sourceY: Double,
    _ sourceWidth: Double,
    _ sourceHeight: Double
) -> UnsafeMutableRawPointer? {
    guard let writer = CapturesMediaWriter(
        path: String(cString: path),
        width: Int(width),
        height: Int(height),
        framesPerSecond: Int(framesPerSecond),
        capturesAudio: capturesAudio,
        mono: mono,
        highlightClicks: highlightClicks,
        sourceX: sourceX,
        sourceY: sourceY,
        sourceWidth: sourceWidth,
        sourceHeight: sourceHeight
    ) else { return nil }
    return Unmanaged.passRetained(writer).toOpaque()
}

@_cdecl("captures_media_writer_append")
public func capturesMediaWriterAppend(
    _ handle: UnsafeMutableRawPointer,
    _ sample: UnsafeMutableRawPointer,
    _ kind: Int32
) -> Bool {
    let writer = Unmanaged<CapturesMediaWriter>.fromOpaque(handle).takeUnretainedValue()
    let sampleBuffer = Unmanaged<CMSampleBuffer>.fromOpaque(sample).takeUnretainedValue()
    return writer.append(sampleBuffer, kind: kind)
}

@_cdecl("captures_media_writer_finish")
public func capturesMediaWriterFinish(_ handle: UnsafeMutableRawPointer) -> Bool {
    Unmanaged<CapturesMediaWriter>.fromOpaque(handle).takeUnretainedValue().finish()
}

@_cdecl("captures_media_writer_has_video_frame")
public func capturesMediaWriterHasVideoFrame(_ handle: UnsafeMutableRawPointer) -> Bool {
    Unmanaged<CapturesMediaWriter>.fromOpaque(handle).takeUnretainedValue().hasVideoFrame
}

@_cdecl("captures_media_writer_duration_ms")
public func capturesMediaWriterDuration(_ handle: UnsafeMutableRawPointer) -> UInt64 {
    Unmanaged<CapturesMediaWriter>.fromOpaque(handle).takeUnretainedValue().durationMilliseconds
}

@_cdecl("captures_media_writer_dropped_frames")
public func capturesMediaWriterDroppedFrames(_ handle: UnsafeMutableRawPointer) -> UInt64 {
    Unmanaged<CapturesMediaWriter>.fromOpaque(handle).takeUnretainedValue().droppedFrames
}

@_cdecl("captures_media_writer_error")
public func capturesMediaWriterError(
    _ handle: UnsafeMutableRawPointer,
    _ buffer: UnsafeMutablePointer<CChar>?,
    _ capacity: Int
) -> Int {
    Unmanaged<CapturesMediaWriter>.fromOpaque(handle).takeUnretainedValue()
        .copyFailure(into: buffer, capacity: capacity)
}

@_cdecl("captures_media_writer_release")
public func capturesMediaWriterRelease(_ handle: UnsafeMutableRawPointer) {
    Unmanaged<CapturesMediaWriter>.fromOpaque(handle).release()
}
