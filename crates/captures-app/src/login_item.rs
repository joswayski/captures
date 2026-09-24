//! Per-profile, user-owned development login registration.

use sha2::{Digest, Sha256};
#[cfg(any(unix, test))]
use std::fs;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

const ARGUMENTS: [&str; 4] = ["--live", "--scene", "idle", "--history-root"];

/// Queries or explicitly changes the login item for this development profile.
///
/// The profile identity is derived from the canonical History directory. Existing
/// entries are changed only when their complete contents match what we own.
pub fn configure(
    history_root: &Path,
    settings_file: &Path,
    enabled: Option<bool>,
) -> Result<bool, String> {
    let history_root = history_root
        .canonicalize()
        .map_err(|error| format!("History root is not an existing directory: {error}"))?;
    if !history_root.is_dir() {
        return Err("History root is not a directory".into());
    }
    let settings_file = std::path::absolute(settings_file).map_err(|e| e.to_string())?;
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not locate the current executable: {error}"))?;
    if !executable.is_absolute() || !executable.is_file() {
        return Err("Current executable is not an absolute existing file".into());
    }
    for path in [&executable, &history_root, &settings_file] {
        if path.to_str().is_none_or(|value| value.contains('\0')) {
            return Err("Login item paths must be valid Unicode without NUL characters".into());
        }
    }
    configure_platform(&executable, &history_root, &settings_file, enabled)
}

fn identity(history_root: &Path) -> String {
    let digest = Sha256::digest(history_root.as_os_str().as_encoded_bytes());
    format!("{:x}", digest)[..24].to_owned()
}

fn arguments(executable: &Path, history_root: &Path, settings_file: &Path) -> Vec<String> {
    let mut result = vec![
        executable
            .to_str()
            .expect("validated executable")
            .to_owned(),
    ];
    result.extend(ARGUMENTS.map(str::to_owned));
    result.push(
        history_root
            .to_str()
            .expect("validated History root")
            .to_owned(),
    );
    result.push("--settings-file".into());
    result.push(
        settings_file
            .to_str()
            .expect("validated settings path")
            .to_owned(),
    );
    result
}

#[cfg(any(unix, test))]
fn reconcile_file(path: &Path, expected: &[u8], enabled: Option<bool>) -> Result<bool, String> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && !metadata.file_type().is_file()
    {
        return Err("Login item is not a regular file; it was left unchanged".into());
    }
    let existing = match fs::read(path) {
        Ok(value) => Some(value),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("Could not read login item: {error}")),
    };
    if existing.as_deref().is_some_and(|value| value != expected) {
        return Err(format!(
            "A different login item already exists at {}",
            path.display()
        ));
    }
    match enabled {
        None => Ok(existing.is_some()),
        Some(true) if existing.is_some() => Ok(true),
        Some(true) => {
            let parent = path.parent().ok_or("Login item has no parent directory")?;
            fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create login item directory: {error}"))?;
            use std::io::Write;
            let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
            file.write_all(expected).map_err(|e| e.to_string())?;
            file.as_file().sync_all().map_err(|e| e.to_string())?;
            file.persist_noclobber(path)
                .map_err(|e| format!("Could not create login item: {e}"))?;
            Ok(true)
        }
        Some(false) if existing.is_some() => {
            fs::remove_file(path)
                .map_err(|error| format!("Could not remove login item: {error}"))?;
            Ok(false)
        }
        Some(false) => Ok(false),
    }
}

#[cfg(target_os = "linux")]
fn configure_platform(
    executable: &Path,
    history_root: &Path,
    settings_file: &Path,
    enabled: Option<bool>,
) -> Result<bool, String> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or("HOME and XDG_CONFIG_HOME are unavailable")?;
    if !base.is_absolute() {
        return Err("Login item directory must be absolute".into());
    }
    let path = base.join("autostart").join(format!(
        "captures-native-{}.desktop",
        identity(history_root)
    ));
    let exec = desktop_command(&arguments(executable, history_root, settings_file))?;
    let content = format!(
        "[Desktop Entry]\nType=Application\nName=Captures Native ({})\nExec={}\nTerminal=false\nX-Captures-Profile={}\n",
        identity(history_root),
        exec,
        identity(history_root)
    );
    reconcile_file(&path, content.as_bytes(), enabled)
}

