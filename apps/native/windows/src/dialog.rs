//! The system folder picker (`IFileOpenDialog` with `FOS_PICKFOLDERS`), as
//! shipping's `open({ directory: true })`.
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::*;
use windows::core::*;

/// Ask for a folder, starting at `current`; `None` when cancelled. Modal to
/// `owner`; it runs its own message loop, so call it without holding the host.
pub fn pick_folder(owner: HWND, title: &str, current: &str) -> Result<Option<String>> {
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        let options = dialog.GetOptions()?;
        dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)?;
        dialog.SetTitle(&HSTRING::from(title))?;
        if let Ok(folder) =
            SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(current), None)
        {
            let _ = dialog.SetFolder(&folder);
        }
        if let Err(error) = dialog.Show(Some(owner)) {
            if error.code() == ERROR_CANCELLED.to_hresult() {
                return Ok(None);
            }
            return Err(error);
        }
        let name = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = name.to_string();
        CoTaskMemFree(Some(name.0 as *const _));
        Ok(Some(path.map_err(|_| {
            Error::new(E_FAIL_HRESULT, "folder path is not Unicode")
        })?))
    }
}

const E_FAIL_HRESULT: HRESULT = windows::Win32::Foundation::E_FAIL;
