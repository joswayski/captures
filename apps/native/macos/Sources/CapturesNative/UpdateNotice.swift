import AppKit
import CCapturesSettings
import QuartzCore

/// Pure JSON transport to the shared update notice copy, fixtures and placement.
protocol UpdateNoticeTransport {
    func request(_ object: [String: Any]) throws -> [String: Any]
}

struct UpdateNoticeBridge: UpdateNoticeTransport {
    func request(_ object: [String: Any]) throws -> [String: Any] {
        let data = try JSONSerialization.data(withJSONObject: object)
        let pointer = String(decoding: data, as: UTF8.self).withCString { captures_update_notice_request_v1($0) }
        guard let pointer else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(pointer) }
        let responseData = Data(bytes: pointer, count: strlen(pointer))
        guard let response = try JSONSerialization.jsonObject(with: responseData) as? [String: Any],
              let ok = response["ok"] as? Bool else { throw AppBridgeError.invalidResponse }
        guard ok else { throw AppBridgeError.backend(response.string("error", "The update notice is unavailable.")) }
        guard let result = response["result"] as? [String: Any] else { throw AppBridgeError.invalidResponse }
        return result
    }
}

enum UpdateNoticeAction: Equatable {
    case dismiss, install, check, showNotes, hideNotes, openDownloadPage
    case openPullRequest(String)

    init?(_ value: Any?) {
        guard let value = value as? [String: Any], let name = value["action"] as? String else { return nil }
        switch name {
        case "dismiss": self = .dismiss
        case "install": self = .install
        case "check": self = .check
        case "show_notes": self = .showNotes
        case "hide_notes": self = .hideNotes
        case "open_download_page": self = .openDownloadPage
        case "open_pull_request":
            guard let url = value["url"] as? String else { return nil }
            self = .openPullRequest(url)
        default: return nil
        }
    }

    var name: String {
        switch self {
        case .dismiss: return "dismiss"
        case .install: return "install"
        case .check: return "check"
        case .showNotes: return "show_notes"
        case .hideNotes: return "hide_notes"
        case .openDownloadPage: return "open_download_page"
        case .openPullRequest: return "open_pull_request"
        }
    }
}

struct UpdateNoticeButton: Equatable {
    let label: String
    let enabled: Bool
    let action: UpdateNoticeAction

    init?(_ value: Any?) {
        guard let value = value as? [String: Any], let label = value["label"] as? String,
              let action = UpdateNoticeAction(value["action"]) else { return nil }
        self.label = label
        enabled = value["enabled"] as? Bool ?? true
        self.action = action
    }
}

struct UpdateNoticeNoteItem: Equatable {
    let text: String
    let pullNumber: Int?
    let pullURL: String?
}

struct UpdateNoticeNoteGroup: Equatable {
    let displayVersion: String
    let items: [UpdateNoticeNoteItem]
}

struct UpdateNoticeNotes: Equatable {
    let heading: String
    let hide: UpdateNoticeButton
    let stacked: Bool
    let intro: String?
    let empty: String?
    let groups: [UpdateNoticeNoteGroup]
}

struct UpdateNoticeDownload: Equatable {
    let label: String
    let detail: String
    let percent: Int?
    let percentLabel: String?
    let accessibleValue: String
}

struct UpdateNoticeError: Equatable {
    let message: String
    let fallbackPrefix: String
    let fallbackLink: String
    let fallbackSuffix: String
}

/// The shared presentation from `captures_app::update_notice::present`.
struct UpdateNoticePresentation: Equatable {
    let visualState: String
    let icon: String
    let iconTone: String
    let title: String
    let description: String
    let revealNotes: UpdateNoticeButton?
    let notes: UpdateNoticeNotes?
    let download: UpdateNoticeDownload?
    let restartMessage: String?
    let restartSeconds: Int
    let error: UpdateNoticeError?
    let statusMessage: String?
    let closeWarning: String?
    let dismiss: UpdateNoticeButton?
    let primary: UpdateNoticeButton?
    let hasFooter: Bool
    let dismissBlocked: Bool
    let cardWidth: CGFloat
    let cardHeight: CGFloat

