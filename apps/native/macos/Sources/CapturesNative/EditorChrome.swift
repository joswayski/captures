import AppKit
import CCapturesSettings

/// Shipping screenshot-editor chrome copy and rules from
/// `captures-app::editor_chrome` (`captures_editor_chrome_v1`), shared with
/// the wgpu host: header, tool rail, Shapes and Layers copy, icon names, the
/// 1040pt header rule and the Recenter offscreen test.
enum EditorChrome {
    struct Tool {
        let key: String
        let icon: String
        let label: String
        /// Accessible name and tooltip, e.g. "Select & move (V)".
        let name: String
    }

    struct HeaderLayout: Equatable {
        let showHistory: Bool
        let sliderWidth: CGFloat
        let presetWidth: CGFloat
    }

    /// The `result` of one request, or nil for an error envelope.
    static func request(_ object: [String: Any]) -> Any? {
        guard let data = try? JSONSerialization.data(withJSONObject: object) else { return nil }
        let response = String(decoding: data, as: UTF8.self).withCString {
            captures_editor_chrome_v1($0)
        }
        guard let response else { return nil }
        defer { captures_settings_free_v1(response) }
        guard let envelope = try? JSONSerialization.jsonObject(
                with: Data(bytes: response, count: strlen(response))) as? [String: Any],
              envelope["ok"] as? Bool == true else { return nil }
        return envelope["result"]
    }

    static let copy: [String: Any] = request(["operation": "copy"]) as? [String: Any] ?? [:]

    /// A copy string such as `text("header", "fit_tooltip")`.
    static func text(_ group: String, _ key: String) -> String {
        (copy[group] as? [String: Any])?[key] as? String ?? ""
    }

    static func metric(_ key: String) -> CGFloat {
        CGFloat(((copy["metrics"] as? [String: Any])?[key] as? NSNumber)?.doubleValue ?? 0)
    }

    private static func tools(_ key: String) -> [Tool] {
        (copy[key] as? [[String: Any]] ?? []).compactMap { item in
            guard let key = item["key"] as? String, let icon = item["icon"] as? String,
                  let label = item["label"] as? String, let name = item["name"] as? String else { return nil }
            return Tool(key: key, icon: icon, label: label, name: name)
        }
    }

    static let rail = tools("rail")
    static let shapes = tools("shapes")

    static func headerLayout(width: CGFloat) -> HeaderLayout {
        let result = request(["operation": "header_layout", "width": Double(width)]) as? [String: Any]
        return HeaderLayout(
            showHistory: result?["show_history"] as? Bool ?? true,
            sliderWidth: CGFloat((result?["zoom_slider_width"] as? NSNumber)?.doubleValue ?? 92),
            presetWidth: CGFloat((result?["zoom_preset_width"] as? NSNumber)?.doubleValue ?? 76))
    }

    static func shapesTooltip(_ current: String) -> String {
        request(["operation": "shapes_tooltip", "current": current]) as? String ?? "Shapes"
    }

    /// The inspector heading for a tool key (rail keys, shape keys, or
    /// "pen"/"wand"/"erase"/"restore"), like shipping's `toolLabel`.
    static func toolLabel(_ key: String) -> String {
        request(["operation": "tool_label", "key": key]) as? String ?? "Properties"
    }

    static func zoomLabel(_ percent: Double) -> String {
        request(["operation": "zoom_label", "percent": percent]) as? String ?? "\(percent)%"
    }

    /// Shipping shows Recenter only while pan leaves the canvas mostly off screen.
    static func canvasOffscreen(viewport: NSRect, canvas: NSRect) -> Bool {
        func array(_ rect: NSRect) -> [Double] {
            [Double(rect.minX), Double(rect.minY), Double(rect.width), Double(rect.height)]
        }
        return request(["operation": "canvas_offscreen", "viewport": array(viewport),
                        "canvas": array(canvas)]) as? Bool ?? false
    }
}
