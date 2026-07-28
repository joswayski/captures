import AVFoundation
import CoreMedia
import Foundation

private final class CapturesMediaWriter {
    private let writer: AVAssetWriter
    private let videoInput: AVAssetWriterInput
    private let videoAdaptor: AVAssetWriterInputPixelBufferAdaptor
    private let audioInput: AVAssetWriterInput?
    private let lock = NSLock()
    private var started = false
    private var finished = false
    private var firstTimestamp = CMTime.invalid
    private var lastTimestamp = CMTime.invalid
    private var firstFrameUptime: TimeInterval?
    private var lastVideoSample: CMSampleBuffer?
    private var videoFramesWritten: UInt64 = 0
    private var heartbeatTimer: DispatchSourceTimer?
    private let activity: NSObjectProtocol
    private(set) var droppedFrames: UInt64 = 0
    private(set) var failure: String?

    init?(path: String, width: Int, height: Int, framesPerSecond: Int, capturesAudio: Bool, mono: Bool) {
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
        videoAdaptor = AVAssetWriterInputPixelBufferAdaptor(
            assetWriterInput: videoInput,
            sourcePixelBufferAttributes: nil
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
    }

    deinit {
        heartbeatTimer?.cancel()
        ProcessInfo.processInfo.endActivity(activity)
    }

    private func startHeartbeatIfNeeded() {
        guard heartbeatTimer == nil else { return }
        // ScreenCaptureKit may stop emitting frames when nothing on screen
        // changes. Periodically repeat the latest pixel buffer so the encoded
        // timeline and fragmented recovery data continue advancing.
        let timer = DispatchSource.makeTimerSource(
            queue: DispatchQueue.global(qos: .userInitiated)
        )
        timer.schedule(
            deadline: .now() + .milliseconds(250),
            repeating: .milliseconds(250),
            leeway: .milliseconds(20)
        )
        timer.setEventHandler { [weak self] in
            self?.appendHeartbeat()
        }
        heartbeatTimer = timer
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

    private func appendHeartbeat() {
        lock.lock()
        defer { lock.unlock() }
        guard
            !finished,
            failure == nil,
            let timestamp = currentPresentationTimestamp()
        else { return }
        if !appendHeartbeatVideoFrame(at: timestamp, waitUntilReady: false) {
            failure = writer.error?.localizedDescription
                ?? "AVAssetWriter could not maintain the recording timeline"
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
        if
            kind == 0,
            lastTimestamp.isValid,
            CMTimeCompare(timestamp, lastTimestamp) <= 0
        {
            return true
        }
        let input = kind == 0 ? videoInput : audioInput
        guard let input else { return true }
        guard input.isReadyForMoreMediaData else {
            if kind == 0 { droppedFrames += 1 }
            return true
        }
        if kind == 0 {
            guard
                let imageBuffer = CMSampleBufferGetImageBuffer(sample),
                videoAdaptor.append(imageBuffer, withPresentationTime: timestamp)
            else {
                failure = writer.error?.localizedDescription
                    ?? "AVAssetWriter rejected a video frame"
                return false
            }
        } else if !input.append(sample) {
            failure = writer.error?.localizedDescription ?? "AVAssetWriter rejected an audio sample"
            return false
        }
        if kind == 0 {
            lastTimestamp = timestamp
            lastVideoSample = sample
            videoFramesWritten += 1
            startHeartbeatIfNeeded()
        }
        return true
    }

    private func appendHeartbeatVideoFrame(
        at presentationTimestamp: CMTime,
        waitUntilReady: Bool
    ) -> Bool {
        guard
            let lastVideoSample,
            lastTimestamp.isValid,
            CMTimeCompare(presentationTimestamp, lastTimestamp) > 0,
            let imageBuffer = CMSampleBufferGetImageBuffer(lastVideoSample)
        else { return true }
        if waitUntilReady {
            for _ in 0..<100 where !videoInput.isReadyForMoreMediaData {
                Thread.sleep(forTimeInterval: 0.002)
            }
        }
        guard videoInput.isReadyForMoreMediaData else {
            return !waitUntilReady
        }

        guard videoAdaptor.append(imageBuffer, withPresentationTime: presentationTimestamp) else {
            return false
        }

        lastTimestamp = presentationTimestamp
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
        heartbeatTimer?.cancel()
        heartbeatTimer = nil
        guard started, failure == nil else {
            if failure == nil { failure = "the recording did not contain a complete video frame" }
            writer.cancelWriting()
            lock.unlock()
            return false
        }
        if let endTimestamp = currentPresentationTimestamp() {
            guard appendHeartbeatVideoFrame(at: endTimestamp, waitUntilReady: true) else {
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
    _ mono: Bool
) -> UnsafeMutableRawPointer? {
    guard let writer = CapturesMediaWriter(
        path: String(cString: path),
        width: Int(width),
        height: Int(height),
        framesPerSecond: Int(framesPerSecond),
        capturesAudio: capturesAudio,
        mono: mono
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
    return writer.append(unsafeBitCast(sample, to: CMSampleBuffer.self), kind: kind)
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