    init(_ value: [String: Any]) throws {
        guard let visualState = value["visual_state"] as? String, let icon = value["icon"] as? String,
              let iconTone = value["icon_tone"] as? String, let title = value["title"] as? String,
              let description = value["description"] as? String,
              let cardWidth = value["card_width"] as? Double, let cardHeight = value["card_height"] as? Double
        else { throw AppBridgeError.invalidResponse }
        self.visualState = visualState; self.icon = icon; self.iconTone = iconTone
        self.title = title; self.description = description
        self.cardWidth = CGFloat(cardWidth); self.cardHeight = CGFloat(cardHeight)
        revealNotes = UpdateNoticeButton(value["reveal_notes"])
        if let notes = value["notes"] as? [String: Any], let hide = UpdateNoticeButton(notes["hide"]) {
            let groups = (notes["groups"] as? [[String: Any]] ?? []).map { group in
                UpdateNoticeNoteGroup(displayVersion: group.string("display_version"),
                    items: (group["items"] as? [[String: Any]] ?? []).map { item in
                        let pull = item["pull_request"] as? [String: Any]
                        return UpdateNoticeNoteItem(text: item.string("text"),
                            pullNumber: pull?["number"] as? Int, pullURL: pull?["url"] as? String)
                    })
            }
            self.notes = UpdateNoticeNotes(heading: notes.string("heading"), hide: hide,
                stacked: notes.bool("stacked"), intro: notes["intro"] as? String,
                empty: notes["empty"] as? String, groups: groups)
        } else {
            notes = nil
        }
        if let download = value["download"] as? [String: Any] {
            self.download = UpdateNoticeDownload(label: download.string("label"), detail: download.string("detail"),
                percent: download["percent"] as? Int, percentLabel: download["percent_label"] as? String,
                accessibleValue: download.string("accessible_value"))
        } else {
            download = nil
        }
        let restart = value["restart"] as? [String: Any]
        restartMessage = restart?["message"] as? String
        restartSeconds = restart?["seconds_remaining"] as? Int ?? 0
        if let error = value["error"] as? [String: Any] {
            self.error = UpdateNoticeError(message: error.string("message"),
                fallbackPrefix: error.string("fallback_prefix"), fallbackLink: error.string("fallback_link"),
                fallbackSuffix: error.string("fallback_suffix"))
        } else {
            error = nil
        }
        statusMessage = value["status_message"] as? String
        closeWarning = value["close_warning"] as? String
        let footer = value["footer"] as? [String: Any]
        hasFooter = footer != nil
        dismiss = UpdateNoticeButton(footer?["dismiss"])
        primary = UpdateNoticeButton(footer?["primary"])
        dismissBlocked = value["dismiss_blocked"] as? Bool ?? false
    }
}

/// Top-left logical desktop placement from `captures_app::tray_notice`.
struct UpdateNoticePlacement: Equatable {
    let frame: CGRect
    let caret: String
    let caretX: CGFloat
    let card: CGRect

    init(_ value: [String: Any]) throws {
        guard let placement = value["placement"] as? [String: Any], let card = value["card"] as? [String: Any]
        else { throw AppBridgeError.invalidResponse }
        func rect(_ value: [String: Any]) -> CGRect {
            CGRect(x: value["x"] as? Double ?? 0, y: value["y"] as? Double ?? 0,
                   width: value["width"] as? Double ?? 0, height: value["height"] as? Double ?? 0)
        }
        let frame = rect(placement)
        self.frame = frame
        caret = placement.string("caret", "none")
        caretX = CGFloat(placement["caret_x"] as? Double ?? Double(frame.width / 2))
        self.card = rect(card)
    }

    /// Converts a top-left desktop rectangle to an AppKit screen frame.
    static func cocoaFrame(_ rect: CGRect, primaryHeight: CGFloat) -> NSRect {
        NSRect(x: rect.minX, y: primaryHeight - rect.maxY, width: rect.width, height: rect.height)
    }

    static func topLeft(_ rect: NSRect, primaryHeight: CGFloat) -> CGRect {
        CGRect(x: rect.minX, y: primaryHeight - rect.maxY, width: rect.width, height: rect.height)
    }
}

/// Workbench model over the shared, deterministic stub status source. It never
/// downloads, installs, relaunches or opens URLs.
final class UpdateNoticeModel {
    /// Matches `captures_app::update_notice::FIXTURES`.
    static let fixtures = ["available", "single", "closing", "manual", "downloading",
                           "restarting", "error", "checking", "up-to-date"]
    private let transport: UpdateNoticeTransport
    private(set) var status: [String: Any]?
    private(set) var visible = false
    private(set) var simulating = false
    private(set) var tickInterval: TimeInterval?
    var showChangelog = true
    var changed: () -> Void = {}
    var persistShowChangelog: (Bool) -> Void = { _ in }
    var event: (String, [String: Any]) -> Void = { name, fields in
        var row = fields; row["event"] = name; Metrics.write(row)
    }

    init(transport: UpdateNoticeTransport = UpdateNoticeBridge()) { self.transport = transport }

    func load(fixture: String) throws {
        guard let status = try transport.request(["operation": "fixture", "name": fixture])["status"] as? [String: Any]
        else { throw AppBridgeError.invalidResponse }
        self.status = status
        visible = true; simulating = false; tickInterval = nil
        changed()
    }

    func presentation() throws -> UpdateNoticePresentation {
        try UpdateNoticePresentation(transport.request([
            "operation": "present", "status": status.map { $0 as Any } ?? NSNull(),
            "view": ["show_changelog": showChangelog, "action_error": NSNull(), "installing": false],
        ]))
    }

