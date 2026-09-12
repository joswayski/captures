import AppKit
import AVFoundation
import AVKit
import SwiftUI

private struct RecordingProbe: Equatable {
    var width: Int
    var height: Int
    var durationMS: Double
    var hasSystemAudio: Bool
    var hasMicrophoneAudio: Bool
}

private struct RecordingCrop: Equatable {
    var x: Double
    var y: Double
    var width: Double
    var height: Double
}

private enum RecordingCropHandle: CaseIterable, Equatable {
    case northWest, north, northEast, east, southEast, south, southWest, west
}

private enum RecordingTimelineTarget: Equatable {
    case start, end, playhead
}

private struct RecordingPlayerSurface: NSViewRepresentable {
    let player: AVPlayer

    func makeNSView(context: Context) -> AVPlayerView {
        let view = AVPlayerView()
        view.player = player
        view.controlsStyle = .none
        view.videoGravity = .resizeAspect
        return view
    }

    func updateNSView(_ view: AVPlayerView, context: Context) {
        view.player = player
    }
}

@MainActor
private final class RecordingEditorModel: ObservableObject {
    @Published var probe: RecordingProbe?
    @Published var trimStartMS: Double = 0
    @Published var trimEndMS: Double = 1
    @Published var playheadMS: Double = 0
    @Published var cropEnabled = false
    @Published var crop = RecordingCrop(x: 0, y: 0, width: 1, height: 1)
    @Published var exporting = false
    @Published var status = "Reading recording…"
    @Published var comparisonBefore: NSImage?
    @Published var comparisonAfter: NSImage?
    @Published var comparisonPending = false
    @Published var comparisonError = ""
    @Published var timelineFrames: [NSImage] = []
    var loopEnabled = false

    let artifact: Artifact
    let player: AVPlayer
    private var periodicObserver: Any?
    private var comparisonGeneration = 0

    init(artifact: Artifact) {
        self.artifact = artifact
        player = AVPlayer(url: artifact.url)
        periodicObserver = player.addPeriodicTimeObserver(forInterval: CMTime(value: 1, timescale: 20), queue: .main) { [weak self] time in
            // AVPlayer guarantees this callback runs on the requested main queue.
            MainActor.assumeIsolated {
                guard let self else { return }
                let milliseconds = max(0, time.seconds * 1_000)
                self.playheadMS = milliseconds
                if milliseconds >= self.trimEndMS {
                    if self.loopEnabled {
                        self.seek(to: self.trimStartMS)
                        self.player.play()
                    } else {
                        self.player.pause()
                    }
                }
            }
        }
        probeMedia()
    }

    deinit {
        if let periodicObserver { player.removeTimeObserver(periodicObserver) }
    }

    func probeMedia() {
        Backend.shared.call("media_probe", ["path": artifact.path]) { [weak self] result in
            guard let self else { return }
            switch result {
            case let .success(response):
                guard let width = Self.number(response["width"]),
                      let height = Self.number(response["height"]),
                      let duration = Self.number(response["duration_ms"]),
                      width >= 1, height >= 1, duration > 0 else {
                    let error = NSError(domain: "CapturesNative.RecordingEditor", code: 1, userInfo: [NSLocalizedDescriptionKey: "media_probe returned incomplete recording metadata."])
                    self.status = error.localizedDescription
                    AppStore.shared.report(error)
                    return
                }
                self.probe = RecordingProbe(
                    width: Int(width), height: Int(height), durationMS: duration,
                    hasSystemAudio: response["has_system_audio"] as? Bool ?? false,
                    hasMicrophoneAudio: response["has_microphone_audio"] as? Bool ?? false
                )
                self.trimEndMS = duration
                self.crop = RecordingCrop(x: 0, y: 0, width: width, height: height)
                self.status = "Ready"
                self.loadTimelineFrames(durationMS: duration)
            case let .failure(error):
                self.status = error.localizedDescription
                AppStore.shared.report(error)
            }
        }
    }

    func seek(to milliseconds: Double) {
        let target = min(max(milliseconds, trimStartMS), trimEndMS)
        player.seek(to: CMTime(seconds: target / 1_000, preferredTimescale: 600), toleranceBefore: .zero, toleranceAfter: .zero)
        playheadMS = target
    }

    func togglePlayback(loop: Bool) {
        loopEnabled = loop
        if player.rate == 0 {
            if playheadMS < trimStartMS || playheadMS >= trimEndMS { seek(to: trimStartMS) }
            player.play()
        } else {
            player.pause()
        }
        if loop, playheadMS >= trimEndMS { seek(to: trimStartMS); player.play() }
    }

    func updateTrimStart(_ value: Double) {
        trimStartMS = min(max(0, value), trimEndMS - 1)
        seek(to: trimStartMS)
    }

    func updateTrimEnd(_ value: Double) {
        guard let probe else { return }
        trimEndMS = min(max(trimStartMS + 1, value), probe.durationMS)
        seek(to: trimEndMS)
    }

    func constrainCrop() {
        guard let probe else { return }
        crop.width = min(max(2, crop.width.rounded()), Double(probe.width))
        crop.height = min(max(2, crop.height.rounded()), Double(probe.height))
        crop.x = min(max(0, crop.x.rounded()), Double(probe.width) - crop.width)
        crop.y = min(max(0, crop.y.rounded()), Double(probe.height) - crop.height)
    }

