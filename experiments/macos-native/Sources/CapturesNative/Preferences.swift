import AppKit
import ServiceManagement
import SwiftUI

struct PreferencesView: View {
    @ObservedObject private var store = AppStore.shared
    @Environment(\.colorScheme) private var scheme
    @State private var selected = "Appearance"
    @State private var query = ""
    @State private var devices: [[String: String]] = []
    @State private var microphonePermission = "unknown"
    @State private var loginEnabled = SMAppService.mainApp.status == .enabled
    @FocusState private var finding: Bool
    private let sections = ["Appearance", "Capture", "Shortcuts", "Recording", "GIF export", "Updates", "About"]

    var body: some View {
        ScrollViewReader { proxy in
            HStack(spacing: 0) {
                VStack(alignment: .leading, spacing: NativeTheme.metric("s-6")) {
                    HStack(spacing: NativeTheme.metric("s-4")) {
                        Image(systemName: "viewfinder").padding(NativeTheme.metric("s-3"))
                            .background(NativeTheme.accent).foregroundColor(.black)
                            .cornerRadius(NativeTheme.metric("r-md"))
                        Text("Captures").fontWeight(.semibold)
                    }.padding(.horizontal, NativeTheme.metric("s-3"))
                    VStack(spacing: 1) {
                        ForEach(sections, id: \.self) { section in
                            Button {
                                selected = section; query = ""
                                withAnimation(NativeTheme.motion) { proxy.scrollTo(section, anchor: .top) }
                            } label: {
                                Text(section).frame(maxWidth: .infinity, alignment: .leading)
                                    .fontWeight(.medium)
                                    .foregroundColor(NativeTheme.color(selected == section ? "text" : "text-subtle", scheme))
                                    .padding(.horizontal, NativeTheme.metric("s-4"))
                                    .frame(height: NativeTheme.metric("h-md"))
                                    .background(selected == section ? NativeTheme.color("surface-active", scheme) : .clear)
                                    .cornerRadius(NativeTheme.metric("r-md"))
                            }.buttonStyle(.plain)
                        }
                    }
                    Spacer()
                    Text("Native experiment").font(.system(size: NativeTheme.metric("text-xs")))
                        .foregroundColor(NativeTheme.muted(scheme))
                }
                .padding(.vertical, NativeTheme.metric("s-6")).padding(.horizontal, NativeTheme.metric("s-5"))
                .frame(width: 196).background(NativeTheme.color("surface-sunken", scheme))
                Divider()
                VStack(spacing: 0) {
                    header
                    Divider()
                    ScrollView {
                        VStack(alignment: .leading, spacing: NativeTheme.metric("s-6")) {
                            ForEach(sections, id: \.self) { section in
                                sectionBody(section).id(section)
                            }
                        }.padding(NativeTheme.metric("s-8")).frame(maxWidth: 720)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
            }
            .foregroundColor(NativeTheme.text(scheme)).background(NativeTheme.canvas(scheme))
            .onChange(of: query) { value in
                let keywords = ["Appearance":"theme accent color dark light system",
                                "Capture":"cursor freeze countdown automatic preview clipboard folder save format",
                                "Shortcuts":"keyboard keys",
                                "Recording":"microphone audio fps resolution controls video",
                                "GIF export":"gif width colors frames",
                                "Updates":"preview version", "About":"permission startup login"]
                guard !value.isEmpty, let section = sections.first(where: {
                    ($0 + " " + (keywords[$0] ?? "")).localizedCaseInsensitiveContains(value)
                }) else { return }
                selected = section
                proxy.scrollTo(section, anchor: .top)
            }
            .background(Button("") { finding = true }.keyboardShortcut("f", modifiers: .command).hidden())
        }
    }

    private var header: some View {
        HStack(spacing: NativeTheme.metric("s-6")) {
            VStack(alignment: .leading, spacing: NativeTheme.metric("s-1")) {
                Text("Preferences").font(.system(size: NativeTheme.metric("text-xl"), weight: .semibold))
                Text(store.saveStatus).font(.system(size: NativeTheme.metric("text-sm")))
                    .foregroundColor(NativeTheme.color("text-subtle", scheme))
            }
            Spacer()
            if finding || !query.isEmpty {
                TextField("Find a setting…", text: $query).textFieldStyle(.roundedBorder)
                    .frame(width: 150).focused($finding).accessibilityLabel("Find a setting")
            }
            Button("Capture History…") { store.showHistory() }.buttonStyle(CaptureButtonStyle())
        }.padding(.horizontal, NativeTheme.metric("s-8")).padding(.vertical, NativeTheme.metric("s-6"))
    }

    @ViewBuilder private func sectionBody(_ section: String) -> some View {
        switch section {
        case "Appearance": appearance
        case "Capture": capture
        case "Shortcuts": shortcuts
        case "Recording": recording
        case "GIF export": gif
        case "Updates":
            card {
                SectionTitle("Updates")
                setting("Native updates", "This separate experiment has no update channel. The shipping Tauri Preview remains unchanged.") {
                    Button("Check for updates") {}.disabled(true)
                }
            }
        default: about
        }
    }

    private var appearance: some View {
        card {
            SectionTitle("Appearance", subtitle: "One look across every Captures window. Capture overlays stay dark so they read on any desktop.")
            setting("Interface theme", "Follow the system setting, or lock Captures to light or dark.") {
                HStack(spacing: NativeTheme.metric("s-1")) {
                    ForEach(["system", "light", "dark"], id: \.self) { appearance in
                        Button { store.settings.appearance = appearance } label: {
                            Text(appearance.capitalized).padding(.horizontal, NativeTheme.metric("s-5"))
                                .frame(height: NativeTheme.metric("h-xs"))
                                .background(store.settings.appearance == appearance ? NativeTheme.raised(scheme) : .clear)
                                .cornerRadius(NativeTheme.metric("r-sm"))
                        }.buttonStyle(.plain)
                    }
                }.padding(NativeTheme.metric("s-2")).background(NativeTheme.color("surface-sunken", scheme))
                    .cornerRadius(NativeTheme.metric("r-lg"))
            }
            Divider()
            VStack(alignment: .leading, spacing: NativeTheme.metric("s-4")) {
                Text("Accent color").fontWeight(.medium)
                Text("Used for the capture action, selection, and focus. Status colors keep their meaning.")
                    .font(.system(size: NativeTheme.metric("text-sm")))
                    .foregroundColor(NativeTheme.color("text-subtle", scheme))
                    .frame(maxWidth: NativeTheme.settingsCopyWidth, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
                LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: NativeTheme.metric("s-2")), count: 5), spacing: NativeTheme.metric("s-2")) {
                    ForEach(["mustard", "ember", "rose", "violet", "cobalt", "aqua", "mint", "lime", "mono", "custom"], id: \.self) { name in
                        paletteButton(name)
                    }
                }
                if store.settings.theme == "custom" {
                    HStack(spacing: NativeTheme.metric("s-5")) {
                        colorField("Accent", key: \.accentHex)
                        colorField("Recording / destructive", key: \.signalHex)
                    }
                }
            }
        }
    }

