use std::{
    ffi::{CStr, c_void},
    sync::OnceLock,
};

type SetSymbolicHotKeyEnabled = unsafe extern "C" fn(i32, bool) -> i32;

/// Disable macOS symbolic hotkeys in WindowServer immediately.
///
/// Screenshot.app keeps listening until this call, even after
/// `com.apple.symbolichotkeys` is written. CleanShot-style takeover uses the
/// same SkyLight/ApplicationServices entry point so ⌘⇧3 / ⌘⇧4 / ⌘⇧5 stop firing
/// the system overlay before Captures registers those chords.
pub fn disable_symbolic_hotkeys(ids: &[u32]) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    let set_enabled = symbolic_hotkey_set_enabled()?;
    let mut errors = Vec::new();
    for id in ids {
        // SAFETY: `set_enabled` is CGSSetSymbolicHotKeyEnabled. The id is an
        // Apple symbolic-hotkey constant; false disables that action process-wide
        // in WindowServer for the current login session.
        let status = unsafe { set_enabled(*id as i32, false) };
        if status != 0 {
            errors.push(format!(
                "CGSSetSymbolicHotKeyEnabled({id}) returned {status}"
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn symbolic_hotkey_set_enabled() -> Result<SetSymbolicHotKeyEnabled, String> {
    static FUNCTION: OnceLock<Result<SetSymbolicHotKeyEnabled, String>> = OnceLock::new();
    FUNCTION.get_or_init(load_set_enabled).clone()
}

fn load_set_enabled() -> Result<SetSymbolicHotKeyEnabled, String> {
    let symbol = c"CGSSetSymbolicHotKeyEnabled";
    // SAFETY: RTLD_DEFAULT searches already-loaded images. AppKit/SkyLight are
    // present in this process; a null return means we try explicit frameworks.
    let mut pointer = unsafe { libc::dlsym(libc::RTLD_DEFAULT, symbol.as_ptr()) };
    if pointer.is_null() {
        pointer = dlsym_in_framework(
            c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",
            symbol,
        );
    }
    if pointer.is_null() {
        pointer = dlsym_in_framework(
            c"/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices",
            symbol,
        );
    }
    if pointer.is_null() {
        return Err("CGSSetSymbolicHotKeyEnabled is unavailable".to_owned());
    }
    // SAFETY: The SkyLight/ApplicationServices symbol has the documented
    // `(CGSSymbolicHotKey, bool) -> CGError` signature.
    Ok(unsafe { std::mem::transmute::<*mut c_void, SetSymbolicHotKeyEnabled>(pointer) })
}

fn dlsym_in_framework(path: &CStr, symbol: &CStr) -> *mut c_void {
    // SAFETY: dlopen of a system framework path; the handle is leaked so the
    // resolved function pointer stays valid for the process lifetime.
    let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
    if handle.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: `handle` is a live dlopen result for a system framework.
    unsafe { libc::dlsym(handle, symbol.as_ptr()) }
}

#[cfg(test)]
mod tests {
    use super::disable_symbolic_hotkeys;

    #[test]
    fn disable_with_no_ids_is_a_noop() {
        assert!(disable_symbolic_hotkeys(&[]).is_ok());
    }
}
