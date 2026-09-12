import AppKit
import AVFoundation
import ImageIO
import SwiftUI

struct HistoryView: View {
    @ObservedObject private var store = AppStore.shared
    @Environment(\.colorScheme) private var scheme
    @State private var filter = "all"
    @State private var confirmClear = false
    private var filtered: [Artifact] { store.artifacts.filter { filter == "all" || $0.kind == filter } }
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: NativeTheme.metric("s-8")) {
                HStack(alignment: .bottom) {
                    VStack(alignment: .leading, spacing: NativeTheme.metric("s-3")) {
                        Text("YOUR CAPTURES").font(.system(size: NativeTheme.metric("text-xs"), weight: .semibold))
                            .foregroundColor(NativeTheme.muted(scheme))
                        Text("Capture History").font(.system(size: NativeTheme.metric("text-3xl"), weight: .semibold))
                        Text("Recent screenshots, videos, and GIFs. Kept here for 30 days.")
                            .foregroundColor(NativeTheme.muted(scheme))
                    }
                    Spacer()
                    Button("Open file…") { store.chooseFile() }.buttonStyle(CaptureButtonStyle())
                    Button("New capture") { CaptureController.shared.show() }.buttonStyle(CaptureButtonStyle(primary: true))
                }
                RecoveryView()
                HStack {
                    ForEach(["all", "image", "video", "gif"], id: \.self) { kind in
                        Button {
                            filter = kind
                        } label: {
                            HStack(spacing: NativeTheme.metric("s-3")) {
                                Text(["all":"All", "image":"Screenshots", "video":"Videos", "gif":"GIFs"][kind]!)
                                Text("\(store.artifacts.filter { kind == "all" || $0.kind == kind }.count)").opacity(0.6)
                            }.padding(.horizontal, NativeTheme.metric("s-4"))
                                .frame(height: NativeTheme.metric("h-sm"))
                                .background(filter == kind ? NativeTheme.raised(scheme) : .clear)
                                .clipShape(Capsule()).overlay(Capsule().strokeBorder(filter == kind ? NativeTheme.border(scheme) : .clear))
                        }.buttonStyle(.plain)
                    }
                    Spacer()
                    if confirmClear {
                        Button("Cancel") { confirmClear = false }.buttonStyle(CaptureButtonStyle())
                    }
                    Button(confirmClear ? "Clear forever" : "Clear history") {
                        if confirmClear { store.clearHistory(); confirmClear = false }
                        else {
                            confirmClear = true
                            DispatchQueue.main.asyncAfter(deadline: .now() + 4) { confirmClear = false }
                        }
                    }
                    .buttonStyle(CaptureButtonStyle(destructive: true))
                    .disabled(store.artifacts.isEmpty)
                }
                Divider()
                if filtered.isEmpty {
                    VStack(spacing: NativeTheme.metric("s-5")) {
                        Image(systemName: "photo.on.rectangle.angled").font(.system(size: NativeTheme.metric("s-10")))
                        Text("No captures yet").font(.system(size: NativeTheme.metric("text-xl"), weight: .semibold))
                        Text("Take a screenshot or start a recording to see it here.").foregroundColor(NativeTheme.muted(scheme))
                    }.frame(maxWidth: .infinity).padding(.vertical, NativeTheme.metric("s-12"))
                } else {
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 240), spacing: NativeTheme.metric("s-6"))], spacing: NativeTheme.metric("s-6")) {
                        ForEach(filtered) { HistoryCard(artifact: $0) }
                    }
                }
            }.padding(NativeTheme.metric("s-9"))
        }
        .foregroundColor(NativeTheme.text(scheme)).background(NativeTheme.canvas(scheme))
    }
}

private struct HistoryCard: View {
    let artifact: Artifact
    @ObservedObject private var store = AppStore.shared
    @Environment(\.colorScheme) private var scheme
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var hovered = false
    @State private var removing = false
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button { store.open(artifact) } label: {
                ArtifactThumbnail(artifact: artifact).frame(height: 168).frame(maxWidth: .infinity)
                    .clipped().background(NativeTheme.color("surface-sunken", scheme))
            }.buttonStyle(.plain).accessibilityLabel("Open \(artifact.url.lastPathComponent)")
            VStack(alignment: .leading, spacing: NativeTheme.metric("s-5")) {
                Text(artifact.url.lastPathComponent).fontWeight(.medium).lineLimit(1)
                Text(artifact.createdAt.formatted(date: .abbreviated, time: .shortened))
                    .font(.system(size: NativeTheme.metric("text-sm"))).foregroundColor(NativeTheme.muted(scheme))
                HStack {
                    Text(artifact.kind == "image" ? "Screenshot" : artifact.kind.uppercased())
                    Spacer()
                    if artifact.width > 0 { Text("\(artifact.width) × \(artifact.height)") }
                }.font(.system(size: NativeTheme.metric("text-xs"))).foregroundColor(NativeTheme.muted(scheme))
                if removing {
                    Text("Remove from history? The saved file stays.").font(.system(size: NativeTheme.metric("text-sm")))
                    HStack {
                        Button("Cancel") { removing = false }.buttonStyle(CaptureButtonStyle())
                        Button("Remove") { store.removeFromHistory(artifact) }.buttonStyle(CaptureButtonStyle(destructive: true))
                    }
                } else {
                    HStack(spacing: NativeTheme.metric("s-2")) {
                        Button { store.open(artifact) } label: {
                            Label("Edit", systemImage: "pencil").frame(maxWidth: .infinity)
                        }
                            .buttonStyle(CaptureButtonStyle(primary: true))
                            .frame(maxWidth: .infinity)
                        Button {
                            if !store.previews.contains(where: { $0.path == artifact.path }) { store.previews.insert(artifact, at: 0) }
                            PreviewController.shared.refresh()
                        } label: {
                            Label("Restore", systemImage: "rectangle.stack").frame(maxWidth: .infinity)
                        }
                            .buttonStyle(CaptureButtonStyle())
                            .frame(maxWidth: .infinity)
                        Button { removing = true } label: {
                            Label("Delete", systemImage: "trash").frame(maxWidth: .infinity)
                        }
                            .buttonStyle(CaptureButtonStyle(destructive: true))
                            .frame(maxWidth: .infinity)
                    }
                    Button { NSWorkspace.shared.activateFileViewerSelecting([artifact.url]) } label: {
                        Label("Show in Finder", systemImage: "folder")
                    }
                    .buttonStyle(.plain)
                    .font(.system(size: NativeTheme.metric("text-sm"), weight: .medium))
                    .foregroundColor(NativeTheme.muted(scheme))
                }
            }.padding(NativeTheme.metric("s-6"))
        }
        .background(NativeTheme.raised(scheme)).cornerRadius(NativeTheme.metric("r-xl"))
        .overlay(RoundedRectangle(cornerRadius: NativeTheme.metric("r-xl")).strokeBorder(NativeTheme.color(hovered ? "border-strong" : "border-subtle", scheme)))
        .offset(y: hovered && !reduceMotion ? -NativeTheme.metric("s-1") : 0)
        .animation(reduceMotion ? nil : NativeTheme.standard, value: hovered).onHover { hovered = $0 }
    }
}