    private var capture: some View {
        card {
            SectionTitle("Capture", subtitle: "Where captures go and what happens right after you take one.")
            VStack(alignment: .leading, spacing: NativeTheme.metric("s-4")) {
                Text("Save captures to").fontWeight(.medium)
                HStack {
                    TextField("Save captures to", text: $store.settings.outputDirectory).textFieldStyle(.roundedBorder)
                    Button("Choose…") { chooseDirectory() }.buttonStyle(CaptureButtonStyle())
                }
            }
            Divider()
            toggle("Copy screenshots to clipboard", "Also keep the saved file and a history entry.", $store.settings.autoCopy)
            Divider()
            toggle("Start capture as soon as a target is selected", "Drawing a region, choosing a window, or clicking Full screen immediately starts the capture. When this is off, press Enter in the capture menu to confirm.", $store.settings.autoStart)
            Divider()
            toggle("Show mini previews", "Keep recent captures in a floating stack.", $store.settings.showPreviews)
            setting("Mini preview corner") {
                CaptureChoice(title: "Mini preview corner", selection: $store.settings.previewPlacement, options: [
                    CaptureOption(label: "Bottom left", value: "bottom-left"),
                    CaptureOption(label: "Bottom right", value: "bottom-right"),
                    CaptureOption(label: "Top left", value: "top-left"),
                    CaptureOption(label: "Top right", value: "top-right"),
                ]).frame(width: 170)
            }
            toggle("Freeze screen", "Keep a still desktop while selecting a screenshot region.", $store.settings.freezeScreen)
            toggle("Include cursor", "Show the pointer in screenshots.", $store.settings.showCursor)
            setting("Screenshot countdown") { seconds($store.settings.screenshotCountdown) }
            setting("Default screenshot format", "The captured original stays PNG until you save or export from the editor.") {
                CaptureSegments(selection: $store.settings.screenshotFormat, options: [
                    CaptureOption(label: "PNG", value: "png"), CaptureOption(label: "JPEG", value: "jpeg"),
                    CaptureOption(label: "WebP", value: "webp"),
                ]).frame(width: 180)
            }
        }
    }

