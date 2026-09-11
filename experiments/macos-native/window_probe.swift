import AppKit
import CoreGraphics
import Foundation

// Build once before measurement: xcrun swiftc window_probe.swift -o build/window-probe
// No Screen Recording prompt is issued silently. Grant it to the calling terminal.
guard CommandLine.arguments.count == 2, let pid = Int32(CommandLine.arguments[1]) else {
    fputs("Usage: window-probe PID\n", stderr); exit(2)
}
let entries = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
let windows: [[String: Any]] = entries.compactMap { entry in
    guard (entry[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid,
          (entry[kCGWindowLayer as String] as? NSNumber)?.intValue == 0,
          let bounds = entry[kCGWindowBounds as String] as? [String: Any],
          let width = bounds["Width"] as? Double, let height = bounds["Height"] as? Double,
          width >= 100, height >= 100,
          let number = entry[kCGWindowNumber as String] as? Int else { return nil }
    return ["window_id": number, "width": width, "height": height,
            "title": entry[kCGWindowName as String] as? String ?? ""]
}
let data = try JSONSerialization.data(withJSONObject: windows, options: [.sortedKeys])
FileHandle.standardOutput.write(data)
