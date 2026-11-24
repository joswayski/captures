// Shortcut-conflict probe. Registers ⌘⇧Space exactly like Captures'
// vendored global-hotkey (RegisterEventHotKey on GetApplicationEventTarget,
// options 0) or with kEventHotKeyExclusive, then logs every press it gets.
// usage: hotkey <name> <exclusive 0|1> <seconds>
import AppKit
import Carbon

setvbuf(stdout, nil, _IOLBF, 0)
let args = CommandLine.arguments
let name = args[1]
let exclusive = args[2] == "1"
let seconds = Double(args[3])!

let app = NSApplication.shared
app.setActivationPolicy(.accessory)

var types = [EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))]
var handler: EventHandlerRef?
InstallEventHandler(GetApplicationEventTarget(), { _, _, _ in
    print("\(CommandLine.arguments[1]): PRESSED")
    return noErr
}, 1, &types, nil, &handler)

var ref: EventHotKeyRef?
let status = RegisterEventHotKey(UInt32(kVK_Space), UInt32(cmdKey | shiftKey),
                                 EventHotKeyID(signature: 0x68747273, id: 1),
                                 GetApplicationEventTarget(),
                                 exclusive ? OptionBits(kEventHotKeyExclusive) : 0, &ref)
print("\(name): register exclusive=\(exclusive) status=\(status)")
Timer.scheduledTimer(withTimeInterval: seconds, repeats: false) { _ in exit(0) }
app.run()