    private var shortcuts: some View {
        card {
            SectionTitle("Shortcuts")
            ForEach($store.settings.shortcuts) { $shortcut in
                setting(shortcut.title) { ShortcutField(shortcut: $shortcut).frame(width: 180, height: NativeTheme.metric("h-md")) }
            }
            Text("Click a shortcut, then press a new combination. Escape cancels. macOS Screenshot bindings are never changed automatically by this experiment.")
                .font(.system(size: NativeTheme.metric("text-sm"))).foregroundColor(NativeTheme.muted(scheme))
            if !store.shortcutWarning.isEmpty { Text(store.shortcutWarning).foregroundColor(NativeTheme.signal) }
            Button("Restore defaults") { store.settings.shortcuts = Shortcut.defaults }.buttonStyle(CaptureButtonStyle())
        }
    }

    private var recording: some View {
        card {
            SectionTitle("Recording")
            setting("Frame rate") {
                CaptureSegments(selection: $store.settings.videoFPS,
                                options: [15, 30, 60].map { CaptureOption(label: "\($0)", value: $0) })
                    .frame(width: 140)
            }
            setting("Maximum resolution") {
                CaptureChoice(title: "Maximum resolution", selection: $store.settings.maxResolution, options: [
                    CaptureOption(label: "Original", value: "original"),
                    CaptureOption(label: "1080p", value: "p1080"),
                    CaptureOption(label: "720p", value: "p720"),
                ]).frame(width: 150)
            }
            setting("Recording countdown") { seconds($store.settings.recordingCountdown) }
            toggle("Desktop audio", "Include sound playing on this Mac.", $store.settings.systemAudio)
            setting("Microphone") {
                HStack {
                    CaptureChoice(title: "Microphone", selection: $store.settings.microphoneID,
                                  options: [CaptureOption(label: "None", value: "")] + devices.map {
                                    CaptureOption(label: $0["name"] ?? "Microphone", value: $0["id"] ?? "")
                                  }).frame(width: 180)
                    Button("Refresh") { loadDevices() }.buttonStyle(CaptureButtonStyle())
                }
            }
            .onChange(of: store.settings.microphoneID) { identifier in
                if !identifier.isEmpty { loadDevices(requestPermission: true) }
            }
            if microphonePermission == "denied" {
                Text("Microphone access is denied. Allow Captures in System Settings › Privacy & Security › Microphone, then try again.")
                    .font(.system(size: NativeTheme.metric("text-sm")))
                    .foregroundColor(NativeTheme.signal)
                Button("Open Microphone Settings") {
                    NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")!)
                }.buttonStyle(CaptureButtonStyle())
            } else if microphonePermission == "not_determined" {
                Button("Allow microphone access") { loadDevices(requestPermission: true) }
                    .buttonStyle(CaptureButtonStyle())
            }
            toggle("Exclude Captures controls", "Keep this experiment's windows out of recordings.", $store.settings.excludeControls)
            toggle("Open editor after recording", "Review, trim, crop, and export the recording.", $store.settings.openEditorAfterRecording)
        }.onAppear { loadDevices() }
    }