    func export(
        to url: URL, format: String, width: Int?, quality: String,
        maxBytes: Int?, systemVolume: Double, microphoneVolume: Double, mono: Bool, gifFPS: Int,
        replacing source: URL? = nil
    ) {
        constrainCrop()
        let fields = exportFields(
            output: url, format: format, width: width, quality: quality, maxBytes: maxBytes,
            systemVolume: systemVolume, microphoneVolume: microphoneVolume, mono: mono, gifFPS: gifFPS,
            trimStartMS: trimStartMS, trimEndMS: trimEndMS, cropEnabled: cropEnabled, crop: crop
        )
        exporting = true
        status = "Exporting…"
        Backend.shared.call("media_export", fields) { [weak self] result in
            guard let self else { return }
            self.exporting = false
            switch result {
            case let .success(response):
                do {
                    let output = try Artifact(response: response)
                    if let source {
                        _ = try FileManager.default.replaceItemAt(source, withItemAt: output.url)
                        self.status = "Saved \(source.lastPathComponent)"
                    } else {
                        AppStore.shared.addArtifact(output)
                        self.status = "Saved \(output.url.lastPathComponent)"
                    }
                } catch {
                    if source != nil { try? FileManager.default.removeItem(at: url) }
                    self.status = error.localizedDescription
                    AppStore.shared.report(error)
                }
            case let .failure(error):
                if source != nil { try? FileManager.default.removeItem(at: url) }
                self.status = error.localizedDescription
                AppStore.shared.report(error)
            }
        }
    }

    func prepareComparison(
        format: String, width: Int?, quality: String,
        maxBytes: Int?, systemVolume: Double, microphoneVolume: Double, mono: Bool, gifFPS: Int
    ) {
        guard !comparisonPending else { return }
        player.pause()
        constrainCrop()
        comparisonGeneration += 1
        let generation = comparisonGeneration
        let trimStartSnapshot = trimStartMS
        let trimEndSnapshot = trimEndMS
        let playheadSnapshot = playheadMS
        let cropEnabledSnapshot = cropEnabled
        let cropSnapshot = crop
        let sourceTime = min(
            max(trimStartSnapshot, playheadSnapshot),
            max(trimStartSnapshot, trimEndSnapshot - 1)
        )
        let outputTime = min(
            max(0, sourceTime - trimStartSnapshot),
            max(0, trimEndSnapshot - trimStartSnapshot - 1)
        )
        var comparisonDirectory: URL?
        do {
            let directory = AppStore.dataDirectory.appendingPathComponent("recording-comparisons/\(UUID().uuidString)", isDirectory: true)
            comparisonDirectory = directory
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            let output = directory.appendingPathComponent("after.\(format)")
            comparisonBefore = try previewFrame(at: artifact.url, milliseconds: sourceTime)
            comparisonAfter = nil
            comparisonError = ""
            comparisonPending = true
            let fields = exportFields(
                output: output, format: format, width: width, quality: quality, maxBytes: maxBytes,
                systemVolume: systemVolume, microphoneVolume: microphoneVolume, mono: mono, gifFPS: gifFPS,
                trimStartMS: trimStartSnapshot, trimEndMS: trimEndSnapshot,
                cropEnabled: cropEnabledSnapshot, crop: cropSnapshot
            )
            Backend.shared.call("media_export", fields) { [weak self] result in
                defer { try? FileManager.default.removeItem(at: directory) }
                guard let self else { return }
                guard self.comparisonGeneration == generation else { return }
                self.comparisonPending = false
                switch result {
                case .success:
                    do {
                        self.comparisonAfter = try self.previewFrame(at: output, milliseconds: outputTime)
                    } catch {
                        self.comparisonError = "The encoded comparison could not be decoded: \(error.localizedDescription)"
                    }
                case let .failure(error):
                    self.comparisonError = error.localizedDescription
                }
            }
        } catch {
            if let comparisonDirectory { try? FileManager.default.removeItem(at: comparisonDirectory) }
            guard comparisonGeneration == generation else { return }
            comparisonPending = false
            comparisonError = error.localizedDescription
        }
    }

    func dismissComparison() {
        comparisonGeneration += 1
        comparisonPending = false
        comparisonBefore = nil
        comparisonAfter = nil
        comparisonError = ""
    }

    private func loadTimelineFrames(durationMS: Double) {
        let url = artifact.url
        DispatchQueue.global(qos: .utility).async {
            let asset = AVURLAsset(url: url)
            let generator = AVAssetImageGenerator(asset: asset)
            generator.appliesPreferredTrackTransform = true
            generator.maximumSize = CGSize(width: 240, height: 136)
            generator.requestedTimeToleranceBefore = CMTime(seconds: 0.1, preferredTimescale: 600)
            generator.requestedTimeToleranceAfter = CMTime(seconds: 0.1, preferredTimescale: 600)
            let frames = (0..<12).compactMap { index -> NSImage? in
                let fraction = (Double(index) + 0.5) / 12
                guard let image = try? generator.copyCGImage(
                    at: CMTime(seconds: durationMS * fraction / 1_000, preferredTimescale: 600),
                    actualTime: nil
                ) else { return nil }
                return NSImage(cgImage: image, size: .zero)
            }
            DispatchQueue.main.async { [weak self] in self?.timelineFrames = frames }
        }
    }