    func placement(monitor: CGRect, workArea: CGRect, tray: CGRect?, cardWidth: CGFloat,
                   cardHeight: CGFloat) throws -> UpdateNoticePlacement {
        func rect(_ value: CGRect) -> [String: Any] {
            ["x": value.minX, "y": value.minY, "width": value.width, "height": value.height]
        }
        return try UpdateNoticePlacement(transport.request([
            "operation": "placement", "monitor": rect(monitor), "work_area": rect(workArea),
            "tray": tray.map { rect($0) as Any } ?? NSNull(), "menu_bar_at_top": true,
            "card_width": cardWidth, "card_height": cardHeight,
        ]))
    }

    private func step(_ event: String) {
        guard let status else { return }
        do {
            let result = try transport.request(["operation": "stub", "status": status, "event": event])
            self.status = result["status"] as? [String: Any]
            tickInterval = (result["tick_ms"] as? Double).map { $0 / 1000 }
            if self.status == nil {
                visible = false; simulating = false
                self.event("update-notice-restart-simulated", [:])
            }
        } catch {
            self.event("update-notice-error", ["error": error.localizedDescription])
        }
    }

    func perform(_ action: UpdateNoticeAction) {
        event("update-notice-action", ["action": action.name])
        switch action {
        case .dismiss:
            if (try? presentation().dismissBlocked) == true {
                event("update-notice-dismiss-blocked", [:]); return
            }
            visible = false; simulating = false; tickInterval = nil
            event("update-notice-dismissed", [:])
        case .showNotes, .hideNotes:
            showChangelog = action == .showNotes
            persistShowChangelog(showChangelog)
        case .install, .check:
            if action == .install, status?["state"] as? String == "available",
               status?["installable"] as? Bool == false {
                event("update-notice-open-url", ["kind": "release", "opened": false]); return
            }
            simulating = true
            step(action == .install ? "install" : "check")
        case .openPullRequest(let url):
            event("update-notice-open-url", ["kind": "pull_request", "url": url, "opened": false])
        case .openDownloadPage:
            event("update-notice-open-url", ["kind": "download", "url": "https://captur.es/#download", "opened": false])
        }
        changed()
    }

    func tick() {
        guard simulating, visible else { return }
        step("tick")
        changed()
    }
}

/// Header icon: tone fill with an SF Symbol or a spinner.
final class UpdateNoticeIconView: NSView {
    init(frame: NSRect, icon: String, tone: String, tokens: Tokens) {
        super.init(frame: frame)
        wantsLayer = true
        let fill: String, ink: String
        switch tone {
        case "accent": fill = "theme-accent"; ink = "theme-accent-ink"
        case "positive": fill = "positive"; ink = "positive-ink"
        case "signal": fill = "theme-signal"; ink = "theme-signal-ink"
        default: fill = "surface-sunken"; ink = "text"
        }
        layer?.backgroundColor = tokens.color(fill).cgColor
        layer?.cornerRadius = tokens.number("r-lg")
        if tone == "neutral" { layer?.borderWidth = 1; layer?.borderColor = tokens.color("border").cgColor }
        setAccessibilityElement(false)
        if icon == "spinner" {
            let spinner = NSProgressIndicator(frame: NSRect(x: (frame.width - 16) / 2, y: (frame.height - 16) / 2, width: 16, height: 16))
            spinner.style = .spinning; spinner.controlSize = .small; spinner.isIndeterminate = true
            spinner.startAnimation(nil)
            addSubview(spinner)
            return
        }
        let symbol: String
        switch icon {
        case "check": symbol = "checkmark"
        case "warning": symbol = "exclamationmark.triangle"
        default: symbol = "viewfinder"
        }
        let image = NSImageView(frame: NSRect(x: (frame.width - 18) / 2, y: (frame.height - 18) / 2, width: 18, height: 18))
        image.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: 14, weight: .semibold))
        image.contentTintColor = tokens.color(ink)
        addSubview(image)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
}

/// Link-only text: clicks call the action instead of opening a browser.
final class UpdateNoticeLinkText: NSTextView, NSTextViewDelegate {
    var linkAction: (String) -> Void = { _ in }
    // TextKit 1: the view does not own a storage it did not create.
    private var storage: NSTextStorage?

    init(frame: NSRect, text: NSAttributedString) {
        let storage = NSTextStorage(attributedString: text)
        let manager = NSLayoutManager()
        storage.addLayoutManager(manager)
        let container = NSTextContainer(containerSize: NSSize(width: frame.width, height: CGFloat.greatestFiniteMagnitude))
        container.lineFragmentPadding = 0
        container.widthTracksTextView = true
        manager.addTextContainer(container)
        self.storage = storage
        super.init(frame: frame, textContainer: container)
        isEditable = false; isSelectable = true; drawsBackground = false
        textContainerInset = .zero
        delegate = self
    }
    override init(frame: NSRect, textContainer: NSTextContainer?) { super.init(frame: frame, textContainer: textContainer) }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func fittingHeight() -> CGFloat {
        guard let container = textContainer, let manager = layoutManager else { return 0 }
        manager.ensureLayout(for: container)
        return ceil(manager.usedRect(for: container).height)
    }

