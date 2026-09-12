import AppKit
import SwiftUI

struct NativeReferenceFixture {
    let name: String
    let size: CGSize
    let scheme: ColorScheme
    let makeView: @MainActor () -> AnyView
}

@MainActor
func nativeReferenceFixtures(imageURL: URL, recordingURL: URL) -> [NativeReferenceFixture] {
    let image = NSImage(contentsOf: imageURL)!
    let imageArtifacts = (0..<4).map { _ in
        Artifact(path: imageURL.path, kind: "image", width: 960, height: 540)
    }
    let recording = Artifact(path: recordingURL.path, kind: "video", width: 960, height: 540)

    AppStore.shared.artifacts = [imageArtifacts[0], recording, imageArtifacts[1], imageArtifacts[2]]
    return [
        NativeReferenceFixture(name: "preferences-light", size: CGSize(width: 980, height: 720), scheme: .light,
                               makeView: { AnyView(PreferencesView()) }),
        NativeReferenceFixture(name: "preferences-dark", size: CGSize(width: 980, height: 720), scheme: .dark,
                               makeView: { AnyView(PreferencesView()) }),
        NativeReferenceFixture(name: "capture-region", size: CGSize(width: 1120, height: 700), scheme: .dark,
                               makeView: { captureReferenceView(recording: false) }),
        NativeReferenceFixture(name: "recording-selector", size: CGSize(width: 1120, height: 700), scheme: .dark,
                               makeView: { captureReferenceView(recording: true, windowTarget: true) }),
        NativeReferenceFixture(name: "previews-collapsed", size: CGSize(width: 340, height: 264), scheme: .dark,
                               makeView: { previewReferenceView(artifacts: imageArtifacts, image: image, expanded: false) }),
        NativeReferenceFixture(name: "previews-collapsed-fanned", size: CGSize(width: 340, height: 264), scheme: .dark,
                               makeView: { previewReferenceView(artifacts: imageArtifacts, image: image, expanded: false, fanned: true) }),
        NativeReferenceFixture(name: "previews-expanded", size: CGSize(width: 340, height: 720), scheme: .dark,
                               makeView: { previewReferenceView(artifacts: imageArtifacts, image: image, expanded: true) }),
        NativeReferenceFixture(name: "preview-dust-layout", size: CGSize(width: 340, height: 264), scheme: .dark,
                               makeView: { previewReferenceView(artifacts: imageArtifacts, image: image, expanded: false, deleting: true) }),
        NativeReferenceFixture(name: "recording-hud", size: CGSize(width: 430, height: 102), scheme: .dark,
                               makeView: { recordingHUDReferenceView(countdown: false) }),
        NativeReferenceFixture(name: "recording-restart-countdown", size: CGSize(width: 430, height: 102), scheme: .dark,
                               makeView: { recordingHUDReferenceView(countdown: true) }),
        NativeReferenceFixture(name: "image-editor", size: CGSize(width: 1280, height: 760), scheme: .dark,
                               makeView: { AnyView(ImageEditorView(artifact: imageArtifacts[0])) }),
        NativeReferenceFixture(name: "recording-editor-layout", size: CGSize(width: 1100, height: 800), scheme: .dark,
                               makeView: { AnyView(RecordingEditorView(artifact: recording)) }),
        NativeReferenceFixture(name: "history", size: CGSize(width: 980, height: 720), scheme: .light,
                               makeView: { AnyView(HistoryView()) }),
        NativeReferenceFixture(name: "delete-confirmation", size: CGSize(width: 430, height: 210), scheme: .dark,
                               makeView: dialogReferenceView),
    ]
}
