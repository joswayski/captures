use super::{Action, tray::NativeTray};
use std::{
    cell::UnsafeCell,
    collections::VecDeque,
    ffi::OsStr,
    hash::{DefaultHasher, Hash, Hasher},
    os::windows::ffi::OsStrExt,
    path::Path,
    ptr,
    sync::mpsc,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, GetLastError,
        GlobalFree, HANDLE, HWND, LocalFree, POINT,
    },
    Graphics::Gdi::{CombineRgn, CreateRectRgn, DeleteObject, RGN_OR, SetWindowRgn},
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
        DataExchange::{
            COPYDATASTRUCT, CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
        },
        LibraryLoader::GetModuleHandleW,
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
        Registry::{
            HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
            RegCreateKeyExW, RegDeleteValueW, RegSetValueExW,
        },
        Threading::{
            CreateMutexW, GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
    UI::{
        HiDpi::{DPI_AWARENESS_CONTEXT_UNAWARE, SetThreadDpiAwarenessContext},
        Shell::DROPFILES,
        WindowsAndMessaging::{
            CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, EnumWindows,
            FindWindowW, GWL_EXSTYLE, GWLP_USERDATA, GetCursorPos, GetWindowLongPtrW,
            GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, HWND_TOPMOST,
            RegisterClassW, SMTO_ABORTIFHUNG, SWP_NOACTIVATE, SWP_NOSIZE, SendMessageTimeoutW,
            SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, WDA_EXCLUDEFROMCAPTURE,
            WDA_NONE, WM_COPYDATA, WM_NCCREATE, WNDCLASSW, WS_EX_LAYERED, WS_EX_TRANSPARENT,
        },
    },
};

const MAX_COMMAND_BYTES: usize = 64 * 1024;
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "CapturesGpui";
const CF_HDROP: u32 = 15;

pub struct Integration(NativeTray);

impl Integration {
    pub fn new(sender: mpsc::Sender<Action>) -> anyhow::Result<Self> {
        Ok(Self(NativeTray::new(sender)?))
    }

    pub fn poll(&self) {
        self.0.poll();
    }
}

pub fn ensure_supported_session() -> anyhow::Result<()> {
    Ok(())
}

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

fn win_error(context: &str) -> String {
    format!("{context}: {}", std::io::Error::last_os_error())
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
    pub fn acquire(args: &[String]) -> anyhow::Result<Option<Self>> {
        private_directory(&crate::settings::data_dir())?;
        let args = resolve_args(args)?;
        let user_sid = current_user_sid().map_err(anyhow::Error::msg)?;
        let mut hash = DefaultHasher::new();
        crate::settings::data_dir()
            .to_string_lossy()
            .to_lowercase()
            .hash(&mut hash);
        let id = format!("{:016x}", hash.finish());
        let mutex_name = wide(format!("Local\\CapturesGpui-{id}"));
        let mutex = unsafe { CreateMutexW(ptr::null(), 0, mutex_name.as_ptr()) };
        if mutex.is_null() {
            anyhow::bail!(win_error("cannot create instance mutex"));
        }
        let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let class_name = wide(format!("CapturesGpui-{id}"));
        if existed {
            unsafe { CloseHandle(mutex) };
            forward(&class_name, &args).map_err(anyhow::Error::msg)?;
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
            anyhow::bail!(win_error("cannot register activation window class"));
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
            anyhow::bail!(win_error("cannot create activation window"));
        }
        Ok(Some(Self { mutex, hwnd, state }))
    }

    pub fn commands(&self) -> Vec<Action> {
        let mut commands: Vec<_> = unsafe {
            (&mut *self.state.get())
                .commands
                .drain(..)
                .map(Action::Arguments)
                .collect()
        };
        commands.extend(super::take_native_events());
        commands
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
            CloseHandle(self.mutex);
        }
    }
}

fn resolve_args(args: &[String]) -> anyhow::Result<Vec<String>> {
    let cwd = std::env::current_dir()?;
    let paths = matches!(
        args.first().map(String::as_str),
        Some("--open" | "--previews")
    );
    Ok(args
        .iter()
        .enumerate()
        .map(|(index, arg)| {
            if (!paths && args.first().is_some_and(|arg| arg.starts_with('-')))
                || (paths && index == 0)
                || Path::new(arg).is_absolute()
            {
                arg.clone()
            } else {
                cwd.join(arg).to_string_lossy().into_owned()
            }
        })
        .collect())
}

