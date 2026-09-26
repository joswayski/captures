import AppKit

/// Card commands from `captures_app::history_view::CardAction`.
enum HistoryCardAction: String, CaseIterable {
    case edit
    case copy
    case saveImage = "save_image"
    case saveFile = "save_file"
    case showInFolder = "show_in_folder"
}

/// Capture History copy from `captures_app::history_view` (shared with the
/// wgpu host and identical to the shipping Tauri `CaptureHistory`).
struct HistoryCopy: Equatable {
    let eyebrow, title, lede, loading, emptyTitle, emptyBody, filteredEmpty: String
    let filterGroupLabel, gridLabel, deleteAll, deleteAllConfirm, deleteAllBusy: String
    let deleteAllLabel, deleteAllConfirmLabel, cancel, cancelLabel, missing: String
    let recoveryTitle, recoveryHelp: String
    let actionLabels: [HistoryCardAction: String]
    let actionBusyLabels: [HistoryCardAction: String]
    /// Two-step Delete / Delete all revert after this many seconds.
    let confirmTimeout: TimeInterval

    static let current: HistoryCopy = {
        do { return try HistoryCopy(transport: SettingsBridge()) }
        catch { preconditionFailure("History copy is unavailable: \(error.localizedDescription)") }
    }()

    init(transport: SettingsTransport) throws {
        let response = try transport.request(["operation": "history_copy"])
        guard let copy = response["copy"] as? [String: Any],
              let actions = response["actions"] as? [String: Any],
              let timeout = response["confirm_timeout_ms"] as? NSNumber
        else { throw SettingsStoreError.invalidResponse }
        func text(_ key: String) throws -> String {
            guard let value = copy[key] as? String else { throw SettingsStoreError.invalidResponse }
            return value
        }
        eyebrow = try text("eyebrow"); title = try text("title"); lede = try text("lede")
        loading = try text("loading"); emptyTitle = try text("empty_title")
        emptyBody = try text("empty_body"); filteredEmpty = try text("filtered_empty")
        filterGroupLabel = try text("filter_group_label"); gridLabel = try text("grid_label")
        deleteAll = try text("delete_all"); deleteAllConfirm = try text("delete_all_confirm")
        deleteAllBusy = try text("delete_all_busy"); deleteAllLabel = try text("delete_all_label")
        deleteAllConfirmLabel = try text("delete_all_confirm_label")
        cancel = try text("cancel"); cancelLabel = try text("cancel_label")
        missing = try text("missing")
        recoveryTitle = try text("recovery_title"); recoveryHelp = try text("recovery_help")
        var labels: [HistoryCardAction: String] = [:]
        var busy: [HistoryCardAction: String] = [:]
        for action in HistoryCardAction.allCases {
            guard let value = actions[action.rawValue] as? [String: Any],
                  let label = value["label"] as? String,
                  let busyLabel = value["busy"] as? String
            else { throw SettingsStoreError.invalidResponse }
            labels[action] = label; busy[action] = busyLabel
        }
        actionLabels = labels; actionBusyLabels = busy
        confirmTimeout = timeout.doubleValue / 1_000
    }

    func label(_ action: HistoryCardAction) -> String { actionLabels[action] ?? action.rawValue }
    func busyLabel(_ action: HistoryCardAction) -> String { actionBusyLabels[action] ?? label(action) }
}

/// One card's shared presentation (`history_view::Card`).
struct HistoryCard: Equatable {
    let kindLabel, date, details: String
    let warning: String?
    let missing: Bool
    let imageLabel: String
    let openLabel: String?
    let deleteLabel, deleteConfirmLabel, deleteConfirmTitle: String
    let deleteRequiresConfirmation: Bool
    let actions: [HistoryCardAction]
    let menu: [HistoryCardAction]

    init?(_ value: Any?) {
        guard let value = value as? [String: Any],
              let kindLabel = value["kind_label"] as? String,
              let date = value["date"] as? String,
              let details = value["details"] as? String,
              let missing = value["missing"] as? Bool,
              let imageLabel = value["image_label"] as? String,
              let deleteLabel = value["delete_label"] as? String,
              let deleteConfirmLabel = value["delete_confirm_label"] as? String,
              let deleteConfirmTitle = value["delete_confirm_title"] as? String,
              let deleteRequiresConfirmation = value["delete_requires_confirmation"] as? Bool,
              let actions = value["actions"] as? [String],
              let menu = value["menu"] as? [String]
        else { return nil }
        self.kindLabel = kindLabel; self.date = date; self.details = details
        warning = value["warning"] as? String
        self.missing = missing; self.imageLabel = imageLabel
        openLabel = value["open_label"] as? String
        self.deleteLabel = deleteLabel; self.deleteConfirmLabel = deleteConfirmLabel
        self.deleteConfirmTitle = deleteConfirmTitle
        self.deleteRequiresConfirmation = deleteRequiresConfirmation
        self.actions = actions.compactMap(HistoryCardAction.init(rawValue:))
        self.menu = menu.compactMap(HistoryCardAction.init(rawValue:))
    }