struct ArtifactThumbnail: View {
    let artifact: Artifact
    @State private var image: NSImage?
    var body: some View {
        Group {
            if let image { Image(nsImage: image).resizable().scaledToFit() }
            else { Image(systemName: artifact.kind == "image" ? "photo" : "film").foregroundColor(.secondary) }
        }.onAppear(perform: load)
    }
    private func load() {
        guard image == nil else { return }
        let url = artifact.url
        let isImage = artifact.kind == "image" || artifact.kind == "gif"
        DispatchQueue.global(qos: .utility).async {
            let thumbnail: NSImage? = autoreleasepool {
                if isImage {
                    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
                          let cg = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                            kCGImageSourceCreateThumbnailFromImageAlways: true,
                            kCGImageSourceThumbnailMaxPixelSize: 600,
                            kCGImageSourceCreateThumbnailWithTransform: true] as CFDictionary) else { return nil }
                    return NSImage(cgImage: cg, size: .zero)
                }
                let generator = AVAssetImageGenerator(asset: AVURLAsset(url: url))
                generator.appliesPreferredTrackTransform = true
                generator.maximumSize = CGSize(width: 600, height: 400)
                guard let cg = try? generator.copyCGImage(at: .zero, actualTime: nil) else { return nil }
                return NSImage(cgImage: cg, size: .zero)
            }
            DispatchQueue.main.async { self.image = thumbnail }
        }
    }
}

struct RecoveryView: View {
    @ObservedObject private var store = AppStore.shared
    @State private var drafts: [[String: String]] = []
    @State private var busy = false
    @State private var discardID: String?
    var body: some View {
        VStack(alignment: .leading, spacing: NativeTheme.metric("s-5")) {
            if !drafts.isEmpty {
                SectionTitle("Interrupted recordings", subtitle: "Recover completed segments, or explicitly discard the private draft.")
                ForEach(drafts.indices, id: \.self) { index in
                    HStack {
                        Text(drafts[index]["name"] ?? "Recording draft").lineLimit(1)
                        Spacer()
                        Button("Recover") { recover(drafts[index]["id"]!) }.buttonStyle(CaptureButtonStyle(primary: true)).disabled(busy)
                        if discardID == drafts[index]["id"] {
                            Button("Cancel") { discardID = nil }.buttonStyle(CaptureButtonStyle()).disabled(busy)
                            Button("Discard forever") { discard(drafts[index]["id"]!) }
                                .buttonStyle(CaptureButtonStyle(destructive: true)).disabled(busy)
                        } else {
                            Button("Discard…") { discardID = drafts[index]["id"] }
                                .buttonStyle(CaptureButtonStyle(destructive: true)).disabled(busy)
                        }
                    }
                }
            }
        }.onAppear(perform: reload)
    }
    private func reload() {
        Backend.shared.call("recover_list") { result in
            switch result {
            case .success(let value): drafts = (value["drafts"] as? [[String: Any]] ?? []).compactMap {
                guard let id = $0["id"] as? String else { return nil }
                return ["id": id, "name": $0["name"] as? String ?? id]
            }
            case .failure(let error): store.report(error)
            }
        }
    }
    private func recover(_ id: String) {
        busy = true
        Backend.shared.call("recover", ["id": id, "output_dir": store.settings.outputDirectory]) { result in
            busy = false
            do { let artifact = try Artifact(response: result.get()); store.addArtifact(artifact); store.open(artifact) }
            catch { store.report(error) }
            reload()
        }
    }
    private func discard(_ id: String) {
        busy = true
        Backend.shared.call("recover_discard", ["id": id]) { result in
            busy = false
            discardID = nil
            if case .failure(let error) = result { store.report(error) }
            reload()
        }
    }
}
