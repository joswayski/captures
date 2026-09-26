import AppKit
import CCapturesSettings

enum FeedbackBridge {
    private static let queue = DispatchQueue(label: "es.captures.native.feedback", qos: .utility)

    /// One synchronous round trip through `captures_feedback_request_v1`.
    static func call(_ object: [String: Any]) throws -> [String: Any] {
        let data = try JSONSerialization.data(withJSONObject: object)
        let pointer = String(decoding: data, as: UTF8.self).withCString { captures_feedback_request_v1($0) }
        guard let pointer else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(pointer) }
        let responseData = Data(bytes: pointer, count: strlen(pointer))
        guard let response = try JSONSerialization.jsonObject(with: responseData) as? [String: Any],
              let ok = response["ok"] as? Bool else { throw AppBridgeError.invalidResponse }
        guard ok else { throw AppBridgeError.backend(response.string("error", "Feedback could not be sent.")) }
        guard let value = response["result"] as? [String: Any] else { throw AppBridgeError.invalidResponse }
        return value
    }

    static func request(_ object: [String: Any], completion: @escaping (Result<[String: Any], Error>) -> Void) {
        queue.async {
            let result = Result { try call(object) }
            DispatchQueue.main.async { completion(result) }
        }
    }
}

/// Shipping `Feedback.tsx` copy, placeholders and limits, shared with the wgpu
/// host through `captures_app::feedback` (the bridge's local `copy` operation).
enum FeedbackCopy {
    struct Category {
        let id: String
        let label: String
        let detail: String
        let placeholder: String
    }

    static let values: [String: Any] = {
        do { return try FeedbackBridge.call(["operation": "copy"]) } catch {
            preconditionFailure("Feedback copy is unavailable: \(error)")
        }
    }()

    static func text(_ key: String) -> String { values.string(key) }
    static func number(_ key: String) -> CGFloat { CGFloat((values[key] as? NSNumber)?.doubleValue ?? 0) }
    static func limit(_ key: String) -> Int { (values[key] as? NSNumber)?.intValue ?? 0 }

    static let categories: [Category] = ((values["categories"] as? [[String: Any]]) ?? []).map {
        Category(id: $0.string("id"), label: $0.string("label"), detail: $0.string("description"),
                 placeholder: $0.string("placeholder"))
    }
}

/// A label that never takes clicks, e.g. the message placeholder drawn over
/// the text view.
final class FeedbackPassthroughLabel: NSTextField {
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

/// Shipping `.ui-input` text: horizontal padding and vertical centering for the
/// borderless Contact field, while drawing and while editing.
final class FeedbackFieldCell: NSTextFieldCell {
    var horizontalInset: CGFloat = 12
    private var editingOrSelecting = false

    override func drawingRect(forBounds rect: NSRect) -> NSRect {
        var frame = super.drawingRect(forBounds: rect)
        guard !editingOrSelecting else { return frame }
        frame = frame.insetBy(dx: horizontalInset, dy: 0)
        let lineHeight = ceil((font?.ascender ?? 10) - (font?.descender ?? -3) + (font?.leading ?? 0))
        if lineHeight < frame.height {
            frame.origin.y += (frame.height - lineHeight) / 2
            frame.size.height = lineHeight
        }
        return frame
    }

    override func select(withFrame rect: NSRect, in controlView: NSView, editor textObj: NSText,
                         delegate: Any?, start selStart: Int, length selLength: Int) {
        let frame = drawingRect(forBounds: rect)
        editingOrSelecting = true
        super.select(withFrame: frame, in: controlView, editor: textObj, delegate: delegate,
                     start: selStart, length: selLength)
        editingOrSelecting = false
    }

    override func edit(withFrame rect: NSRect, in controlView: NSView, editor textObj: NSText,
                       delegate: Any?, event: NSEvent?) {
        let frame = drawingRect(forBounds: rect)
        editingOrSelecting = true
        super.edit(withFrame: frame, in: controlView, editor: textObj, delegate: delegate, event: event)
        editingOrSelecting = false
    }
}

/// One shipping `.feedback-category` radio card: a label over a description.
final class FeedbackCategoryButton: PreferenceHoverButton {
    var tokens: Tokens { didSet { needsDisplay = true } }
    let optionTitle: String
    let optionDetail: String
    var active = false { didSet { setAccessibilityValue(active); needsDisplay = true } }