    func textView(_ textView: NSTextView, clickedOnLink link: Any, at charIndex: Int) -> Bool {
        linkAction((link as? URL)?.absoluteString ?? (link as? String) ?? "")
        return true
    }
}

/// Solid `--surface-raised` card with a tray caret in a transparent window.
final class UpdateNoticeView: NSView {
    private let tokens: Tokens
    private var targets: [UpdateNoticeButtonTarget] = []
    var onAction: (UpdateNoticeAction) -> Void = { _ in }
    override var isFlipped: Bool { true }

    init(frame: NSRect, tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = NSColor.clear.cgColor
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    private func label(_ text: String, size: String, color: String, weight: NSFont.Weight = .regular,
                       wraps: Bool = false) -> NSTextField {
        let field = wraps ? NSTextField(wrappingLabelWithString: text) : NSTextField(labelWithString: text)
        field.font = .systemFont(ofSize: tokens.number(size), weight: weight)
        field.textColor = tokens.color(color)
        field.isSelectable = false
        return field
    }

    private func height(_ field: NSTextField, width: CGFloat) -> CGFloat {
        ceil(field.cell?.cellSize(forBounds: NSRect(x: 0, y: 0, width: width, height: 10_000)).height ?? 17)
    }

    private func caption(_ button: UpdateNoticeButton, parent: NSView, rightEdge: CGFloat, y: CGFloat) {
        let control = NSButton(title: "", target: nil, action: nil)
        control.isBordered = false
        control.attributedTitle = NSAttributedString(string: button.label.uppercased(), attributes: [
            .font: NSFont.systemFont(ofSize: tokens.number("text-xs"), weight: .semibold),
            .foregroundColor: tokens.color("text-subtle"), .kern: 0.6,
        ])
        control.sizeToFit()
        control.frame = NSRect(x: rightEdge - control.frame.width - 8, y: y,
                               width: control.frame.width + 8, height: 24)
        control.setAccessibilityLabel(button.label)
        let action = button.action
        let target = UpdateNoticeButtonTarget { [weak self] in self?.onAction(action) }
        control.target = target; control.action = #selector(UpdateNoticeButtonTarget.fire)
        targets.append(target)
        parent.addSubview(control)
    }

    private func sunkenBox(_ frame: NSRect, parent: NSView) -> Surface {
        let box = Surface(frame: frame)
        box.wantsLayer = true
        box.layer?.backgroundColor = tokens.color("surface-sunken").cgColor
        box.layer?.borderColor = tokens.color("border-subtle").cgColor
        box.layer?.borderWidth = 1
        box.layer?.cornerRadius = tokens.number("r-lg")
        parent.addSubview(box)
        return box
    }

    private func bar(_ frame: NSRect, fraction: CGFloat?, parent: NSView) {
        let track = NSView(frame: frame)
        track.wantsLayer = true
        track.layer?.backgroundColor = tokens.color("surface-canvas").cgColor
        track.layer?.cornerRadius = 2
        let fill = CALayer()
        fill.backgroundColor = tokens.color("info").cgColor
        fill.cornerRadius = 2
        let width = frame.width * min(max(fraction ?? 0.34, 0), 1)
        fill.frame = CGRect(x: 0, y: 0, width: width, height: frame.height)
        track.layer?.addSublayer(fill)
        if fraction == nil {
            let slide = CABasicAnimation(keyPath: "position.x")
            slide.fromValue = -width / 2; slide.toValue = frame.width + width / 2
            slide.duration = 1.25; slide.repeatCount = .infinity
            fill.add(slide, forKey: "indeterminate")
        }
        parent.addSubview(track)
    }

    func render(_ p: UpdateNoticePresentation, placement: UpdateNoticePlacement) {
        subviews.forEach { $0.removeFromSuperview() }
        targets.removeAll()
        layer?.sublayers?.filter { $0.name == "update-caret" }.forEach { $0.removeFromSuperlayer() }
        let cardFrame = NSRect(x: placement.card.minX, y: placement.card.minY,
                               width: placement.card.width, height: placement.card.height)
        let card = Surface(frame: cardFrame)
        card.wantsLayer = true
        card.layer?.backgroundColor = tokens.color("surface-raised").cgColor
        card.layer?.borderColor = tokens.color("border").cgColor
        card.layer?.borderWidth = 1
        card.layer?.cornerRadius = tokens.number("r-xl")
        card.layer?.shadowColor = NSColor.black.cgColor
        card.layer?.shadowOpacity = 0.35
        card.layer?.shadowRadius = 10
        card.layer?.shadowOffset = NSSize(width: 0, height: -8)
        card.setAccessibilityRole(.group)
        card.setAccessibilityLabel(p.title)
        card.setAccessibilityHelp(p.description)
        addSubview(card)

        // CSS-style triangle caret (not a rotated square) toward the tray icon.
        if placement.caret != "none" {
            let caret = CAShapeLayer()
            caret.name = "update-caret"
            let path = CGMutablePath()
            let half: CGFloat = 7, size: CGFloat = 8, x = placement.caretX
            if placement.caret == "top" {
                path.move(to: CGPoint(x: x, y: 0))
                path.addLine(to: CGPoint(x: x + half, y: size))
                path.addLine(to: CGPoint(x: x - half, y: size))
            } else {
                let bottom = bounds.height
                path.move(to: CGPoint(x: x - half, y: bottom - size))
                path.addLine(to: CGPoint(x: x + half, y: bottom - size))
                path.addLine(to: CGPoint(x: x, y: bottom))
            }
            path.closeSubpath()
            caret.path = path
            caret.fillColor = tokens.color("surface-raised").cgColor
            caret.zPosition = 10
            layer?.addSublayer(caret)
        }

        let s3 = tokens.number("s-3"), s4 = tokens.number("s-4")
        let s5 = tokens.number("s-5"), s6 = tokens.number("s-6")
        let width = cardFrame.width, heightTotal = cardFrame.height
        // Header.
        card.addSubview(UpdateNoticeIconView(frame: NSRect(x: s6, y: s6, width: 36, height: 36),
            icon: p.icon, tone: p.iconTone, tokens: tokens))
        let revealWidth: CGFloat = p.revealNotes == nil ? 0 : 96
        let copyX = s6 + 36 + s4, copyWidth = width - copyX - s6 - revealWidth
        let title = label(p.title, size: "text-lg", color: "text", weight: .semibold)
        title.frame = NSRect(x: copyX, y: s6 - 2, width: copyWidth, height: 20)
        title.lineBreakMode = .byTruncatingTail
        card.addSubview(title)
        let description = label(p.description, size: "text-sm", color: "text-subtle")
        description.frame = NSRect(x: copyX, y: s6 + 20, width: copyWidth, height: 17)
        description.lineBreakMode = .byTruncatingTail
        card.addSubview(description)
        if let reveal = p.revealNotes { caption(reveal, parent: card, rightEdge: width - s6, y: s6 + 6) }
        let headerBottom = s6 + 36 + s5 + 3

        // Footer and warning anchored to the bottom.
        let hMd = tokens.number("h-md")
        let footerTop = p.hasFooter ? heightTotal - (s4 + hMd + s6) : heightTotal - s4
        if p.hasFooter {
            let y = footerTop + s4
            if let dismiss = p.dismiss {
                let size = (dismiss.label as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: tokens.number("text-md"), weight: .medium)])
                let button = CaptureButton(dismiss.label, frame: NSRect(x: s6, y: y, width: max(72, ceil(size.width) + 32), height: hMd), tokens: tokens) { [weak self] in self?.onAction(dismiss.action) }
                button.isEnabled = dismiss.enabled
                card.addSubview(button)
            }
            if let primary = p.primary {
                let size = (primary.label as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: tokens.number("text-md"), weight: .medium)])
                let buttonWidth = max(104, ceil(size.width) + 32)
                let button = CaptureButton(primary.label, frame: NSRect(x: width - s6 - buttonWidth, y: y, width: buttonWidth, height: hMd), tokens: tokens) { [weak self] in self?.onAction(primary.action) }
                button.primary = true
                button.isEnabled = primary.enabled
                button.keyEquivalent = "\r"
                card.addSubview(button)
            }
        }
        var bodyBottom = footerTop - s4
        if let warning = p.closeWarning {
            let text = label(warning, size: "text-sm", color: "caution-text", wraps: true)
            let textWidth = width - s6 * 2 - s4 * 2 - 16 - s3
            let textHeight = height(text, width: textWidth)
            let boxHeight = textHeight + s3 * 2
            let box = Surface(frame: NSRect(x: s6, y: footerTop - s3 - boxHeight, width: width - s6 * 2, height: boxHeight))
            box.wantsLayer = true
            box.layer?.backgroundColor = tokens.color("caution-surface").cgColor
            box.layer?.borderColor = tokens.color("caution-text").withAlphaComponent(0.32).cgColor
            box.layer?.borderWidth = 1
            box.layer?.cornerRadius = tokens.number("r-md")
            let glyph = NSImageView(frame: NSRect(x: s4, y: (boxHeight - 16) / 2, width: 16, height: 16))
            glyph.image = NSImage(systemSymbolName: "exclamationmark.triangle", accessibilityDescription: nil)
            glyph.contentTintColor = tokens.color("caution-text")
            box.addSubview(glyph)
            text.frame = NSRect(x: s4 + 16 + s3, y: s3, width: textWidth, height: textHeight)
            box.addSubview(text)
            box.setAccessibilityElement(true); box.setAccessibilityRole(.staticText); box.setAccessibilityLabel(warning)
            card.addSubview(box)
            bodyBottom = box.frame.minY - s4
        }

        // Body.
        let bodyWidth = width - s6 * 2
        var y = headerBottom
        if let notes = p.notes {
            let box = sunkenBox(NSRect(x: s6, y: y, width: bodyWidth, height: max(40, bodyBottom - y)), parent: card)
            let heading = label(notes.heading.uppercased(), size: "text-2xs", color: "text-subtle", weight: .semibold)
            heading.frame = NSRect(x: s5, y: s5, width: 160, height: 16)
            box.addSubview(heading)
            caption(notes.hide, parent: box, rightEdge: box.frame.width - s5 + 8, y: s5 - 4)
            var innerY = s5 + 16 + s4
            let innerWidth = box.frame.width - s5 * 2
            if let intro = notes.intro {
                let field = label(intro, size: "text-sm", color: "text-subtle", wraps: true)
                let h = height(field, width: innerWidth)
                field.frame = NSRect(x: s5, y: innerY, width: innerWidth, height: h)
                box.addSubview(field)
                innerY += h + s4
            }
            let text = NSMutableAttributedString()
            let paragraph = NSMutableParagraphStyle()
            paragraph.headIndent = s5; paragraph.firstLineHeadIndent = 0
            paragraph.paragraphSpacing = s3
            paragraph.tabStops = [NSTextTab(textAlignment: .left, location: s5, options: [:])]
            let groupStyle = NSMutableParagraphStyle()
            groupStyle.paragraphSpacing = s3; groupStyle.paragraphSpacingBefore = s4
            let body: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: tokens.number("text-sm")),
                .foregroundColor: tokens.color("text-muted"), .paragraphStyle: paragraph,
            ]
            if let empty = notes.empty {
                text.append(NSAttributedString(string: empty, attributes: [
                    .font: NSFont.systemFont(ofSize: tokens.number("text-sm")),
                    .foregroundColor: tokens.color("text-subtle"),
                ]))
            }
            for (index, group) in notes.groups.enumerated() {
                if notes.stacked {
                    let style: NSParagraphStyle = index == 0 ? NSParagraphStyle.default : groupStyle
                    text.append(NSAttributedString(string: (text.length > 0 ? "\n" : "") + group.displayVersion + "\n", attributes: [
                        .font: NSFont.monospacedDigitSystemFont(ofSize: tokens.number("text-sm"), weight: .semibold),
                        .foregroundColor: tokens.color("text"), .paragraphStyle: style,
                    ]))
                }
                for item in group.items {
                    if text.length > 0, !text.string.hasSuffix("\n") { text.append(NSAttributedString(string: "\n", attributes: body)) }
                    var bullet = body
                    bullet[.foregroundColor] = tokens.color("text-faint")
                    text.append(NSAttributedString(string: "•\t", attributes: bullet))
                    text.append(NSAttributedString(string: item.text, attributes: body))
                    if let number = item.pullNumber, let url = item.pullURL {
                        var link = body
                        link[.link] = url
                        link[.foregroundColor] = tokens.color("text")
                        link[.underlineStyle] = NSUnderlineStyle.single.rawValue
                        text.append(NSAttributedString(string: " ", attributes: body))
                        text.append(NSAttributedString(string: "#\(number)", attributes: link))
                    }
                }
            }
            let scroll = NSScrollView(frame: NSRect(x: s5, y: innerY, width: innerWidth,
                height: max(20, box.frame.height - innerY - s5)))
            scroll.drawsBackground = false; scroll.hasVerticalScroller = true; scroll.autohidesScrollers = true
            scroll.useTokenScrollers(tokens)
            scroll.borderType = .noBorder
            let list = UpdateNoticeLinkText(frame: NSRect(x: 0, y: 0, width: innerWidth - 4, height: 10), text: text)
            list.linkTextAttributes = [.foregroundColor: tokens.color("text"), .underlineStyle: NSUnderlineStyle.single.rawValue,
                                       .cursor: NSCursor.pointingHand]
            list.frame.size.height = max(list.fittingHeight(), scroll.frame.height)
            list.setAccessibilityLabel(notes.heading)
            list.linkAction = { [weak self] url in self?.onAction(.openPullRequest(url)) }
            scroll.documentView = list
            box.addSubview(scroll)
        }
        if let download = p.download {
            let box = sunkenBox(NSRect(x: s6, y: y, width: bodyWidth, height: s5 * 2 + 17 + s4 + 4), parent: card)
            let title = label(download.label, size: "text-sm", color: "text-subtle")
            title.sizeToFit()
            title.frame.origin = NSPoint(x: s5, y: s5)
            box.addSubview(title)
            let detail = label(download.detail, size: "text-sm", color: "text-faint")
            detail.frame = NSRect(x: title.frame.maxX + s2(), y: s5, width: 180, height: 17)
            box.addSubview(detail)
            if let percent = download.percentLabel {
                let value = label(percent, size: "text-sm", color: "text", weight: .semibold)
                value.alignment = .right
                value.frame = NSRect(x: box.frame.width - s5 - 60, y: s5, width: 60, height: 17)
                box.addSubview(value)
            }
            bar(NSRect(x: s5, y: s5 + 17 + s4, width: box.frame.width - s5 * 2, height: 4),
                fraction: download.percent.map { CGFloat($0) / 100 }, parent: box)
            box.setAccessibilityElement(true); box.setAccessibilityRole(.progressIndicator)
            box.setAccessibilityLabel("Downloading update"); box.setAccessibilityValue(download.accessibleValue)
            y = box.frame.maxY + s3
        }
        if let message = p.restartMessage {
            let box = sunkenBox(NSRect(x: s6, y: y, width: bodyWidth, height: s5 * 2 + 17 + s4 + 4), parent: card)
            let text = label(message, size: "text-sm", color: "text-subtle")
            text.frame = NSRect(x: s5, y: s5, width: box.frame.width - s5 * 2, height: 17)
            box.addSubview(text)
            bar(NSRect(x: s5, y: s5 + 17 + s4, width: box.frame.width - s5 * 2, height: 4),
                fraction: CGFloat(p.restartSeconds) / 3, parent: box)
            box.setAccessibilityElement(true); box.setAccessibilityRole(.staticText); box.setAccessibilityLabel(message)
            y = box.frame.maxY + s3
        }
        if let error = p.error {
            let message = label(error.message, size: "text-sm", color: "danger-text", wraps: true)
            let messageHeight = height(message, width: bodyWidth - s5 * 2)
            let box = Surface(frame: NSRect(x: s6, y: y, width: bodyWidth, height: messageHeight + s5 * 2))
            box.wantsLayer = true
            box.layer?.backgroundColor = tokens.color("danger-surface").cgColor
            box.layer?.borderColor = tokens.color("danger-border").cgColor
            box.layer?.borderWidth = 1
            box.layer?.cornerRadius = tokens.number("r-lg")
            message.frame = NSRect(x: s5, y: s5, width: bodyWidth - s5 * 2, height: messageHeight)
            message.setAccessibilityRole(.staticText)
            box.addSubview(message)
            card.addSubview(box)
            let plain: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: tokens.number("text-sm")), .foregroundColor: tokens.color("text-subtle"),
            ]
            var link = plain
            link[.link] = "https://captur.es/#download"
            link[.foregroundColor] = tokens.color("text")
            link[.underlineStyle] = NSUnderlineStyle.single.rawValue
            let fallback = NSMutableAttributedString(string: error.fallbackPrefix, attributes: plain)
            fallback.append(NSAttributedString(string: error.fallbackLink, attributes: link))
            fallback.append(NSAttributedString(string: error.fallbackSuffix, attributes: plain))
            let text = UpdateNoticeLinkText(frame: NSRect(x: s6, y: box.frame.maxY + s3, width: bodyWidth, height: 18), text: fallback)
            text.linkTextAttributes = [.foregroundColor: tokens.color("text"), .underlineStyle: NSUnderlineStyle.single.rawValue,
                                       .cursor: NSCursor.pointingHand]
            text.linkAction = { [weak self] _ in self?.onAction(.openDownloadPage) }
            card.addSubview(text)
            y = text.frame.maxY + s3
        }
        if let message = p.statusMessage {
            let text = label(message, size: "text-sm", color: "text-subtle")
            text.frame = NSRect(x: s6, y: y, width: bodyWidth, height: 17)
            text.setAccessibilityRole(.staticText)
            card.addSubview(text)
        }
    }

    private func s2() -> CGFloat { tokens.number("s-2") }
}

