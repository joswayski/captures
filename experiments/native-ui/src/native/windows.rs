//! Win32 integration for the GTK3 frontend.
//!
//! GTK3's Win32 backend manages process DPI awareness itself.  In particular,
//! setting per-monitor-v2 awareness here would make GTK's coordinate system
//! inconsistent, so initialization intentionally does not change DPI state.

use std::{
    cell::UnsafeCell,
    collections::VecDeque,
    ffi::OsStr,
    hash::{DefaultHasher, Hash, Hasher},
    os::windows::ffi::OsStrExt,
    path::Path,
    ptr,
    time::{Duration, Instant},
};

use gtk::{glib::translate::ToGlibPtr, prelude::*};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, GetLastError, HANDLE, HWND, LocalFree,
    },
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
        },
        DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetTokenInformation,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    },
    Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, GetFileAttributesW,
        INVALID_FILE_ATTRIBUTES,
    },
    System::{
        DataExchange::COPYDATASTRUCT,
        LibraryLoader::GetModuleHandleW,
        Registry::{
            HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_DWORD,
            RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegSetValueExW,
        },
        Threading::{
            CreateMutexW, GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
    UI::WindowsAndMessaging::{
        CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowW, GWLP_USERDATA,
        GetWindowLongPtrW, GetWindowThreadProcessId, HWND_TOPMOST, RegisterClassW,
        SMTO_ABORTIFHUNG, SWP_NOACTIVATE, SendMessageTimeoutW, SetWindowDisplayAffinity,
        SetWindowLongPtrW, SetWindowPos, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WM_COPYDATA,
        WM_NCCREATE, WNDCLASSW,
    },
};

const MAX_COMMAND_BYTES: usize = 64 * 1024;
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "CapturesWindowsNative";

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

fn win_error(context: &str) -> String {
    format!("{context}: {}", std::io::Error::last_os_error())
}

/// Configure a relocated portable folder before GTK or app workers start. A
/// source build without adjacent runtime data uses the developer's MSYS2 setup.
pub fn runtime() -> Result<Option<tempfile::NamedTempFile>, String> {
    use std::{io::Write, os::windows::process::CommandExt, process::Command};
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let bin = exe.parent().ok_or("Missing executable directory")?;
    let root = bin.parent().ok_or("Missing runtime directory")?;
    let schemas = root.join("share/glib-2.0/schemas");
    if !schemas.is_dir() {
        return Ok(None);
    }
    let loader_dir = root.join("lib/gdk-pixbuf-2.0/2.10.0/loaders");
    let paths = std::iter::once(bin.to_owned()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect::<Vec<_>>(),
    );
    let path = std::env::join_paths(paths).map_err(|e| e.to_string())?;
    // SAFETY: called before GTK initialization and application threads. Windows
    // also synchronizes access to its process environment.
    unsafe {
        std::env::set_var("PATH", path);
        std::env::set_var("XDG_DATA_DIRS", root.join("share"));
        std::env::set_var("GSETTINGS_SCHEMA_DIR", schemas);
        std::env::set_var("GDK_PIXBUF_MODULEDIR", loader_dir);
        std::env::set_var("FONTCONFIG_PATH", root.join("etc/fonts"));
    }
    let output = Command::new(bin.join("gdk-pixbuf-query-loaders.exe"))
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() || !String::from_utf8_lossy(&output.stdout).contains("svg") {
        return Err(format!(
            "GTK image loaders unavailable: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let mut cache = tempfile::Builder::new()
        .prefix("captures-pixbuf-")
        .tempfile()
        .map_err(|e| e.to_string())?;
    cache.write_all(&output.stdout).map_err(|e| e.to_string())?;
    cache.flush().map_err(|e| e.to_string())?;
    // SAFETY: same pre-initialization condition as above.
    unsafe {
        std::env::set_var("GDK_PIXBUF_MODULE_FILE", cache.path());
    }
    Ok(Some(cache))
}

struct CommandState {
    commands: VecDeque<Vec<String>>,
    user_sid: Vec<u8>,
}

pub struct Instance {
    mutex: HANDLE,
    hwnd: HWND,
    state: Box<UnsafeCell<CommandState>>,
}

impl Instance {
    pub fn take_commands(&self) -> Vec<Vec<String>> {
        // Instance and its HWND are created and consumed on GTK's owning
        // thread; SendMessage dispatches the callback on that same thread.
        unsafe { (&mut *self.state.get()).commands.drain(..).collect() }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: both handles are owned by this instance and remain live until drop.
        unsafe {
            DestroyWindow(self.hwnd);
            CloseHandle(self.mutex);
        }
    }
}

pub fn instance(args: Vec<String>) -> Result<Option<Instance>, String> {
    // Resolve file arguments in the invoking process, not the primary's cwd.
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let args: Vec<String> = args
        .into_iter()
        .map(|arg| {
            if arg.starts_with('-') || Path::new(&arg).is_absolute() {
                arg
            } else {
                cwd.join(arg).to_string_lossy().into_owned()
            }
        })
        .collect();
    let user_sid = current_user_sid()?;
    let mut hash = DefaultHasher::new();
    crate::settings::data_dir()
        .to_string_lossy()
        .to_lowercase()
        .hash(&mut hash);
    let id = format!("{:016x}", hash.finish());
    let mutex_name = wide(format!("Local\\CapturesWindowsNative-{id}"));
    // The default DACL comes from this process token, preventing another user
    // from opening the mutex. `Local` additionally scopes it to this session.
    let mutex = unsafe { CreateMutexW(ptr::null(), 0, mutex_name.as_ptr()) };
    if mutex.is_null() {
        return Err(win_error("cannot create instance mutex"));
    }
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let class_name = wide(format!("CapturesWindowsNative-{id}"));
    if existed {
        unsafe { CloseHandle(mutex) };
        forward(&class_name, &args)?;
        return Ok(None);
    }

    let module = unsafe { GetModuleHandleW(ptr::null()) };
    let class = WNDCLASSW {
        lpfnWndProc: Some(instance_wndproc),
        hInstance: module,
        lpszClassName: class_name.as_ptr(),
        ..unsafe { std::mem::zeroed() }
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        unsafe { CloseHandle(mutex) };
        return Err(win_error("cannot register activation window class"));
    }
    let state = Box::new(UnsafeCell::new(CommandState {
        commands: VecDeque::new(),
        user_sid,
    }));
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            module,
            state.get().cast(),
        )
    };
    if hwnd.is_null() {
        unsafe { CloseHandle(mutex) };
        return Err(win_error("cannot create activation window"));
    }
    Ok(Some(Instance { mutex, hwnd, state }))
}

fn forward(class_name: &[u16], args: &[String]) -> Result<(), String> {
    let payload =
        serde_json::to_vec(args).map_err(|e| format!("cannot encode command line: {e}"))?;
    if payload.len() > MAX_COMMAND_BYTES {
        return Err("activation command line is too large".into());
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    let hwnd = loop {
        let hwnd = unsafe { FindWindowW(class_name.as_ptr(), ptr::null()) };
        if !hwnd.is_null() {
            break hwnd;
        }
        if Instant::now() >= deadline {
            return Err("primary instance did not create its activation window".into());
        }
        std::thread::sleep(Duration::from_millis(30));
    };
    let data = COPYDATASTRUCT {
        dwData: 0x4350_5452,
        cbData: payload.len() as u32,
        lpData: payload.as_ptr().cast_mut().cast(),
    };
    // WM_COPYDATA identifies the sender by HWND. Create a message-only-style
    // hidden top-level window so the receiver can verify this process's token.
    let module = unsafe { GetModuleHandleW(ptr::null()) };
    let sender_class = wide(format!("CapturesWindowsNativeSender-{}", unsafe {
        GetCurrentProcessId()
    }));
    let class = WNDCLASSW {
        lpfnWndProc: Some(DefWindowProcW),
        hInstance: module,
        lpszClassName: sender_class.as_ptr(),
        ..unsafe { std::mem::zeroed() }
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(win_error("cannot register activation sender window class"));
    }
    let sender = unsafe {
        CreateWindowExW(
            0,
            sender_class.as_ptr(),
            sender_class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            module,
            ptr::null(),
        )
    };
    if sender.is_null() {
        return Err(win_error("cannot create activation sender window"));
    }
    let mut result = 0usize;
    let sent = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            sender as usize,
            ptr::from_ref(&data) as isize,
            SMTO_ABORTIFHUNG,
            1_000,
            &mut result,
        )
    };
    unsafe { DestroyWindow(sender) };
    if sent == 0 {
        return Err(win_error("primary instance did not accept activation"));
    }
    if result != 1 {
        return Err("primary instance rejected activation".into());
    }
    Ok(())
}

unsafe extern "system" fn instance_wndproc(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize) };
    } else if message == WM_COPYDATA {
        let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut CommandState };
        if state.is_null()
            || wparam == 0
            || !sender_is_current_user(wparam as HWND, unsafe { &(*state).user_sid })
        {
            return 0;
        }
        let data = unsafe { &*(lparam as *const COPYDATASTRUCT) };
        if data.dwData != 0x4350_5452
            || data.cbData == 0
            || data.cbData as usize > MAX_COMMAND_BYTES
            || data.lpData.is_null()
            || unsafe { (*state).commands.len() } >= 32
        {
            return 0;
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(data.lpData.cast::<u8>(), data.cbData as usize) };
        if let Ok(args) = serde_json::from_slice::<Vec<String>>(bytes) {
            unsafe { (*state).commands.push_back(args) };
            return 1;
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn sender_is_current_user(hwnd: HWND, expected: &[u8]) -> bool {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == 0 || pid == unsafe { GetCurrentProcessId() } {
        return pid != 0;
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return false;
    }
    let sid = token_user_sid(process).ok();
    unsafe { CloseHandle(process) };
    sid.as_deref() == Some(expected)
}

fn current_user_sid() -> Result<Vec<u8>, String> {
    token_user_sid(unsafe { GetCurrentProcess() })
}

fn token_user_sid(process: HANDLE) -> Result<Vec<u8>, String> {
    let mut token = ptr::null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(win_error("cannot open process token"));
    }
    let mut needed = 0;
    unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed) };
    // TOKEN_USER contains pointers and therefore requires pointer alignment;
    // a byte vector would not provide that guarantee.
    let words = (needed as usize).div_ceil(size_of::<usize>());
    let mut buffer = vec![0usize; words];
    if needed == 0
        || unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        } == 0
    {
        unsafe { CloseHandle(token) };
        return Err(win_error("cannot read process user SID"));
    }
    let sid = unsafe { (*(buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid };
    let length = unsafe { windows_sys::Win32::Security::GetLengthSid(sid) };
    let bytes = unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), length as usize) }.to_vec();
    unsafe { CloseHandle(token) };
    Ok(bytes)
}

pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let command = if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        Some(wide(format!("\"{}\" --background", exe.display())))
    } else {
        None
    };
    let key_name = wide(RUN_KEY);
    let value_name = wide(RUN_VALUE);
    let mut key = ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key_name.as_ptr(),
            0,
            ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!(
            "cannot open autostart registry key: OS error {status}"
        ));
    }
    let status = if let Some(command) = command {
        let bytes =
            unsafe { std::slice::from_raw_parts(command.as_ptr().cast::<u8>(), command.len() * 2) };
        unsafe {
            RegSetValueExW(
                key,
                value_name.as_ptr(),
                0,
                REG_SZ,
                bytes.as_ptr(),
                bytes.len() as u32,
            )
        }
    } else {
        unsafe { RegDeleteValueW(key, value_name.as_ptr()) }
    };
    unsafe { RegCloseKey(key) };
    if status == ERROR_SUCCESS
        || (!enabled && status == windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND)
    {
        Ok(())
    } else {
        Err(format!(
            "cannot update autostart registry value: OS error {status}"
        ))
    }
}

pub fn system_dark() -> bool {
    let key = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let value = wide("AppsUseLightTheme");
    let mut light = 1u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            ptr::null_mut(),
            ptr::from_mut(&mut light).cast(),
            &mut size,
        )
    };
    status == ERROR_SUCCESS && light == 0
}

