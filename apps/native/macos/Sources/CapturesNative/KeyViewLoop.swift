import AppKit

/// A custom-drawn view that can hold keyboard focus even while its
/// `acceptsFirstResponder` is temporarily false (for example a disabled
/// handle). A `keyViewLeaf` view handles keys for its own subviews, so the
/// loop stops at it instead of visiting them.
protocol KeyViewParticipant: NSView {
    var keyViewLeaf: Bool { get }
}

extension KeyViewParticipant {
    var keyViewLeaf: Bool { false }
}

/// Explicit Tab order for windows laid out with absolute frames. AppKit's
/// default loop orders views by geometry, which does not follow the shipping
/// DOM order. Hidden, disabled or (without Full Keyboard Access) button
/// entries stay in the loop; AppKit skips views that cannot become key.
enum KeyViewLoop {
    /// Focusable views under `root`, depth first in subview order, including
    /// `root` itself.
    static func candidates(in root: NSView) -> [NSView] {
        var found: [NSView] = []
        func visit(_ view: NSView) {
            if isCandidate(view) { found.append(view) }
            if let participant = view as? KeyViewParticipant, participant.keyViewLeaf { return }
            view.subviews.forEach(visit)
        }
        visit(root)
        return found
    }

    static func isCandidate(_ view: NSView) -> Bool {
        if view is KeyViewParticipant { return true }
        if view is NSScroller || view is NSText { return false }
        if let field = view as? NSTextField { return field.isEditable }
        if let image = view as? NSImageView { return image.isEditable }
        return view is NSControl || view.acceptsFirstResponder
    }

    /// Links the focusable views of `order` (controls or containers, expanded
    /// depth first) into one closed loop, keeping each view's first position.
    /// With a window, the loop replaces AppKit's automatic one and `initial`
    /// (default: the first entry) becomes the initial first responder.
    @discardableResult
    static func install(_ order: [NSView], window: NSWindow? = nil, initial: NSView? = nil) -> [NSView] {
        var seen = Set<ObjectIdentifier>()
        var chain: [NSView] = []
        for container in order {
            for view in candidates(in: container) where seen.insert(ObjectIdentifier(view)).inserted {
                chain.append(view)
            }
        }
        for (index, view) in chain.enumerated() {
            view.nextKeyView = chain[(index + 1) % chain.count]
        }
        if let window, let first = initial ?? chain.first {
            window.autorecalculatesKeyViewLoop = false
            window.initialFirstResponder = first
        }
        return chain
    }

    /// The loop starting at `start`, as AppKit's `nextKeyView` links it.
    static func order(from start: NSView, limit: Int = 512) -> [NSView] {
        var result: [NSView] = [start]
        var next = start.nextKeyView
        while let view = next, view !== start, result.count < limit {
            result.append(view); next = view.nextKeyView
        }
        return result
    }
}
