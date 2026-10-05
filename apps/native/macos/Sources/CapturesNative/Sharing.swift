import AppKit

/// A regular appearance-aware window. Closing it retains its form and worker;
/// capture hides it regardless of mini-preview inclusion preferences.
final class SharingController: NSObject, NSWindowDelegate, NSTextFieldDelegate {
    let window: NSWindow
    let root = Surface()
    let model: SharingModel
    let scroll = NSScrollView()
    let document = Surface()
    let footer = Surface()
    let email = NSTextField(frame: .zero)
    let code = NSSecureTextField(frame: .zero)
    let password = NSSecureTextField(frame: .zero)
    let expiry = NSTextField(frame: .zero)
    private let image = NSImageView()
    private let heading = NSTextField(labelWithString: "Share capture")
    private let intro = NSTextField(wrappingLabelWithString: "Local captures stay private. Upload only when you choose.")
    private let name = NSTextField(labelWithString: "Capture.png")
    private let authNote = NSTextField(wrappingLabelWithString: "")
    private let emailLabel = NSTextField(labelWithString: "Email")
    private let codeLabel = NSTextField(labelWithString: "Six-character email code")
    private let linkHeading = NSTextField(labelWithString: "Link access")
    private let linkHelp = NSTextField(wrappingLabelWithString: "Anyone with the link can open it. Links are not publicly indexed.")
    private let passwordLabel = NSTextField(labelWithString: "Password (optional)")
    private let expiryLabel = NSTextField(wrappingLabelWithString: "Expiry (optional, RFC3339 with timezone)")
    private let status = NSTextField(wrappingLabelWithString: "Ready")
    private let fixtureNote = NSTextField(wrappingLabelWithString: "Rendering fixture — sign-in and uploads disabled.")
    private let progress = NSProgressIndicator()
    private let progressLabel = NSTextField(labelWithString: "")
    private let sharedDate = NSTextField(wrappingLabelWithString: "")
    private let linkLabel = NSTextField(wrappingLabelWithString: "")
    private let trashNote = NSTextField(wrappingLabelWithString: "Cloud file is in Trash. Restore keeps it private.")
    private var buttons: [String: CaptureButton] = [:]
    private var tokens: Tokens
    private var captureHidden = false
    private var restoreAfterCapture = false
    private var confirmTrash = false
    private var previewID: String?