    /// Presentation for raw History artifacts (`{entry, missing, ...}`), in
    /// order. I/O-free; a malformed entry yields nil for that card only.
    static func cards(for artifacts: [[String: Any]],
                      transport: SettingsTransport = SettingsBridge()) throws -> [HistoryCard?] {
        guard !artifacts.isEmpty else { return [] }
        let response = try transport.request(["operation": "history_cards", "cards": artifacts])
        guard let cards = response["cards"] as? [Any], cards.count == artifacts.count
        else { throw SettingsStoreError.invalidResponse }
        return cards.map { HistoryCard($0) }
    }
}

/// Auto-fill grid metrics for one content width (`history_view::grid`).
struct HistoryGridLayout: Equatable {
    let columns: Int
    let cardWidth: CGFloat
    let cardHeight: CGFloat
    let gap: CGFloat

    static func make(width: CGFloat, transport: SettingsTransport = SettingsBridge()) -> HistoryGridLayout? {
        guard let grid = try? transport.request(["operation": "history_grid", "width": Double(width)])["grid"]
                as? [String: Any],
              let columns = grid["columns"] as? NSNumber, columns.intValue > 0,
              let cardWidth = grid["card_width"] as? NSNumber,
              let cardHeight = grid["card_height"] as? NSNumber,
              let gap = grid["gap"] as? NSNumber
        else { return nil }
        return HistoryGridLayout(columns: columns.intValue, cardWidth: CGFloat(cardWidth.doubleValue),
                                 cardHeight: CGFloat(cardHeight.doubleValue), gap: CGFloat(gap.doubleValue))
    }

    var rowStride: CGFloat { cardHeight + gap }
    func rows(_ count: Int) -> Int { (count + columns - 1) / columns }
    func contentHeight(_ count: Int) -> CGFloat {
        let rows = rows(count)
        return rows == 0 ? 0 : CGFloat(rows) * cardHeight + CGFloat(rows - 1) * gap
    }
    func frame(_ index: Int) -> NSRect {
        NSRect(x: CGFloat(index % columns) * (cardWidth + gap), y: CGFloat(index / columns) * rowStride,
               width: cardWidth, height: cardHeight)
    }
    /// Item indices whose rows intersect `rect` (grid coordinates).
    func visibleItems(_ count: Int, in rect: NSRect) -> Range<Int> {
        let rows = rows(count)
        guard rows > 0, rect.height > 0 else { return 0..<0 }
        let first = min(rows, max(0, Int(floor(rect.minY / rowStride))))
        let last = min(rows, max(first, Int(floor(rect.maxY / rowStride)) + 1))
        return min(count, first * columns)..<min(count, last * columns)
    }
    /// Keyboard navigation target, clamped to the collection.
    func step(_ index: Int, count: Int, columns deltaColumns: Int, rows deltaRows: Int) -> Int {
        guard count > 0 else { return 0 }
        return min(count - 1, max(0, index + deltaColumns + deltaRows * columns))
    }
}

/// Token-styled History control. Keeps NSButton keyboard, target/action and
/// accessibility behavior; only the drawing is custom.
final class HistoryButton: NSButton {
    enum Style { case primary, secondary, danger, confirm, ghost, overlay, confirmOverlay }
    enum Glyph { case edit, save, trash }

    var tokens: Tokens
    var style: Style { didSet { needsDisplay = true } }
    var glyph: Glyph? { didSet { needsDisplay = true } }
    var actionBlock: () -> Void
    private var hovered = false
    private var tracking: NSTrackingArea?

