import AppKit
import SwiftUI

private struct FeedbackCategory: Identifiable {
    let id: String
    let title: String
    let detail: String
}

@MainActor
private final class FeedbackModel: ObservableObject {
    @Published var category = "bug"
    @Published var message = ""
    @Published var contact = ""
    @Published var status = ""
    @Published var sending = false

    let categories = [
        FeedbackCategory(id: "bug", title: "Bug", detail: "Something is broken or unexpected"),
        FeedbackCategory(id: "idea", title: "Idea", detail: "A feature or improvement"),
        FeedbackCategory(id: "other", title: "Other", detail: "Anything else"),
    ]

    var placeholder: String {
        switch category {
        case "idea": return "What's the idea? What problem would it solve?"
        case "other": return "What would you like us to know?"
        default: return "What happened? What did you expect?"
        }
    }

    var canSubmit: Bool { !message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !sending }

    func submit() {
        guard canSubmit else { return }
        sending = true
        status = ""
        let submittedMessage = message
        let trimmedContact = contact.trimmingCharacters(in: .whitespacesAndNewlines)
        let draft: [String: Any] = [
            "category": category,
            "message": submittedMessage,
            "contact": trimmedContact.isEmpty ? NSNull() as Any : trimmedContact as Any,
        ]
        Backend.shared.call("feedback_submit", [
            "draft": draft,
            "context": Self.context,
        ]) { [weak self] result in
            guard let self else { return }
            self.sending = false
            switch result {
            case .success:
                if self.message == submittedMessage { self.message = "" }
                self.status = "Thanks — feedback sent."
            case .failure(let error):
                self.status = error.localizedDescription
            }
        }
    }

    static var context: [String: String] {
        let info = ProcessInfo.processInfo
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "unknown"
        return [
            "app_version": version,
            "os": "macos",
            "os_version": info.operatingSystemVersionString,
            "arch": architecture,
        ]
    }

    private static var architecture: String {
        #if arch(arm64)
        return "arm64"
        #elseif arch(x86_64)
        return "x86_64"
        #else
        return "unknown"
        #endif
    }
}

struct FeedbackView: View {
    @StateObject private var model = FeedbackModel()
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: NativeTheme.metric("s-6")) {
                VStack(alignment: .leading, spacing: NativeTheme.metric("s-2")) {
                    Text("CAPTURES").font(.caption.weight(.bold)).tracking(1.2)
                        .foregroundStyle(NativeTheme.muted(scheme))
                    Text("Send feedback").font(.system(size: NativeTheme.metric("text-2xl"), weight: .bold))
                    Text("Tell us what broke, what is missing, or what you wish worked better. Captures sends what you type here plus the app and system details listed below.")
                        .foregroundStyle(NativeTheme.muted(scheme)).fixedSize(horizontal: false, vertical: true)
                }

                card {
                    SectionTitle("Category")
                    HStack(spacing: NativeTheme.metric("s-3")) {
                        ForEach(model.categories) { category in
                            Button {
                                model.category = category.id
                            } label: {
                                VStack(alignment: .leading, spacing: 4) {
                                    Text(category.title).fontWeight(.semibold)
                                    Text(category.detail).font(.caption).foregroundStyle(NativeTheme.muted(scheme))
                                        .fixedSize(horizontal: false, vertical: true)
                                }
                                .frame(maxWidth: .infinity, minHeight: 54, alignment: .leading)
                                .padding(NativeTheme.metric("s-4"))
                                .background(model.category == category.id
                                    ? NativeTheme.color("surface-selected", scheme)
                                    : NativeTheme.canvas(scheme))
                                .clipShape(RoundedRectangle(cornerRadius: NativeTheme.metric("r-md")))
                                .overlay(RoundedRectangle(cornerRadius: NativeTheme.metric("r-md"))
                                    .stroke(model.category == category.id ? NativeTheme.accent : NativeTheme.border(scheme),
                                            lineWidth: model.category == category.id ? 2 : 1))
                            }
                            .buttonStyle(.plain)
                            .accessibilityValue(model.category == category.id ? "Selected" : "Not selected")
                        }
                    }
                    Text("Message").fontWeight(.medium)
                    ZStack(alignment: .topLeading) {
                        TextEditor(text: $model.message)
                            .font(.body).frame(minHeight: 150)
                            .scrollContentBackground(.hidden)
                            .padding(6)
                            .background(NativeTheme.field(scheme), in: RoundedRectangle(cornerRadius: NativeTheme.metric("r-md")))
                            .overlay(RoundedRectangle(cornerRadius: NativeTheme.metric("r-md")).stroke(NativeTheme.border(scheme)))
                        if model.message.isEmpty {
                            Text(model.placeholder).foregroundStyle(NativeTheme.muted(scheme)).padding(12).allowsHitTesting(false)
                        }
                    }
                    .onChange(of: model.message) { value in
                        if value.count > 8_000 { model.message = String(value.prefix(8_000)) }
                    }
                    Text("Contact  optional").fontWeight(.medium)
                    TextField("X handle, GitHub username, email…", text: $model.contact)
                        .textFieldStyle(.roundedBorder)
                        .onChange(of: model.contact) { value in
                            if value.count > 200 { model.contact = String(value.prefix(200)) }
                        }
                    Text("Optional — we may use this if we need to ask a follow-up question.")
                        .font(.caption).foregroundStyle(NativeTheme.muted(scheme))
                }
                .disabled(model.sending)

                card {
                    SectionTitle("Included automatically")
                    contextRow("App version", FeedbackModel.context["app_version"] ?? "unknown")
                    contextRow("System", [FeedbackModel.context["os"], FeedbackModel.context["os_version"], FeedbackModel.context["arch"]]
                        .compactMap { $0 }.joined(separator: " · "))
                }

                HStack {
                    Text(model.status).font(.callout)
                        .foregroundStyle(model.status.hasPrefix("Thanks") ? Color.green : NativeTheme.signal)
                    Spacer()
                    Button(model.sending ? "Sending…" : "Send feedback", action: model.submit)
                        .buttonStyle(CaptureButtonStyle(primary: true)).disabled(!model.canSubmit)
                }
            }
            .padding(NativeTheme.metric("s-8"))
            .frame(maxWidth: 660)
            .frame(maxWidth: .infinity)
        }
        .foregroundStyle(NativeTheme.text(scheme))
        .background(NativeTheme.canvas(scheme))
    }

    private func card<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: NativeTheme.metric("s-4"), content: content)
            .padding(NativeTheme.metric("s-6")).frame(maxWidth: .infinity, alignment: .leading)
            .background(NativeTheme.raised(scheme), in: RoundedRectangle(cornerRadius: NativeTheme.metric("r-lg")))
            .overlay(RoundedRectangle(cornerRadius: NativeTheme.metric("r-lg")).stroke(NativeTheme.border(scheme)))
    }

    private func contextRow(_ title: String, _ value: String) -> some View {
        HStack { Text(title).foregroundStyle(NativeTheme.muted(scheme)); Spacer(); Text(value).monospacedDigit() }
    }
}

