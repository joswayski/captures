// Posts one ⌘⇧Space through the HID event tap, like a real keyboard.
import ApplicationServices
import CoreGraphics
import Foundation

let source = CGEventSource(stateID: .hidSystemState)
for down in [true, false] {
    let event = CGEvent(keyboardEventSource: source, virtualKey: 49, keyDown: down)!
    event.flags = [.maskCommand, .maskShift]
    event.post(tap: .cghidEventTap)
    usleep(50_000)
}
print("press: posted, trusted=\(AXIsProcessTrusted())")