    init(_ title: String, frame: NSRect, tokens: Tokens, style: Style, glyph: Glyph? = nil,
         action: @escaping () -> Void) {
        self.tokens = tokens; self.style = style; self.glyph = glyph; actionBlock = action
        super.init(frame: frame)
        self.title = title
        isBordered = false
        setButtonType(.momentaryPushIn)
        target = self
        self.action = #selector(activate)
        setAccessibilityLabel(title)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func activate() { actionBlock() }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
                                  owner: self, userInfo: nil)
        addTrackingArea(area); tracking = area
    }
    override func mouseEntered(with event: NSEvent) { hovered = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hovered = false; needsDisplay = true }
    override func becomeFirstResponder() -> Bool { needsDisplay = true; return super.becomeFirstResponder() }
    override func resignFirstResponder() -> Bool { needsDisplay = true; return super.resignFirstResponder() }

    override func draw(_ dirtyRect: NSRect) {
        let active = isEnabled && (hovered || cell?.isHighlighted == true)
        let fill: NSColor?, border: NSColor?, text: NSColor
        switch (style, active) {
        case (.primary, false): fill = tokens.color("theme-accent"); border = nil; text = tokens.color("theme-accent-ink")
        case (.primary, true): fill = tokens.color("theme-accent-hover"); border = nil; text = tokens.color("theme-accent-ink")
        case (.secondary, false): fill = tokens.color("control"); border = tokens.color("control-border"); text = tokens.color("text")
        case (.secondary, true): fill = tokens.color("control-hover"); border = tokens.color("border-strong"); text = tokens.color("text")
        case (.danger, false): fill = nil; border = nil; text = tokens.color("text-subtle")
        case (.danger, true): fill = tokens.color("danger-surface"); border = nil; text = tokens.color("danger-text")
        case (.confirm, false), (.confirmOverlay, false):
            fill = tokens.color("theme-signal-strong"); border = nil; text = tokens.color("theme-signal-ink")
        case (.confirm, true), (.confirmOverlay, true):
            fill = tokens.color("theme-signal-deep"); border = nil; text = tokens.color("theme-signal-ink")
        case (.ghost, false): fill = nil; border = nil; text = tokens.color("text-muted")
        case (.ghost, true): fill = tokens.color("surface-hover"); border = nil; text = tokens.color("text")
        case (.overlay, false):
            fill = tokens.color("surface-raised").withAlphaComponent(0.88); border = tokens.color("border-subtle")
            text = tokens.color("text")
        case (.overlay, true): fill = tokens.color("theme-signal"); border = nil; text = tokens.color("theme-signal-ink")
        }
        let alpha: CGFloat = isEnabled ? 1 : 0.45
        let radius = tokens.number("r-md")
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: radius, yRadius: radius)
        if let fill { fill.withAlphaComponent(fill.alphaComponent * alpha).setFill(); path.fill() }
        if let border { border.withAlphaComponent(border.alphaComponent * alpha).setStroke(); path.lineWidth = 1; path.stroke() }
        let foreground = text.withAlphaComponent(alpha)
        let font = NSFont.systemFont(ofSize: tokens.number("text-sm"),
                                     weight: style == .primary ? .semibold : .medium)
        let attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: foreground]
        let size = title.isEmpty ? .zero : (title as NSString).size(withAttributes: attributes)
        let iconSide: CGFloat = style == .overlay || style == .confirmOverlay ? 16 : 15
        let gap = title.isEmpty ? 0 : tokens.number("s-3")
        let content = size.width + (glyph == nil ? 0 : iconSide + gap)
        var x = max(4, (bounds.width - content) / 2)
        if let glyph {
            HistoryGlyph.draw(glyph, in: NSRect(x: x, y: (bounds.height - iconSide) / 2, width: iconSide, height: iconSide),
                              color: foreground, flipped: isFlipped)
            x += iconSide + gap
        }
        if !title.isEmpty {
            (title as NSString).draw(with: NSRect(x: x, y: (bounds.height - size.height) / 2,
                                                  width: max(0, bounds.width - x - 4), height: size.height),
                                     options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine],
                                     attributes: attributes)
        }
        if window?.firstResponder === self {
            tokens.color("theme-accent").setStroke()
            let ring = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1), xRadius: radius, yRadius: radius)
            ring.lineWidth = 2; ring.stroke()
        }
    }
}

/// Shipping 24-unit icon paths (`EditIcon`, `SaveIcon`, `TrashIcon`, `HistoryIcon`).
enum HistoryGlyph {
    static func draw(_ glyph: HistoryButton.Glyph, in rect: NSRect, color: NSColor, flipped: Bool) {
        let path = NSBezierPath()
        path.lineWidth = 1.7 * rect.width / 24
        path.lineCapStyle = .round; path.lineJoinStyle = .round
        func point(_ x: CGFloat, _ y: CGFloat) -> NSPoint {
            NSPoint(x: rect.minX + x * rect.width / 24,
                    y: flipped ? rect.minY + y * rect.height / 24 : rect.maxY - y * rect.height / 24)
        }
        func line(_ points: [(CGFloat, CGFloat)]) {
            path.move(to: point(points[0].0, points[0].1))
            points.dropFirst().forEach { path.line(to: point($0.0, $0.1)) }
        }
        switch glyph {
        case .trash:
            line([(4, 7), (20, 7)]); line([(9, 7), (9, 4), (15, 4), (15, 7)])
            line([(18, 7), (17, 20), (7, 20), (6, 7)]); line([(10, 11), (10, 16)]); line([(14, 11), (14, 16)])
        case .edit:
            line([(4, 16), (3, 21), (8, 20), (19, 9), (15, 5), (4, 16)]); line([(13.5, 6.5), (17.5, 10.5)])
        case .save:
            line([(5, 4), (17, 4), (19, 6), (19, 20), (5, 20), (5, 4)])
            line([(8, 4), (8, 10), (16, 10), (16, 4)]); line([(8, 20), (8, 14), (16, 14), (16, 20)])
        }
        color.setStroke(); path.stroke()
    }