fn forward(class_name: &[u16], args: &[String]) -> Result<(), String> {
    let payload = serde_json::to_vec(args).map_err(|error| error.to_string())?;
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
    let module = unsafe { GetModuleHandleW(ptr::null()) };
    let sender_class = wide(format!("CapturesGpuiSender-{}", unsafe {
        GetCurrentProcessId()
    }));
    let class = WNDCLASSW {
        lpfnWndProc: Some(DefWindowProcW),
        hInstance: module,
        lpszClassName: sender_class.as_ptr(),
        ..unsafe { std::mem::zeroed() }
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(win_error("cannot register activation sender class"));
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
    if sent == 0 || result != 1 {
        Err("primary instance rejected activation".into())
    } else {
        Ok(())
    }
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
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
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

pub fn login(enabled: bool) -> Result<(), String> {
    let command = if enabled {
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
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
    if status == ERROR_SUCCESS || (!enabled && status == ERROR_FILE_NOT_FOUND) {
        Ok(())
    } else {
        Err(format!(
            "cannot update autostart registry value: OS error {status}"
        ))
    }
}

pub fn pointer() -> (i32, i32) {
    let mut point = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut point) } != 0 {
        (point.x, point.y)
    } else {
        (0, 0)
    }
}

struct FindWindow {
    title: Vec<u16>,
    hwnd: HWND,
}

unsafe extern "system" fn find_owned_window(hwnd: HWND, value: isize) -> i32 {
    let state = unsafe { &mut *(value as *mut FindWindow) };
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid != unsafe { GetCurrentProcessId() } {
        return 1;
    }
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return 1;
    }
    let mut title = vec![0u16; length as usize + 1];
    unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) };
    if title == state.title {
        state.hwnd = hwnd;
        0
    } else {
        1
    }
}

pub fn position_guide(title: &str, x: i32, y: i32) -> anyhow::Result<()> {
    let mut found = FindWindow {
        title: wide(title),
        hwnd: ptr::null_mut(),
    };
    unsafe { EnumWindows(Some(find_owned_window), ptr::from_mut(&mut found) as isize) };
    if found.hwnd.is_null() {
        return Ok(());
    }
    let style = unsafe { GetWindowLongPtrW(found.hwnd, GWL_EXSTYLE) };
    unsafe {
        SetWindowLongPtrW(
            found.hwnd,
            GWL_EXSTYLE,
            style | WS_EX_LAYERED as isize | WS_EX_TRANSPARENT as isize,
        );
    }
    // Inputs are GPUI logical desktop DIPs. Temporarily request Windows API
    // coordinate virtualization so SetWindowPos maps them to the target
    // monitor's physical coordinates under mixed DPI.
    let previous_awareness = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_UNAWARE) };
    let positioned = unsafe {
        SetWindowPos(
            found.hwnd,
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOSIZE,
        )
    };
    if !previous_awareness.is_null() {
        unsafe { SetThreadDpiAwarenessContext(previous_awareness) };
    }
    if positioned == 0 {
        anyhow::bail!(win_error("cannot position recording guide"));
    }
    if unsafe { SetWindowDisplayAffinity(found.hwnd, WDA_EXCLUDEFROMCAPTURE) } == 0 {
        anyhow::bail!(win_error("cannot exclude recording guide from capture"));
    }
    Ok(())
}