    init(tokens: Tokens, root profileRoot: String? = nil, transport: SharingTransport? = nil,
         fixtureWindow: NSWindow? = nil) {
        self.tokens = tokens
        model = SharingModel(transport: transport ?? profileRoot.map { NativeSharingTransport(root: $0) })
        window = fixtureWindow ?? NSWindow(contentRect: NSRect(x: 0, y: 0, width: 480, height: 720),
            styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        super.init()
        window.title = "Share capture"
        window.isReleasedWhenClosed = false
        window.contentMinSize = NSSize(width: 380, height: 520)
        window.sharingType = .none
        if fixtureWindow == nil { window.delegate = self }
        root.frame = NSRect(origin: .zero, size: window.contentLayoutRect.size)
        root.autoresizingMask = [.width, .height]; root.wantsLayer = true
        window.contentView = root
        scroll.hasVerticalScroller = true; scroll.autohidesScrollers = true
        scroll.drawsBackground = false; scroll.borderType = .noBorder
        scroll.useTokenScrollers(tokens); scroll.documentView = document
        root.addSubview(scroll); root.addSubview(footer); footer.wantsLayer = true
        image.imageScaling = .scaleProportionallyUpOrDown
        image.setAccessibilityLabel("Selected capture preview")
        for view in [heading, intro, image, name, authNote, emailLabel, email, codeLabel, code,
                     linkHeading, linkHelp, passwordLabel, password, expiryLabel, expiry] as [NSView] {
            document.addSubview(view)
        }
        for (field, label, placeholder) in [(email, "Email", "you@example.com"),
            (code as NSTextField, "Email code", ""), (password as NSTextField, "Share password", ""),
            (expiry, "Share expiry", "2027-01-31T18:00:00Z")] {
            field.isBordered = false; field.drawsBackground = false; field.wantsLayer = true
            field.focusRingType = .none; field.delegate = self
            field.placeholderString = placeholder; field.setAccessibilityLabel(label)
            (field.cell as? NSTextFieldCell)?.isScrollable = true
        }
        for view in [status, fixtureNote, progress, progressLabel, sharedDate, linkLabel, trashNote] as [NSView] {
            footer.addSubview(view)
        }
        linkLabel.isSelectable = true
        progress.isIndeterminate = false; progress.style = .bar; progress.minValue = 0; progress.maxValue = 1
        addButton("request_code", "Send code", in: document) { [weak self] in
            guard let self else { return }
            self.send(["operation": "request_code", "email": self.model.email.trimmingCharacters(in: .whitespacesAndNewlines)], "Sending code…")
        }
        addButton("verify", "Verify and sign in", in: document) { [weak self] in
            guard let self else { return }
            self.send(["operation": "verify", "code": self.model.code.trimmingCharacters(in: .whitespacesAndNewlines).uppercased()], "Verifying code…")
        }
        addButton("retry_save", "Retry saving session", in: document) { [weak self] in self?.send(["operation": "retry_save"], "Saving session…") }
        addButton("retry_account", "Retry account", in: document) { [weak self] in self?.send(["operation": "refresh"], "Checking account…") }
        addButton("logout", "Sign out", in: document) { [weak self] in self?.send(["operation": "logout"], "Signing out…") }
        addButton("remove_password", "Remove password", in: document) { [weak self] in
            guard let self else { return }; self.model.removePassword.toggle(); self.refresh()
        }
        addButton("remove_expiry", "Remove expiry", in: document) { [weak self] in
            guard let self else { return }; self.model.removeExpiry.toggle(); self.refresh()
        }
        for key in ["remove_password", "remove_expiry"] { buttons[key]?.setAccessibilityRole(.checkBox) }
        addButton("upload", "Upload and share", in: footer, primary: true) { [weak self] in
            guard let self else { return }
            self.send(["operation": "upload", "patch": self.model.patch], "Uploading and configuring share…")
        }
        addButton("stop", "Stop sharing", in: footer) { [weak self] in self?.send(["operation": "configure", "enabled": false], "Stopping sharing…") }
        addButton("trash", "Move cloud file to Trash", in: footer) { [weak self] in
            guard let self else { return }
            if self.confirmTrash { self.confirmTrash = false; self.send(["operation": "trash"], "Moving to Trash…") }
            else { self.confirmTrash = true; self.refresh() }
        }
        addButton("keep", "Keep cloud file", in: footer) { [weak self] in self?.confirmTrash = false; self?.refresh() }
        addButton("restore", "Restore cloud file", in: footer) { [weak self] in self?.send(["operation": "restore"], "Restoring…") }
        addButton("refresh", "Refresh share status", in: footer) { [weak self] in self?.send(["operation": "refresh"], "Refreshing…") }
        addButton("cancel", "Cancel upload", in: footer) { [weak self] in self?.model.cancel() }
        addButton("copy", "Copy link", in: footer) { [weak self] in
            guard let self, self.model.isLive, let link = self.model.link else { return }
            NSPasteboard.general.clearContents(); NSPasteboard.general.setString(link, forType: .string)
        }
        addButton("open", "Open link", in: footer) { [weak self] in
            guard let self, self.model.isLive, let link = self.model.link, let url = URL(string: link) else { return }
            NSWorkspace.shared.open(url)
        }
        root.sizeDidChange = { [weak self] _ in self?.layoutForm() }
        model.didChange = { [weak self] in self?.refresh() }
        if !model.isLive {
            image.image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1), size: NSSize(width: 284, height: 160))
            model.loadFixture(ProcessInfo.processInfo.environment["CAPTURES_NATIVE_SHARE_FIXTURE"] ?? "")
        }
        restyle(tokens)
    }

    private func addButton(_ key: String, _ title: String, in parent: NSView, primary: Bool = false,
                           action: @escaping () -> Void) {
        let button = CaptureButton(title, frame: .zero, tokens: tokens, action: action)
        button.primary = primary; parent.addSubview(button); buttons[key] = button
    }

    private func send(_ command: [String: Any], _ status: String) {
        _ = model.send(command, status: status)
    }

    func present(_ artifact: CaptureArtifact) {
        let previousID = model.artifact?.id
        _ = model.open(artifact)
        if previousID != model.artifact?.id { confirmTrash = false }
        if !captureHidden {
            if !window.isVisible { window.center() }
            if window.isMiniaturized { window.deminiaturize(nil) }
            window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
        }
        refresh()
    }

    func dismiss() {
        restoreAfterCapture = false; window.orderOut(nil)
        model.code = ""; code.stringValue = ""
    }
    func windowShouldClose(_ sender: NSWindow) -> Bool { dismiss(); return false }

    func setCaptureHidden(_ hidden: Bool) {
        guard hidden != captureHidden else { return }
        captureHidden = hidden
        if hidden { restoreAfterCapture = window.isVisible; window.orderOut(nil) }
        else if restoreAfterCapture { restoreAfterCapture = false; window.orderFront(nil) }
    }

    func restyle(_ tokens: Tokens) {
        self.tokens = tokens
        window.appearance = NSAppearance(named: tokens.color("text").brightnessComponent > 0.5 ? .darkAqua : .aqua)
        root.layer?.backgroundColor = tokens.color("surface-canvas").cgColor
        footer.layer?.backgroundColor = tokens.color("surface-raised").cgColor
        scroll.useTokenScrollers(tokens)
        for label in [heading, intro, name, authNote, emailLabel, codeLabel, linkHeading, linkHelp,
                      passwordLabel, expiryLabel, status, fixtureNote, progressLabel, sharedDate, linkLabel, trashNote] {
            label.font = .systemFont(ofSize: tokens.number("text-sm"))
            label.textColor = tokens.color("text")
            label.maximumNumberOfLines = 0
        }
        heading.font = .systemFont(ofSize: tokens.number("text-xl"), weight: .semibold)
        for label in [intro, authNote, fixtureNote, progressLabel] { label.textColor = tokens.color("text-subtle") }
        for field in [email, code as NSTextField, password as NSTextField, expiry] {
            field.font = .systemFont(ofSize: tokens.number("text-md")); field.textColor = tokens.color("text")
            field.layer?.backgroundColor = tokens.color("surface-field").cgColor
            field.layer?.borderWidth = 1; field.layer?.cornerRadius = tokens.number("r-md")
        }
        buttons.values.forEach { $0.tokens = tokens; $0.needsDisplay = true }
        refresh()
    }

    func refresh() {
        let editable = model.isLive && !model.busy
        let signIn = ["signed_out", "code_sent"].contains(model.auth)
        authNote.stringValue = signIn ? "Sign in with email to share this capture."
            : model.auth == "signed_in" ? "Signed in as \(model.userEmail)"
            : model.auth == "save_required" ? "Code accepted. Unlock your credential vault to save the session."
            : "Account or credential vault unavailable. Retry to continue sharing."
        for view in [emailLabel, email] { view.isHidden = !signIn }
        for view in [codeLabel, code] { view.isHidden = model.auth != "code_sent" }
        buttons["request_code"]?.isHidden = !signIn
        buttons["request_code"]?.title = model.auth == "code_sent" ? "Send another code" : "Send code"
        buttons["verify"]?.isHidden = model.auth != "code_sent"
        buttons["retry_save"]?.isHidden = model.auth != "save_required"
        buttons["retry_account"]?.isHidden = model.auth != "unavailable"
        buttons["logout"]?.isHidden = model.auth != "signed_in"
        for (field, value) in [(email, model.email), (code as NSTextField, model.code),
                               (password as NSTextField, model.password), (expiry, model.expiry)] {
            if field.stringValue != value { field.stringValue = value }
            field.isEnabled = editable
        }
        password.isEnabled = editable && !model.removePassword; expiry.isEnabled = editable && !model.removeExpiry
        passwordLabel.stringValue = model.share?["passwordProtected"] as? Bool == true
            ? "New password (blank keeps current)" : "Password (optional)"
        for (key, on) in [("remove_password", model.removePassword), ("remove_expiry", model.removeExpiry)] {
            buttons[key]?.selected = on; buttons[key]?.setAccessibilityValue(on)
            buttons[key]?.icon = on ? .shipping("check") : nil
        }
        if let artifact = model.artifact {
            name.stringValue = artifact.sharingSelection?.string("name") ?? "Capture"
            if previewID != artifact.id {
                previewID = artifact.id; image.image = NSImage(contentsOfFile: artifact.previewPath)
            }
        }
        for button in buttons.values {
            button.isEnabled = editable; button.setAccessibilityLabel(button.title); button.needsDisplay = true
        }
        let signedIn = model.auth == "signed_in"
        for key in ["upload", "stop", "trash", "keep", "restore", "refresh"] { buttons[key]?.isEnabled = editable && signedIn }
        buttons["upload"]?.title = model.share != nil ? "Save share settings" : model.asset != nil ? "Share" : "Upload and share"
        buttons["trash"]?.title = confirmTrash ? "Confirm move cloud file to Trash" : "Move cloud file to Trash"
        buttons["upload"]?.isHidden = model.trashed
        buttons["stop"]?.isHidden = model.share == nil || model.trashed
        buttons["trash"]?.isHidden = model.asset == nil || model.trashed
        buttons["keep"]?.isHidden = !confirmTrash || model.trashed
        buttons["restore"]?.isHidden = !model.trashed; trashNote.isHidden = !model.trashed
        buttons["cancel"]?.isHidden = !model.busy || !model.uploading
        buttons["cancel"]?.isEnabled = model.isLive && model.busy && model.uploading
        for key in ["copy", "open"] { buttons[key]?.isHidden = model.link == nil; buttons[key]?.isEnabled = model.isLive && model.link != nil }
        sharedDate.isHidden = model.link == nil; linkLabel.isHidden = model.link == nil
        sharedDate.stringValue = "Shared \(model.share?.string("sharedAt") ?? "")"
        linkLabel.stringValue = model.link ?? ""
        status.stringValue = model.status
        status.textColor = tokens.color(model.failure == nil ? "text-subtle" : "theme-signal-text")
        fixtureNote.isHidden = model.isLive
        progress.isHidden = model.progress == nil; progressLabel.isHidden = model.progress == nil
        if let (read, total) = model.progress {
            progress.doubleValue = Double(read) / Double(max(1, total))
            progressLabel.stringValue = "\(read) / \(total) bytes read"
        }
        buttons.values.forEach { $0.setAccessibilityLabel($0.title) }
        updateBorders(); layoutForm()
    }

    private func updateBorders() {
        for field in [email, code as NSTextField, password as NSTextField, expiry] {
            field.layer?.borderColor = tokens.color(field.currentEditor() == nil ? "control-border" : "theme-accent").cgColor
        }
    }
    func controlTextDidBeginEditing(_ obj: Notification) { updateBorders() }
    func controlTextDidEndEditing(_ obj: Notification) { updateBorders() }
    func controlTextDidChange(_ obj: Notification) {
        if obj.object as? NSTextField === code, code.stringValue.count > 6 { code.stringValue = String(code.stringValue.prefix(6)) }
        model.email = email.stringValue; model.code = code.stringValue
        model.password = password.stringValue; model.expiry = expiry.stringValue
    }

    /// Wrapped fixed footer measured first; the settings document scrolls in
    /// exactly the remaining space, including the 380×520 minimum window.
    func layoutForm() {
        let pad = tokens.number("s-6"), gap = tokens.number("s-3")
        let width = max(1, root.bounds.width - pad * 2)
        func height(_ label: NSTextField) -> CGFloat {
            FeedbackController.textHeight(label.stringValue, font: label.font ?? .systemFont(ofSize: 13), width: width)
        }
        func label(_ view: NSTextField, _ y: inout CGFloat) {
            guard !view.isHidden else { return }
            view.frame = NSRect(x: pad, y: y, width: width, height: height(view)); y = view.frame.maxY + gap
        }
        func row(_ keys: [String], _ y: inout CGFloat) {
            var x = pad, bottom = y
            for key in keys {
                guard let button = buttons[key], !button.isHidden else { continue }
                let font = NSFont.systemFont(ofSize: tokens.number("text-sm"), weight: .medium)
                let size = min(width, ceil((button.title as NSString).size(withAttributes: [.font: font]).width) + pad * 2)
                if x > pad && x + size > pad + width { y = bottom + gap; x = pad }
                button.frame = NSRect(x: x, y: y, width: size, height: tokens.number("h-md"))
                x = button.frame.maxX + gap; bottom = button.frame.maxY
            }
            if x > pad { y = bottom + gap }
        }
        var fy = pad
        if !progress.isHidden {
            progress.frame = NSRect(x: pad, y: fy, width: width, height: tokens.number("s-4"))
            fy = progress.frame.maxY + gap; label(progressLabel, &fy)
        }
        label(status, &fy); row(["cancel"], &fy); label(fixtureNote, &fy); label(trashNote, &fy)
        row(["upload", "stop"], &fy); row(["trash", "keep", "restore"], &fy); row(["refresh"], &fy)
        label(sharedDate, &fy); label(linkLabel, &fy); row(["copy", "open"], &fy)
        let footerHeight = fy - gap + pad
        footer.frame = NSRect(x: 0, y: root.bounds.height - footerHeight, width: root.bounds.width, height: footerHeight)
        scroll.frame = NSRect(x: 0, y: 0, width: root.bounds.width, height: max(0, footer.frame.minY))
        var y = pad
        label(heading, &y); label(intro, &y)
        image.frame = NSRect(x: pad, y: y, width: width, height: 150); y = image.frame.maxY + gap
        label(name, &y); y += gap; label(authNote, &y)
        label(emailLabel, &y)
        if !email.isHidden { email.frame = NSRect(x: pad, y: y, width: width, height: tokens.number("h-lg")); y = email.frame.maxY + gap }
        row(["request_code"], &y); label(codeLabel, &y)
        if !code.isHidden { code.frame = NSRect(x: pad, y: y, width: width, height: tokens.number("h-lg")); y = code.frame.maxY + gap }
        row(["verify", "retry_save", "retry_account", "logout"], &y); y += gap
        label(linkHeading, &y); label(linkHelp, &y); label(passwordLabel, &y)
        password.frame = NSRect(x: pad, y: y, width: width, height: tokens.number("h-lg")); y = password.frame.maxY + gap
        row(["remove_password"], &y); label(expiryLabel, &y)
        expiry.frame = NSRect(x: pad, y: y, width: width, height: tokens.number("h-lg")); y = expiry.frame.maxY + gap
        row(["remove_expiry"], &y)
        document.frame = NSRect(x: 0, y: 0, width: root.bounds.width, height: y + pad)
        let order: [NSView] = [email, buttons["request_code"]!, code, buttons["verify"]!, buttons["retry_save"]!,
            buttons["retry_account"]!, buttons["logout"]!, password, buttons["remove_password"]!, expiry,
            buttons["remove_expiry"]!, buttons["cancel"]!, buttons["upload"]!, buttons["stop"]!, buttons["trash"]!,
            buttons["keep"]!, buttons["restore"]!, buttons["refresh"]!, buttons["copy"]!, buttons["open"]!]
        KeyViewLoop.install(order, window: window)
    }
}
