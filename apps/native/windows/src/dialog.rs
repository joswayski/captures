//! System pickers: the folder picker (`IFileOpenDialog` with
//! `FOS_PICKFOLDERS`), as shipping's `open({ directory: true })`. Colors use
//! Captures' own picker (`color_picker.rs`).
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

/// Ask for image files, as shipping's Add images `open({ multiple: true,
/// filters: Images })`; empty when cancelled. Modal, like [`pick_folder`].
pub fn pick_images(owner: HWND, title: &str) -> Result<Vec<std::path::PathBuf>> {
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        let options = dialog.GetOptions()?;
        dialog
            .SetOptions(options | FOS_ALLOWMULTISELECT | FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST)?;
        dialog.SetTitle(&HSTRING::from(title))?;
        let spec = w!("*.png;*.jpg;*.jpeg;*.webp;*.tif;*.tiff;*.gif;*.bmp;*.svg");
        dialog.SetFileTypes(&[windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC {
            pszName: w!("Images"),
            pszSpec: spec,
        }])?;
        if let Err(error) = dialog.Show(Some(owner)) {
            if error.code() == ERROR_CANCELLED.to_hresult() {
                return Ok(Vec::new());
            }
            return Err(error);
        }
        let items = dialog.GetResults()?;
        let mut paths = Vec::new();
        for i in 0..items.GetCount()? {
            let name = items.GetItemAt(i)?.GetDisplayName(SIGDN_FILESYSPATH)?;
            if let Ok(path) = name.to_string() {
                paths.push(path.into());
            }
            CoTaskMemFree(Some(name.0 as *const _));
        }
        Ok(paths)
    }
}