    private var gif: some View {
        card {
            SectionTitle("GIF export")
            setting("Frame rate") {
                CaptureChoice(title: "GIF frame rate", selection: $store.settings.gifFPS,
                              options: [8, 10, 12, 15, 20, 24, 30].map { CaptureOption(label: "\($0) fps", value: $0) })
                    .frame(width: 150)
            }
            setting("Maximum width") {
                CaptureChoice(title: "GIF width", selection: $store.settings.gifWidth,
                              options: [320, 480, 640, 800, 1280].map { CaptureOption(label: "\($0) px", value: $0) })
                    .frame(width: 140)
            }
            setting("Palette colors") {
                CaptureSegments(selection: $store.settings.gifColors,
                                options: [64, 128, 256].map { CaptureOption(label: "\($0)", value: $0) })
                    .frame(width: 140)
            }
            Text("GIF captures have no audio.").foregroundColor(NativeTheme.muted(scheme))
        }
    }

    private var about: some View {
        card {
            SectionTitle("About")
            Text("Captures · Native macOS experiment").fontWeight(.semibold)
            Text("SwiftUI and AppKit, with the existing Rust capture and media engines. No webview. Separate settings, history, and permissions from the shipping Preview.")
                .foregroundColor(NativeTheme.muted(scheme))
            setting("Screen recording permission", "macOS must allow this app to capture the desktop.") {
                Button("Request permission") {
                    Backend.shared.call("describe", ["request_permission": true]) { result in
                        if case .failure(let error) = result { store.report(error) }
                    }
                }.buttonStyle(CaptureButtonStyle())
            }
            setting("Launch at login", "Uses macOS Login Items. Register the built app from a stable location.") {
                CaptureToggle(title: "Launch at login", isOn: Binding(get: { loginEnabled }, set: { enabled in
                    do {
                        if enabled { try SMAppService.mainApp.register() }
                        else { try SMAppService.mainApp.unregister() }
                        loginEnabled = SMAppService.mainApp.status == .enabled
                    } catch { store.report(error) }
                }))
            }
            Button("Open experiment data folder") { NSWorkspace.shared.open(AppStore.dataDirectory) }.buttonStyle(CaptureButtonStyle())
        }
    }

    private func loadDevices(requestPermission: Bool = false) {
        Backend.shared.call("microphone_permission", ["request": requestPermission]) { result in
            switch result {
            case .success(let value):
                microphonePermission = value["status"] as? String ?? "unknown"
                devices = (value["devices"] as? [[String: Any]] ?? []).map { ["id": $0["id"] as? String ?? "", "name": $0["name"] as? String ?? "Microphone"] }
                if requestPermission && microphonePermission != "authorized" { store.settings.microphoneID = "" }
            case .failure(let error):
                if requestPermission { store.settings.microphoneID = "" }
                store.report(error)
            }
        }
    }
    private func chooseDirectory() {
        let panel = NSOpenPanel(); panel.canChooseFiles = false; panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        if panel.runModal() == .OK, let url = panel.url { store.settings.outputDirectory = url.path }
    }
    private func colorField(_ title: String, key: WritableKeyPath<NativeSettings, String>) -> some View {
        HStack(spacing: NativeTheme.metric("s-3")) {
            Circle().fill(Color(css: store.settings[keyPath: key])).frame(width: 18, height: 18)
            Text(title).font(.system(size: NativeTheme.metric("text-sm")))
            TextField("#RRGGBB", text: Binding(
                get: { store.settings[keyPath: key] },
                set: { value in
                    let candidate = value.trimmingCharacters(in: .whitespacesAndNewlines)
                    if candidate.range(of: "^#[0-9a-fA-F]{6}$", options: .regularExpression) != nil {
                        store.settings[keyPath: key] = candidate.lowercased()
                    }
                }
            ))
            .textFieldStyle(.roundedBorder)
            .frame(width: 86)
        }
    }
    private func seconds(_ binding: Binding<Int>) -> some View {
        CaptureChoice(title: "Countdown", selection: binding, options: [
            CaptureOption(label: "None", value: 0),
            CaptureOption(label: "3 seconds", value: 3),
            CaptureOption(label: "5 seconds", value: 5),
            CaptureOption(label: "10 seconds", value: 10),
        ]).frame(width: 150)
    }
    private func toggle(_ title: String, _ description: String, _ binding: Binding<Bool>) -> some View {
        setting(title, description, copyWidth: nil) { CaptureToggle(title: title, isOn: binding) }
    }

