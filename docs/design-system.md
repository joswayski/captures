# Captures Studio design system

Captures Studio is the shared visual language for every desktop surface. It is
designed for a cross-platform capture utility: the interface stays quiet while
the user is working, then becomes explicit at decision points.

## Direction

The system translates common strengths of modern product interfaces without
copying any one product:

- Vercel and Linear: restrained neutral chrome, compact controls, and clear
  hierarchy.
- Notion and Cursor: dense tools that remain readable and predictable.
- Apple and ente: content-first presentation, calm setup, and privacy-aware
  language.
- Intercom and Clay: friendly empty states, polished forms, and useful status
  feedback.

The result is deliberately platform-neutral. Native windows still provide
placement, accessibility, and operating-system integration; React owns a
consistent studio surface inside them.

## Principles

1. **The capture is the hero.** Editors use dark neutral chrome so images and
   video carry the color.
2. **Color communicates.** The selected theme marks primary actions, selection,
   and focus. Recording and destructive states use the paired signal color.
3. **Depth is functional.** Borders define persistent structure. Shadows are
   reserved for floating windows, menus, media, and transient notices.
4. **Decisions are obvious.** Primary actions have one clear treatment;
   secondary actions are neutral; destructive actions stay quiet until hover or
   confirmation.
5. **Density follows the task.** Capture controls and editor chrome are compact.
   Setup, history, preferences, and feedback use more breathing room.
6. **Motion explains change.** Existing selection, preview, and save transitions
   remain; reduced-motion preferences continue to suppress nonessential motion.

## Foundations

- Canvas: near-black neutral (`#0c0d0f`)
- Surface: quiet charcoal (`#141517`)
- Raised surface: `#18191c`
- Field: recessed charcoal (`#0f1012`)
- Text: soft white (`#f4f4f2`)
- Muted text: cool gray (`#a4a5aa`)
- Corners: 6–18 px according to scale, never fully rounded by default
- Typography: the operating system's modern sans-serif stack, with monospace
  reserved for dimensions, time, size, and shortcut values

Theme accent and signal values continue to come from `shared/themes.css`.
Changing a theme never changes the neutral hierarchy or the semantic meaning of
status colors.

## Surface map

| Surface | Design treatment |
| --- | --- |
| First-run setup | Calm light canvas, one permission card, direct next action |
| Capture overlay | Dimmed content, precise selection ring, compact guidance |
| Capture controls | Floating command bar with grouped capture and target modes |
| Screenshot countdown | Full-display number with minimal supporting copy |
| Recording HUD | Compact glass toolbar with isolated status and actions |
| Mini previews | Media-first cards; actions appear on intent |
| Screenshot viewer | Neutral toolbar and distraction-free media stage |
| Screenshot editor | Dark studio canvas, compact tool rail, structured layers and properties |
| Video/GIF editor | Preview-first layout, explicit timeline, grouped output controls |
| Compression comparison | Focused modal with a direct before/after scrubber |
| Capture history | Spacious responsive card grid with quiet metadata |
| Preferences | Sticky section index and one readable settings column |
| Feedback | Focused form cards with clear privacy copy |
| Updates and notices | Small, high-contrast transient surfaces with restrained motion |

## Cross-platform behavior

- No visual treatment depends on macOS traffic lights, Windows titlebar
  geometry, or one desktop environment's control styling.
- System font fallbacks cover macOS, Windows, and Linux.
- Custom inputs, selects, checkboxes, toggles, sliders, and focus rings keep
  decision controls consistent across WebKit and WebView2.
- Transparent-window surfaces include opaque fallbacks when backdrop filters
  are unavailable.
- Narrow-window rules preserve access to settings and actions without assuming
  a desktop's default window size.

## Accessibility

- Keyboard focus uses a visible theme-aware two-pixel ring.
- Selection is never conveyed by color alone: active controls also use fill,
  border, position, or checkmarks.
- Primary text, muted text, controls, and status surfaces retain strong contrast
  against their backgrounds.
- Existing semantic labels, radiogroups, status regions, sliders, and reduced
  motion behavior remain intact.