#[link(name = "gdk-3")]
unsafe extern "C" {
    fn gdk_win32_window_get_handle(window: *mut gtk::gdk::ffi::GdkWindow) -> HWND;
}

fn window_handle(window: &gtk::Window) -> Result<HWND, String> {
    if !window.is_realized() {
        window.realize();
    }
    let gdk = window.window().ok_or("GTK window could not be realized")?;
    let hwnd = unsafe { gdk_win32_window_get_handle(gdk.to_glib_none().0) };
    if hwnd.is_null() {
        return Err("GDK did not provide a Win32 window handle".into());
    }
    Ok(hwnd)
}

pub fn place_window(
    window: &gtk::Window,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<(), String> {
    let hwnd = window_handle(window)?;
    // SAFETY: GTK owns this live HWND; the operation changes its bounds only.
    if unsafe { SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height, SWP_NOACTIVATE) } == 0 {
        return Err(win_error("cannot position capture window"));
    }
    Ok(())
}

pub fn exclude_from_capture(window: &gtk::Window, excluded: bool) -> Result<(), String> {
    let hwnd = window_handle(window)?;
    let affinity = if excluded {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    if unsafe { SetWindowDisplayAffinity(hwnd, affinity) } == 0 {
        Err(win_error("cannot set window display affinity"))
    } else {
        Ok(())
    }
}

pub fn private_directory(path: &Path) -> std::io::Result<()> {
    let path_w = wide(path.as_os_str());
    let attributes = unsafe { GetFileAttributesW(path_w.as_ptr()) };
    if attributes == INVALID_FILE_ATTRIBUTES {
        return Err(std::io::Error::last_os_error());
    }
    if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path is not a directory",
        ));
    }
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to secure a reparse-point directory",
        ));
    }

    let sid = current_user_sid().map_err(std::io::Error::other)?;
    let mut sid_text = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid.as_ptr().cast_mut().cast(), &mut sid_text) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let len = (0..)
        .take_while(|&i| unsafe { *sid_text.add(i) } != 0)
        .count();
    let sid_string = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(sid_text, len) });
    unsafe { LocalFree(sid_text.cast()) };
    let sddl = wide(format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{sid_string})"));
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    let mut dacl_present = 0;
    let mut dacl_defaulted = 0;
    let mut dacl = ptr::null_mut();
    if unsafe {
        GetSecurityDescriptorDacl(
            descriptor,
            &mut dacl_present,
            &mut dacl,
            &mut dacl_defaulted,
        )
    } == 0
        || dacl_present == 0
        || dacl.is_null()
    {
        unsafe { LocalFree(descriptor.cast()) };
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the descriptor remains allocated while its validated DACL is
    // passed to SetNamedSecurityInfoW, then is released with LocalFree.
    let status = unsafe {
        SetNamedSecurityInfoW(
            path_w.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            dacl,
            ptr::null(),
        )
    };
    unsafe { LocalFree(descriptor.cast()) };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(status as i32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wide_is_terminated() {
        assert_eq!(wide("test"), [116, 101, 115, 116, 0]);
    }
}