    static func drawHistory(in rect: NSRect, color: NSColor, flipped: Bool) {
        let path = NSBezierPath()
        path.lineWidth = 1.6 * rect.width / 24
        path.lineCapStyle = .round; path.lineJoinStyle = .round
        func point(_ x: CGFloat, _ y: CGFloat) -> NSPoint {
            NSPoint(x: rect.minX + x * rect.width / 24,
                    y: flipped ? rect.minY + y * rect.height / 24 : rect.maxY - y * rect.height / 24)
        }
        let radius = 9 * rect.width / 24
        path.appendOval(in: NSRect(x: point(12, 12).x - radius, y: point(12, 12).y - radius,
                                   width: radius * 2, height: radius * 2))
        path.move(to: point(3, 3)); path.line(to: point(3, 8)); path.line(to: point(8, 8))
        path.move(to: point(12, 7)); path.line(to: point(12, 12)); path.line(to: point(15, 14))
        color.setStroke(); path.stroke()
    }
}

/// Data for one rendered card.
struct HistoryGridItem {
    let id: String
    let card: HistoryCard?
    let image: NSImage?
    let confirmingDelete: Bool
    let busy: HistoryCardAction?
}

/// The thumbnail: `object-fit: contain` inside the card's sunken image area,
/// clipped to the card's top corners. Clicking opens the capture's editor.
final class HistoryThumbnailView: NSView {
    override var isFlipped: Bool { true }
    var tokens: Tokens
    var image: NSImage? { didSet { needsDisplay = true } }
    var missing = false { didSet { needsDisplay = true } }
    var openable = false
    var onOpen: () -> Void = {}
    private var hovered = false
    private var tracking: NSTrackingArea?

    init(tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: .zero)
        setAccessibilityElement(true)
        setAccessibilityRole(.button)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
                                  owner: self, userInfo: nil)
        addTrackingArea(area); tracking = area
    }
    override func mouseEntered(with event: NSEvent) { hovered = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hovered = false; needsDisplay = true }
    override func mouseDown(with event: NSEvent) {
        if let card = superview as? HistoryCardView { card.requestSelection() }
        if openable { onOpen() }
    }
    override func accessibilityPerformPress() -> Bool {
        guard openable else { return false }
        onOpen(); return true
    }
    override func menu(for event: NSEvent) -> NSMenu? { superview?.menu(for: event) }

    static func contain(_ size: NSSize, in area: NSRect) -> NSRect {
        guard size.width > 0, size.height > 0 else { return NSRect(x: area.midX, y: area.midY, width: 0, height: 0) }
        let scale = min(area.width / size.width, area.height / size.height)
        let fitted = NSSize(width: size.width * scale, height: size.height * scale)
        return NSRect(x: area.midX - fitted.width / 2, y: area.midY - fitted.height / 2,
                      width: fitted.width, height: fitted.height)
    }

    override func draw(_ dirtyRect: NSRect) {
        let radius = tokens.number("r-xl")
        let clip = NSBezierPath()
        // Rounded top corners only, in flipped coordinates.
        clip.move(to: NSPoint(x: 0, y: bounds.maxY))
        clip.line(to: NSPoint(x: 0, y: radius))
        clip.appendArc(withCenter: NSPoint(x: radius, y: radius), radius: radius, startAngle: 180, endAngle: 270)
        clip.line(to: NSPoint(x: bounds.maxX - radius, y: 0))
        clip.appendArc(withCenter: NSPoint(x: bounds.maxX - radius, y: radius), radius: radius,
                       startAngle: 270, endAngle: 360)
        clip.line(to: NSPoint(x: bounds.maxX, y: bounds.maxY))
        clip.close()
        NSGraphicsContext.saveGraphicsState()
        clip.addClip()
        tokens.color("surface-sunken").setFill(); bounds.fill()
        if openable && hovered {
            tokens.color("theme-accent").withAlphaComponent(0.12).setFill(); bounds.fill(using: .sourceOver)
        }
        if let image {
            image.draw(in: Self.contain(image.size, in: bounds), from: .zero, operation: .sourceOver,
                       fraction: missing ? 0.3 : 1, respectFlipped: true, hints: nil)
        }
        NSGraphicsContext.restoreGraphicsState()
    }
}

/// One `.history-card`: thumbnail, trash control, date, details, optional
/// dropped-frame warning and two actions. Secondary click lists every command.
final class HistoryCardView: NSView {
    override var isFlipped: Bool { true }
    let tokens: Tokens
    private(set) var artifactID = ""
    private(set) var card: HistoryCard?
    let thumbnail: HistoryThumbnailView
    let dateLabel = NSTextField(labelWithString: "")
    let detailsLabel = NSTextField(labelWithString: "")
    let warningLabel = NSTextField(labelWithString: "")
    let missingLabel = NSTextField(labelWithString: "")
    private(set) var actionButtons: [HistoryButton] = []
    private(set) var deleteButton: HistoryButton!
    var selected = false { didSet { needsDisplay = true } }
    var enabled = true
    var onSelect: () -> Void = {}
    var onOpen: () -> Void = {}
    var onAction: (HistoryCardAction) -> Void = { _ in }
    var onDelete: () -> Void = {}
    private var hovered = false
    private var tracking: NSTrackingArea?