@MainActor
private final class CrashDiagnosticsModel: ObservableObject {
    @Published var status = ""
    @Published var busy = false
    @Published var submitted = false

    let summary: String
    let hasExceptionEvidence: Bool

    init(preview: [String: Any]) {
        hasExceptionEvidence = preview["has_exception_evidence"] as? Bool ?? false
        let panicSummary = preview["rust_panic"] as? String
        let osSummary = preview["os_report"] as? String
        let evidence = [panicSummary, osSummary].compactMap { $0 }.filter { !$0.isEmpty }
        summary = evidence.isEmpty
            ? "The previous Captures session did not close cleanly. No panic or matching macOS exception report was found."
            : evidence.joined(separator: "\n\n")
    }

    func send() {
        guard !busy, !submitted else { return }
        busy = true
        status = ""
        Backend.shared.call("feedback_submit", [
            "draft": ["category": "crash", "message": summary, "contact": NSNull()],
            "context": FeedbackModel.context,
        ]) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success:
                self.submitted = true
                self.dismiss(sent: true)
            case .failure(let error):
                self.busy = false
                self.status = error.localizedDescription
            }
        }
    }

    func dismiss(sent: Bool = false) {
        guard !busy || sent else { return }
        busy = true
        Backend.shared.call("crash_dismiss") { [weak self] result in
            guard let self else { return }
            self.busy = false
            switch result {
            case .success:
                AppStore.shared.closeCrashDiagnostics()
            case .failure(let error):
                self.status = sent
                    ? "Feedback was sent, but retained evidence could not be dismissed: \(error.localizedDescription)"
                    : error.localizedDescription
            }
        }
    }
}

struct CrashDiagnosticsView: View {
    @StateObject private var model: CrashDiagnosticsModel
    @Environment(\.colorScheme) private var scheme

    init(preview: [String: Any]) {
        _model = StateObject(wrappedValue: CrashDiagnosticsModel(preview: preview))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: NativeTheme.metric("s-6")) {
            Image(systemName: "exclamationmark.arrow.triangle.2.circlepath")
                .font(.system(size: 24, weight: .semibold))
                .frame(width: 48, height: 48)
                .foregroundStyle(Color.black)
                .background(NativeTheme.accent, in: RoundedRectangle(cornerRadius: NativeTheme.metric("r-lg")))
            VStack(alignment: .leading, spacing: NativeTheme.metric("s-2")) {
                Text("CAPTURES").font(.caption.weight(.bold)).tracking(1.2)
                    .foregroundStyle(NativeTheme.muted(scheme))
                Text(model.hasExceptionEvidence ? "Captures closed unexpectedly" : "Captures did not close cleanly")
                    .font(.system(size: NativeTheme.metric("text-2xl"), weight: .bold))
                Text("Review the local, redacted summary below. Nothing is sent unless you choose Send report; captures and raw diagnostic files are never attached.")
                    .foregroundStyle(NativeTheme.muted(scheme)).fixedSize(horizontal: false, vertical: true)
            }
            ScrollView {
                Text(model.summary)
                    .font(.system(.callout, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(NativeTheme.metric("s-5"))
            }
            .background(NativeTheme.field(scheme), in: RoundedRectangle(cornerRadius: NativeTheme.metric("r-md")))
            .overlay(RoundedRectangle(cornerRadius: NativeTheme.metric("r-md")).stroke(NativeTheme.border(scheme)))
            if !model.status.isEmpty {
                Text(model.status).font(.callout).foregroundStyle(NativeTheme.signal)
            }
            HStack {
                Button("Dismiss") { model.dismiss() }
                    .buttonStyle(CaptureButtonStyle()).disabled(model.busy)
                Spacer()
                Button(model.busy && !model.submitted ? "Sending…" : "Send report") { model.send() }
                    .buttonStyle(CaptureButtonStyle(primary: true))
                    .disabled(model.busy || model.submitted)
            }
        }
        .padding(NativeTheme.metric("s-8"))
        .foregroundStyle(NativeTheme.text(scheme))
        .background(NativeTheme.canvas(scheme))
    }
}