/// Retained closure target for AppKit buttons that are not CaptureButtons.
final class UpdateNoticeButtonTarget: NSObject {
    let block: () -> Void
    init(_ block: @escaping () -> Void) { self.block = block }
    @objc func fire() { block() }
}

final class UpdateNoticePanel: NSPanel {
    var escape: () -> Void = {}
    override var canBecomeKey: Bool { true }
    override func cancelOperation(_ sender: Any?) { escape() }
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { escape() } else { super.keyDown(with: event) }
    }
}

/// Presents the workbench update notice from the stub status source.
final class UpdateNoticeController {
    let model: UpdateNoticeModel
    private let tokens: Tokens
    private let tray: String
    private var panel: UpdateNoticePanel?
    private var view: UpdateNoticeView?
    private var timer: Timer?
    private var lastReport: [String: Any] = [:]
    /// When shipping's `update-restart-exit` (3 s into the restart state) ends;
    /// the stub "restart" keeps the faded card until then.
    private(set) var restartExitEnd: Date?

    init(tokens: Tokens, tray: String = "top", model: UpdateNoticeModel = UpdateNoticeModel()) {
        self.tokens = tokens; self.tray = tray; self.model = model
        model.changed = { [weak self] in self?.render() }
    }

    func present(fixture: String) {
        do { try model.load(fixture: fixture) } catch {
            Metrics.write(["event": "update-notice-error", "error": error.localizedDescription])
        }
    }