    init(_ category: FeedbackCopy.Category, tokens: Tokens, onPress: @escaping () -> Void) {
        self.tokens = tokens; optionTitle = category.label; optionDetail = category.detail
        super.init(frame: .zero)
        title = category.label; actionBlock = onPress
        configure(label: category.label, role: .radioButton)
        setAccessibilityHelp(category.detail)
        setAccessibilityValue(false)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    private var padding: CGFloat { tokens.number("s-4") }
    private var titleFont: NSFont { .systemFont(ofSize: tokens.number("text-md"), weight: .medium) }
    private var detailFont: NSFont { .systemFont(ofSize: tokens.number("text-xs")) }

    /// The card height for `width`, at least shipping's 62 pt.
    func fittingHeight(width: CGFloat) -> CGFloat {
        let textWidth = max(1, width - padding * 2)
        return max(62, padding * 2 + FeedbackController.textHeight(optionTitle, font: titleFont, width: textWidth)
            + 3 + FeedbackController.textHeight(optionDetail, font: detailFont, width: textWidth))
    }

    override func draw(_ dirtyRect: NSRect) {
        let alpha: CGFloat = isEnabled ? 1 : 0.55
        let radius = tokens.number("r-md")
        let hover = isEnabled && hovered && !active
        let fill = active ? "surface-selected" : hover ? "surface-hover" : "surface-canvas"
        let border = active ? "theme-accent" : hover ? "border-strong" : "border"
        // Shipping's 1 pt border plus a 1 pt inset accent shadow when active.
        let width: CGFloat = active ? 2 : 1
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: width / 2, dy: width / 2),
                                xRadius: radius, yRadius: radius)
        tokens.color(fill).withAlphaComponent(alpha).setFill(); path.fill()
        tokens.color(border).withAlphaComponent(alpha).setStroke(); path.lineWidth = width; path.stroke()
        let textWidth = max(1, bounds.width - padding * 2)
        let titleHeight = FeedbackController.textHeight(optionTitle, font: titleFont, width: textWidth)
        (optionTitle as NSString).draw(with: NSRect(x: padding, y: padding, width: textWidth, height: titleHeight),
            options: [.usesLineFragmentOrigin, .usesFontLeading],
            attributes: [.font: titleFont, .foregroundColor: tokens.color("text").withAlphaComponent(alpha)])
        let detailTop = padding + titleHeight + 3
        (optionDetail as NSString).draw(with: NSRect(x: padding, y: detailTop, width: textWidth,
                                                     height: max(0, bounds.height - detailTop)),
            options: [.usesLineFragmentOrigin, .usesFontLeading],
            attributes: [.font: detailFont, .foregroundColor: tokens.color("text-subtle").withAlphaComponent(alpha)])
        if isFocused { drawFocusRing(tokens, in: bounds, radius: radius) }
    }
}

/// Shipping `.feedback-submit`: the one accent action, dimmed while disabled.
final class FeedbackSendButton: PreferenceHoverButton {
    var tokens: Tokens { didSet { needsDisplay = true } }

    init(tokens: Tokens, onPress: @escaping () -> Void) {
        self.tokens = tokens
        super.init(frame: .zero)
        title = FeedbackCopy.text("send"); actionBlock = onPress
        configure(label: title, role: .button)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    var labelFont: NSFont { .systemFont(ofSize: tokens.number("text-md"), weight: .semibold) }

    func fittingWidth() -> CGFloat {
        ceil((title as NSString).size(withAttributes: [.font: labelFont]).width) + tokens.number("s-6") * 2
    }

    override func draw(_ dirtyRect: NSRect) {
        let alpha: CGFloat = isEnabled ? 1 : 0.45
        let radius = tokens.number("r-md")
        let path = NSBezierPath(roundedRect: bounds, xRadius: radius, yRadius: radius)
        tokens.color(isEnabled && (hovered || cell?.isHighlighted == true) ? "theme-accent-hover" : "theme-accent")
            .withAlphaComponent(alpha).setFill()
        path.fill()
        let attributes: [NSAttributedString.Key: Any] = [
            .font: labelFont, .foregroundColor: tokens.color("theme-accent-ink").withAlphaComponent(alpha),
        ]
        let size = (title as NSString).size(withAttributes: attributes)
        (title as NSString).draw(at: NSPoint(x: (bounds.width - size.width) / 2, y: (bounds.height - size.height) / 2),
                                 withAttributes: attributes)
        if isFocused { drawFocusRing(tokens, in: bounds.insetBy(dx: -2, dy: -2), radius: radius + 2) }
    }
}

/// The message field; reports focus so its border can follow `:focus`.
final class FeedbackTextView: NSTextView {
    var focusChanged: ((Bool) -> Void)?
    override func becomeFirstResponder() -> Bool {
        let accepted = super.becomeFirstResponder()
        if accepted { focusChanged?(true) }
        return accepted
    }
    override func resignFirstResponder() -> Bool {
        let accepted = super.resignFirstResponder()
        if accepted { focusChanged?(false) }
        return accepted
    }
}

/// Reports every size change so the form can reflow, like shipping's window.
final class FeedbackScrollView: NSScrollView {
    var onResize: (() -> Void)?
    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        onResize?()
    }
}

