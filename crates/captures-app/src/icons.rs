//! The shipping SVG icon set (24-unit viewBox, stroked, `fill: none`),
//! flattened to polylines so every native host strokes identical geometry.
//!
//! Path data is copied from the Tauri UI (`App.tsx`, and `EditorIcon` in
//! `ScreenshotEditor.tsx` for the editor chrome). `<rect rx>` and
//! `<circle>` elements are written as equivalent path data.

/// Stroked path data for a named shipping icon.
pub fn paths(name: &str) -> Option<&'static [&'static str]> {
    Some(match name {
        "pause" => &["M8 5v14M16 5v14"],
        "resume" => &["m8 5 11 7-11 7Z"],
        "restart" => &["M4 11a8 8 0 1 1 2 5.3", "M4 5v6h6"],
        // Vector counterpart to shipping's font-dependent clockwise `↻`.
        "loop" => &["M20 11a8 8 0 1 0-2 5.3", "M20 5v6h-6"],
        "capture" => &[
            "M9 4H7a3 3 0 0 0-3 3v2M15 4h2a3 3 0 0 1 3 3v2M20 15v2a3 3 0 0 1-3 3h-2M9 20H7a3 3 0 0 1-3-3v-2",
            "M12 8.5c.4 1.8 1.7 3.1 3.5 3.5-1.8.4-3.1 1.7-3.5 3.5-.4-1.8-1.7-3.1-3.5-3.5 1.8-.4 3.1-1.7 3.5-3.5Z",
        ],
        // Capture menu targets (`CaptureTargetIcon`). Its `h.01` title-bar
        // dots are written as tiny circles so hosts without round caps still
        // draw them.
        "target-region" => &[
            "M5 9V6a1 1 0 0 1 1-1h3M15 5h3a1 1 0 0 1 1 1v3M19 15v3a1 1 0 0 1-1 1h-3M9 19H6a1 1 0 0 1-1-1v-3",
            "M10 9h4a1 1 0 0 1 1 1v4a1 1 0 0 1 -1 1h-4a1 1 0 0 1 -1 -1v-4a1 1 0 0 1 1 -1Z",
        ],
        "target-window" => &[
            "M6.5 6h11a2.5 2.5 0 0 1 2.5 2.5v8a2.5 2.5 0 0 1 -2.5 2.5h-11a2.5 2.5 0 0 1 -2.5 -2.5v-8a2.5 2.5 0 0 1 2.5 -2.5Z",
            "M4 10h16",
            "M6.8 8a0.2 0.2 0 1 0 0.4 0a0.2 0.2 0 1 0 -0.4 0",
            "M9.8 8a0.2 0.2 0 1 0 0.4 0a0.2 0.2 0 1 0 -0.4 0",
        ],
        "target-display" => &[
            "M5.5 4h13a2.5 2.5 0 0 1 2.5 2.5v9a2.5 2.5 0 0 1 -2.5 2.5h-13a2.5 2.5 0 0 1 -2.5 -2.5v-9a2.5 2.5 0 0 1 2.5 -2.5Z",
            "M9 21h6M12 18v3",
        ],
        "microphone" => &[
            "M12 3a3 3 0 0 1 3 3v5a3 3 0 0 1-6 0V6a3 3 0 0 1 3-3Z",
            "M6 11a6 6 0 0 0 11.4 2.6M12 18v3M9 21h6",
        ],
        "microphone-muted" => &[
            "M12 3a3 3 0 0 1 3 3v5a3 3 0 0 1-6 0V6a3 3 0 0 1 3-3Z",
            "M6 11a6 6 0 0 0 11.4 2.6M12 18v3M9 21h6",
            "m4 4 16 16",
        ],
        "trash" => &["M4 7h16M9 7V4h6v3m3 0-1 13H7L6 7m4 4v5m4-5v5"],
        "hide-controls" => &[
            "m2 2 20 20",
            "M6.7 6.7C4.9 8 3.7 9.7 3 12c1.7 4.1 5 7 9 7 1.8 0 3.5-.6 4.9-1.6",
            "M10.7 5.1A10.9 10.9 0 0 1 12 5c4 0 7.3 2.9 9 7-.3.8-.7 1.5-1.2 2.2",
            "M14.1 14.1a3 3 0 0 1-4.2-4.2",
        ],
        "close" => &["m6 6 12 12M18 6 6 18"],
        "check" => &["m5 12 4 4L19 6"],
        // Lucide Share2 for the native sharing entry point, on every OS.
        "share" => &[
            "M15 5a3 3 0 1 0 6 0a3 3 0 1 0-6 0",
            "M3 12a3 3 0 1 0 6 0a3 3 0 1 0-6 0",
            "M15 19a3 3 0 1 0 6 0a3 3 0 1 0-6 0",
            "m8.59 13.51 6.83 3.98M15.41 6.51l-6.82 3.98",
        ],
        "warning" => &[
            "M10.29 4.86 1.82 19a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 4.86a2 2 0 0 0-3.42 0Z",
            "M12 9.5v5.2M12 17.6h.01",
        ],
        "history" => &["M3 12a9 9 0 1 0 3-6.7L3 8", "M3 3v5h5M12 7v5l3 2"],
        // These shipping SVGs use a 16-unit viewBox. `polylines`
        // normalizes them to the native hosts' shared 24-unit coordinate space.
        "external-link" => &[
            "M6.5 3H4a1 1 0 0 0-1 1v8a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1V9.5",
            "M9 3h4v4M8.5 7.5 13 3",
        ],
        "preview-stack" => &[
            "m3 5.5 5-2.75 5 2.75-5 2.75L3 5.5Z",
            "m3.5 8.5 4.5 2.5 4.5-2.5",
            "m4.5 11 3.5 2 3.5-2",
        ],
        "preview-overflow-up" => &["M3.5 10 8 5.5 12.5 10"],
        "preview-overflow-down" => &["M3.5 6 8 10.5 12.5 6"],
        // Text alignment (`EditorIcon` `align-*`).
        "align-left" => &["M5 6h14M5 10h10M5 14h14M5 18h10"],
        "align-center" => &["M5 6h14M8 10h8M5 14h14M8 18h8"],
        "align-right" => &["M5 6h14M9 10h10M5 14h14M9 18h10"],
        "restore" => &["M4 12a8 8 0 1 0 2.3-5.7L4 8", "M4 4v4h4"],
        "copy" => &[
            "M10 8h7a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2h-7a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2Z",
            "M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2",
        ],
        "save" => &["M5 4h12l2 2v14H5Z", "M8 4v6h8V4M8 20v-6h8v6"],
        "folder" => &[
            "M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z",
            "M14 13.5a2.5 2.5 0 1 0 5 0a2.5 2.5 0 1 0-5 0",
            "m18.3 15.3 2.2 2.2",
        ],
        "edit" => &["m4 16-1 5 5-1L19 9l-4-4ZM13.5 6.5l4 4M4 16l4 4"],
        // Preferences find steps and select triggers (16-unit `m4 10 4-4 4 4`
        // and `m4 6 4 4 4-4`, scaled to the 24-unit grid).
        "chevron-up" => &["m6 15 6-6 6 6"],
        "chevron-down" => &["m6 9 6 6 6-6"],
        // Screenshot editor chrome (`EditorIcon` in `ScreenshotEditor.tsx`).
        "editor-chevron-down" => &["m7 9 5 5 5-5"],
        "editor-chevron-up" => &["m7 15 5-5 5 5"],
        "select" => &["m5 3 13 9-7 2-3 7Z"],
        "crop" => &["M7 3v14a2 2 0 0 0 2 2h12M3 7h14a2 2 0 0 1 2 2v12"],
        "trim" => &[
            "M9.2 8h5.6a1.2 1.2 0 0 1 1.2 1.2v5.6a1.2 1.2 0 0 1 -1.2 1.2h-5.6a1.2 1.2 0 0 1 -1.2 -1.2v-5.6a1.2 1.2 0 0 1 1.2 -1.2Z",
            "M8 4H5a1 1 0 0 0-1 1v3M16 4h3a1 1 0 0 1 1 1v3M4 16v3a1 1 0 0 0 1 1h3M20 16v3a1 1 0 0 1-1 1h-3",
        ],
        "text" => &["M5 5h14M12 5v14M8 19h8"],
        "shapes" => &[
            "M5 8.5h8a1.5 1.5 0 0 1 1.5 1.5v8a1.5 1.5 0 0 1 -1.5 1.5h-8a1.5 1.5 0 0 1 -1.5 -1.5v-8a1.5 1.5 0 0 1 1.5 -1.5Z",
            "M10 9.75a5.25 5.25 0 1 0 10.5 0a5.25 5.25 0 1 0 -10.5 0",
        ],
        "rectangle" => {
            &["M6 5h12a2 2 0 0 1 2 2v10a2 2 0 0 1 -2 2h-12a2 2 0 0 1 -2 -2v-10a2 2 0 0 1 2 -2Z"]
        }
        "ellipse" => &["M4 12a8 6.5 0 1 0 16 0a8 6.5 0 1 0 -16 0"],
        "line" => &["M5 19 19 5"],
        "triangle" => &["M12 4 20.5 19.5H3.5Z"],
        "diamond" => &["M12 3.5 20.5 12 12 20.5 3.5 12Z"],
        "star" => &[
            "M12 3L14.06 9.16L20.56 9.22L15.34 13.08L17.29 19.28L12 15.51L6.71 19.28L8.66 13.08L3.44 9.22L9.94 9.16Z",
        ],
        "arrow" => &["M4 20 20 4M12 4h8v8"],
        "pen" => &["M4 16c4-7 6-8 8-3s4 4 8-4M4 20h16"],
        "remove-bg" => &[
            "m14.8 20.5-7.4-7.4a2.4 2.4 0 0 1 0-3.4L13.2 4a2.4 2.4 0 0 1 3.4 0l3.4 3.4a2.4 2.4 0 0 1 0 3.4l-7.4 7.4a2.4 2.4 0 0 1-3.4 0Z",
            "m8.6 11.8 3.6 3.6",
            "M4 21h8",
        ],
        "undo" => &["m9 7-5 5 5 5M5 12h8a6 6 0 0 1 6 6"],
        "redo" => &["m15 7 5 5-5 5M19 12h-8a6 6 0 0 0-6 6"],
        "fit" => &["M9 4H4v5M15 4h5v5M9 20H4v-5M15 20h5v-5"],
        "image" => &[
            "M5 4h14a2 2 0 0 1 2 2v12a2 2 0 0 1 -2 2h-14a2 2 0 0 1 -2 -2v-12a2 2 0 0 1 2 -2Z",
            "M6.5 9a1.5 1.5 0 1 0 3 0a1.5 1.5 0 1 0 -3 0",
            "m5 18 5-5 3 3 2-2 4 4",
        ],
        "plus" => &["M12 5v14M5 12h14"],
        "minus" => &["M5 12h14"],
        "lock" => &[
            "M7 10h10a2 2 0 0 1 2 2v7a2 2 0 0 1 -2 2h-10a2 2 0 0 1 -2 -2v-7a2 2 0 0 1 2 -2Z",
            "M8 10V7a4 4 0 0 1 8 0v3",
        ],
        "unlock" => &[
            "M7 10h10a2 2 0 0 1 2 2v7a2 2 0 0 1 -2 2h-10a2 2 0 0 1 -2 -2v-7a2 2 0 0 1 2 -2Z",
            "M9 10V7a4 4 0 0 1 7.5-2",
        ],
        "eye" => &[
            "M3 12s3.5-6 9-6 9 6 9 6-3.5 6-9 6-9-6-9-6Z",
            "M9.5 12a2.5 2.5 0 1 0 5 0a2.5 2.5 0 1 0 -5 0",
        ],
        "eye-off" => &[
            "m4 4 16 16M9.5 6.4A9 9 0 0 1 12 6c5.5 0 9 6 9 6a15 15 0 0 1-2.2 2.9M14.4 17.6A9 9 0 0 1 12 18c-5.5 0-9-6-9-6a15 15 0 0 1 2.1-2.8",
        ],
        "more" => &[
            "M10.6 5a1.4 1.4 0 1 0 2.8 0a1.4 1.4 0 1 0 -2.8 0",
            "M10.6 12a1.4 1.4 0 1 0 2.8 0a1.4 1.4 0 1 0 -2.8 0",
            "M10.6 19a1.4 1.4 0 1 0 2.8 0a1.4 1.4 0 1 0 -2.8 0",
        ],
        "grip" => &[
            "M8.2 7a0.8 0.8 0 1 0 1.6 0a0.8 0.8 0 1 0 -1.6 0",
            "M14.2 7a0.8 0.8 0 1 0 1.6 0a0.8 0.8 0 1 0 -1.6 0",
            "M8.2 12a0.8 0.8 0 1 0 1.6 0a0.8 0.8 0 1 0 -1.6 0",
            "M14.2 12a0.8 0.8 0 1 0 1.6 0a0.8 0.8 0 1 0 -1.6 0",
            "M8.2 17a0.8 0.8 0 1 0 1.6 0a0.8 0.8 0 1 0 -1.6 0",
            "M14.2 17a0.8 0.8 0 1 0 1.6 0a0.8 0.8 0 1 0 -1.6 0",
        ],
        "duplicate" => &[
            "M10 8h7a2 2 0 0 1 2 2v7a2 2 0 0 1 -2 2h-7a2 2 0 0 1 -2 -2v-7a2 2 0 0 1 2 -2Z",
            "M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3M13.5 11v5M11 13.5h5",
        ],
        "rotate-counterclockwise" => &["M8 7H4V3", "M4.7 7.2A8 8 0 1 1 4 14"],
        "rotate-clockwise" => &["M16 7h4V3", "M19.3 7.2A8 8 0 1 0 20 14"],
        "flip-horizontal" => &["M12 3v18M9 5 4 12l5 7ZM15 5l5 7-5 7Z"],
        "flip-vertical" => &["M3 12h18M5 9l7-5 7 5ZM5 15l7 5 7-5Z"],
        "bring-front" => &[
            "M6.2 12h7.6a1.2 1.2 0 0 1 1.2 1.2v5.6a1.2 1.2 0 0 1 -1.2 1.2h-7.6a1.2 1.2 0 0 1 -1.2 -1.2v-5.6a1.2 1.2 0 0 1 1.2 -1.2Z",
            "M10.2 4h7.6a1.2 1.2 0 0 1 1.2 1.2v5.6a1.2 1.2 0 0 1 -1.2 1.2h-7.6a1.2 1.2 0 0 1 -1.2 -1.2v-5.6a1.2 1.2 0 0 1 1.2 -1.2Z",
        ],
        "send-back" => &[
            "M10.2 4h7.6a1.2 1.2 0 0 1 1.2 1.2v5.6a1.2 1.2 0 0 1 -1.2 1.2h-7.6a1.2 1.2 0 0 1 -1.2 -1.2v-5.6a1.2 1.2 0 0 1 1.2 -1.2Z",
            "M6.2 12h7.6a1.2 1.2 0 0 1 1.2 1.2v5.6a1.2 1.2 0 0 1 -1.2 1.2h-7.6a1.2 1.2 0 0 1 -1.2 -1.2v-5.6a1.2 1.2 0 0 1 1.2 -1.2Z",
        ],
        "merge-down" => &["M7 4h10v4H7z", "M12 9v5", "m9 12 3 3 3-3", "M5 17h14v3H5z"],
        "merge-visible" => &[
            "M7 3h10v3H7z",
            "M7 8h10v3H7z",
            "M12 12v3",
            "m9 13.5 3 3 3-3",
            "M5 18h14v3H5z",
        ],
        "flatten" => &[
            "M6 4h12v2.5H6z",
            "M6 8.5h12v2.5H6z",
            "M6 13h12v2.5H6z",
            "M4 18h16v2.5H4z",
        ],
        _ => return None,
    })
}

