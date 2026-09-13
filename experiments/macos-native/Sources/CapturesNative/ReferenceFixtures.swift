import AppKit
import SwiftUI

struct NativeReferenceFixture {
    let name: String
    let size: CGSize
    let scheme: ColorScheme
    let makeView: @MainActor () -> AnyView
}

@MainActor private var nativeReferenceEstimateLabel: String?
@MainActor private var nativeReferenceEstimateState: String?
@MainActor private var nativeReferenceCombineControlsVisible: Bool?
@MainActor private var nativeReferenceLayerCanvasOffscreen: Bool?

@MainActor
func resolvedNativeReferenceEstimateLabel() -> String? { nativeReferenceEstimateLabel }

@MainActor
func resolvedNativeReferenceEstimateState() -> String? { nativeReferenceEstimateState }

@MainActor
func resolvedNativeReferenceCombineControlsVisible() -> Bool? { nativeReferenceCombineControlsVisible }

@MainActor
func resolvedNativeReferenceLayerCanvasOffscreen() -> Bool? { nativeReferenceLayerCanvasOffscreen }

@MainActor
func recordNativeReferenceCombineControlsVisible(_ visible: Bool) {
    nativeReferenceCombineControlsVisible = visible
}

@MainActor
func recordNativeReferenceLayerCanvasOffscreen(_ offscreen: Bool) {
    nativeReferenceLayerCanvasOffscreen = offscreen
}