/// Shipping `Feedback.tsx` in its own window. Retained by the workbench, so
/// closing/reopening preserves the draft and any in-flight request. No network
/// call happens until explicit Send.
final class FeedbackController: NSObject, NSTextViewDelegate, NSTextFieldDelegate, NSWindowDelegate {
    typealias Request = ([String: Any], @escaping (Result<[String: Any], Error>) -> Void) -> Void
    let window: NSWindow
    let scrollView = FeedbackScrollView()
    let document = Surface()
    let message = FeedbackTextView()
    let contact = NSTextField(string: "")
    private(set) var categoryButtons: [FeedbackCategoryButton] = []
    private(set) var selectedCategory = 0
    private(set) var sendButton: FeedbackSendButton!
    private(set) var sending = false
    let status = NSTextField(wrappingLabelWithString: "")
    let statusBox = Surface()
    let messagePlaceholder = FeedbackPassthroughLabel(labelWithString: "")
    let versionValue = NSTextField(wrappingLabelWithString: FeedbackCopy.text("loading"))
    let systemValue = NSTextField(wrappingLabelWithString: FeedbackCopy.text("loading"))
    private let eyebrow = NSTextField(labelWithString: FeedbackCopy.text("eyebrow").uppercased())
    private let heading = NSTextField(labelWithString: FeedbackCopy.text("title"))
    private let intro = NSTextField(wrappingLabelWithString: FeedbackCopy.text("intro"))
    private let formCard = Surface()
    private let metaCard = Surface()
    private let categoryLabel = NSTextField(labelWithString: FeedbackCopy.text("category_label"))
    private let categoryGroup = Surface()
    private let messageLabel = NSTextField(labelWithString: FeedbackCopy.text("message_label"))
    private let messageScroll = NSScrollView()
    private let contactLabel = NSTextField(labelWithString: FeedbackCopy.text("contact_label"))
    private let optionalBadge = NSTextField(labelWithString: FeedbackCopy.text("optional_badge").uppercased())
    private let contactHelp = NSTextField(wrappingLabelWithString: FeedbackCopy.text("contact_help"))
    private let metaTitle = NSTextField(labelWithString: FeedbackCopy.text("meta_title"))
    private let versionLabel = NSTextField(labelWithString: FeedbackCopy.text("app_version_label"))
    private let systemLabel = NSTextField(labelWithString: FeedbackCopy.text("system_label"))
    private var rules: [Surface] = []
    private let request: Request
    private let live: Bool
    private var contextReady = false
    private var contextError = false
    private var sent = false
    private var failure: String?
    private var messageFocused = false
    private var tokens: Tokens
    private var laidOutWidth: CGFloat = -1
    private let messageLimit = FeedbackCopy.limit("message_limit")
    private let contactLimit = FeedbackCopy.limit("contact_limit")

    init(tokens: Tokens, live: Bool, request: @escaping Request = FeedbackBridge.request) {
        self.tokens = tokens; self.live = live; self.request = request
        let size = NSSize(width: FeedbackCopy.number("window_width"), height: FeedbackCopy.number("window_height"))
        window = NSWindow(contentRect: NSRect(origin: .zero, size: size),
                          styleMask: [.titled, .closable, .miniaturizable, .resizable],
                          backing: .buffered, defer: false)
        super.init()
        window.isReleasedWhenClosed = false
        window.title = FeedbackCopy.text("window_title"); window.delegate = self
        window.contentMinSize = NSSize(width: FeedbackCopy.number("window_min_width"),
                                       height: FeedbackCopy.number("window_min_height"))
        scrollView.hasVerticalScroller = true; scrollView.autohidesScrollers = true
        scrollView.useTokenScrollers(tokens)
        scrollView.borderType = .noBorder; scrollView.drawsBackground = true
        scrollView.documentView = document
        window.contentView = scrollView
        build()
        restyle(tokens)
        scrollView.onResize = { [weak self] in self?.layoutForm() }
        layoutForm()
        loadContext()
    }