    init(tokens: Tokens) {
        self.tokens = tokens
        thumbnail = HistoryThumbnailView(tokens: tokens)
        super.init(frame: .zero)
        addSubview(thumbnail)
        thumbnail.onOpen = { [weak self] in self?.onOpen() }
        for (label, size, color) in [(dateLabel, "text-md", "text"), (detailsLabel, "text-sm", "text-subtle"),
                                     (warningLabel, "text-sm", "caution-text")] {
            label.font = .systemFont(ofSize: tokens.number(size), weight: label === dateLabel ? .medium : .regular)
            label.textColor = tokens.color(color)
            label.lineBreakMode = .byTruncatingTail
            label.maximumNumberOfLines = 1
            addSubview(label)
        }
        missingLabel.font = .systemFont(ofSize: tokens.number("text-sm"), weight: .medium)
        missingLabel.textColor = tokens.color("text-muted")
        missingLabel.alignment = .center
        missingLabel.wantsLayer = true
        missingLabel.drawsBackground = true
        missingLabel.backgroundColor = tokens.color("surface-raised")
        missingLabel.layer?.cornerRadius = tokens.number("r-md")
        missingLabel.layer?.masksToBounds = true
        missingLabel.layer?.borderWidth = 1
        missingLabel.layer?.borderColor = tokens.color("border").cgColor
        missingLabel.isHidden = true
        thumbnail.addSubview(missingLabel)
        for slot in 0..<2 {
            let button = HistoryButton("", frame: .zero, tokens: tokens, style: .secondary) { [weak self] in
                guard let self, let card = self.card, card.actions.indices.contains(slot) else { return }
                self.requestSelection(); self.onAction(card.actions[slot])
            }
            actionButtons.append(button); addSubview(button)
        }
        deleteButton = HistoryButton("", frame: .zero, tokens: tokens, style: .overlay, glyph: .trash) { [weak self] in
            self?.requestSelection(); self?.onDelete()
        }
        addSubview(deleteButton)
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func configure(_ item: HistoryGridItem, selected: Bool, enabled: Bool, copy: HistoryCopy) {
        artifactID = item.id; card = item.card; self.selected = selected; self.enabled = enabled
        let idle = enabled && item.busy == nil
        thumbnail.image = item.image
        thumbnail.missing = item.card?.missing == true
        thumbnail.openable = idle && item.card?.openLabel != nil
        thumbnail.setAccessibilityLabel(item.card?.openLabel ?? item.card?.imageLabel)
        thumbnail.setAccessibilityEnabled(thumbnail.openable)
        missingLabel.stringValue = copy.missing
        missingLabel.isHidden = item.card?.missing != true
        dateLabel.stringValue = item.card?.date ?? ""
        detailsLabel.stringValue = item.card?.details ?? ""
        warningLabel.stringValue = item.card?.warning ?? ""
        warningLabel.isHidden = item.card?.warning == nil
        let actions = item.card?.actions ?? []
        for (slot, button) in actionButtons.enumerated() {
            button.isHidden = !actions.indices.contains(slot)
            guard actions.indices.contains(slot) else { continue }
            let action = actions[slot]
            button.title = item.busy == action ? copy.busyLabel(action) : copy.label(action)
            button.setAccessibilityLabel(button.title)
            button.style = action == .edit ? .primary : .secondary
            switch action {
            case .edit: button.glyph = HistoryButton.Glyph.edit
            case .saveImage, .saveFile: button.glyph = HistoryButton.Glyph.save
            case .copy, .showInFolder: button.glyph = nil
            }
            button.isEnabled = idle
        }
        let deleteLabel = item.confirmingDelete ? item.card?.deleteConfirmLabel : item.card?.deleteLabel
        deleteButton.style = item.confirmingDelete ? .confirmOverlay : .overlay
        deleteButton.setAccessibilityLabel(deleteLabel)
        deleteButton.toolTip = item.confirmingDelete ? item.card?.deleteConfirmTitle : item.card?.deleteLabel
        deleteButton.isEnabled = idle
        setAccessibilityLabel([item.card?.kindLabel, item.card?.date, item.card?.details]
            .compactMap { $0 }.joined(separator: " · "))
        setAccessibilitySelected(selected)
        needsLayout = true; needsDisplay = true
    }

    /// Selection stays available while card actions are busy.
    func requestSelection() { onSelect() }

    /// Labels are not interactive: clicks on them select the card.
    override func hitTest(_ point: NSPoint) -> NSView? {
        let hit = super.hitTest(point)
        return hit is NSTextField ? self : hit
    }

    override func layout() {
        super.layout()
        let imageHeight: CGFloat = 168
        thumbnail.frame = NSRect(x: 0, y: 0, width: bounds.width, height: imageHeight)
        let chipWidth = ceil(missingLabel.intrinsicContentSize.width) + 2 * tokens.number("s-4")
        let chipHeight = tokens.number("text-sm") + 2 * tokens.number("s-3") + 2
        missingLabel.frame = NSRect(x: (bounds.width - chipWidth) / 2, y: (imageHeight - chipHeight) / 2,
                                    width: chipWidth, height: chipHeight)
        let pad = tokens.number("s-5"), inner = bounds.width - 2 * pad
        var y = imageHeight + 1 + pad
        dateLabel.frame = NSRect(x: pad, y: y, width: inner, height: 18)
        y += 18 + tokens.number("s-2")
        detailsLabel.frame = NSRect(x: pad, y: y, width: inner, height: 16)
        y += 16 + tokens.number("s-2")
        warningLabel.frame = NSRect(x: pad, y: y, width: inner, height: 16)
        let gap = tokens.number("s-3"), height = tokens.number("h-md")
        let width = (inner - gap) / 2
        for (slot, button) in actionButtons.enumerated() {
            button.frame = NSRect(x: pad + CGFloat(slot) * (width + gap), y: bounds.height - pad - height,
                                  width: width, height: height)
        }
        deleteButton.frame = NSRect(x: bounds.width - gap - height, y: gap, width: height, height: height)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
                                  owner: self, userInfo: nil)
        addTrackingArea(area); tracking = area
    }
    override func mouseEntered(with event: NSEvent) { hovered = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hovered = false; needsDisplay = true }
    override func mouseDown(with event: NSEvent) { requestSelection() }