#[cfg(target_os = "linux")]
fn desktop_command(arguments: &[String]) -> Result<String, String> {
    // Like development Open With packaging, respect GIO's executable check,
    // which runs before %% field-code expansion. Arguments can still contain %.
    if arguments[0].contains(['\r', '\n', '\0', '=', '%']) {
        return Err("Move the executable to a path without newlines, NUL, '=' or '%' before enabling launch at login".into());
    }
    Ok(arguments
        .iter()
        .map(|argument| desktop_exec_argument(argument))
        .collect::<Vec<_>>()
        .join(" "))
}

#[cfg(target_os = "linux")]
fn desktop_exec_argument(value: &str) -> String {
    // First escape for Exec's quoted-argument grammar, then for the Desktop Entry
    // string layer. Percent is an Exec field-code introducer and must be doubled.
    let mut exec = String::from("\"");
    for character in value.chars() {
        if matches!(character, '"' | '\\' | '`' | '$') {
            exec.push('\\');
        }
        if character == '%' {
            exec.push('%');
        }
        exec.push(character);
    }
    exec.push('"');
    exec.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(target_os = "macos")]
fn configure_platform(
    executable: &Path,
    history_root: &Path,
    settings_file: &Path,
    enabled: Option<bool>,
) -> Result<bool, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("HOME is unavailable")?;
    let id = identity(history_root);
    let path = home
        .join("Library/LaunchAgents")
        .join(format!("dev.captures.native.{id}.plist"));
    let content = launch_agent(&id, &arguments(executable, history_root, settings_file));
    reconcile_file(&path, content.as_bytes(), enabled)
}

#[cfg(any(target_os = "macos", test))]
fn launch_agent(id: &str, arguments: &[String]) -> String {
    let xml = |value: &str| {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
            .replace('\r', "&#13;")
    };
    let values = arguments
        .iter()
        .map(|value| format!("    <string>{}</string>", xml(value)))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n  <key>Label</key><string>dev.captures.native.{id}</string>\n  <key>ProgramArguments</key><array>\n{values}\n  </array>\n  <key>RunAtLoad</key><true/>\n</dict></plist>\n"
    )
}