    // MARK: Structure

    private func build() {
        for view in [eyebrow, heading, intro, formCard, metaCard, statusBox] as [NSView] { document.addSubview(view) }
        formCard.addSubview(categoryLabel)
        categoryGroup.setAccessibilityElement(true)
        categoryGroup.setAccessibilityRole(.radioGroup)
        categoryGroup.setAccessibilityLabel(FeedbackCopy.text("category_label"))
        formCard.addSubview(categoryGroup)
        for (index, category) in FeedbackCopy.categories.enumerated() {
            let button = FeedbackCategoryButton(category, tokens: tokens) { [weak self] in self?.selectCategory(index) }
            button.active = index == selectedCategory
            categoryGroup.addSubview(button); categoryButtons.append(button)
        }
        messageLabel.identifier = NSUserInterfaceItemIdentifier("feedback-message-label")
        formCard.addSubview(messageLabel)
        messageScroll.hasVerticalScroller = true; messageScroll.autohidesScrollers = true
        messageScroll.useTokenScrollers(tokens)
        messageScroll.borderType = .noBorder; messageScroll.drawsBackground = false
        messageScroll.wantsLayer = true
        message.isRichText = false; message.isVerticallyResizable = true
        message.isHorizontallyResizable = false; message.allowsUndo = true
        message.textContainer?.widthTracksTextView = true
        message.textContainerInset = NSSize(width: tokens.number("s-5") - 5, height: tokens.number("s-4"))
        message.drawsBackground = false
        message.delegate = self; message.setAccessibilityLabel("Feedback message")
        message.focusChanged = { [weak self] focused in
            self?.messageFocused = focused; self?.updateFieldBorders()
        }
        messagePlaceholder.isBordered = false; messagePlaceholder.drawsBackground = false
        messagePlaceholder.setAccessibilityElement(false)
        message.addSubview(messagePlaceholder)
        messageScroll.documentView = message
        formCard.addSubview(messageScroll)
        formCard.addSubview(contactLabel)
        optionalBadge.alignment = .center; optionalBadge.wantsLayer = true
        optionalBadge.setAccessibilityElement(false)
        formCard.addSubview(optionalBadge)
        let cell = FeedbackFieldCell(textCell: "")
        cell.isEditable = true; cell.isSelectable = true; cell.isScrollable = true
        cell.usesSingleLineMode = true; cell.wraps = false
        contact.cell = cell
        contact.isBordered = false; contact.drawsBackground = false; contact.focusRingType = .none
        contact.wantsLayer = true
        contact.placeholderString = FeedbackCopy.text("contact_placeholder")
        contact.delegate = self; contact.setAccessibilityLabel("Contact (optional)")
        formCard.addSubview(contact)
        formCard.addSubview(contactHelp)
        for view in [metaTitle, versionLabel, versionValue, systemLabel, systemValue] { metaCard.addSubview(view) }
        for _ in 0..<3 {
            let rule = Surface(); rule.wantsLayer = true; rules.append(rule)
        }
        formCard.addSubview(rules[0]); formCard.addSubview(rules[1]); metaCard.addSubview(rules[2])
        status.maximumNumberOfLines = 0
        statusBox.wantsLayer = true; statusBox.addSubview(status)
        sendButton = FeedbackSendButton(tokens: tokens) { [weak self] in
            guard let self else { return }
            if self.contextError { self.loadContext() } else { self.submit() }
        }
        document.addSubview(sendButton)
        for card in [formCard, metaCard] { card.wantsLayer = true }
        updatePlaceholder()
    }

    // MARK: Presentation

    /// Shipping `show_feedback`: reuse and focus the one window.
    func present() {
        if !window.isVisible { window.center() }
        window.makeKeyAndOrderFront(nil)
        if window.firstResponder == nil || window.firstResponder === window { window.makeFirstResponder(message) }
        NSApp.activate(ignoringOtherApps: true)
    }