/// Every named icon's polylines in 24-unit space (y down).
pub fn polylines(name: &str) -> Option<Vec<Vec<[f32; 2]>>> {
    let mut lines: Vec<Vec<[f32; 2]>> = paths(name)?.iter().flat_map(|d| flatten(d)).collect();
    if matches!(
        name,
        "external-link" | "preview-stack" | "preview-overflow-up" | "preview-overflow-down"
    ) {
        for point in lines.iter_mut().flatten() {
            point[0] *= 1.5;
            point[1] *= 1.5;
        }
    }
    Some(lines)
}

/// Flatten SVG path data to polylines. Curves and arcs become short segments.
pub fn flatten(d: &str) -> Vec<Vec<[f32; 2]>> {
    let tokens = tokenize(d);
    let mut out: Vec<Vec<[f32; 2]>> = Vec::new();
    let mut current = [0f32; 2];
    let mut start = [0f32; 2];
    // Reflection points for S/T.
    let mut last_cubic: Option<[f32; 2]> = None;
    let mut last_quad: Option<[f32; 2]> = None;
    let mut index = 0;
    let mut command = 'M';
    let number = |tokens: &[Token], index: &mut usize| -> Option<f32> {
        match tokens.get(*index) {
            Some(Token::Number(value)) => {
                *index += 1;
                Some(*value)
            }
            _ => None,
        }
    };
    while index < tokens.len() {
        if let Token::Command(next) = tokens[index] {
            command = next;
            index += 1;
            if matches!(command, 'Z' | 'z') {
                if let Some(path) = out.last_mut() {
                    path.push(start);
                }
                current = start;
                last_cubic = None;
                last_quad = None;
                continue;
            }
        }
        let relative = command.is_ascii_lowercase();
        let base = if relative { current } else { [0., 0.] };
        let mut cubic = None;
        let mut quad = None;
        match command.to_ascii_uppercase() {
            'M' => {
                let (Some(x), Some(y)) = (number(&tokens, &mut index), number(&tokens, &mut index))
                else {
                    break;
                };
                current = [base[0] + x, base[1] + y];
                start = current;
                out.push(vec![current]);
                // Further pairs after a moveto are implicit linetos.
                command = if relative { 'l' } else { 'L' };
            }
            'L' => {
                let (Some(x), Some(y)) = (number(&tokens, &mut index), number(&tokens, &mut index))
                else {
                    break;
                };
                current = [base[0] + x, base[1] + y];
                push(&mut out, current);
            }
            'H' => {
                let Some(x) = number(&tokens, &mut index) else {
                    break;
                };
                current = [base[0] + x, current[1]];
                push(&mut out, current);
            }
            'V' => {
                let Some(y) = number(&tokens, &mut index) else {
                    break;
                };
                current = [current[0], base[1] + y];
                push(&mut out, current);
            }
            'C' | 'S' => {
                let first = if command.eq_ignore_ascii_case(&'C') {
                    let (Some(x), Some(y)) =
                        (number(&tokens, &mut index), number(&tokens, &mut index))
                    else {
                        break;
                    };
                    [base[0] + x, base[1] + y]
                } else {
                    last_cubic.map_or(current, |control| {
                        [2. * current[0] - control[0], 2. * current[1] - control[1]]
                    })
                };
                let (Some(x2), Some(y2), Some(x), Some(y)) = (
                    number(&tokens, &mut index),
                    number(&tokens, &mut index),
                    number(&tokens, &mut index),
                    number(&tokens, &mut index),
                ) else {
                    break;
                };
                let second = [base[0] + x2, base[1] + y2];
                let end = [base[0] + x, base[1] + y];
                for step in 1..=CURVE_STEPS {
                    let t = step as f32 / CURVE_STEPS as f32;
                    let u = 1. - t;
                    let point = |axis: usize| {
                        u * u * u * current[axis]
                            + 3. * u * u * t * first[axis]
                            + 3. * u * t * t * second[axis]
                            + t * t * t * end[axis]
                    };
                    push(&mut out, [point(0), point(1)]);
                }
                cubic = Some(second);
                current = end;
            }
            'Q' | 'T' => {
                let control = if command.eq_ignore_ascii_case(&'Q') {
                    let (Some(x), Some(y)) =
                        (number(&tokens, &mut index), number(&tokens, &mut index))
                    else {
                        break;
                    };
                    [base[0] + x, base[1] + y]
                } else {
                    last_quad.map_or(current, |control| {
                        [2. * current[0] - control[0], 2. * current[1] - control[1]]
                    })
                };
                let (Some(x), Some(y)) = (number(&tokens, &mut index), number(&tokens, &mut index))
                else {
                    break;
                };
                let end = [base[0] + x, base[1] + y];
                for step in 1..=CURVE_STEPS {
                    let t = step as f32 / CURVE_STEPS as f32;
                    let u = 1. - t;
                    let point = |axis: usize| {
                        u * u * current[axis] + 2. * u * t * control[axis] + t * t * end[axis]
                    };
                    push(&mut out, [point(0), point(1)]);
                }
                quad = Some(control);
                current = end;
            }
            'A' => {
                let (
                    Some(rx),
                    Some(ry),
                    Some(rotation),
                    Some(large),
                    Some(sweep),
                    Some(x),
                    Some(y),
                ) = (
                    number(&tokens, &mut index),
                    number(&tokens, &mut index),
                    number(&tokens, &mut index),
                    number(&tokens, &mut index),
                    number(&tokens, &mut index),
                    number(&tokens, &mut index),
                    number(&tokens, &mut index),
                )
                else {
                    break;
                };
                let end = [base[0] + x, base[1] + y];
                for point in arc(current, end, rx, ry, rotation, large != 0., sweep != 0.) {
                    push(&mut out, point);
                }
                current = end;
            }
            _ => break,
        }
        last_cubic = cubic;
        last_quad = quad;
    }
    out.retain(|path| path.len() > 1);
    out
}