@MainActor
func nativeReferenceFixtures(
    imageURL: URL,
    recordingURL: URL,
    includeAnimationCapture: Bool
) -> [NativeReferenceFixture] {
    nativeReferenceEstimateLabel = nil
    nativeReferenceEstimateState = nil
    nativeReferenceCombineControlsVisible = nil
    nativeReferenceLayerCanvasOffscreen = nil
    let image = NSImage(contentsOf: imageURL)!
    let imageArtifacts = (0..<4).map { _ in
        Artifact(path: imageURL.path, kind: "image", width: 960, height: 540)
    }
    let recording = Artifact(path: recordingURL.path, kind: "video", width: 960, height: 540)

    AppStore.shared.artifacts = [imageArtifacts[0], recording, imageArtifacts[1], imageArtifacts[2]]
    var fixtures = [
        NativeReferenceFixture(name: "onboarding", size: CGSize(width: 760, height: 620), scheme: .light,
                               makeView: { AnyView(OnboardingView()) }),
        NativeReferenceFixture(name: "feedback", size: CGSize(width: 720, height: 700), scheme: .light,
                               makeView: { AnyView(FeedbackView()) }),
        NativeReferenceFixture(name: "crash-consent", size: CGSize(width: 720, height: 620), scheme: .light,
                               makeView: {
                                   AnyView(CrashDiagnosticsView(preview: [
                                       "unclean_exit": true,
                                       "has_exception_evidence": true,
                                       "rust_panic": "Captures stopped in editor_worker at src/editor.rs:184. Personal paths and values were redacted locally.",
                                   ]))
                               }),
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
        NativeReferenceFixture(name: "recording-hud", size: CGSize(width: 430, height: 102), scheme: .dark,
                               makeView: { recordingHUDReferenceView(countdown: false) }),
        NativeReferenceFixture(name: "recording-restart-countdown", size: CGSize(width: 430, height: 102), scheme: .dark,
                               makeView: { recordingHUDReferenceView(countdown: true) }),
        NativeReferenceFixture(name: "image-editor", size: CGSize(width: 1280, height: 760), scheme: .dark,
                               makeView: { imageEditorReferenceView(artifact: imageArtifacts[0], state: "default") }),
        NativeReferenceFixture(name: "image-editor-shapes", size: CGSize(width: 1280, height: 760), scheme: .dark,
                               makeView: { imageEditorReferenceView(artifact: imageArtifacts[0], state: "shapes") }),
        NativeReferenceFixture(name: "image-editor-properties", size: CGSize(width: 1280, height: 760), scheme: .dark,
                               makeView: { imageEditorReferenceView(artifact: imageArtifacts[0], state: "properties") }),
        NativeReferenceFixture(name: "image-editor-overflow", size: CGSize(width: 1280, height: 760), scheme: .dark,
                               makeView: { imageEditorReferenceView(artifact: imageArtifacts[0], state: "overflow") }),
        NativeReferenceFixture(name: "image-editor-snap-guides", size: CGSize(width: 1280, height: 760), scheme: .dark,
                               makeView: { imageEditorReferenceView(artifact: imageArtifacts[0], state: "snap-guides") }),
        NativeReferenceFixture(name: "image-editor-viewport-pan-zoom", size: CGSize(width: 1280, height: 760), scheme: .dark,
                               makeView: { imageEditorReferenceView(artifact: imageArtifacts[0], state: "viewport") }),
        NativeReferenceFixture(name: "image-editor-layer-compositing", size: CGSize(width: 1280, height: 900), scheme: .dark,
                               makeView: { imageEditorReferenceView(artifact: imageArtifacts[0], state: "layers") }),
        NativeReferenceFixture(name: "image-editor-layer-combine-controls", size: CGSize(width: 1280, height: 900), scheme: .dark,
                               makeView: { imageEditorReferenceView(artifact: imageArtifacts[0], state: "layer-combine") }),
        NativeReferenceFixture(name: "image-editor-text-styles", size: CGSize(width: 1280, height: 900), scheme: .dark,
                               makeView: { imageEditorReferenceView(artifact: imageArtifacts[0], state: "text-styles") }),
        NativeReferenceFixture(name: "image-editor-text-defaults", size: CGSize(width: 1280, height: 760), scheme: .dark,
                               makeView: { imageEditorReferenceView(artifact: imageArtifacts[0], state: "text-defaults") }),
        NativeReferenceFixture(name: "image-editor-erase", size: CGSize(width: 1280, height: 760), scheme: .dark,
                               makeView: { imageEditorReferenceView(artifact: imageArtifacts[0], state: "erase") }),
        NativeReferenceFixture(name: "image-editor-wand", size: CGSize(width: 1280, height: 760), scheme: .dark,
                               makeView: { imageEditorReferenceView(artifact: imageArtifacts[0], state: "wand") }),
        NativeReferenceFixture(name: "recording-editor-layout", size: CGSize(width: 1100, height: 800), scheme: .dark,
                               makeView: { AnyView(RecordingEditorView(artifact: recording)) }),
        NativeReferenceFixture(name: "recording-editor-quality-estimate", size: CGSize(width: 760, height: 520), scheme: .dark,
                               makeView: {
                                   AnyView(RecordingEditorView(
                                       artifact: recording, referenceQualityOnly: true,
                                       onEstimateReady: { nativeReferenceEstimateLabel = $0 },
                                       onEstimateStateChanged: { nativeReferenceEstimateState = $0 }
                                   ))
                               }),
        NativeReferenceFixture(name: "history", size: CGSize(width: 980, height: 720), scheme: .light,
                               makeView: { AnyView(HistoryView()) }),
        NativeReferenceFixture(name: "delete-confirmation", size: CGSize(width: 430, height: 210), scheme: .dark,
                               makeView: dialogReferenceView),
        NativeReferenceFixture(name: "control-states", size: CGSize(width: 520, height: 300), scheme: .dark,
                               makeView: { AnyView(ControlStatesReferenceView()) }),
    ]
    if includeAnimationCapture {
        // One deleting card and no intact cards underneath: a missing Core
        // Animation presentation cannot be mistaken for a successful effect.
        fixtures.append(NativeReferenceFixture(
            name: "preview-dust-compositor-source",
            size: CGSize(width: 340, height: 264),
            scheme: .dark,
            makeView: {
                previewReferenceView(
                    artifacts: [imageArtifacts[0]], image: image,
                    expanded: false, deleting: true
                )
            }
        ))
    }
    return fixtures
}

private struct ControlStatesReferenceView: View {
    @State private var switchOn = true
    @State private var switchOff = false
    @State private var checked = true
    @State private var unchecked = false
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        VStack(alignment: .leading, spacing: NativeTheme.metric("s-6")) {
            SectionTitle("On, off & unavailable", subtitle: "Captures-owned controls")
            Group {
                CaptureToggleRow(title: "Show cursor", isOn: $switchOn)
                CaptureToggleRow(title: "Show clicks", isOn: $switchOff)
                CaptureToggleRow(title: "Show clicks (cursor off)", isOn: $switchOff).disabled(true)
                Divider()
                CaptureCheckboxRow(title: "Crop recording", isOn: $checked)
                CaptureCheckboxRow(title: "Lock aspect ratio", isOn: $unchecked)
                CaptureCheckboxRow(title: "Lock aspect ratio (crop off)", isOn: $unchecked).disabled(true)
            }
            .frame(maxWidth: 360)
        }
        .padding(NativeTheme.metric("s-8"))
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .foregroundStyle(NativeTheme.text(scheme))
        .background(NativeTheme.canvas(scheme))
    }
}