    override func menu(for event: NSEvent) -> NSMenu? {
        guard enabled, let card else { return nil }
        requestSelection()
        let menu = NSMenu()
        let copy = HistoryCopy.current
        for action in card.menu {
            let item = ClosureMenuItem(title: copy.label(action)) { [weak self] in self?.onAction(action) }
            menu.addItem(item)
        }
        if !card.menu.isEmpty { menu.addItem(.separator()) }
        menu.addItem(ClosureMenuItem(title: card.deleteLabel) { [weak self] in self?.onDelete() })
        return menu
    }

    override func draw(_ dirtyRect: NSRect) {
        let radius = tokens.number("r-xl")
        let shape = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: radius, yRadius: radius)
        tokens.color("surface-raised").setFill(); shape.fill()
        tokens.color("border-subtle").setFill()
        NSRect(x: 0, y: 168, width: bounds.width, height: 1).fill()
        let ring = selected || window?.firstResponder === self
        (ring ? tokens.color("theme-accent") : hovered && enabled ? tokens.color("border-strong")
            : tokens.color("border-subtle")).setStroke()
        let outline = NSBezierPath(roundedRect: bounds.insetBy(dx: ring ? 1 : 0.5, dy: ring ? 1 : 0.5),
                                   xRadius: radius, yRadius: radius)
        outline.lineWidth = ring ? 2 : 1
        outline.stroke()
    }
}

