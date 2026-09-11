import AppKit
import Carbon
import SwiftUI

final class ShortcutManager {
    static let shared = ShortcutManager()
    private var hotkeys: [EventHotKeyRef] = []
    private var actions: [UInt32: String] = [:]
    private var handler: EventHandlerRef?

    private init() {
        var event = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        InstallEventHandler(GetApplicationEventTarget(), { _, event, _ in
            guard let event else { return OSStatus(eventNotHandledErr) }
            var identifier = EventHotKeyID()
            let status = GetEventParameter(event, EventParamName(kEventParamDirectObject),
                                          EventParamType(typeEventHotKeyID), nil,
                                          MemoryLayout<EventHotKeyID>.size, nil, &identifier)
            guard status == noErr else { return status }
            DispatchQueue.main.async { ShortcutManager.shared.invoke(identifier.id) }
            return noErr
        }, 1, &event, nil, &handler)
    }

    func register(_ shortcuts: [Shortcut]) {
        hotkeys.forEach { UnregisterEventHotKey($0) }
        hotkeys.removeAll(); actions.removeAll()
        var failed: [String] = []
        for (index, shortcut) in shortcuts.enumerated() {
            var reference: EventHotKeyRef?
            let identifier = EventHotKeyID(signature: 0x43505452, id: UInt32(index + 1))
            let status = RegisterEventHotKey(shortcut.keyCode, shortcut.modifiers, identifier,
                                            GetApplicationEventTarget(), 0, &reference)
            if status == noErr, let reference {
                hotkeys.append(reference); actions[identifier.id] = shortcut.id
            } else { failed.append(shortcut.title) }
        }
        AppStore.shared.shortcutWarning = failed.isEmpty ? "" :
            "Unavailable shortcuts: \(failed.joined(separator: ", ")). Quit the other Captures app or change conflicting macOS Screenshot shortcuts in System Settings."
    }

    private func invoke(_ id: UInt32) { if let action = actions[id] { route(action) } }
    func route(_ action: String) {
        switch action {
        case "capture", "region": CaptureController.shared.show(kind: "image", target: "region")
        case "window": CaptureController.shared.show(kind: "image", target: "window")
        case "display": CaptureController.shared.show(kind: "image", target: "display")
        case "record": CaptureController.shared.show(kind: "video", target: "region")
        case "record-window": CaptureController.shared.show(kind: "video", target: "window")
        case "record-display": CaptureController.shared.show(kind: "video", target: "display")
        default: break
        }
    }
}

struct ShortcutField: NSViewRepresentable {
    @Binding var shortcut: Shortcut
    func makeNSView(context: Context) -> Recorder {
        let view = Recorder()
        view.bezelStyle = .rounded
        view.setAccessibilityLabel(shortcut.title)
        return view
    }
    func updateNSView(_ view: Recorder, context: Context) {
        view.value = shortcut
        view.changed = { shortcut = $0 }
        view.title = Recorder.display(shortcut)
    }
    final class Recorder: NSButton {
        var value = Shortcut.defaults[0]
        var changed: ((Shortcut) -> Void)?
        private var recording = false
        override var acceptsFirstResponder: Bool { true }
        override func mouseDown(with event: NSEvent) {
            recording = true; title = "Press a shortcut…"; window?.makeFirstResponder(self)
        }
        override func keyDown(with event: NSEvent) {
            guard recording else { super.keyDown(with: event); return }
            if event.keyCode == 53 { recording = false; title = Self.display(value); return }
            let flags = event.modifierFlags
            guard flags.contains(.command) || flags.contains(.control) || flags.contains(.option) else { NSSound.beep(); return }
            value.keyCode = UInt32(event.keyCode)
            value.modifiers = (flags.contains(.command) ? UInt32(cmdKey) : 0)
                | (flags.contains(.shift) ? UInt32(shiftKey) : 0)
                | (flags.contains(.option) ? UInt32(optionKey) : 0)
                | (flags.contains(.control) ? UInt32(controlKey) : 0)
            recording = false; title = Self.display(value); changed?(value)
        }
        static func display(_ value: Shortcut) -> String {
            let names: [UInt32: String] = [0:"A",1:"S",2:"D",3:"F",4:"H",5:"G",6:"Z",7:"X",8:"C",9:"V",
                11:"B",12:"Q",13:"W",14:"E",15:"R",16:"Y",17:"T",18:"1",19:"2",20:"3",21:"4",22:"6",23:"5",
                25:"9",26:"7",28:"8",29:"0",31:"O",32:"U",34:"I",35:"P",37:"L",38:"J",40:"K",45:"N",46:"M",49:"Space"]
            return (value.modifiers & UInt32(controlKey) != 0 ? "⌃" : "")
                + (value.modifiers & UInt32(optionKey) != 0 ? "⌥" : "")
                + (value.modifiers & UInt32(shiftKey) != 0 ? "⇧" : "")
                + (value.modifiers & UInt32(cmdKey) != 0 ? "⌘" : "") + (names[value.keyCode] ?? "Key \(value.keyCode)")
        }
    }
}
