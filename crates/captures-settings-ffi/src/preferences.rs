//! Shipping Preferences copy and presentation policy for AppKit, from
//! `captures-app::preferences` (shared with the wgpu host). Requests are rare:
//! one `copy` per process plus find labels while the find bar is open.
use super::region::{response, text};
use captures_app::{preferences, shortcuts::ShortcutPlatform};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    ffi::c_char,
    panic::{AssertUnwindSafe, catch_unwind},
};

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum PreferencesRequest {
    Copy {
        #[serde(default = "macos")]
        platform: ShortcutPlatform,
    },
    Description {
        key: String,
        #[serde(default)]
        show_mini_previews: bool,
        #[serde(default)]
        include: bool,
        #[serde(default)]
        can_exclude: bool,
    },
    Find {
        query: String,
        texts: Vec<String>,
        index: usize,
    },
}

fn macos() -> ShortcutPlatform {
    ShortcutPlatform::Macos
}

fn pairs(values: &[(&str, &str)]) -> Value {
    values
        .iter()
        .map(|(value, label)| json!({"value": value, "label": label}))
        .collect()
}

fn numbers(values: &[u16], label: impl Fn(u16) -> String) -> Value {
    values
        .iter()
        .map(|value| json!({"value": value, "label": label(*value)}))
        .collect()
}

fn copy(platform: ShortcutPlatform) -> Value {
    let help = preferences::shortcut_help(platform);
    let custom = &preferences::CUSTOM_THEME;
    json!({
        "title": preferences::TITLE,
        "subtitle": preferences::SUBTITLE,
        "history": preferences::HISTORY_ACTION,
        "loading": preferences::LOADING,
        "saving": preferences::SAVING,
        "saved": preferences::SAVED,
        "sections": preferences::SECTIONS.iter().map(|section| json!({
            "id": section.id, "title": section.title, "description": section.description,
        })).collect::<Vec<_>>(),
        "rows": preferences::ROWS.iter().map(|row| (row.key.to_owned(), json!({
            "title": row.title,
            "description": row.description,
            "accessibility_label": preferences::select_accessibility_label(row.key),
        }))).collect::<serde_json::Map<_, _>>(),
        "recording_toggles": preferences::RECORDING_TOGGLES,
        "appearance_modes": pairs(&preferences::APPEARANCE_MODES),
        "themes": preferences::THEMES.iter().map(|theme| json!({
            "id": theme.id, "name": theme.name, "description": theme.description,
            "accessibility_label": preferences::theme_accessibility_label(theme),
        })).collect::<Vec<_>>(),
        "theme_columns": preferences::THEME_COLUMNS,
        "custom_theme": {
            "title": custom.title, "description": custom.description, "reset": custom.reset,
            "fields": custom.fields.iter().map(|(key, label, description)| json!({
                "key": key, "label": label, "description": description,
            })).collect::<Vec<_>>(),
        },
        "mini_preview_placements": pairs(&preferences::MINI_PREVIEW_PLACEMENTS),
        "options": {
            "screenshot_format": pairs(&preferences::SCREENSHOT_FORMATS),
            "screenshot_countdown_seconds": preferences::COUNTDOWN_SECONDS
                .map(|value| json!({"value": value, "label": preferences::countdown_label(value)}))
                .collect::<Vec<_>>(),
            "recording.video_format": pairs(&preferences::RECORDING_FORMATS),
            "recording.video_fps": numbers(&preferences::RECORDING_FPS, preferences::fps_label),
            "recording.video_max_resolution": pairs(&preferences::RESOLUTIONS),
            "recording.countdown_seconds": preferences::COUNTDOWN_SECONDS
                .map(|value| json!({"value": value, "label": preferences::countdown_label(value)}))
                .collect::<Vec<_>>(),
            "recording.gif_fps": numbers(&preferences::GIF_FPS, preferences::fps_label),
            "recording.gif_max_width": numbers(&preferences::GIF_MAX_WIDTHS, preferences::width_label),
            "recording.gif_max_colors": numbers(&preferences::GIF_PALETTE_COLORS, |value| value.to_string()),
        },
        "microphone_off": preferences::MICROPHONE_OFF,
        "microphones_loading": preferences::MICROPHONES_LOADING,
        "shortcuts": {
            "rows": preferences::SHORTCUT_ROWS.iter().map(|(label, path)| json!({
                "label": label, "path": path,
            })).collect::<Vec<_>>(),
            "prompt": preferences::SHORTCUT_PROMPT,
            "fixture_note": preferences::FIXTURE_SHORTCUTS_NOTE,
            "intro": help.intro,
            "system_title": help.system_title,
            "system_body": help.system_body,
            "system_action": help.system_action,
            "system_targets": preferences::keyboard_settings_targets(platform)
                .iter().map(|target| target[0]).collect::<Vec<_>>(),
        },
        "updates": {
            "title": preferences::UPDATES_TITLE,
            "detail": preferences::UPDATES_DETAIL,
            "action": preferences::UPDATES_ACTION,
        },
        "feedback": {
            "title": preferences::FEEDBACK_TITLE,
            "detail": preferences::FEEDBACK_DETAIL,
            "action": preferences::FEEDBACK_ACTION,
        },
        "login_item": {
            "title": preferences::LOGIN_ITEM_TITLE,
            "detail": preferences::LOGIN_ITEM_DETAIL,
            "checking": preferences::LOGIN_ITEM_CHECKING,
            "retry": preferences::LOGIN_ITEM_RETRY,
            "error_template": preferences::login_item_error("{error}"),
            "unavailable": preferences::login_item_unavailable(platform),
        },
        "find": {
            "placeholder": preferences::FIND_PLACEHOLDER,
            "previous": preferences::FIND_PREVIOUS,
            "next": preferences::FIND_NEXT,
            "close": preferences::FIND_CLOSE,
        },
        "save_error_template": preferences::save_error("{error}"),
        "load_error_template": preferences::load_error("{error}"),
    })
}