    private func paletteButton(_ name: String) -> some View {
        let palette = NativeTheme.palettes[name]
        let accent = Color(css: palette?["accent"] ?? store.settings.accentHex)
        let signal = Color(css: palette?["signal"] ?? store.settings.signalHex)
        return Button {
            store.settings.theme = name
            if let palette {
                store.settings.accentHex = palette["accent"]!
                store.settings.signalHex = palette["signal"]!
            }
        } label: {
            HStack(spacing: NativeTheme.metric("s-3")) {
                ZStack {
                    accent
                    Path { path in
                        path.move(to: CGPoint(x: 18, y: 0)); path.addLine(to: CGPoint(x: 18, y: 18))
                        path.addLine(to: CGPoint(x: 9, y: 18)); path.closeSubpath()
                    }.fill(signal)
                }.frame(width: NativeTheme.metric("text-xl"), height: NativeTheme.metric("text-xl"))
                    .cornerRadius(NativeTheme.metric("r-sm"))
                Text(name.capitalized).font(.system(size: NativeTheme.metric("text-sm"))).lineLimit(1)
                Spacer(minLength: 0)
                if store.settings.theme == name { Image(systemName: "checkmark").font(.system(size: NativeTheme.metric("text-2xs"))) }
            }.padding(NativeTheme.metric("s-3"))
                .frame(maxWidth: .infinity, minHeight: NativeTheme.metric("h-md") + NativeTheme.metric("s-1"), alignment: .leading)
                .foregroundColor(NativeTheme.color(store.settings.theme == name ? "text" : "text-muted", scheme))
                .background(store.settings.theme == name ? NativeTheme.field(scheme) : .clear)
                .cornerRadius(NativeTheme.metric("r-md"))
                .overlay(RoundedRectangle(cornerRadius: NativeTheme.metric("r-md")).strokeBorder(store.settings.theme == name ? NativeTheme.border(scheme) : .clear))
        }.buttonStyle(.plain).accessibilityLabel(name.capitalized)
            .accessibilityAddTraits(store.settings.theme == name ? .isSelected : [])
    }

    private func setting<Control: View>(_ title: String, _ description: String = "", copyWidth: CGFloat? = NativeTheme.settingsCopyWidth, @ViewBuilder control: () -> Control) -> some View {
        HStack(spacing: NativeTheme.metric("s-8")) {
            VStack(alignment: .leading, spacing: NativeTheme.metric("s-2")) {
                Text(title).fontWeight(.medium)
                if !description.isEmpty { Text(description).font(.system(size: NativeTheme.metric("text-sm")))
                    .foregroundColor(NativeTheme.color("text-subtle", scheme))
                    .frame(maxWidth: copyWidth, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true) }
            }
            Spacer(minLength: 0)
            control().accessibilityLabel(title)
        }.padding(.vertical, NativeTheme.metric("s-2"))
    }
    private func card<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: NativeTheme.metric("s-5"), content: content)
            .padding(NativeTheme.metric("s-6")).frame(maxWidth: .infinity, alignment: .leading)
            .background(NativeTheme.raised(scheme)).cornerRadius(NativeTheme.metric("r-xl"))
            .overlay(RoundedRectangle(cornerRadius: NativeTheme.metric("r-xl")).strokeBorder(NativeTheme.color("border-subtle", scheme)))
            .shadow(color: .black.opacity(scheme == .dark ? 0.32 : 0.08), radius: NativeTheme.metric("s-1"), y: 1)
    }
}
