import AppKit
import SwiftUI

/// Captures-owned confirmations and errors. File/permission pickers remain
/// system-owned, but application decisions never fall back to stock NSAlert.
@MainActor
final class CaptureDialogController {
    static let shared = CaptureDialogController()

    private var panel: CaptureDialogPanel?
    private var cancellation: (() -> Void)?

    func present(
        title: String,
        message: String,
        action: String,
        destructive: Bool = false,
        cancel: String? = "Cancel",
        alternate: String? = nil,
        onConfirm: @escaping () -> Void,
        onAlternate: (() -> Void)? = nil,
        onCancel: (() -> Void)? = nil
    ) {
        dismiss()
        let panel = CaptureDialogPanel(
            contentRect: NSRect(x: 0, y: 0, width: 430, height: 210),
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )
        panel.level = .modalPanel
        panel.collectionBehavior = [.moveToActiveSpace, .fullScreenAuxiliary]
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.sharingType = .none
        panel.contentView = NSHostingView(rootView: CaptureDialogView(
            title: title,
            message: message,
            action: action,
            destructive: destructive,
            cancel: cancel,
            alternate: alternate,
            confirm: { [weak self] in self?.dismiss(); onConfirm() },
            chooseAlternate: { [weak self] in self?.dismiss(); onAlternate?() },
            dismiss: { [weak self] in self?.dismiss(); onCancel?() }
        ))
        self.panel = panel
        cancellation = onCancel
        if let owner = NSApp.keyWindow, owner !== panel {
            let frame = owner.frame
            panel.setFrameOrigin(NSPoint(x: frame.midX - panel.frame.width / 2, y: frame.midY - panel.frame.height / 2))
        } else {
            panel.center()
        }
        NSApp.activate(ignoringOtherApps: true)
        panel.makeKeyAndOrderFront(nil)
    }

    func report(_ error: Error) {
        present(
            title: "Captures couldn’t finish that",
            message: error.localizedDescription,
            action: "OK",
            cancel: nil,
            onConfirm: {}
        )
    }

    func dismiss() {
        panel?.orderOut(nil)
        panel?.contentView = nil
        panel = nil
        cancellation = nil
    }

    fileprivate func cancel() {
        let action = cancellation
        dismiss()
        action?()
    }
}

private final class CaptureDialogPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    override func cancelOperation(_ sender: Any?) {
        CaptureDialogController.shared.cancel()
    }
}

private struct CaptureDialogView: View {
    let title: String
    let message: String
    let action: String
    let destructive: Bool
    let cancel: String?
    let alternate: String?
    let confirm: () -> Void
    let chooseAlternate: () -> Void
    let dismiss: () -> Void
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        VStack(alignment: .leading, spacing: NativeTheme.metric("s-6")) {
            HStack(alignment: .top, spacing: NativeTheme.metric("s-4")) {
                Image(systemName: destructive ? "trash" : "exclamationmark")
                    .font(.system(size: 15, weight: .bold))
                    .foregroundColor(destructive ? NativeTheme.signal : .black.opacity(0.82))
                    .frame(width: 34, height: 34)
                    .background((destructive ? NativeTheme.signal : NativeTheme.accent).opacity(destructive ? 0.14 : 1))
                    .clipShape(RoundedRectangle(cornerRadius: NativeTheme.metric("r-md")))
                VStack(alignment: .leading, spacing: NativeTheme.metric("s-2")) {
                    Text(title).font(.system(size: NativeTheme.metric("text-xl"), weight: .semibold))
                    Text(message)
                        .font(.system(size: NativeTheme.metric("text-sm")))
                        .foregroundColor(NativeTheme.muted(scheme))
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 0)
            HStack(spacing: NativeTheme.metric("s-3")) {
                Spacer()
                if let cancel {
                    Button(cancel, action: dismiss).buttonStyle(CaptureButtonStyle())
                        .keyboardShortcut(.cancelAction)
                }
                if let alternate {
                    Button(alternate, action: chooseAlternate)
                        .buttonStyle(CaptureButtonStyle(destructive: true))
                }
                Button(action, action: confirm)
                    .buttonStyle(CaptureButtonStyle(primary: !destructive, destructive: destructive))
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(NativeTheme.metric("s-7"))
        .foregroundColor(NativeTheme.text(scheme))
        .background(.regularMaterial)
        .background(NativeTheme.raised(scheme).opacity(0.94))
        .clipShape(RoundedRectangle(cornerRadius: NativeTheme.metric("r-xl")))
        .overlay(RoundedRectangle(cornerRadius: NativeTheme.metric("r-xl")).stroke(NativeTheme.border(scheme)))
        .shadow(color: .black.opacity(0.28), radius: 28, y: 12)
        .padding(18)
    }
}

@MainActor
func dialogReferenceView() -> AnyView {
    AnyView(CaptureDialogView(
        title: "Delete capture?",
        message: "This file will be moved to Finder Trash and removed from Capture History.",
        action: "Delete",
        destructive: true,
        cancel: "Cancel",
        alternate: nil,
        confirm: {},
        chooseAlternate: {},
        dismiss: {}
    ).frame(width: 430, height: 210))
}