    func close() {
        timer?.invalidate(); timer = nil; panel?.close(); panel = nil; view = nil; restartExitEnd = nil
    }
    func refresh() { render() }

    private func render() {
        timer?.invalidate(); timer = nil
        if !model.visible, let end = restartExitEnd, end.timeIntervalSinceNow > 0, let panel {
            // The stub restart finished: let the exit fade end, accepting no input.
            panel.ignoresMouseEvents = true
            timer = Timer.scheduledTimer(withTimeInterval: end.timeIntervalSinceNow, repeats: false) {
                [weak self] _ in self?.close()
            }
            return
        }
        guard model.visible, let screen = NSScreen.main ?? NSScreen.screens.first,
              let primaryHeight = NSScreen.screens.first?.frame.maxY else { close(); return }
        do {
            let presentation = try model.presentation()
            let monitor = UpdateNoticePlacement.topLeft(screen.frame, primaryHeight: primaryHeight)
            let workArea = UpdateNoticePlacement.topLeft(screen.visibleFrame, primaryHeight: primaryHeight)
            let trayRect: CGRect?
            switch tray {
            case "none": trayRect = nil
            case "bottom": trayRect = CGRect(x: monitor.maxX - 180, y: monitor.maxY - 36, width: 24, height: 36)
            default: trayRect = CGRect(x: monitor.maxX - 180, y: monitor.minY, width: 24, height: 24)
            }
            let placement = try model.placement(monitor: monitor, workArea: workArea, tray: trayRect,
                cardWidth: presentation.cardWidth, cardHeight: presentation.cardHeight)
            let frame = UpdateNoticePlacement.cocoaFrame(placement.frame, primaryHeight: primaryHeight)
            let created = panel == nil
            if panel == nil {
                let panel = UpdateNoticePanel(contentRect: frame, styleMask: [.borderless, .nonactivatingPanel],
                    backing: .buffered, defer: false)
                panel.title = "Captures Update"; panel.isReleasedWhenClosed = false
                panel.isOpaque = false; panel.backgroundColor = .clear; panel.hasShadow = false
                panel.level = .floating; panel.hidesOnDeactivate = false
                panel.becomesKeyOnlyIfNeeded = true
                panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
                panel.escape = { [weak self] in self?.model.perform(.dismiss) }
                let view = UpdateNoticeView(frame: NSRect(origin: .zero, size: frame.size), tokens: tokens)
                view.onAction = { [weak self] action in self?.model.perform(action) }
                panel.contentView = view
                self.panel = panel; self.view = view
            }
            panel?.setFrame(frame, display: false)
            view?.frame = NSRect(origin: .zero, size: frame.size)
            view?.render(presentation, placement: placement)
            panel?.orderFrontRegardless()
            panel?.ignoresMouseEvents = false
            if let view {
                if created {
                    // Shipping `.update-notice`: ui-pop-in over --dur-4.
                    NativeMotion.play("update_notice_in", on: view, tokens: tokens)
                }
                if presentation.visualState == "restarting" {
                    if restartExitEnd == nil {
                        let seconds = NativeMotion.play("update_notice_restart_exit", on: view,
                                                        tokens: tokens, holdEnd: true)
                        restartExitEnd = Date(timeIntervalSinceNow: seconds)
                    }
                } else if restartExitEnd != nil {
                    restartExitEnd = nil
                    NativeMotion.cancel(on: view)
                }
            }
            report(presentation, placement: placement)
            if model.simulating, let interval = model.tickInterval {
                timer = Timer.scheduledTimer(withTimeInterval: interval, repeats: false) { [weak self] _ in
                    self?.model.tick()
                }
            }
        } catch {
            Metrics.write(["event": "update-notice-error", "error": error.localizedDescription])
        }
    }

    private func report(_ p: UpdateNoticePresentation, placement: UpdateNoticePlacement) {
        let row: [String: Any] = [
            "visualState": p.visualState, "title": p.title, "description": p.description,
            "notes": p.notes != nil, "revealNotes": p.revealNotes != nil,
            "primary": p.primary.map { $0.label as Any } ?? NSNull(),
            "dismiss": p.dismiss.map { $0.label as Any } ?? NSNull(),
            "dismissBlocked": p.dismissBlocked, "cardHeight": p.cardHeight, "caret": placement.caret,
        ]
        guard !NSDictionary(dictionary: row).isEqual(to: lastReport) else { return }
        lastReport = row
        var event = row; event["event"] = "update-notice"
        Metrics.write(event)
    }

    deinit { timer?.invalidate(); panel?.close() }
}