pub fn exclude_from_capture(title: &str, excluded: bool) -> Result<(), String> {
    let mut found = FindWindow {
        title: wide(title),
        hwnd: ptr::null_mut(),
    };
    unsafe { EnumWindows(Some(find_owned_window), ptr::from_mut(&mut found) as isize) };
    if found.hwnd.is_null() {
        return Ok(());
    }
    let affinity = if excluded {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    if unsafe { SetWindowDisplayAffinity(found.hwnd, affinity) } == 0 {
        Err(win_error("cannot update window capture exclusion"))
    } else {
        Ok(())
    }
}

pub fn set_preview_input_region(
    rectangles: &[(f32, f32, f32, f32)],
    scale: f32,
    _: (f32, f32),
) -> Result<(), String> {
    let mut found = FindWindow {
        title: wide("Captures GPUI Previews"),
        hwnd: ptr::null_mut(),
    };
    unsafe { EnumWindows(Some(find_owned_window), ptr::from_mut(&mut found) as isize) };
    if found.hwnd.is_null() {
        return Ok(());
    }
    let scale = scale.max(0.25);
    let region = unsafe { CreateRectRgn(0, 0, 0, 0) };
    if region.is_null() {
        return Err(win_error("create preview input region"));
    }
    for &(x, y, width, height) in rectangles {
        if width <= 0.0 || height <= 0.0 {
            continue;
        }
        let rect = unsafe {
            CreateRectRgn(
                (x * scale).round() as i32,
                (y * scale).round() as i32,
                ((x + width) * scale).round() as i32,
                ((y + height) * scale).round() as i32,
            )
        };
        if !rect.is_null() {
            unsafe {
                CombineRgn(region, region, rect, RGN_OR);
                DeleteObject(rect);
            }
        }
    }
    if unsafe { SetWindowRgn(found.hwnd, region, 1) } == 0 {
        unsafe { DeleteObject(region) };
        Err(win_error("set preview input region"))
    } else {
        // SetWindowRgn owns the region after success.
        Ok(())
    }
}

pub fn clear_preview_input_region() {}

pub fn copy_file(path: &Path) -> Result<(), String> {
    // EmptyClipboard assigns ownership to this HWND. Passing NULL would leave
    // no owner and make the following SetClipboardData call fail.
    let mut owner = FindWindow {
        title: wide("Captures GPUI Previews"),
        hwnd: ptr::null_mut(),
    };
    unsafe { EnumWindows(Some(find_owned_window), ptr::from_mut(&mut owner) as isize) };
    if owner.hwnd.is_null() {
        return Err("The preview window is unavailable for file copying.".into());
    }
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let path = wide(path.as_os_str());
    let bytes = size_of::<DROPFILES>() + path.len() * size_of::<u16>() + size_of::<u16>();
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) };
    if memory.is_null() {
        return Err(win_error("allocate file clipboard data"));
    }
    let data = unsafe { GlobalLock(memory) };
    if data.is_null() {
        unsafe { GlobalFree(memory) };
        return Err(win_error("lock file clipboard data"));
    }
    unsafe {
        ptr::write_bytes(data, 0, bytes);
        let drop = data.cast::<DROPFILES>();
        (*drop).pFiles = size_of::<DROPFILES>() as u32;
        (*drop).fWide = 1;
        ptr::copy_nonoverlapping(
            path.as_ptr(),
            data.byte_add(size_of::<DROPFILES>()).cast::<u16>(),
            path.len(),
        );
        GlobalUnlock(memory);
    }
    if unsafe { OpenClipboard(owner.hwnd) } == 0 {
        unsafe { GlobalFree(memory) };
        return Err(win_error("open clipboard"));
    }
    let success = unsafe { EmptyClipboard() != 0 && !SetClipboardData(CF_HDROP, memory).is_null() };
    unsafe { CloseClipboard() };
    if success {
        Ok(())
    } else {
        unsafe { GlobalFree(memory) };
        Err(win_error("set file clipboard data"))
    }
}

pub fn reveal(path: &Path) -> Result<(), String> {
    std::process::Command::new("explorer.exe")
        .arg(format!("/select,{}", path.display()))
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("show {}: {error}", path.display()))
}

pub fn private_directory(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    secure_path(path, true)
}

pub fn private_file(path: &Path) -> std::io::Result<()> {
    secure_path(path, false)
}

fn secure_path(path: &Path, directory: bool) -> std::io::Result<()> {
    let path_w = wide(path.as_os_str());
    let attributes = unsafe { GetFileAttributesW(path_w.as_ptr()) };
    if attributes == INVALID_FILE_ATTRIBUTES {
        return Err(std::io::Error::last_os_error());
    }
    if (attributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
        || attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "private path has an unsafe file type",
        ));
    }
    let sid = current_user_sid().map_err(std::io::Error::other)?;
    let mut sid_text = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid.as_ptr().cast_mut().cast(), &mut sid_text) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let length = (0..)
        .take_while(|&index| unsafe { *sid_text.add(index) } != 0)
        .count();
    let sid_string =
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(sid_text, length) });
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
    let mut present = 0;
    let mut defaulted = 0;
    let mut dacl = ptr::null_mut();
    if unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) }
        == 0
        || present == 0
        || dacl.is_null()
    {
        unsafe { LocalFree(descriptor.cast()) };
        return Err(std::io::Error::last_os_error());
    }
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
    #[test]
    fn wide_strings_are_terminated() {
        assert_eq!(super::wide("test"), [116, 101, 115, 116, 0]);
    }

    #[test]
    fn activation_resolves_files_but_preserves_option_values() {
        let appearance = vec!["--appearance".into(), "dark".into()];
        assert_eq!(super::resolve_args(&appearance).unwrap(), appearance);
        let relative = "a capture with spaces.png";
        let expected = std::env::current_dir().unwrap().join(relative);
        assert_eq!(
            super::resolve_args(&["--open".into(), relative.into()]).unwrap(),
            ["--open".to_owned(), expected.to_string_lossy().into_owned()]
        );
    }
}