    private func exportFields(
        output: URL, format: String, width: Int?, quality: String,
        maxBytes: Int?, systemVolume: Double, microphoneVolume: Double, mono: Bool, gifFPS: Int,
        trimStartMS: Double, trimEndMS: Double, cropEnabled: Bool, crop: RecordingCrop
    ) -> [String: Any] {
        var fields: [String: Any] = [
            "path": artifact.path, "output": output.path, "format": format,
            "start_ms": Int(trimStartMS.rounded()), "end_ms": Int(trimEndMS.rounded()),
            "quality": quality, "system_volume": systemVolume,
            "microphone_volume": microphoneVolume, "mono": mono,
        ]
        if cropEnabled {
            fields["crop"] = ["x": Int(crop.x), "y": Int(crop.y), "width": Int(crop.width), "height": Int(crop.height)]
        }
        if let width { fields["width"] = width }
        if let maxBytes { fields["max_bytes"] = maxBytes }
        if format == "gif" { fields["fps"] = gifFPS }
        return fields
    }

    private func previewFrame(at url: URL, milliseconds: Double) throws -> NSImage {
        if url.pathExtension.lowercased() == "gif", let image = NSImage(contentsOf: url) { return image }
        let asset = AVURLAsset(url: url)
        let generator = AVAssetImageGenerator(asset: asset)
        generator.appliesPreferredTrackTransform = true
        let image = try generator.copyCGImage(
            at: CMTime(seconds: max(0, milliseconds) / 1_000, preferredTimescale: 600),
            actualTime: nil
        )
        return NSImage(cgImage: image, size: .zero)
    }

    private static func number(_ value: Any?) -> Double? {
        if let number = value as? NSNumber { return number.doubleValue }
        if let number = value as? Double { return number }
        if let number = value as? Int { return Double(number) }
        return nil
    }
}

struct RecordingEditorView: View {
    let artifact: Artifact
    @StateObject private var model: RecordingEditorModel
    @Environment(\.colorScheme) private var colorScheme
    @State private var previewActualSize = false
    @State private var loop = false
    @State private var aspectLocked = true
    @State private var outputFormat = "mp4"
    @State private var outputSize = "original"
    @State private var customWidth = 1920
    @State private var qualityMode = "preserve"
    @State private var quality = "standard"
    @State private var maximumSizeMB = 10.0
    @State private var makeCopy: Bool
    @State private var exportDirectory: URL
    @State private var exportFilename: String
    @State private var comparisonExpanded = true
    @State private var comparisonWork: DispatchWorkItem?
    @State private var gifFPS = 15
    @State private var comparisonSplit: CGFloat = 0.5
    @State private var systemVolume = 1.0
    @State private var microphoneVolume = 1.0
    @State private var muteSystem = false
    @State private var muteMicrophone = false
    @State private var mono = false
    @State private var cropDragStart: CGPoint?
    @State private var cropAtDragStart: RecordingCrop?
    @State private var timelineDragTarget: RecordingTimelineTarget?
    @State private var timelineDragInitialMS: Double?