    func dismiss() {
        window.orderOut(nil)
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool { dismiss(); return false }

    func restyle(_ tokens: Tokens) {
        self.tokens = tokens
        let dark = tokens.color("text").brightnessComponent > 0.5
        window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
        scrollView.backgroundColor = tokens.color("surface-canvas")
        restyleTokenScrollers(in: scrollView, tokens)
        document.wantsLayer = true; document.layer?.backgroundColor = tokens.color("surface-canvas").cgColor
        func style(_ label: NSTextField, size: String, color: String, weight: NSFont.Weight = .regular) {
            label.font = .systemFont(ofSize: tokens.number(size), weight: weight)
            label.textColor = tokens.color(color)
        }
        style(eyebrow, size: "text-2xs", color: "text-subtle", weight: .semibold)
        eyebrow.attributedStringValue = NSAttributedString(string: FeedbackCopy.text("eyebrow").uppercased(), attributes: [
            .font: NSFont.systemFont(ofSize: tokens.number("text-2xs"), weight: .semibold),
            .foregroundColor: tokens.color("text-subtle"), .kern: tokens.number("text-2xs") * 0.04,
        ])
        style(heading, size: "text-2xl", color: "text", weight: .semibold)
        style(intro, size: "text-sm", color: "text-subtle")
        for label in [categoryLabel, messageLabel, contactLabel] { style(label, size: "text-sm", color: "text-muted", weight: .medium) }
        style(optionalBadge, size: "text-2xs", color: "text-subtle", weight: .medium)
        optionalBadge.attributedStringValue = NSAttributedString(string: FeedbackCopy.text("optional_badge").uppercased(), attributes: [
            .font: NSFont.systemFont(ofSize: tokens.number("text-2xs"), weight: .medium),
            .foregroundColor: tokens.color("text-subtle"), .kern: tokens.number("text-2xs") * 0.04,
        ])
        optionalBadge.layer?.backgroundColor = tokens.color("surface-sunken").cgColor
        style(contactHelp, size: "text-sm", color: "text-subtle")
        style(metaTitle, size: "text-md", color: "text", weight: .medium)
        for label in [versionLabel, systemLabel] { style(label, size: "text-sm", color: "text-subtle") }
        for value in [versionValue, systemValue] {
            value.font = .monospacedSystemFont(ofSize: tokens.number("text-sm"), weight: .regular)
            value.textColor = tokens.color("text")
        }
        for card in [formCard, metaCard] {
            card.layer?.cornerRadius = tokens.number("r-xl")
            card.layer?.backgroundColor = tokens.color("surface-raised").cgColor
            card.layer?.borderWidth = 1; card.layer?.borderColor = tokens.color("border-subtle").cgColor
            card.layer?.shadowColor = NSColor.black.cgColor
            card.layer?.shadowOpacity = dark ? 0.32 : 0.08
            card.layer?.shadowRadius = dark ? 3 : 1.5
            card.layer?.shadowOffset = NSSize(width: 0, height: dark ? -2 : -1)
        }
        for rule in rules { rule.layer?.backgroundColor = tokens.color("border-subtle").cgColor }
        message.textColor = tokens.color("text")
        message.insertionPointColor = tokens.color("text")
        message.font = .systemFont(ofSize: tokens.number("text-md"))
        messagePlaceholder.font = .systemFont(ofSize: tokens.number("text-md"))
        messagePlaceholder.textColor = tokens.color("text-faint")
        contact.font = .systemFont(ofSize: tokens.number("text-md"))
        contact.textColor = tokens.color("text")
        (contact.cell as? FeedbackFieldCell)?.horizontalInset = tokens.number("s-5")
        contact.placeholderAttributedString = NSAttributedString(string: FeedbackCopy.text("contact_placeholder"), attributes: [
            .font: NSFont.systemFont(ofSize: tokens.number("text-md")), .foregroundColor: tokens.color("text-faint"),
        ])
        for field in [messageScroll as NSView, contact] {
            field.layer?.cornerRadius = tokens.number("r-md")
            field.layer?.backgroundColor = tokens.color("surface-field").cgColor
            field.layer?.borderWidth = 1
        }
        status.font = .systemFont(ofSize: tokens.number("text-sm"))
        statusBox.layer?.cornerRadius = tokens.number("r-md")
        for button in categoryButtons { button.tokens = tokens }
        sendButton.tokens = tokens
        updateFieldBorders()
        updateStatus()
        laidOutWidth = -1
        layoutForm()
    }

    // MARK: Layout

    static func textHeight(_ text: String, font: NSFont, width: CGFloat) -> CGFloat {
        guard !text.isEmpty else { return 0 }
        let bounds = (text as NSString).boundingRect(
            with: NSSize(width: max(1, width - 4), height: .greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading], attributes: [.font: font])
        return ceil(bounds.height) + 2
    }

    private func height(_ label: NSTextField, width: CGFloat) -> CGFloat {
        Self.textHeight(label.stringValue, font: label.font ?? .systemFont(ofSize: 12), width: width)
    }

    /// Shipping `.feedback` / `.feedback-shell`: a centered 640 pt column with
    /// `--s-9`/`--s-8`/`--s-11` padding, two settings cards and the footer.
    func layoutForm() {
        let width = scrollView.contentSize.width
        guard width > 0 else { return }
        let s = { (name: String) in self.tokens.number(name) }
        let shell = min(width - s("s-8") * 2, FeedbackCopy.number("shell_max_width"))
        let x = (width - shell) / 2
        var y = s("s-9")
        eyebrow.frame = NSRect(x: x, y: y, width: shell, height: height(eyebrow, width: shell))
        y = eyebrow.frame.maxY + s("s-2")
        heading.frame = NSRect(x: x, y: y, width: shell, height: height(heading, width: shell))
        y = heading.frame.maxY + s("s-4")
        let introWidth = min(shell, 56 * s("text-sm") * 0.56)
        intro.frame = NSRect(x: x, y: y, width: introWidth, height: height(intro, width: introWidth))
        y = intro.frame.maxY + s("s-6")

        // Form card: Category, Message, Contact separated by subtle rules.
        let pad = s("s-6"), inner = shell - pad * 2
        var cy = pad
        categoryLabel.frame = NSRect(x: pad, y: cy, width: inner, height: height(categoryLabel, width: inner))
        cy = categoryLabel.frame.maxY + s("s-3")
        let gap = s("s-3")
        let cardWidth = floor((inner - gap * 2) / 3)
        let cardHeight = categoryButtons.map { $0.fittingHeight(width: cardWidth) }.max() ?? 62
        categoryGroup.frame = NSRect(x: pad, y: cy, width: inner, height: cardHeight)
        for (index, button) in categoryButtons.enumerated() {
            let left = CGFloat(index) * (cardWidth + gap)
            button.frame = NSRect(x: left, y: 0, width: index == 2 ? inner - left : cardWidth, height: cardHeight)
        }
        cy = categoryGroup.frame.maxY + s("s-5")
        rules[0].frame = NSRect(x: pad, y: cy, width: inner, height: 1)
        cy += 1 + s("s-5")
        messageLabel.frame = NSRect(x: pad, y: cy, width: inner, height: height(messageLabel, width: inner))
        cy = messageLabel.frame.maxY + s("s-3")
        messageScroll.frame = NSRect(x: pad, y: cy, width: inner, height: 148)
        let messageWidth = messageScroll.contentSize.width
        if message.frame.width != messageWidth || message.frame.height < messageScroll.contentSize.height {
            message.frame = NSRect(x: 0, y: 0, width: messageWidth,
                                   height: max(message.frame.height, messageScroll.contentSize.height))
        }
        message.minSize = NSSize(width: 0, height: messageScroll.contentSize.height)
        message.maxSize = NSSize(width: messageWidth, height: .greatestFiniteMagnitude)
        let padding = message.textContainer?.lineFragmentPadding ?? 5
        let placeholderX = message.textContainerInset.width + padding
        let placeholderWidth = max(1, messageWidth - placeholderX * 2)
        messagePlaceholder.frame = NSRect(x: placeholderX, y: message.textContainerInset.height,
            width: placeholderWidth, height: height(messagePlaceholder, width: placeholderWidth))
        cy = messageScroll.frame.maxY + s("s-5")
        rules[1].frame = NSRect(x: pad, y: cy, width: inner, height: 1)
        cy += 1 + s("s-5")
        let labelSize = contactLabel.intrinsicContentSize
        contactLabel.frame = NSRect(x: pad, y: cy, width: ceil(labelSize.width), height: height(contactLabel, width: inner))
        let badgeSize = optionalBadge.attributedStringValue.size()
        let badgeHeight = ceil(badgeSize.height) + 2
        optionalBadge.frame = NSRect(x: contactLabel.frame.maxX + s("s-2"),
            y: contactLabel.frame.midY - badgeHeight / 2,
            width: ceil(badgeSize.width) + s("s-3") * 2 + 4, height: badgeHeight)
        optionalBadge.layer?.cornerRadius = badgeHeight / 2
        cy = contactLabel.frame.maxY + s("s-3")
        contact.frame = NSRect(x: pad, y: cy, width: inner, height: s("h-lg"))
        cy = contact.frame.maxY + s("s-3")
        contactHelp.frame = NSRect(x: pad, y: cy, width: inner, height: height(contactHelp, width: inner))
        cy = contactHelp.frame.maxY + pad
        formCard.frame = NSRect(x: x, y: y, width: shell, height: cy)
        y = formCard.frame.maxY + s("s-6")

        // "Included automatically": a 112 pt label column and monospace values.
        var my = pad
        metaTitle.frame = NSRect(x: pad, y: my, width: inner, height: height(metaTitle, width: inner))
        my = metaTitle.frame.maxY + s("s-4")
        rules[2].frame = NSRect(x: pad, y: my, width: inner, height: 1)
        my += 1 + s("s-5")
        let valueX = pad + 112 + s("s-4"), valueWidth = max(1, shell - pad - valueX)
        for (label, value) in [(versionLabel, versionValue), (systemLabel, systemValue)] {
            let rowHeight = max(height(label, width: 112), height(value, width: valueWidth))
            label.frame = NSRect(x: pad, y: my, width: 112, height: rowHeight)
            value.frame = NSRect(x: valueX, y: my, width: valueWidth, height: rowHeight)
            my += rowHeight + s("s-3")
        }
        metaCard.frame = NSRect(x: x, y: y, width: shell, height: my - s("s-3") + pad)
        y = metaCard.frame.maxY + s("s-6")

        // `.feedback-actions`: Send on the right, the status filling the rest.
        let buttonWidth = sendButton.fittingWidth(), buttonHeight = s("h-lg")
        let statusWidth = max(1, shell - buttonWidth - s("s-5"))
        let statusTextWidth = max(1, statusWidth - s("s-4") * 2)
        let statusHeight = statusBox.isHidden ? 0 : height(status, width: statusTextWidth) + s("s-3") * 2
        let rowHeight = max(buttonHeight, statusHeight)
        sendButton.frame = NSRect(x: x + shell - buttonWidth, y: y + (rowHeight - buttonHeight) / 2,
                                  width: buttonWidth, height: buttonHeight)
        statusBox.frame = NSRect(x: x, y: y + (rowHeight - statusHeight) / 2, width: statusWidth, height: statusHeight)
        status.frame = NSRect(x: s("s-4"), y: s("s-3"), width: statusTextWidth, height: max(0, statusHeight - s("s-3") * 2))
        y += rowHeight + s("s-11")
        document.frame = NSRect(x: 0, y: 0, width: width, height: max(y, scrollView.contentSize.height))
        laidOutWidth = width
    }

    // MARK: State

    func selectCategory(_ index: Int) {
        guard index >= 0, index < categoryButtons.count, !sending else { return }
        selectedCategory = index
        for (other, button) in categoryButtons.enumerated() { button.active = other == index }
        updatePlaceholder()
    }

    private func updatePlaceholder() {
        let categories = FeedbackCopy.categories
        messagePlaceholder.stringValue = selectedCategory < categories.count ? categories[selectedCategory].placeholder : ""
        messagePlaceholder.isHidden = !message.string.isEmpty
    }

    private func updateFieldBorders() {
        messageScroll.layer?.borderColor = tokens.color(messageFocused ? "theme-accent" : "control-border").cgColor
        let contactFocused = contact.currentEditor() != nil
        contact.layer?.borderColor = tokens.color(contactFocused ? "theme-accent" : "control-border").cgColor
    }

    /// Shipping's footer note: the error or sent message, or why a fixture
    /// cannot send.
    private func updateStatus() {
        let kind: String
        if let failure {
            status.stringValue = failure; kind = "error"
        } else if sent {
            status.stringValue = FeedbackCopy.text("sent"); kind = "sent"
        } else if !live {
            status.stringValue = FeedbackCopy.text("fixture_disabled"); kind = "note"
        } else {
            status.stringValue = ""; kind = "note"
        }
        status.textColor = tokens.color(kind == "error" ? "danger-text" : kind == "sent" ? "positive-text" : "text-muted")
        statusBox.layer?.backgroundColor = tokens.color(kind == "error" ? "danger-surface"
            : kind == "sent" ? "positive-surface" : "surface-sunken").cgColor
        let hidden = status.stringValue.isEmpty
        if statusBox.isHidden != hidden || !hidden {
            statusBox.isHidden = hidden
            if laidOutWidth >= 0 { layoutForm() }
        }
    }

    private func loadContext() {
        contextError = false; failure = nil
        versionValue.stringValue = FeedbackCopy.text("loading"); systemValue.stringValue = FeedbackCopy.text("loading")
        updateControls()
        request(["operation": "context"]) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let value):
                guard let version = value["app_version"] as? String,
                      let system = value["system_label"] as? String else {
                    contextFailed(AppBridgeError.invalidResponse); return
                }
                versionValue.stringValue = version
                systemValue.stringValue = system
                contextReady = true
            case .failure(let error): contextFailed(error); return
            }
            updateControls()
            layoutForm()
        }
    }

    private func contextFailed(_ error: Error) {
        contextError = true; contextReady = false
        versionValue.stringValue = FeedbackCopy.text("context_error"); systemValue.stringValue = ""
        failure = error.localizedDescription
        updateControls()
        layoutForm()
    }

    func updateControls() {
        let valid = copyCanSubmit()
        message.isEditable = !sending; contact.isEnabled = !sending
        for button in categoryButtons { button.isEnabled = !sending }
        sendButton.title = contextError ? FeedbackCopy.text("retry_context")
            : sending ? FeedbackCopy.text("sending") : FeedbackCopy.text("send")
        sendButton.setAccessibilityLabel(sendButton.title)
        sendButton.isEnabled = contextError || (live && contextReady && valid)
        sendButton.needsDisplay = true
        updatePlaceholder()
        updateStatus()
        if laidOutWidth >= 0, abs(sendButton.frame.width - sendButton.fittingWidth()) > 0.5 { layoutForm() }
    }

    /// `captures_app::feedback::can_submit`, with limits from the shared copy.
    private func copyCanSubmit() -> Bool {
        !sending && !message.string.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && message.string.unicodeScalars.count <= messageLimit
            && contact.stringValue.unicodeScalars.count <= contactLimit
    }

    /// Shipping `maxLength`: typing or pasting past the limit is truncated.
    private static func clamp(_ text: String, to limit: Int) -> String? {
        guard text.unicodeScalars.count > limit else { return nil }
        var scalars = String.UnicodeScalarView()
        scalars.append(contentsOf: text.unicodeScalars.prefix(limit))
        return String(scalars)
    }

    func textDidChange(_ notification: Notification) {
        if let clamped = Self.clamp(message.string, to: messageLimit) { message.string = clamped }
        // Shipping clears a sent/error note as soon as the message changes.
        failure = nil; sent = false
        updateControls()
    }

    func controlTextDidBeginEditing(_ obj: Notification) { updateFieldBorders() }
    func controlTextDidEndEditing(_ obj: Notification) { updateFieldBorders() }

    func controlTextDidChange(_ notification: Notification) {
        if let clamped = Self.clamp(contact.stringValue, to: contactLimit) { contact.stringValue = clamped }
        updateControls()
    }

    func submit() {
        updateControls()
        guard live, contextReady, !sending, sendButton.isEnabled else { return }
        let contactText = contact.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let categories = FeedbackCopy.categories
        let draft: [String: Any] = ["message": message.string.trimmingCharacters(in: .whitespacesAndNewlines),
            "contact": contactText.isEmpty ? NSNull() : contactText as Any,
            "category": selectedCategory < categories.count ? categories[selectedCategory].id : "other"]
        sending = true; sent = false; failure = nil; updateControls()
        request(["operation": "submit", "draft": draft]) { [weak self] result in
            guard let self else { return }
            sending = false
            switch result {
            case .success:
                sent = true
                message.string = ""
            case .failure(let error):
                failure = error.localizedDescription
            }
            updateControls()
        }
    }
}
