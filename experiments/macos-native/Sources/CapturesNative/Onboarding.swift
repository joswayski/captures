import AppKit
import SwiftUI

@MainActor
final class OnboardingModel: ObservableObject {
    @Published var screenGranted = false
    @Published var microphoneStatus = "unknown"
    @Published var requestedScreenThisLaunch = false
    @Published var busy = false
    @Published var error = ""

    func refresh() {
        Backend.shared.call("describe", ["request_permission": false]) { [weak self] result in
            guard let self else { return }
            self.screenGranted = (try? result.get()) != nil
        }
        Backend.shared.call("microphone_permission", ["request": false]) { [weak self] result in
            guard let self else { return }
            if case .success(let value) = result {
                self.microphoneStatus = value["status"] as? String ?? "unknown"
            }
        }
    }

    func requestScreen() {
        busy = true
        error = ""
        requestedScreenThisLaunch = true
        Backend.shared.call("describe", ["request_permission": true]) { [weak self] result in
            guard let self else { return }
            self.busy = false
            switch result {
            case .success:
                self.screenGranted = true
            case .failure(let error):
                self.screenGranted = false
                let message = error.localizedDescription
                if (message.contains("permission was requested") || message.contains("permission was denied")),
                   let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture") {
                    NSWorkspace.shared.open(url)
                } else {
                    self.error = message
                }
            }
        }
    }

    func requestMicrophone() {
        busy = true
        error = ""
        Backend.shared.call("microphone_permission", ["request": true]) { [weak self] result in
            guard let self else { return }
            self.busy = false
            switch result {
            case .success(let value):
                self.microphoneStatus = value["status"] as? String ?? "unknown"
                if self.microphoneStatus == "denied",
                   let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone") {
                    NSWorkspace.shared.open(url)
                }
            case .failure(let error):
                self.error = error.localizedDescription
            }
        }
    }
}

struct OnboardingView: View {
    @StateObject private var model = OnboardingModel()
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: NativeTheme.metric("s-7")) {
                VStack(alignment: .leading, spacing: NativeTheme.metric("s-3")) {
                    Image(systemName: "viewfinder")
                        .font(.system(size: 24, weight: .semibold))
                        .frame(width: 48, height: 48)
                        .foregroundStyle(Color.black)
                        .background(NativeTheme.accent, in: RoundedRectangle(cornerRadius: NativeTheme.metric("r-lg")))
                    Text("WELCOME TO CAPTURES")
                        .font(.caption.weight(.bold)).tracking(1.2)
                        .foregroundStyle(NativeTheme.muted(scheme))
                    Text("Required permissions")
                        .font(.system(size: NativeTheme.metric("text-2xl"), weight: .bold))
                    Text("Screen Recording access lets Captures read your display. Only the content you choose to capture is saved, and images are never uploaded automatically.")
                        .foregroundStyle(NativeTheme.muted(scheme))
                        .fixedSize(horizontal: false, vertical: true)
                }

                VStack(spacing: 0) {
                    permissionRow(
                        icon: "display", title: "Screen capture",
                        description: screenDescription,
                        status: model.screenGranted ? "Granted" : model.requestedScreenThisLaunch ? "Restart required" : nil,
                        action: model.screenGranted ? nil : model.requestedScreenThisLaunch ? "Open Settings" : "Allow access",
                        perform: model.requestScreen
                    )
                    Divider().padding(.leading, 64)
                    permissionRow(
                        icon: "mic", title: "Microphone", optional: true,
                        description: microphoneDescription,
                        status: model.microphoneStatus == "authorized" ? "Granted" : nil,
                        action: model.microphoneStatus == "authorized" ? nil : "Allow microphone",
                        perform: model.requestMicrophone
                    )
                }
                .background(NativeTheme.raised(scheme), in: RoundedRectangle(cornerRadius: NativeTheme.metric("r-xl")))
                .overlay(RoundedRectangle(cornerRadius: NativeTheme.metric("r-xl")).stroke(NativeTheme.border(scheme)))

                if !model.error.isEmpty {
                    Text(model.error).foregroundStyle(NativeTheme.signal).font(.callout)
                }
                HStack {
                    Spacer()
                    if model.requestedScreenThisLaunch, !model.screenGranted {
                        Button("Restart Captures", action: restart)
                            .buttonStyle(CaptureButtonStyle(primary: true)).disabled(model.busy)
                    } else {
                        Button("Start capturing") { AppStore.shared.completeOnboarding() }
                            .buttonStyle(CaptureButtonStyle(primary: true))
                            .disabled(!model.screenGranted || model.busy)
                    }
                }
            }
            .padding(NativeTheme.metric("s-8"))
            .frame(maxWidth: 680)
            .frame(maxWidth: .infinity)
        }
        .foregroundStyle(NativeTheme.text(scheme))
        .background(NativeTheme.canvas(scheme))
        .onAppear { model.refresh() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            model.refresh()
        }
    }

    private var screenDescription: String {
        if model.screenGranted {
            return "Captures can read the display so it can save the display, window, or region you choose. Only that chosen capture is saved."
        }
        if model.requestedScreenThisLaunch {
            return "Turn the switch on next to this copy of Captures, then restart. A local build is a different row from a downloaded app."
        }
        return "Allow screen access so Captures can save the display, window, or region you select."
    }

    private var microphoneDescription: String {
        switch model.microphoneStatus {
        case "authorized": return "macOS will not ask again. Turn the microphone on when you start a recording."
        case "denied": return "Turn Captures on in Microphone settings. Microphone recording is optional."
        default: return "Allow it now so a recording does not pause to ask, or wait until you pick a mic."
        }
    }

    private func permissionRow(
        icon: String, title: String, optional: Bool = false, description: String,
        status: String?, action: String?, perform: @escaping () -> Void
    ) -> some View {
        HStack(alignment: .center, spacing: NativeTheme.metric("s-5")) {
            Image(systemName: icon).font(.system(size: 20, weight: .medium))
                .frame(width: 40, height: 40)
                .background(NativeTheme.field(scheme), in: RoundedRectangle(cornerRadius: NativeTheme.metric("r-md")))
            VStack(alignment: .leading, spacing: NativeTheme.metric("s-2")) {
                HStack(spacing: NativeTheme.metric("s-2")) {
                    Text(title).fontWeight(.semibold)
                    if optional { Text("Optional").font(.caption).foregroundStyle(NativeTheme.muted(scheme)) }
                }
                Text(description).font(.callout).foregroundStyle(NativeTheme.muted(scheme))
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: NativeTheme.metric("s-4"))
            if let status {
                Label(status, systemImage: status == "Granted" ? "checkmark" : "arrow.clockwise")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(status == "Granted" ? Color.green : NativeTheme.muted(scheme))
            }
            if let action {
                Button(action, action: perform).buttonStyle(CaptureButtonStyle()).disabled(model.busy)
            }
        }
        .padding(NativeTheme.metric("s-6"))
    }

    private func restart() {
        guard let delegate = NSApp.delegate as? AppDelegate else {
            model.error = "Couldn’t restart Captures. Quit and reopen it to finish granting access."
            return
        }
        delegate.requestRestart()
    }
}