const CURVE_STEPS: usize = 12;

fn push(out: &mut Vec<Vec<[f32; 2]>>, point: [f32; 2]) {
    match out.last_mut() {
        Some(path) => path.push(point),
        None => out.push(vec![point]),
    }
}

/// SVG endpoint arc (spec F.6.5) as points after `from`, ending exactly at `to`.
fn arc(
    from: [f32; 2],
    to: [f32; 2],
    rx: f32,
    ry: f32,
    rotation: f32,
    large: bool,
    sweep: bool,
) -> Vec<[f32; 2]> {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0. || ry == 0. || from == to {
        return vec![to];
    }
    let phi = rotation.to_radians();
    let (sin, cos) = phi.sin_cos();
    let dx = (from[0] - to[0]) / 2.;
    let dy = (from[1] - to[1]) / 2.;
    let x1 = cos * dx + sin * dy;
    let y1 = -sin * dx + cos * dy;
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1. {
        rx *= lambda.sqrt();
        ry *= lambda.sqrt();
    }
    let numerator = (rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1).max(0.);
    let denominator = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut coefficient = (numerator / denominator).sqrt();
    if large == sweep {
        coefficient = -coefficient;
    }
    let cx1 = coefficient * rx * y1 / ry;
    let cy1 = -coefficient * ry * x1 / rx;
    let cx = cos * cx1 - sin * cy1 + (from[0] + to[0]) / 2.;
    let cy = sin * cx1 + cos * cy1 + (from[1] + to[1]) / 2.;
    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| {
        let dot = ux * vx + uy * vy;
        let length = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let value = (dot / length).clamp(-1., 1.).acos();
        if ux * vy - uy * vx < 0. {
            -value
        } else {
            value
        }
    };
    let start = angle(1., 0., (x1 - cx1) / rx, (y1 - cy1) / ry);
    let mut delta = angle(
        (x1 - cx1) / rx,
        (y1 - cy1) / ry,
        (-x1 - cx1) / rx,
        (-y1 - cy1) / ry,
    );
    if !sweep && delta > 0. {
        delta -= std::f32::consts::TAU;
    } else if sweep && delta < 0. {
        delta += std::f32::consts::TAU;
    }
    let steps = ((delta.abs() / 10f32.to_radians()).ceil() as usize).max(2);
    let mut points: Vec<[f32; 2]> = (1..steps)
        .map(|step| {
            let theta = start + delta * step as f32 / steps as f32;
            let (s, c) = theta.sin_cos();
            [
                cos * rx * c - sin * ry * s + cx,
                sin * rx * c + cos * ry * s + cy,
            ]
        })
        .collect();
    points.push(to);
    points
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Token {
    Command(char),
    Number(f32),
}

fn tokenize(d: &str) -> Vec<Token> {
    let bytes = d.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_alphabetic() && !matches!(byte, b'e' | b'E') {
            tokens.push(Token::Command(byte as char));
            index += 1;
        } else if byte.is_ascii_digit() || matches!(byte, b'-' | b'+' | b'.') {
            let start = index;
            index += 1;
            let mut seen_dot = byte == b'.';
            while index < bytes.len() {
                let next = bytes[index];
                if next.is_ascii_digit() {
                    index += 1;
                } else if next == b'.' && !seen_dot {
                    seen_dot = true;
                    index += 1;
                } else if matches!(next, b'e' | b'E') {
                    index += 1;
                    if index < bytes.len() && matches!(bytes[index], b'-' | b'+') {
                        index += 1;
                    }
                } else {
                    break;
                }
            }
            if let Ok(value) = d[start..index].parse() {
                tokens.push(Token::Number(value));
            }
        } else {
            index += 1;
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    #[test]
    fn lines_relative_commands_and_implicit_linetos_flatten_exactly() {
        assert_eq!(
            flatten("m6 6 12 12M18 6 6 18"),
            vec![vec![[6., 6.], [18., 18.]], vec![[18., 6.], [6., 18.]]]
        );
        assert_eq!(
            flatten("M5 4h12l2 2v14H5Z"),
            vec![vec![
                [5., 4.],
                [17., 4.],
                [19., 6.],
                [19., 20.],
                [5., 20.],
                [5., 4.]
            ]]
        );
        assert_eq!(flatten("m8 5 11 7-11 7Z")[0].last(), Some(&[8., 5.]));
    }

    #[test]
    fn curves_and_arcs_end_on_their_endpoints() {
        let arc = flatten("M4 11a8 8 0 1 1 2 5.3");
        assert!(close(*arc[0].last().unwrap(), [6., 16.3]));
        // A large sweep around a radius-8 circle stays on it.
        for point in &arc[0] {
            let _ = point;
        }
        let circle = flatten("M14 13.5a2.5 2.5 0 1 0 5 0a2.5 2.5 0 1 0-5 0");
        for point in &circle[0] {
            let radius = ((point[0] - 16.5).powi(2) + (point[1] - 13.5).powi(2)).sqrt();
            assert!((radius - 2.5).abs() < 1e-3, "{point:?}");
        }
        let spark = flatten(paths("capture").unwrap()[1]);
        assert!(close(*spark[0].last().unwrap(), [12., 8.5]));
        assert!(spark[0].len() > 40, "cubic segments are subdivided");
    }

    #[test]
    fn loop_arrow_points_clockwise_on_the_right() {
        let loop_paths = paths("loop").unwrap();
        let arc = flatten(loop_paths[0]);
        assert_eq!(arc[0][0], [20., 11.]);
        assert!(close(*arc[0].last().unwrap(), [18., 16.3]));
        assert_eq!(
            flatten(loop_paths[1]),
            vec![vec![[20., 5.], [20., 11.], [14., 11.]]]
        );
    }

    #[test]
    fn preview_paths_match_shipping_svg_sources() {
        let app = include_str!("../../../apps/desktop/ui/src/App.tsx");
        for (name, component) in [
            ("external-link", "ExternalPreferenceIcon"),
            ("warning", "WarningIcon"),
            ("capture", "CaptureIcon"),
            ("close", "CloseIcon"),
            ("trash", "TrashIcon"),
            ("edit", "EditIcon"),
            ("save", "SaveIcon"),
            ("check", "CheckIcon"),
            ("history", "HistoryIcon"),
            ("preview-stack", "PreviewStackIcon"),
            ("preview-overflow-up", "ThumbnailOverflowChevron"),
            ("preview-overflow-down", "ThumbnailOverflowChevron"),
        ] {
            let body = app.split_once(&format!("function {component}(")).unwrap().1;
            let body = body.split_once("\n}").unwrap().0;
            for path in paths(name).unwrap() {
                assert!(body.contains(&format!("\"{path}\"")), "{component}: {path}");
            }
        }
    }

    #[test]
    fn editor_disclosure_matches_shipping_path_and_rotated_expansion() {
        let editor = include_str!("../../../apps/desktop/ui/src/ScreenshotEditor.tsx");
        assert!(editor.contains(paths("editor-chevron-down").unwrap()[0]));
        let down = polylines("editor-chevron-down").unwrap();
        let up = polylines("editor-chevron-up").unwrap();
        let rotated: Vec<_> = down[0]
            .iter()
            .rev()
            .map(|p| [24. - p[0], 24. - p[1]])
            .collect();
        assert_eq!(up[0], rotated);
    }

    #[test]
    fn sixteen_unit_preview_icons_normalize_to_twenty_four_units() {
        let external = polylines("external-link").unwrap();
        assert_eq!(external[0][0], [9.75, 4.5]);
        assert_eq!(external[0].last(), Some(&[19.5, 14.25]));
        assert!(
            external[0].len() > 8,
            "rounded box corners must not become square"
        );
        assert_eq!(polylines("preview-stack").unwrap()[0][0], [4.5, 8.25]);
        assert_eq!(
            polylines("preview-overflow-up").unwrap()[0],
            [[5.25, 15.], [12., 8.25], [18.75, 15.]]
        );
        assert_eq!(
            polylines("preview-overflow-down").unwrap()[0],
            [[5.25, 9.], [12., 15.75], [18.75, 9.]]
        );
        assert_eq!(
            polylines("check").unwrap()[0],
            [[5., 12.], [9., 16.], [19., 6.]]
        );
    }

    #[test]
    fn every_named_icon_flattens_to_drawable_paths() {
        for name in [
            "pause",
            "resume",
            "restart",
            "loop",
            "capture",
            "target-region",
            "target-window",
            "target-display",
            "microphone",
            "microphone-muted",
            "trash",
            "hide-controls",
            "close",
            "check",
            "warning",
            "restore",
            "copy",
            "save",
            "folder",
            "edit",
            "history",
            "external-link",
            "preview-stack",
            "preview-overflow-up",
            "preview-overflow-down",
            "chevron-up",
            "chevron-down",
            "editor-chevron-up",
            "editor-chevron-down",
            "align-left",
            "align-center",
            "align-right",
        ] {
            let lines = polylines(name).unwrap();
            assert!(!lines.is_empty(), "{name}");
            for line in lines {
                assert!(line.len() >= 2, "{name}");
                for [x, y] in line {
                    assert!(
                        (-0.5..=24.5).contains(&x) && (-0.5..=24.5).contains(&y),
                        "{name} {x},{y}"
                    );
                }
            }
        }
        assert!(polylines("unknown").is_none());
    }
}