fn handle(request: PreferencesRequest) -> Result<Value, String> {
    Ok(match request {
        PreferencesRequest::Copy { platform } => copy(platform),
        PreferencesRequest::Description {
            key,
            show_mini_previews,
            include,
            can_exclude,
        } => match key.as_str() {
            "include_mini_previews_in_captures" => json!({
                "lead": preferences::mini_previews_in_captures_description(show_mini_previews, include),
                "emphasis": "", "trail": "",
            }),
            "include_recording_controls_in_captures" => {
                let text = preferences::recording_controls_description(can_exclude, include);
                json!({"lead": text.lead, "emphasis": text.emphasis, "trail": text.trail})
            }
            _ => return Err("unknown dynamic Preferences description".into()),
        },
        PreferencesRequest::Find {
            query,
            texts,
            index,
        } => {
            let matches = texts
                .iter()
                .enumerate()
                .filter(|(_, text)| preferences::find_matches(text, &query))
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            json!({
                "matches": matches,
                "label": preferences::find_count_label(&query, matches.len(), index),
            })
        }
    })
}

/// Preferences copy and policy. Free the response with captures_settings_free_v1.
///
/// # Safety
/// `request_json` is readable NUL-terminated UTF-8 during this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preferences_v1(request_json: *const c_char) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input throughout this call.
        let request = serde_json::from_str::<PreferencesRequest>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        handle(request)
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(result) => json!({"ok": true, "result": result}),
        Err(error) => json!({"ok": false, "error": error}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::captures_settings_free_v1;
    use std::ffi::{CStr, CString};

    fn call(request: Value) -> Value {
        let input = CString::new(request.to_string()).unwrap();
        // SAFETY: input is a live NUL-terminated string for this call.
        let output = unsafe { captures_preferences_v1(input.as_ptr()) };
        // SAFETY: output is an owned response string freed exactly once below.
        let value =
            serde_json::from_str(unsafe { CStr::from_ptr(output) }.to_str().unwrap()).unwrap();
        // SAFETY: returned by this ABI and not used afterwards.
        unsafe { captures_settings_free_v1(output) };
        value
    }

    #[test]
    fn header_declares_preferences_abi() {
        let header = include_str!("../include/captures_settings.h");
        assert!(header.contains("char *captures_preferences_v1(const char *request_json);"));
    }

    #[test]
    fn copy_carries_sections_rows_options_and_platform_help() {
        let copy = call(json!({"operation": "copy"}));
        let result = &copy["result"];
        assert_eq!(result["sections"][4]["title"], "GIF export");
        assert_eq!(
            result["rows"]["recording.gif_max_colors"]["accessibility_label"],
            "GIF palette colors"
        );
        assert_eq!(
            result["options"]["screenshot_countdown_seconds"][0]["label"],
            "Off"
        );
        assert_eq!(
            result["options"]["recording.video_max_resolution"][1]["label"],
            "1080p"
        );
        assert_eq!(result["themes"][9]["id"], "custom");
        assert_eq!(
            result["shortcuts"]["rows"][6]["path"][1],
            "display_shortcut"
        );
        assert_eq!(
            result["shortcuts"]["system_title"],
            "macOS Screenshot shortcuts"
        );
        let linux = call(json!({"operation": "copy", "platform": "linux"}));
        assert_eq!(
            linux["result"]["shortcuts"]["system_title"],
            "GNOME screenshot shortcuts"
        );
    }

    #[test]
    fn descriptions_and_find_round_trip() {
        let will = call(json!({"operation": "description",
            "key": "include_recording_controls_in_captures", "include": true, "can_exclude": true}));
        assert_eq!(will["result"]["emphasis"], "will");
        let find = call(json!({"operation": "find", "query": "screen",
            "texts": ["Freeze screen", "Mono", "Full Screen"], "index": 1}));
        assert_eq!(find["result"]["matches"], json!([0, 2]));
        assert_eq!(find["result"]["label"], "2 of 2");
        assert_eq!(
            call(json!({"operation": "description", "key": "nope"}))["ok"],
            false
        );
    }
}
