import Foundation
import CCapturesSettings

/// Rust owns preview membership/order/collapse; Swift keeps images and native
/// resources keyed by these IDs. Calls are serialized on the UI thread.
final class NativePreviewStack {
    private let handle: OpaquePointer

    init() {
        precondition(Thread.isMainThread)
        handle = captures_preview_stack_new_v1()!
    }
    deinit { captures_preview_stack_free_v1(handle) }

    var ids: [String] {
        precondition(Thread.isMainThread)
        return (0..<captures_preview_stack_count_v1(handle)).map { index in
            var bytes = CapturesPreviewID()
            let found = captures_preview_stack_id_v1(handle, index, &bytes)
            precondition(found)
            return String(decoding: UnsafeBufferPointer(start: bytes.data, count: bytes.length), as: UTF8.self)
        }
    }

    @discardableResult
    func insert(_ id: String) -> Bool {
        precondition(Thread.isMainThread)
        guard !id.utf8.contains(0) else { return false }
        return id.withCString { captures_preview_stack_insert_v1(handle, $0) }
    }

    @discardableResult
    func remove(_ id: String) -> Bool {
        precondition(Thread.isMainThread)
        guard !id.utf8.contains(0) else { return false }
        return id.withCString { captures_preview_stack_remove_v1(handle, $0) }
    }

    /// Remove only the caller's snapshot; later arrivals survive.
    @discardableResult
    func removeAll(_ ids: [String]) -> Int {
        precondition(Thread.isMainThread)
        return ids.reduce(0) { $0 + (remove($1) ? 1 : 0) }
    }

    var isCollapsed: Bool {
        precondition(Thread.isMainThread)
        return captures_preview_stack_collapsed_v1(handle)
    }

    var contentHeight: Double {
        precondition(Thread.isMainThread)
        return captures_preview_stack_height_v1(handle)
    }

    func setCollapsed(_ collapsed: Bool) {
        precondition(Thread.isMainThread)
        let updated = captures_preview_stack_set_collapsed_v1(handle, collapsed)
        precondition(updated)
    }

    func cardLayout(index: Int, topAnchor: Bool) -> CapturesPreviewCardLayout? {
        cardLayout(index: index, topAnchor: topAnchor, hovered: false)
    }

    func cardLayout(index: Int, topAnchor: Bool, hovered: Bool) -> CapturesPreviewCardLayout? {
        precondition(Thread.isMainThread)
        guard index >= 0 else { return nil }
        var output = CapturesPreviewCardLayout()
        return captures_preview_stack_card_v2(handle, index, topAnchor, hovered, &output) ? output : nil
    }
}

/// UI-thread ownership of the shipping Rust policy. Generation values belong
/// to this object, not the separate global capture-flow guard.
final class NativePreviewPolicy {
    private let handle: OpaquePointer

    init() {
        precondition(Thread.isMainThread)
        handle = captures_preview_visibility_new_v1()!
    }
    deinit { captures_preview_visibility_free_v1(handle) }

    func beginCapture() -> UInt64? {
        precondition(Thread.isMainThread)
        var generation: UInt64 = 0
        return captures_preview_begin_v1(handle, &generation) ? generation : nil
    }

    @discardableResult
    func wait(generation: UInt64, artifact: String) -> Bool {
        precondition(Thread.isMainThread)
        guard !artifact.utf8.contains(0) else { return false }
        return artifact.withCString { captures_preview_wait_v1(handle, generation, $0) }
    }

    @discardableResult
    func ready(artifact: String) -> Bool {
        precondition(Thread.isMainThread)
        guard !artifact.utf8.contains(0) else { return false }
        return artifact.withCString { captures_preview_ready_v1(handle, $0) }
    }

    @discardableResult
    func restore(generation: UInt64) -> Bool {
        precondition(Thread.isMainThread)
        return captures_preview_restore_v1(handle, generation)
    }

    @discardableResult
    func stopWaiting() -> Bool {
        precondition(Thread.isMainThread)
        return captures_preview_stop_waiting_v1(handle)
    }

    func suppressCaptureUI(_ suppressed: Bool) {
        precondition(Thread.isMainThread)
        _ = captures_preview_capture_ui_v1(handle, suppressed)
    }

    func visible(count: Int, enabled: Bool, includeInCaptures: Bool) -> Bool {
        precondition(Thread.isMainThread)
        guard count >= 0 else { return false }
        return captures_preview_visible_v1(handle, count, enabled, includeInCaptures)
    }
}

enum NativePreviewLayout {
    /// Convert a dragged compact window into the shared pile edge. Always save
    /// the clamped result, so later expansion/arrival retains the visible pile.
    static func movedOrigin(monitor: CapturesPreviewMonitor, count: Int,
                            frameOrigin: NSPoint, placement: String) -> CapturesPreviewOrigin? {
        guard let base = geometry(monitor: monitor, count: count, collapsed: true,
                                  placement: placement) else { return nil }
        let top = placement.hasPrefix("top_")
        let offset = (base.height - base.card_height) / 2
            + (top ? -base.control_gutter : base.card_height + base.control_gutter)
        var origin = CapturesPreviewOrigin(x: frameOrigin.x, edge: frameOrigin.y + offset,
                                            anchor: top ? 1 : 0)
        guard let clamped = geometry(monitor: monitor, count: count, collapsed: true,
                                    origin: origin, placement: placement) else { return nil }
        origin.x = clamped.x; origin.edge = clamped.y + offset
        return origin
    }

    /// Monitor is physical top-left desktop geometry; returned frame is logical
    /// top-left geometry. AppKit's coordinate conversion stays with the host.
    static func geometry(monitor: CapturesPreviewMonitor, count: Int,
                         collapsed: Bool = false, origin: CapturesPreviewOrigin? = nil,
                         placement: String) -> CapturesPreviewGeometry? {
        let code: UInt32
        switch placement {
        case "bottom_left": code = 0
        case "bottom_right": code = 1
        case "top_left": code = 2
        case "top_right": code = 3
        default: return nil
        }
        guard count >= 0 else { return nil }
        var output = CapturesPreviewGeometry()
        let ok: Bool
        if var origin {
            ok = captures_preview_geometry_v1(monitor, count, collapsed, &origin, code, &output)
        } else {
            ok = captures_preview_geometry_v1(monitor, count, collapsed, nil, code, &output)
        }
        return ok ? output : nil
    }
}