/// Menu item that runs a closure; retains nothing beyond the closure.
final class ClosureMenuItem: NSMenuItem {
    private let run: () -> Void
    init(title: String, run: @escaping () -> Void) {
        self.run = run
        super.init(title: title, action: #selector(runAction), keyEquivalent: "")
        target = self
    }
    required init(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func runAction() { run() }
}

/// `.history-grid` document view: virtualized auto-fill cards with an explicit
/// selection. Exposes table-like accessors (`numberOfRows`, `selectedRow`,
/// `selectRowIndexes`) so the controller and tests address cards by position.
final class HistoryGridView: NSView {
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    let tokens: Tokens
    private(set) var items: [HistoryGridItem] = []
    private(set) var selectedRow = -1
    var enabled = true { didSet { if oldValue != enabled { tile(force: true) } } }
    /// Programmatic and user selection changes, like NSTableView's delegate.
    var onSelectionChange: () -> Void = {}
    var onOpen: (Int) -> Void = { _ in }
    var onAction: (Int, HistoryCardAction) -> Void = { _, _ in }
    var onDelete: (Int) -> Void = { _ in }
    var onNeedsThumbnail: (Int) -> Void = { _ in }
    /// Escape: back out of an armed Delete / Delete all.
    var onCancel: () -> Void = {}
    private var layoutCache: (width: CGFloat, layout: HistoryGridLayout)?
    private var cards: [String: HistoryCardView] = [:]
    private var observing = false

    init(tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: .zero)
        setAccessibilityElement(true)
        setAccessibilityRole(.list)
        setAccessibilityLabel(HistoryCopy.current.gridLabel)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    deinit { NotificationCenter.default.removeObserver(self) }

    var numberOfRows: Int { items.count }
    var visibleCards: [HistoryCardView] {
        cards.values.filter { !$0.isHidden }.sorted { ($0.frame.minY, $0.frame.minX) < ($1.frame.minY, $1.frame.minX) }
    }
    func card(at row: Int) -> HistoryCardView? {
        guard items.indices.contains(row) else { return nil }
        return cards[items[row].id]
    }

    func selectRowIndexes(_ indexes: IndexSet, byExtendingSelection extend: Bool) {
        setSelectedRow(indexes.first.flatMap { items.indices.contains($0) ? $0 : nil } ?? -1, notify: true)
    }

    /// Selection without a change callback, for controller-driven reloads.
    func setSelectedRow(_ row: Int, notify: Bool) {
        let row = items.indices.contains(row) ? row : -1
        guard row != selectedRow else { return }
        selectedRow = row
        tile(force: true)
        if row >= 0 { scrollRowToVisible(row) }
        if notify { onSelectionChange() }
    }

    func reload(_ items: [HistoryGridItem]) {
        self.items = items
        if selectedRow >= items.count { selectedRow = -1 }
        tile(force: true)
    }

    func scrollRowToVisible(_ row: Int) {
        guard let layout = currentLayout(), items.indices.contains(row) else { return }
        scrollToVisible(layout.frame(row))
    }

    private func currentLayout() -> HistoryGridLayout? {
        let width = enclosingScrollView?.contentSize.width ?? bounds.width
        if let cache = layoutCache, cache.width == width { return cache.layout }
        guard let layout = HistoryGridLayout.make(width: width, transport: SettingsBridge()) else { return nil }
        layoutCache = (width, layout)
        return layout
    }

    override func viewDidMoveToSuperview() {
        super.viewDidMoveToSuperview()
        guard !observing, let clip = enclosingScrollView?.contentView else { return }
        observing = true
        clip.postsBoundsChangedNotifications = true
        clip.postsFrameChangedNotifications = true
        NotificationCenter.default.addObserver(self, selector: #selector(clipChanged),
                                               name: NSView.boundsDidChangeNotification, object: clip)
        NotificationCenter.default.addObserver(self, selector: #selector(clipChanged),
                                               name: NSView.frameDidChangeNotification, object: clip)
    }
    @objc private func clipChanged(_ notification: Notification) { tile(force: false) }

    /// Size the document to its rows and lay out only the visible cards.
    func tile(force: Bool) {
        guard let scroll = enclosingScrollView, let layout = currentLayout() else { return }
        let size = NSSize(width: scroll.contentSize.width,
                          height: max(scroll.contentSize.height, layout.contentHeight(items.count)))
        if frame.size != size { setFrameSize(size) }
        let visible = layout.visibleItems(items.count, in: visibleRect)
        var retained = Set<String>()
        let copy = HistoryCopy.current
        for index in visible {
            let item = items[index]
            retained.insert(item.id)
            let view: HistoryCardView
            if let existing = cards[item.id] { view = existing }
            else {
                view = HistoryCardView(tokens: tokens)
                cards[item.id] = view; addSubview(view)
                if item.image == nil { onNeedsThumbnail(index) }
            }
            view.isHidden = false
            view.frame = layout.frame(index)
            view.configure(item, selected: index == selectedRow, enabled: enabled, copy: copy)
            let id = item.id
            view.onSelect = { [weak self] in self?.userSelect(id) }
            view.onOpen = { [weak self] in
                guard let self, let row = self.row(for: id) else { return }
                self.onOpen(row)
            }
            view.onAction = { [weak self] action in
                guard let self, let row = self.row(for: id) else { return }
                self.onAction(row, action)
            }
            view.onDelete = { [weak self] in
                guard let self, let row = self.row(for: id) else { return }
                self.onDelete(row)
            }
            if force, item.image == nil, view.thumbnail.image == nil { onNeedsThumbnail(index) }
        }
        for (id, view) in cards where !retained.contains(id) {
            view.removeFromSuperview(); cards[id] = nil
        }
    }

    private func row(for id: String) -> Int? { items.firstIndex { $0.id == id } }

    private func userSelect(_ id: String) {
        guard let row = row(for: id) else { return }
        window?.makeFirstResponder(self)
        setSelectedRow(row, notify: true)
    }

    override func mouseDown(with event: NSEvent) { window?.makeFirstResponder(self) }

    override func keyDown(with event: NSEvent) {
        // Arrows select even while actions are busy; Return opens only when idle.
        guard !items.isEmpty, let layout = currentLayout() else { super.keyDown(with: event); return }
        let delta: (Int, Int)?
        switch event.keyCode {
        case 123: delta = (-1, 0)
        case 124: delta = (1, 0)
        case 125: delta = (0, 1)
        case 126: delta = (0, -1)
        default: delta = nil
        }
        if let delta {
            let next = selectedRow < 0 ? 0 : layout.step(selectedRow, count: items.count, columns: delta.0, rows: delta.1)
            setSelectedRow(next, notify: true)
        } else if (event.keyCode == 36 || event.keyCode == 76), selectedRow >= 0, enabled {
            onOpen(selectedRow)
        } else if event.keyCode == 53 {
            onCancel()
        } else {
            super.keyDown(with: event)
        }
    }
}

/// `.history-filters` pill: a label and a faint count. `title` stays
/// "Label count" so the control is addressable; VoiceOver hears "Label, n captures".
final class HistoryFilterPill: NSButton {
    var tokens: Tokens
    let label: String
    var count = 0 { didSet { update() } }
    var active = false { didSet { state = active ? .on : .off; needsDisplay = true } }
    var actionBlock: () -> Void
    private var hovered = false
    private var tracking: NSTrackingArea?

    init(label: String, tokens: Tokens, action: @escaping () -> Void) {
        self.tokens = tokens; self.label = label; actionBlock = action
        super.init(frame: .zero)
        isBordered = false
        setButtonType(.pushOnPushOff)
        target = self
        self.action = #selector(activate)
        update()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func activate() { actionBlock(); state = active ? .on : .off }

    private func update() {
        title = "\(label) \(count)"
        setAccessibilityLabel("\(label), \(count) captures")
        needsDisplay = true
    }

    private var font: NSFont { .systemFont(ofSize: tokens.number("text-sm"), weight: .medium) }
    var preferredWidth: CGFloat {
        let label = (self.label as NSString).size(withAttributes: [.font: font]).width
        let count = ("\(self.count)" as NSString).size(withAttributes: [.font: font]).width
        return ceil(label + tokens.number("s-3") + count + 2 * tokens.number("s-4"))
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
                                  owner: self, userInfo: nil)
        addTrackingArea(area); tracking = area
    }
    override func mouseEntered(with event: NSEvent) { hovered = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hovered = false; needsDisplay = true }
    override func becomeFirstResponder() -> Bool { needsDisplay = true; return super.becomeFirstResponder() }
    override func resignFirstResponder() -> Bool { needsDisplay = true; return super.resignFirstResponder() }

    override func draw(_ dirtyRect: NSRect) {
        let alpha: CGFloat = isEnabled ? 1 : 0.4
        let radius = bounds.height / 2
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: radius, yRadius: radius)
        let hover = isEnabled && hovered && !active
        if active {
            tokens.color("surface-raised").withAlphaComponent(alpha).setFill(); path.fill()
            tokens.color("border").withAlphaComponent(alpha).setStroke(); path.lineWidth = 1; path.stroke()
        } else if hover {
            tokens.color("surface-hover").setFill(); path.fill()
        }
        let text = tokens.color(active || hover ? "text" : "text-subtle").withAlphaComponent(alpha)
        let labelAttributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: text]
        let countAttributes: [NSAttributedString.Key: Any] = [
            .font: font, .foregroundColor: tokens.color("text-faint").withAlphaComponent(alpha),
        ]
        let labelSize = (label as NSString).size(withAttributes: labelAttributes)
        let x = tokens.number("s-4"), y = (bounds.height - labelSize.height) / 2
        (label as NSString).draw(at: NSPoint(x: x, y: y), withAttributes: labelAttributes)
        ("\(count)" as NSString).draw(at: NSPoint(x: x + labelSize.width + tokens.number("s-3"), y: y),
                                      withAttributes: countAttributes)
        if window?.firstResponder === self {
            tokens.color("theme-accent").setStroke()
            let ring = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1), xRadius: radius - 1, yRadius: radius - 1)
            ring.lineWidth = 2; ring.stroke()
        }
    }
}