#[cfg(target_os = "windows")]
fn configure_platform(
    executable: &Path,
    history_root: &Path,
    settings_file: &Path,
    enabled: Option<bool>,
) -> Result<bool, String> {
    use winreg::{
        RegKey,
        enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE},
    };
    let name = format!("CapturesNative-{}", identity(history_root));
    let expected = arguments(executable, history_root, settings_file)
        .iter()
        .map(|arg| windows_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key_path = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let read = hkcu.open_subkey_with_flags(key_path, KEY_READ);
    let existing: Option<String> = match read {
        Ok(key) => match key.get_value(&name) {
            Ok(v) => Some(v),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.to_string()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    if existing.as_ref().is_some_and(|value| value != &expected) {
        return Err(format!("A different login item already exists for {name}"));
    }
    match enabled {
        None => Ok(existing.is_some()),
        Some(true) if existing.is_some() => Ok(true),
        Some(true) => {
            let (key, _) = hkcu.create_subkey(key_path).map_err(|e| e.to_string())?;
            key.set_value(name, &expected).map_err(|e| e.to_string())?;
            Ok(true)
        }
        Some(false) if existing.is_some() => {
            let key = hkcu
                .open_subkey_with_flags(key_path, KEY_WRITE)
                .map_err(|e| e.to_string())?;
            key.delete_value(name).map_err(|e| e.to_string())?;
            Ok(false)
        }
        Some(false) => Ok(false),
    }
}

#[cfg(target_os = "windows")]
fn windows_quote(value: &str) -> String {
    let mut result = String::from("\"");
    let mut slashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            slashes += 1;
        } else {
            if ch == '"' {
                result.push_str(&"\\".repeat(slashes * 2 + 1));
            } else {
                result.push_str(&"\\".repeat(slashes));
            }
            slashes = 0;
            result.push(ch);
        }
    }
    result.push_str(&"\\".repeat(slashes * 2));
    result.push('"');
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_reconciliation_is_idempotent_and_preserves_conflicts() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested/item");
        assert!(!reconcile_file(&path, b"ours", None).unwrap());
        assert!(!path.parent().unwrap().exists(), "query must not mutate");
        assert!(!reconcile_file(&path, b"ours", Some(false)).unwrap());
        assert!(
            !path.parent().unwrap().exists(),
            "disable must not create directories"
        );
        assert!(reconcile_file(&path, b"ours", Some(true)).unwrap());
        assert!(reconcile_file(&path, b"ours", Some(true)).unwrap());
        assert!(reconcile_file(&path, b"ours", None).unwrap());
        assert!(reconcile_file(&path, b"other", None).is_err());
        assert!(reconcile_file(&path, b"other", Some(true)).is_err());
        assert!(reconcile_file(&path, b"other", Some(false)).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"ours");
        assert!(!reconcile_file(&path, b"ours", Some(false)).unwrap());
        assert!(!path.exists());
    }

    #[test]
    fn profile_identity_is_stable_and_isolates_history_roots() {
        let first = identity(Path::new("/history/one"));
        assert_eq!(first, identity(Path::new("/history/one")));
        assert_ne!(first, identity(Path::new("/history/two")));
        assert_eq!(first.len(), 24);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_arguments_roundtrip_through_process_argv() {
        use std::{os::windows::process::CommandExt, process::Command};
        let expected = vec![
            "C:\\path space\\",
            "quote\"and\\\"slash",
            "",
            "雪%$",
            "line\nline",
        ];
        let command_line = expected
            .iter()
            .map(|value| windows_quote(value))
            .collect::<Vec<_>>()
            .join(" ");
        let result = Command::new("python")
            .args(["-c", "import json,sys; print(json.dumps(sys.argv[1:]))"])
            .raw_arg(command_line)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Vec<String>>(&result.stdout).unwrap(),
            expected
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_login_entries_are_never_followed_or_removed() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("other");
        fs::write(&target, b"ours").unwrap();
        let path = temp.path().join("entry");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        for enabled in [None, Some(true), Some(false)] {
            assert!(reconcile_file(&path, b"ours", enabled).is_err());
        }
        assert!(path.is_symlink());
        assert_eq!(fs::read(target).unwrap(), b"ours");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn desktop_exec_roundtrips_through_gio_without_shell_expansion() {
        use std::{
            os::unix::fs::PermissionsExt,
            process::Command,
            time::{Duration, Instant},
        };
        assert!(desktop_command(&["/tmp/%f app".into()]).is_err());
        assert!(desktop_command(&["/tmp/e=app".into()]).is_err());
        if Command::new("gio").arg("version").output().is_err() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("capture \"$`\\.py");
        let output = temp.path().join("argv.json");
        fs::write(&script, "#!/usr/bin/env python3\nimport json,sys,pathlib\npathlib.Path(sys.argv[1]).write_text(json.dumps(sys.argv[2:]))\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let expected = vec!["a b%\\\"$`", "line\nreturn\rtab\t", "雪", "", "%f"];
        let mut args = vec![
            script.to_str().unwrap().to_owned(),
            output.to_str().unwrap().to_owned(),
        ];
        args.extend(expected.iter().map(|s| s.to_string()));
        let entry = temp.path().join("test.desktop");
        let exec = desktop_command(&args).unwrap();
        fs::write(
            &entry,
            format!("[Desktop Entry]\nType=Application\nName=Test\nExec={exec}\n"),
        )
        .unwrap();
        let result = Command::new("gio")
            .arg("launch")
            .arg(&entry)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !output.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let actual: Vec<String> = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn launch_agent_argv_roundtrips_through_plist_parser() {
        use std::process::Command;
        let python = if cfg!(windows) { "python" } else { "python3" };
        let args = vec!["/app space/Captures".into(), "\"$`%\\雪<&>\r\n\t".into()];
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("test.plist");
        fs::write(&path, launch_agent("fixture", &args)).unwrap();
        let result = Command::new(python).args(["-c", "import plistlib,json,sys; print(json.dumps(plistlib.load(open(sys.argv[1],'rb'))['ProgramArguments']))"])
            .arg(path).output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Vec<String>>(&result.stdout).unwrap(),
            args
        );
    }
}
