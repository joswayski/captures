//! Pure shortcut recorder and display policy, matched to the shipping recorder.
//! Hosts translate native physical keys into DOM codes; this never registers keys.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ShortcutPlatform {
    Macos,
    Windows,
    Linux,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutKeyEvent {
    pub code: String,
    pub ctrl_key: bool,
    pub shift_key: bool,
    pub alt_key: bool,
    pub meta_key: bool,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShortcutRecording {
    Cancel,
    Waiting { keys: Vec<String> },
    Invalid { keys: Vec<String>, message: String },
    Complete { keys: Vec<String>, shortcut: String },
}

pub fn record_shortcut(event: &ShortcutKeyEvent, platform: ShortcutPlatform) -> ShortcutRecording {
    if event.code == "Escape" {
        return ShortcutRecording::Cancel;
    }
    let mut tokens = Vec::new();
    for (pressed, name) in [
        (event.ctrl_key, "Control"),
        (event.shift_key, "Shift"),
        (event.alt_key, "Alt"),
        (event.meta_key, "Super"),
    ] {
        if pressed {
            tokens.push(name);
        }
    }
    let mut keys: Vec<_> = tokens
        .iter()
        .map(|token| display_token(token, platform))
        .collect();
    if matches!(
        event.code.as_str(),
        "AltLeft"
            | "AltRight"
            | "ControlLeft"
            | "ControlRight"
            | "MetaLeft"
            | "MetaRight"
            | "OSLeft"
            | "OSRight"
            | "ShiftLeft"
            | "ShiftRight"
    ) {
        return ShortcutRecording::Waiting { keys };
    }
    keys.push(display_token(&event.code, platform));
    let message = if !supported_code(&event.code) {
        Some("That key cannot be used as a global shortcut.")
    } else if tokens.is_empty() && event.code != "PrintScreen" {
        Some(match platform {
            ShortcutPlatform::Macos => {
                "Include Ctrl, Shift, Option, or Command, or use Print Screen."
            }
            ShortcutPlatform::Windows => "Include Ctrl, Shift, Alt, or Win, or use Print Screen.",
            ShortcutPlatform::Linux => "Include Ctrl, Shift, Alt, or Super, or use Print Screen.",
        })
    } else {
        None
    };
    if let Some(message) = message {
        return ShortcutRecording::Invalid {
            keys,
            message: message.into(),
        };
    }
    tokens.push(&event.code);
    ShortcutRecording::Complete {
        keys,
        shortcut: tokens.join("+"),
    }
}

fn supported_code(code: &str) -> bool {
    let single_ascii = |prefix, predicate: fn(u8) -> bool| {
        code.strip_prefix(prefix)
            .is_some_and(|suffix| suffix.len() == 1 && predicate(suffix.as_bytes()[0]))
    };
    single_ascii("Key", |c| c.is_ascii_uppercase())
        || single_ascii("Digit", |c| c.is_ascii_digit())
        || single_ascii("Numpad", |c| c.is_ascii_digit())
        || code.strip_prefix('F').is_some_and(|suffix| {
            suffix
                .parse::<u8>()
                .is_ok_and(|number| (1..=24).contains(&number) && suffix == number.to_string())
        })
        || matches!(
            code,
            "AudioVolumeDown"
                | "AudioVolumeMute"
                | "AudioVolumeUp"
                | "Backquote"
                | "Backslash"
                | "Backspace"
                | "BracketLeft"
                | "BracketRight"
                | "CapsLock"
                | "Comma"
                | "Delete"
                | "End"
                | "Enter"
                | "Equal"
                | "Home"
                | "Insert"
                | "MediaPause"
                | "MediaPlay"
                | "MediaPlayPause"
                | "MediaStop"
                | "MediaTrackNext"
                | "MediaTrackPrevious"
                | "Minus"
                | "NumLock"
                | "NumpadAdd"
                | "NumpadDecimal"
                | "NumpadDivide"
                | "NumpadEnter"
                | "NumpadEqual"
                | "NumpadMultiply"
                | "NumpadSubtract"
                | "PageDown"
                | "PageUp"
                | "Pause"
                | "Period"
                | "PrintScreen"
                | "Quote"
                | "ScrollLock"
                | "Semicolon"
                | "Slash"
                | "Space"
                | "Tab"
                | "ArrowDown"
                | "ArrowLeft"
                | "ArrowRight"
                | "ArrowUp"
        )
}

pub fn shortcut_display_tokens(shortcut: &str, platform: ShortcutPlatform) -> Vec<String> {
    shortcut
        .split('+')
        .map(|token| display_token(token.trim(), platform))
        .filter(|token| !token.is_empty())
        .collect()
}

fn display_token(token: &str, platform: ShortcutPlatform) -> String {
    let token = token.trim();
    let lower = token.to_ascii_lowercase();
    let mac = matches!(platform, ShortcutPlatform::Macos);
    let name = match lower.as_str() {
        "control" | "ctrl" => Some("Ctrl"),
        "shift" => Some("Shift"),
        "alt" | "option" => Some(if mac { "Option" } else { "Alt" }),
        "command" | "cmd" | "super" | "meta" => Some(match platform {
            ShortcutPlatform::Macos => "Cmd",
            ShortcutPlatform::Windows => "Win",
            ShortcutPlatform::Linux => "Super",
        }),
        "commandorcontrol" | "commandorctrl" | "cmdorcontrol" | "cmdorctrl" => {
            Some(if mac { "Cmd" } else { "Ctrl" })
        }
        "printscreen" | "prtscn" | "print" => Some("PrtScn"),
        _ => None,
    };
    if let Some(name) = name {
        return name.into();
    }
    let name = match token {
        "Backquote" => "`",
        "Backslash" => "\\",
        "BracketLeft" => "[",
        "BracketRight" => "]",
        "Comma" => ",",
        "Equal" => "=",
        "Minus" => "-",
        "Period" => ".",
        "Quote" => "'",
        "Semicolon" => ";",
        "Slash" => "/",
        "Space" => "Space",
        "ArrowDown" => "↓",
        "ArrowLeft" => "←",
        "ArrowRight" => "→",
        "ArrowUp" => "↑",
        "NumpadAdd" => "Num +",
        "NumpadDecimal" => "Num .",
        "NumpadDivide" => "Num /",
        "NumpadEnter" => "Num Enter",
        "NumpadEqual" => "Num =",
        "NumpadMultiply" => "Num ×",
        "NumpadSubtract" => "Num -",
        "Backspace" if mac => "Delete",
        "Enter" if mac => "Return",
        _ => token,
    };
    if name != token {
        return name.into();
    }
    if let Some(key) = lower.strip_prefix("key")
        && key.len() == 1
        && key.as_bytes()[0].is_ascii_lowercase()
    {
        return key.to_ascii_uppercase();
    }
    for (prefix, label) in [("digit", ""), ("numpad", "Num ")] {
        if let Some(key) = lower.strip_prefix(prefix)
            && key.len() == 1
            && key.as_bytes()[0].is_ascii_digit()
        {
            return format!("{label}{key}");
        }
    }
    token.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct RecordCase {
        event: ShortcutKeyEvent,
        platform: ShortcutPlatform,
        expected: ShortcutRecording,
    }
    #[derive(Deserialize)]
    struct DisplayCase {
        shortcut: String,
        platform: ShortcutPlatform,
        expected: Vec<String>,
    }
    #[derive(Deserialize)]
    struct Cases {
        recording: Vec<RecordCase>,
        display: Vec<DisplayCase>,
    }

    #[test]
    fn recorder_and_labels_match_shipping_typescript() {
        let cases: Cases =
            serde_json::from_str(include_str!("../../tests/shortcut-golden.json")).unwrap();
        for (index, case) in cases.recording.into_iter().enumerate() {
            assert_eq!(
                record_shortcut(&case.event, case.platform),
                case.expected,
                "recording case {index}"
            );
        }
        for (index, case) in cases.display.into_iter().enumerate() {
            assert_eq!(
                shortcut_display_tokens(&case.shortcut, case.platform),
                case.expected,
                "display case {index}"
            );
        }
    }
}