/// `.history-empty`: icon tile, title and body, centered in the grid area.
final class HistoryEmptyView: NSView {
    override var isFlipped: Bool { true }
    let tokens: Tokens
    let titleLabel = NSTextField(labelWithString: "")
    let bodyLabel = NSTextField(labelWithString: "")

    init(tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: .zero)
        titleLabel.font = .systemFont(ofSize: tokens.number("text-lg"), weight: .semibold)
        titleLabel.textColor = tokens.color("text"); titleLabel.alignment = .center
        bodyLabel.font = .systemFont(ofSize: tokens.number("text-md"))
        bodyLabel.textColor = tokens.color("text-subtle"); bodyLabel.alignment = .center
        addSubview(titleLabel); addSubview(bodyLabel)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func show(title: String, body: String?) {
        titleLabel.stringValue = title
        bodyLabel.stringValue = body ?? ""
        bodyLabel.isHidden = body == nil
        setAccessibilityLabel([title, body].compactMap { $0 }.joined(separator: ". "))
        needsLayout = true; needsDisplay = true
    }

    private var tile: NSRect {
        NSRect(x: (bounds.width - 52) / 2, y: max(0, (bounds.height - 140) / 2), width: 52, height: 52)
    }

    override func layout() {
        super.layout()
        let top = tile.maxY + tokens.number("s-5")
        titleLabel.frame = NSRect(x: 0, y: top, width: bounds.width, height: 22)
        bodyLabel.frame = NSRect(x: 0, y: top + 22 + tokens.number("s-3"), width: bounds.width, height: 20)
    }

    override func draw(_ dirtyRect: NSRect) {
        let radius = tokens.number("r-xl")
        let path = NSBezierPath(roundedRect: tile.insetBy(dx: 0.5, dy: 0.5), xRadius: radius, yRadius: radius)
        tokens.color("surface-raised").setFill(); path.fill()
        tokens.color("border").setStroke(); path.lineWidth = 1; path.stroke()
        HistoryGlyph.drawHistory(in: tile.insetBy(dx: 13, dy: 13), color: tokens.color("text-subtle"), flipped: true)
    }
}
