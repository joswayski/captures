//! Captures' outline artwork, copied from App.tsx and ScreenshotEditor.tsx.
//! Keep geometry independent of the installed desktop's icon theme.
pub fn body(name: &str) -> Option<&'static str> {
    let name = match name {
        "screenshot" => "capture",
        "microphone-off" => "microphone-muted",
        "loop" => "restart",
        other => other,
    };
    Some(match name {
        "capture" | "camera-photo-symbolic" => {
            r#"<path d="M9 4H7a3 3 0 0 0-3 3v2M15 4h2a3 3 0 0 1 3 3v2M20 15v2a3 3 0 0 1-3 3h-2M9 20H7a3 3 0 0 1-3-3v-2"/><path d="M12 8.5c.4 1.8 1.7 3.1 3.5 3.5-1.8.4-3.1 1.7-3.5 3.5-.4-1.8-1.7-3.1-3.5-3.5 1.8-.4 3.1-1.7 3.5-3.5Z"/>"#
        }
        "region" | "edit-select-all-symbolic" => {
            r#"<path d="M5 9V6a1 1 0 0 1 1-1h3M15 5h3a1 1 0 0 1 1 1v3M19 15v3a1 1 0 0 1-1 1h-3M9 19H6a1 1 0 0 1-1-1v-3"/><rect x="9" y="9" width="6" height="6" rx="1"/>"#
        }
        "window" | "window-new-symbolic" => {
            r#"<rect x="4" y="6" width="16" height="13" rx="2.5"/><path d="M4 10h16M7 8h.01M10 8h.01"/>"#
        }
        "display" | "video-display-symbolic" => {
            r#"<rect x="3" y="4" width="18" height="14" rx="2.5"/><path d="M9 21h6M12 18v3"/>"#
        }
        "close" | "window-close-symbolic" => r#"<path d="m6 6 12 12M18 6 6 18"/>"#,
        "copy" | "edit-copy-symbolic" => {
            r#"<rect x="8" y="8" width="11" height="11" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/>"#
        }
        "folder" | "folder-open-symbolic" => {
            r#"<path d="M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/><circle cx="16.5" cy="13.5" r="2.5"/><path d="m18.3 15.3 2.2 2.2"/>"#
        }
        "edit" | "document-edit-symbolic" => {
            r#"<path d="m4 16-1 5 5-1L19 9l-4-4ZM13.5 6.5l4 4M4 16l4 4"/>"#
        }
        "trash" | "user-trash-symbolic" | "edit-delete-symbolic" => {
            r#"<path d="M4 7h16M9 7V4h6v3m3 0-1 13H7L6 7m4 4v5m4-5v5"/>"#
        }
        "save" | "document-save-symbolic" => {
            r#"<path d="M5 4h12l2 2v14H5Z"/><path d="M8 4v6h8V4M8 20v-6h8v6"/>"#
        }
        "check" | "emblem-ok-symbolic" => r#"<path d="m5 12 4 4L19 6"/>"#,
        "record-dot" => r#"<circle cx="12" cy="12" r="6.75" fill="currentColor" stroke="none"/>"#,
        "play" | "media-playback-start-symbolic" => r#"<path d="m8 5 11 7-11 7Z"/>"#,
        "pause" | "media-playback-pause-symbolic" => r#"<path d="M8 5v14M16 5v14"/>"#,
        "stop" | "media-playback-stop-symbolic" => {
            r#"<rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor" stroke="none"/>"#
        }
        "restart" | "view-refresh-symbolic" => r#"<path d="M4 11a8 8 0 1 1 2 5.3M4 5v6h6"/>"#,
        "history" | "document-open-recent-symbolic" => {
            r#"<path d="M3 12a9 9 0 1 0 3-6.7L3 8M3 3v5h5M12 7v5l3 2"/>"#
        }
        "microphone" | "audio-input-microphone-symbolic" => {
            r#"<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M6 11a6 6 0 0 0 11.4 2.6M12 18v3M9 21h6"/>"#
        }
        "microphone-muted" | "microphone-sensitivity-muted-symbolic" => {
            r#"<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M6 11a6 6 0 0 0 11.4 2.6M12 18v3M9 21h6M4 4l16 16"/>"#
        }
        "hide" | "view-conceal-symbolic" => {
            r#"<path d="m2 2 20 20M6.7 6.7C4.9 8 3.7 9.7 3 12c1.7 4.1 5 7 9 7 1.8 0 3.5-.6 4.9-1.6M10.7 5.1A10.9 10.9 0 0 1 12 5c4 0 7.3 2.9 9 7-.3.8-.7 1.5-1.2 2.2M14.1 14.1a3 3 0 0 1-4.2-4.2"/>"#
        }
        "select" => r#"<path d="m5 3 13 9-7 2-3 7Z"/>"#,
        "crop" => r#"<path d="M7 3v14a2 2 0 0 0 2 2h12M3 7h14a2 2 0 0 1 2 2v12"/>"#,
        "trim" => {
            r#"<rect x="8" y="8" width="8" height="8" rx="1.2"/><path d="M8 4H5a1 1 0 0 0-1 1v3M16 4h3a1 1 0 0 1 1 1v3M4 16v3a1 1 0 0 0 1 1h3M20 16v3a1 1 0 0 1-1 1h-3"/>"#
        }
        "text" => r#"<path d="M5 5h14M12 5v14M8 19h8"/>"#,
        "shapes" => {
            r#"<rect x="3.5" y="8.5" width="11" height="11" rx="1.5"/><circle cx="15.25" cy="9.75" r="5.25"/>"#
        }
        "rectangle" => r#"<rect x="4" y="5" width="16" height="14" rx="2"/>"#,
        "ellipse" => r#"<ellipse cx="12" cy="12" rx="8" ry="6.5"/>"#,
        "line" => r#"<path d="M5 19 19 5"/>"#,
        "triangle" => r#"<path d="M12 4 20.5 19.5H3.5Z"/>"#,
        "diamond" => r#"<path d="M12 3.5 20.5 12 12 20.5 3.5 12Z"/>"#,
        // closedShapePolygon("star", {x:3,y:3,width:18,height:18}), inner ratio .39.
        "star" => {
            r#"<path d="M12 3 L14.063126 9.160350 L20.559509 9.218847 L15.338208 13.084650 L17.290067 19.281153 L12 15.51 L6.709933 19.281153 L8.661792 13.084650 L3.440491 9.218847 L9.936874 9.160350 Z"/>"#
        }
        "arrow" => r#"<path d="M4 20 20 4M12 4h8v8"/>"#,
        "pen" => r#"<path d="M4 16c4-7 6-8 8-3s4 4 8-4M4 20h16"/>"#,
        "remove-bg" => {
            r#"<path d="m14.8 20.5-7.4-7.4a2.4 2.4 0 0 1 0-3.4L13.2 4a2.4 2.4 0 0 1 3.4 0l3.4 3.4a2.4 2.4 0 0 1 0 3.4l-7.4 7.4a2.4 2.4 0 0 1-3.4 0Zm-6.2-8.7 3.6 3.6M4 21h8"/>"#
        }
        "undo" | "edit-undo-symbolic" => r#"<path d="m9 7-5 5 5 5M5 12h8a6 6 0 0 1 6 6"/>"#,
        "redo" | "edit-redo-symbolic" => r#"<path d="m15 7 5 5-5 5M19 12h-8a6 6 0 0 0-6 6"/>"#,
        "fit" | "zoom-fit-best-symbolic" => r#"<path d="M9 4H4v5M15 4h5v5M9 20H4v-5M15 20h5v-5"/>"#,
        "image" | "image-x-generic-symbolic" => {
            r#"<rect x="3" y="4" width="18" height="16" rx="2"/><circle cx="8" cy="9" r="1.5"/><path d="m5 18 5-5 3 3 2-2 4 4"/>"#
        }
        "plus" | "list-add-symbolic" | "zoom-in-symbolic" => r#"<path d="M12 5v14M5 12h14"/>"#,
        "minus" | "zoom-out-symbolic" => r#"<path d="M5 12h14"/>"#,
        "chevron-down" | "go-down-symbolic" => r#"<path d="m7 9 5 5 5-5"/>"#,
        "chevron-up" | "go-up-symbolic" => r#"<path d="m7 15 5-5 5 5"/>"#,
        "lock" | "changes-prevent-symbolic" => {
            r#"<rect x="5" y="10" width="14" height="11" rx="2"/><path d="M8 10V7a4 4 0 0 1 8 0v3"/>"#
        }
        "unlock" | "changes-allow-symbolic" => {
            r#"<rect x="5" y="10" width="14" height="11" rx="2"/><path d="M9 10V7a4 4 0 0 1 7.5-2"/>"#
        }
        "eye" | "view-reveal-symbolic" => {
            r#"<path d="M3 12s3.5-6 9-6 9 6 9 6-3.5 6-9 6-9-6-9-6Z"/><circle cx="12" cy="12" r="2.5"/>"#
        }
        "more" | "view-more-symbolic" => {
            r#"<circle cx="12" cy="5" r="1.4"/><circle cx="12" cy="12" r="1.4"/><circle cx="12" cy="19" r="1.4"/>"#
        }
        _ => return None,
    })
}