    init(artifact: Artifact) {
        self.artifact = artifact
        _model = StateObject(wrappedValue: RecordingEditorModel(artifact: artifact))
        let format = artifact.kind == "gif" ? "gif" : "mp4"
        _outputFormat = State(initialValue: format)
        _makeCopy = State(initialValue: artifact.url.pathExtension.lowercased() != format)
        _exportDirectory = State(initialValue: artifact.url.deletingLastPathComponent())
        _exportFilename = State(initialValue: artifact.url.deletingPathExtension().lastPathComponent)
    }

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: NativeTheme.metric("s-6")) {
                    Text(artifact.kind == "gif" ? "Edit GIF" : "Edit recording")
                        .font(.system(size: NativeTheme.metric("text-2xl"), weight: .bold))
                    previewCard
                    timelineCard
                    LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], alignment: .leading, spacing: NativeTheme.metric("s-6")) {
                        cropCard
                        qualityCard
                        if let probe = model.probe, probe.hasSystemAudio || probe.hasMicrophoneAudio {
                            audioCard(probe).gridCellColumns(2)
                        }
                    }
                }
                .padding(NativeTheme.metric("s-8")).frame(maxWidth: 1220)
                .frame(maxWidth: .infinity)
            }
            saveFooter
        }
        .background(NativeTheme.canvas(colorScheme))
        .foregroundStyle(NativeTheme.text(colorScheme))
        .onChange(of: loop) { value in model.loopEnabled = value }
        .onChange(of: model.probe) { _ in scheduleComparison() }
        .onChange(of: qualityMode) { _ in scheduleComparison() }
        .onChange(of: quality) { _ in scheduleComparison() }
        .onChange(of: maximumSizeMB) { _ in scheduleComparison() }
        .onChange(of: outputSize) { _ in scheduleComparison() }
        .onChange(of: customWidth) { _ in scheduleComparison() }
        .onChange(of: gifFPS) { _ in scheduleComparison() }
        .onChange(of: model.trimStartMS) { _ in scheduleComparison() }
        .onChange(of: model.trimEndMS) { _ in scheduleComparison() }
        .onChange(of: model.cropEnabled) { _ in scheduleComparison() }
        .onChange(of: model.crop) { _ in scheduleComparison() }
        .onChange(of: systemVolume) { _ in scheduleComparison() }
        .onChange(of: microphoneVolume) { _ in scheduleComparison() }
        .onChange(of: muteSystem) { _ in scheduleComparison() }
        .onChange(of: muteMicrophone) { _ in scheduleComparison() }
        .onChange(of: mono) { _ in scheduleComparison() }
        .onChange(of: outputFormat) { _ in
            if formatRequiresCopy { makeCopy = true }
            scheduleComparison()
        }
        .onChange(of: exportFilename) { value in
            if value != artifact.url.deletingPathExtension().lastPathComponent { makeCopy = true }
        }
        .onChange(of: makeCopy) { enabled in
            if enabled {
                let candidate = exportDirectory.appendingPathComponent(exportFilename)
                    .appendingPathExtension(outputFormat)
                if sameFile(candidate, artifact.url) || FileManager.default.fileExists(atPath: candidate.path) {
                    exportFilename = availableCopyFilename()
                }
            } else {
                exportDirectory = artifact.url.deletingLastPathComponent()
                exportFilename = artifact.url.deletingPathExtension().lastPathComponent
            }
        }
        .onDisappear { comparisonWork?.cancel(); model.dismissComparison() }
        .animation(NativeTheme.motion, value: model.cropEnabled)
        .animation(NativeTheme.standard, value: outputFormat)
    }

    private var previewCard: some View {
        VStack(spacing: 0) {
            HStack {
                SectionTitle("Preview")
                Spacer()
                Button { loop.toggle() } label: {
                    Label(loop ? "Looping" : "Loop preview", systemImage: "repeat")
                }
                    .buttonStyle(CaptureButtonStyle(primary: loop))
                CaptureSegments(selection: $previewActualSize, options: [
                    CaptureOption(label: "Fit", value: false),
                    CaptureOption(label: "100%", value: true),
                ]).frame(width: 140)
            }
            .padding(.horizontal, NativeTheme.metric("s-5")).frame(height: 46)
            Divider()
            GeometryReader { geometry in
                ScrollView([.horizontal, .vertical]) {
                    ZStack {
                        RecordingPlayerSurface(player: model.player)
                            .onTapGesture { model.togglePlayback(loop: loop) }
                        if model.cropEnabled { cropOverlay }
                        if model.comparisonBefore != nil { comparisonOverlay }
                        if model.comparisonBefore == nil {
                            Button { model.togglePlayback(loop: loop) } label: {
                                Image(systemName: model.player.rate == 0 ? "play.fill" : "pause.fill")
                                    .font(.title2).frame(width: 54, height: 54)
                            }
                            .buttonStyle(CaptureButtonStyle(primary: true))
                            .clipShape(Circle()).opacity(model.player.rate == 0 ? 1 : 0.25)
                        }
                    }
                    .frame(width: previewSize(in: geometry.size).width, height: previewSize(in: geometry.size).height)
                    .frame(minWidth: geometry.size.width, minHeight: geometry.size.height)
                }
            }
            .frame(minHeight: 260, idealHeight: 410, maxHeight: 520)
            .background(NativeTheme.field(colorScheme))
        }
        .background(NativeTheme.raised(colorScheme))
        .clipShape(RoundedRectangle(cornerRadius: NativeTheme.metric("r-xl")))
        .overlay(RoundedRectangle(cornerRadius: NativeTheme.metric("r-xl")).stroke(NativeTheme.border(colorScheme)))
    }

    @ViewBuilder private var cropOverlay: some View {
        if let probe = model.probe {
            GeometryReader { geometry in
                let sx = geometry.size.width / CGFloat(probe.width)
                let sy = geometry.size.height / CGFloat(probe.height)
                let rect = CGRect(
                    x: CGFloat(model.crop.x) * sx,
                    y: CGFloat(model.crop.y) * sy,
                    width: CGFloat(model.crop.width) * sx,
                    height: CGFloat(model.crop.height) * sy
                )
                ZStack {
                    Path { path in
                        path.addRect(CGRect(origin: .zero, size: geometry.size))
                        path.addRect(rect)
                    }
                    .fill(Color.black.opacity(0.58), style: FillStyle(eoFill: true))
                    Rectangle().stroke(NativeTheme.accent, lineWidth: 3)
                        .frame(width: rect.width, height: rect.height).position(x: rect.midX, y: rect.midY)
                    ForEach(Array(RecordingCropHandle.allCases.enumerated()), id: \.offset) { _, handle in
                        cropHandle(handle, rect: rect, scaleX: sx, scaleY: sy)
                    }
                    Text("\(Int(model.crop.width)) × \(Int(model.crop.height))")
                        .font(.caption.monospaced()).padding(5)
                        .foregroundStyle(NativeTheme.glassText).background(NativeTheme.glass).cornerRadius(5)
                        .position(x: rect.minX + 55, y: rect.minY + 18)
                }
                .contentShape(Rectangle())
                .gesture(DragGesture(minimumDistance: 0).onChanged { value in
                    if cropDragStart == nil {
                        cropDragStart = value.startLocation
                        cropAtDragStart = model.crop
                    }
                    guard let start = cropDragStart, let initial = cropAtDragStart else { return }
                    model.crop.x = initial.x + Double((value.location.x - start.x) / sx)
                    model.crop.y = initial.y + Double((value.location.y - start.y) / sy)
                    model.constrainCrop()
                }.onEnded { _ in cropDragStart = nil; cropAtDragStart = nil })
                .accessibilityLabel("Recording crop selection")
                .accessibilityValue("\(Int(model.crop.width)) by \(Int(model.crop.height)) pixels")
            }
        }
    }

    @ViewBuilder private var comparisonOverlay: some View {
        if let before = model.comparisonBefore {
            GeometryReader { geometry in
                ZStack {
                    Image(nsImage: before).resizable().aspectRatio(contentMode: .fill)
                    if let after = model.comparisonAfter {
                        Image(nsImage: after).resizable().aspectRatio(contentMode: .fill)
                            .mask(alignment: .trailing) {
                                Rectangle().frame(width: geometry.size.width * (1 - comparisonSplit))
                            }
                    } else {
                        ZStack {
                            Color.black.opacity(0.35)
                            ProgressView("Encoding comparison…").foregroundStyle(.white)
                        }
                    }
                    if model.comparisonAfter != nil {
                        Rectangle().fill(Color.white).frame(width: 2)
                            .position(x: geometry.size.width * comparisonSplit, y: geometry.size.height / 2)
                        Circle().fill(NativeTheme.glass).overlay(Image(systemName: "arrow.left.and.right").foregroundStyle(NativeTheme.glassText))
                            .frame(width: 38, height: 38)
                            .position(x: geometry.size.width * comparisonSplit, y: geometry.size.height / 2)
                            .gesture(DragGesture(minimumDistance: 0).onChanged { value in
                                comparisonSplit = min(0.94, max(0.06, value.location.x / geometry.size.width))
                            })
                        HStack {
                            Text("Before").padding(6).background(NativeTheme.glass).cornerRadius(5)
                            Spacer()
                            Text("After").padding(6).background(NativeTheme.glass).cornerRadius(5)
                        }
                        .font(.caption.weight(.semibold)).foregroundStyle(NativeTheme.glassText)
                        .padding(12).frame(maxHeight: .infinity, alignment: .bottom)
                    }
                    Button {
                        comparisonExpanded = false
                        model.dismissComparison()
                    } label: { Image(systemName: "xmark") }
                        .buttonStyle(CaptureButtonStyle(glass: true))
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing)
                        .padding(12)
                }
                .clipped()
            }
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Encoded before and after comparison")
        }
    }

    private var timelineCard: some View {
        editorCard {
            HStack {
                Text("\(time(model.trimStartMS)) – \(time(model.trimEndMS))").font(.headline.monospacedDigit())
                Spacer()
                Text("\(time(max(0, model.trimEndMS - model.trimStartMS))) selected").foregroundStyle(NativeTheme.muted(colorScheme))
            }
            if let probe = model.probe {
                GeometryReader { geometry in
                    ZStack(alignment: .leading) {
                        RoundedRectangle(cornerRadius: 8).fill(NativeTheme.field(colorScheme)).frame(height: 76)
                        HStack(spacing: 1) {
                            if model.timelineFrames.isEmpty {
                                ForEach(0..<12, id: \.self) { _ in
                                    NativeTheme.color("surface-sunken", colorScheme)
                                }
                            } else {
                                ForEach(Array(model.timelineFrames.enumerated()), id: \.offset) { _, frame in
                                    Image(nsImage: frame).resizable().scaledToFill().clipped()
                                }
                            }
                        }
                        .padding(8)
                        .frame(width: geometry.size.width, height: 76)
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                        let start = geometry.size.width * CGFloat(model.trimStartMS / probe.durationMS)
                        let end = geometry.size.width * CGFloat(model.trimEndMS / probe.durationMS)
                        if start > 0 {
                            Color.black.opacity(0.66).frame(width: start, height: 76)
                        }
                        if end < geometry.size.width {
                            Color.black.opacity(0.66)
                                .frame(width: geometry.size.width - end, height: 76)
                                .offset(x: end)
                        }
                        Rectangle().fill(Color.white).frame(width: 2, height: 82)
                            .shadow(color: .black.opacity(0.7), radius: 2)
                            .offset(x: geometry.size.width * CGFloat(model.playheadMS / probe.durationMS))
                            .allowsHitTesting(false)
                        timelineHandle(.start, x: start, duration: probe.durationMS, width: geometry.size.width)
                        timelineHandle(.end, x: end, duration: probe.durationMS, width: geometry.size.width)
                    }
                    .contentShape(Rectangle())
                    .gesture(DragGesture(minimumDistance: 0).onChanged { value in
                        timelineDragTarget = timelineDragTarget ?? .playhead
                        guard timelineDragTarget == .playhead else { return }
                        model.seek(to: Double(min(max(0, value.location.x), geometry.size.width) / geometry.size.width) * probe.durationMS)
                    }.onEnded { _ in timelineDragTarget = nil })
                }
                .frame(height: 76)
                .accessibilityElement(children: .contain)
                Text("Drag the handles to trim; click or drag the filmstrip to scrub.")
                    .font(.caption).foregroundStyle(NativeTheme.muted(colorScheme))
            } else {
                ProgressView().frame(maxWidth: .infinity)
            }
        }
    }

    private var cropCard: some View {
        editorCard {
            SectionTitle("Crop & size")
            CaptureCheckboxRow(title: "Crop recording", isOn: $model.cropEnabled)
            if let probe = model.probe {
                LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: NativeTheme.metric("s-3")) {
                    numberField("X", value: cropBinding(\.x), range: 0...Double(probe.width))
                    numberField("Y", value: cropBinding(\.y), range: 0...Double(probe.height))
                    numberField("Width", value: cropBinding(\.width), range: 2...Double(probe.width))
                    numberField("Height", value: cropBinding(\.height), range: 2...Double(probe.height))
                }.disabled(!model.cropEnabled)
            }
            CaptureCheckboxRow(title: "Lock aspect ratio", isOn: $aspectLocked).disabled(!model.cropEnabled)
            CaptureChoice(title: "Output resolution", selection: $outputSize, options: [
                CaptureOption(label: "Original", value: "original"),
                CaptureOption(label: "1080p maximum", value: "1080"),
                CaptureOption(label: "720p maximum", value: "720"),
                CaptureOption(label: "Custom width", value: "custom"),
            ])
            if outputSize == "custom" {
                HStack {
                    Text("Width: \(customWidth) px").monospacedDigit()
                    Spacer()
                    Button { customWidth = max(2, customWidth - 2) } label: { Image(systemName: "minus") }
                        .buttonStyle(CaptureButtonStyle())
                    Button { customWidth = min(16_384, customWidth + 2) } label: { Image(systemName: "plus") }
                        .buttonStyle(CaptureButtonStyle())
                }
            }
        }
    }

    private var qualityCard: some View {
        editorCard {
            SectionTitle("Save quality")
            CaptureChoice(title: "Quality mode", selection: $qualityMode, options: [
                CaptureOption(label: "Preserve quality", value: "preserve"),
                CaptureOption(label: "Compress", value: "compress"),
                CaptureOption(label: "Maximum file size", value: "maximum"),
            ])
            if qualityMode == "compress" {
                CaptureChoice(title: "Compression quality", selection: $quality, options: [
                    CaptureOption(label: "Highest", value: "highest"),
                    CaptureOption(label: "High", value: "high"),
                    CaptureOption(label: "Balanced", value: "standard"),
                    CaptureOption(label: "Smaller", value: "small"),
                    CaptureOption(label: "Tiny", value: "tiny"),
                ])
            } else if qualityMode == "maximum" {
                HStack {
                    Text("Maximum file size")
                    TextField("MB", value: $maximumSizeMB, format: .number.precision(.fractionLength(1)))
                        .textFieldStyle(.roundedBorder).frame(width: 72)
                    Text("MB")
                }
            }
            if outputFormat == "gif" {
                CaptureChoice(title: "Frame rate", selection: $gifFPS,
                              options: [8, 10, 12, 15, 20, 24, 30].map { CaptureOption(label: "\($0) FPS", value: $0) })
            }
            if qualityMode != "preserve" {
                Button {
                    comparisonExpanded.toggle()
                    if comparisonExpanded { scheduleComparison() } else { model.dismissComparison() }
                } label: {
                    HStack {
                        Image(systemName: "chevron.right").rotationEffect(comparisonExpanded ? .degrees(90) : .zero)
                        Text("Compression comparison")
                        Spacer()
                        Text(model.comparisonPending ? "Encoding…" : "Updates automatically")
                            .foregroundColor(NativeTheme.muted(colorScheme))
                    }
                }
                .buttonStyle(CaptureButtonStyle())
            }
            if !model.comparisonError.isEmpty {
                Text(model.comparisonError).font(.caption).foregroundStyle(NativeTheme.signal)
            }
        }
    }

    private func audioCard(_ probe: RecordingProbe) -> some View {
        editorCard {
            SectionTitle("Audio")
            HStack(spacing: NativeTheme.metric("s-8")) {
                if probe.hasSystemAudio {
                    VStack(alignment: .leading) {
                        CaptureCheckboxRow(title: "System audio", isOn: Binding(
                            get: { !muteSystem }, set: { muteSystem = !$0 }
                        ))
                        HStack { Text("Volume"); Spacer(); Text("\(Int(effectiveSystemVolume * 100))%") }
                        CaptureSlider(value: $systemVolume, range: 0...2).disabled(muteSystem)
                    }
                }
                if probe.hasMicrophoneAudio {
                    VStack(alignment: .leading) {
                        CaptureCheckboxRow(title: "Microphone", isOn: Binding(
                            get: { !muteMicrophone }, set: { muteMicrophone = !$0 }
                        ))
                        HStack { Text("Volume"); Spacer(); Text("\(Int(effectiveMicrophoneVolume * 100))%") }
                        CaptureSlider(value: $microphoneVolume, range: 0...2).disabled(muteMicrophone)
                    }
                }
            }
            CaptureCheckboxRow(title: "Convert to mono", isOn: $mono)
        }
    }

    private var saveFooter: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text("Filename").font(.caption).foregroundStyle(NativeTheme.muted(colorScheme))
                    Text("Saving to \(exportDirectory.path)")
                        .font(.caption).foregroundStyle(NativeTheme.muted(colorScheme)).lineLimit(1)
                    Button("Change…", action: chooseExportDirectory).buttonStyle(.plain)
                }
                HStack(spacing: 5) {
                    TextField("Filename", text: $exportFilename).textFieldStyle(.roundedBorder)
                    CaptureChoice(title: "Format", selection: $outputFormat, options: [
                        CaptureOption(label: ".mp4", value: "mp4"),
                        CaptureOption(label: ".gif", value: "gif"),
                    ], opensAbove: true).frame(width: 82)
                }
            }
            .frame(width: 390)
            Spacer()
            Text(model.status).font(.callout).foregroundStyle(NativeTheme.muted(colorScheme)).lineLimit(1)
            HStack(spacing: 8) {
                CaptureToggle(title: "Save as new file", isOn: $makeCopy, compact: true)
                Text("Save as new file")
            }
            .disabled(formatRequiresCopy || model.exporting)
            Button { chooseExport() } label: {
                Label(model.exporting ? "Exporting…" : "Save", systemImage: "square.and.arrow.down")
            }
                .buttonStyle(CaptureButtonStyle(primary: true)).disabled(model.exporting || model.probe == nil)
        }
        .padding(NativeTheme.metric("s-5")).background(NativeTheme.raised(colorScheme)).overlay(alignment: .top) { Divider() }
    }

    private func editorCard<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: NativeTheme.metric("s-4"), content: content)
            .padding(NativeTheme.metric("s-6")).frame(maxWidth: .infinity, alignment: .leading)
            .background(NativeTheme.raised(colorScheme), in: RoundedRectangle(cornerRadius: NativeTheme.metric("r-lg")))
            .overlay(RoundedRectangle(cornerRadius: NativeTheme.metric("r-lg")).stroke(NativeTheme.border(colorScheme)))
    }

    private func numberField(_ label: String, value: Binding<Double>, range: ClosedRange<Double>) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label).font(.caption).foregroundStyle(NativeTheme.muted(colorScheme))
            TextField(label, value: value, format: .number.precision(.fractionLength(0)))
                .textFieldStyle(.roundedBorder)
        }
    }

    private func cropBinding(_ keyPath: WritableKeyPath<RecordingCrop, Double>) -> Binding<Double> {
        Binding(
            get: { model.crop[keyPath: keyPath] },
            set: { value in
                let oldWidth = max(2, model.crop.width)
                let oldHeight = max(2, model.crop.height)
                model.crop[keyPath: keyPath] = value
                if aspectLocked, keyPath == \RecordingCrop.width {
                    model.crop.height = value * oldHeight / oldWidth
                } else if aspectLocked, keyPath == \RecordingCrop.height {
                    model.crop.width = value * oldWidth / oldHeight
                }
                model.constrainCrop()
            }
        )
    }

    private func cropHandle(_ handle: RecordingCropHandle, rect: CGRect, scaleX: CGFloat, scaleY: CGFloat) -> some View {
        let position = cropHandlePosition(handle, rect: rect)
        return Circle()
            .fill(Color.white)
            .overlay(Circle().stroke(NativeTheme.accent, lineWidth: 2))
            .frame(width: 14, height: 14)
            .position(position)
            .gesture(DragGesture(minimumDistance: 0).onChanged { value in
                if cropDragStart == nil {
                    cropDragStart = value.startLocation
                    cropAtDragStart = model.crop
                }
                guard let start = cropDragStart, let initial = cropAtDragStart else { return }
                resizeCrop(handle, initial: initial, delta: CGSize(width: (value.location.x - start.x) / scaleX, height: (value.location.y - start.y) / scaleY))
            }.onEnded { _ in cropDragStart = nil; cropAtDragStart = nil })
            .accessibilityLabel("Resize crop \(String(describing: handle))")
    }

    private func cropHandlePosition(_ handle: RecordingCropHandle, rect: CGRect) -> CGPoint {
        switch handle {
        case .northWest: return CGPoint(x: rect.minX, y: rect.minY)
        case .north: return CGPoint(x: rect.midX, y: rect.minY)
        case .northEast: return CGPoint(x: rect.maxX, y: rect.minY)
        case .east: return CGPoint(x: rect.maxX, y: rect.midY)
        case .southEast: return CGPoint(x: rect.maxX, y: rect.maxY)
        case .south: return CGPoint(x: rect.midX, y: rect.maxY)
        case .southWest: return CGPoint(x: rect.minX, y: rect.maxY)
        case .west: return CGPoint(x: rect.minX, y: rect.midY)
        }
    }

    private func resizeCrop(_ handle: RecordingCropHandle, initial: RecordingCrop, delta: CGSize) {
        var left = initial.x
        var top = initial.y
        var right = initial.x + initial.width
        var bottom = initial.y + initial.height
        if [.northWest, .west, .southWest].contains(handle) { left += Double(delta.width) }
        if [.northEast, .east, .southEast].contains(handle) { right += Double(delta.width) }
        if [.northWest, .north, .northEast].contains(handle) { top += Double(delta.height) }
        if [.southWest, .south, .southEast].contains(handle) { bottom += Double(delta.height) }
        let width = max(2, right - left)
        let height = max(2, bottom - top)
        if aspectLocked {
            let ratio = initial.width / max(2, initial.height)
            if [.north, .south].contains(handle) {
                let adjustedWidth = height * ratio
                left = initial.x + (initial.width - adjustedWidth) / 2
                right = left + adjustedWidth
            } else {
                let adjustedHeight = width / ratio
                top = initial.y + (initial.height - adjustedHeight) / 2
                bottom = top + adjustedHeight
            }
        }
        model.crop = RecordingCrop(x: min(left, right - 2), y: min(top, bottom - 2), width: max(2, right - left), height: max(2, bottom - top))
        model.constrainCrop()
    }

    private func timelineHandle(_ target: RecordingTimelineTarget, x: CGFloat, duration: Double, width: CGFloat) -> some View {
        let isStart = target == .start
        return RoundedRectangle(cornerRadius: 4)
            .fill(NativeTheme.accent)
            .frame(width: 16, height: 82)
            .overlay {
                HStack(spacing: 4) {
                    Rectangle().frame(width: 1, height: 14)
                    Rectangle().frame(width: 1, height: 14)
                }
                .foregroundStyle(Color.black.opacity(0.45))
            }
            .overlay(RoundedRectangle(cornerRadius: 4).stroke(Color.black.opacity(0.28)))
            .shadow(color: .black.opacity(0.18), radius: 2, y: 1)
            .frame(width: 28, height: 86)
            .contentShape(Rectangle())
            .offset(x: x - 14)
            .gesture(DragGesture(minimumDistance: 0).onChanged { value in
                if timelineDragTarget == nil {
                    timelineDragTarget = target
                    timelineDragInitialMS = isStart ? model.trimStartMS : model.trimEndMS
                }
                let milliseconds = (timelineDragInitialMS ?? 0) + Double(value.translation.width / width) * duration
                if isStart { model.updateTrimStart(milliseconds) } else { model.updateTrimEnd(milliseconds) }
            }.onEnded { _ in timelineDragTarget = nil; timelineDragInitialMS = nil })
            .accessibilityLabel(isStart ? "Trim start" : "Trim end")
            .accessibilityValue(time(isStart ? model.trimStartMS : model.trimEndMS))
            .accessibilityAdjustableAction { direction in
                let delta = direction == .increment ? 100.0 : -100.0
                if isStart { model.updateTrimStart(model.trimStartMS + delta) } else { model.updateTrimEnd(model.trimEndMS + delta) }
            }
    }

    private func previewSize(in available: CGSize) -> CGSize {
        guard let probe = model.probe else { return available }
        if previewActualSize { return CGSize(width: CGFloat(probe.width), height: CGFloat(probe.height)) }
        let scale = min(max(1, available.width - 32) / CGFloat(probe.width), max(1, available.height - 32) / CGFloat(probe.height), 1)
        return CGSize(width: CGFloat(probe.width) * scale, height: CGFloat(probe.height) * scale)
    }

    private var effectiveSystemVolume: Double { muteSystem ? 0 : systemVolume }
    private var effectiveMicrophoneVolume: Double { muteMicrophone ? 0 : microphoneVolume }
    private var effectiveQuality: String { qualityMode == "preserve" ? "preserve" : quality }
    private var maximumBytes: Int? {
        qualityMode == "maximum" ? max(100_000, Int((maximumSizeMB * 1_000_000).rounded())) : nil
    }
    private var sourceFormat: String {
        artifact.url.pathExtension.lowercased()
    }
    private var formatRequiresCopy: Bool { sourceFormat != outputFormat }

    private func time(_ milliseconds: Double) -> String {
        let seconds = max(0, milliseconds) / 1_000
        return String(format: "%d:%06.3f", Int(seconds) / 60, seconds.truncatingRemainder(dividingBy: 60))
    }

    private func outputWidth() -> Int? {
        guard let probe = model.probe else { return nil }
        let sourceWidth = model.cropEnabled ? Int(model.crop.width) : probe.width
        let sourceHeight = model.cropEnabled ? Int(model.crop.height) : probe.height
        switch outputSize {
        case "1080": return sourceHeight > 1080 ? max(2, Int(Double(sourceWidth) * 1080 / Double(sourceHeight)) / 2 * 2) : nil
        case "720": return sourceHeight > 720 ? max(2, Int(Double(sourceWidth) * 720 / Double(sourceHeight)) / 2 * 2) : nil
        case "custom": return customWidth
        default: return nil
        }
    }

    private func chooseExport() {
        if !makeCopy, !formatRequiresCopy {
            let staged = artifact.url.deletingLastPathComponent()
                .appendingPathComponent(".captures-edit-\(UUID().uuidString).\(outputFormat)")
            model.export(
                to: staged, format: outputFormat, width: outputWidth(), quality: effectiveQuality,
                maxBytes: maximumBytes, systemVolume: effectiveSystemVolume,
                microphoneVolume: effectiveMicrophoneVolume, mono: mono, gifFPS: gifFPS,
                replacing: artifact.url
            )
            return
        }
        let filename = exportFilename.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !filename.isEmpty, !filename.contains("/") else {
            showExportError("Enter a filename without folder separators.")
            return
        }
        let url = exportDirectory.appendingPathComponent(filename).appendingPathExtension(outputFormat)
        guard !sameFile(url, artifact.url) else {
            showExportError("Turn off Save as new file to replace the source, or choose another filename.")
            return
        }
        if FileManager.default.fileExists(atPath: url.path) {
            showExportError("Recording export only creates new files. Choose a filename that does not already exist.")
            return
        }
        model.export(
            to: url, format: outputFormat, width: outputWidth(), quality: effectiveQuality,
            maxBytes: maximumBytes, systemVolume: effectiveSystemVolume, microphoneVolume: effectiveMicrophoneVolume,
            mono: mono, gifFPS: gifFPS
        )
    }

    private func chooseExportDirectory() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.directoryURL = exportDirectory
        if panel.runModal() == .OK, let url = panel.url {
            exportDirectory = url
            if url.standardizedFileURL != artifact.url.deletingLastPathComponent().standardizedFileURL {
                makeCopy = true
            }
        }
    }

    private func scheduleComparison() {
        comparisonWork?.cancel()
        guard comparisonExpanded, qualityMode != "preserve", model.probe != nil else {
            model.dismissComparison()
            return
        }
        let work = DispatchWorkItem {
            model.dismissComparison()
            model.prepareComparison(
                format: outputFormat, width: outputWidth(), quality: effectiveQuality,
                maxBytes: maximumBytes, systemVolume: effectiveSystemVolume,
                microphoneVolume: effectiveMicrophoneVolume, mono: mono, gifFPS: gifFPS
            )
        }
        comparisonWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.35, execute: work)
    }

    private func sameFile(_ lhs: URL, _ rhs: URL) -> Bool {
        lhs.standardizedFileURL.resolvingSymlinksInPath() == rhs.standardizedFileURL.resolvingSymlinksInPath()
    }

    private func availableCopyFilename() -> String {
        let source = artifact.url.deletingPathExtension().lastPathComponent
        let base = "\(source) edited"
        var candidate = base
        var suffix = 2
        while FileManager.default.fileExists(
            atPath: exportDirectory.appendingPathComponent(candidate)
                .appendingPathExtension(outputFormat).path
        ) {
            candidate = "\(base) \(suffix)"
            suffix += 1
        }
        return candidate
    }

    private func showExportError(_ message: String) {
        model.status = message
        CaptureDialogController.shared.present(
            title: "Choose another export location",
            message: message,
            action: "OK",
            cancel: nil,
            onConfirm: {}
        )
    }
}
