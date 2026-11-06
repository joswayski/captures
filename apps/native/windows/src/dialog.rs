//! System pickers: the folder picker (`IFileOpenDialog` with
//! `FOS_PICKFOLDERS`), as shipping's `open({ directory: true })`, and the
//! color dialog Chromium opens for `input[type="color"]` on Windows.
use windows::Win32::Foundation::{COLORREF, ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Controls::Dialogs::*;
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

thread_local! {
    /// The color dialog's sixteen custom colors, kept for the session.
    static CUSTOM_COLORS: std::cell::RefCell<[COLORREF; 16]> =
        const { std::cell::RefCell::new([COLORREF(0x00ff_ffff); 16]) };
}

/// Ask for a color starting at `current` (`#rrggbb`); `None` when cancelled.
/// Modal to `owner`, like [`pick_folder`].
pub fn pick_color(owner: HWND, current: &str) -> Result<Option<String>> {
    let channel = |i: usize| {
        current
            .get(i..i + 2)
            .and_then(|v| u32::from_str_radix(v, 16).ok())
            .unwrap_or(0)
    };
    // COLORREF is 0x00BBGGRR.
    let initial = COLORREF(channel(1) | channel(3) << 8 | channel(5) << 16);
    CUSTOM_COLORS.with(|custom| {
        let mut custom = custom.borrow_mut();
        let mut choose = CHOOSECOLORW {
            lStructSize: size_of::<CHOOSECOLORW>() as u32,
            hwndOwner: owner,
            rgbResult: initial,
            lpCustColors: custom.as_mut_ptr(),
            Flags: CC_ANYCOLOR | CC_FULLOPEN | CC_RGBINIT,
            ..Default::default()
        };
        if !unsafe { ChooseColorW(&mut choose) }.as_bool() {
            // Zero is a cancel; anything else is a dialog failure.
            let error = unsafe { CommDlgExtendedError() };
            if error.0 == 0 {
                return Ok(None);
            }
            return Err(Error::new(
                E_FAIL_HRESULT,
                format!("ChooseColor failed: {error:?}"),
            ));
        }
        let rgb = choose.rgbResult.0;
        Ok(Some(format!(
            "#{:02x}{:02x}{:02x}",
            rgb & 0xff,
            (rgb >> 8) & 0xff,
            (rgb >> 16) & 0xff
        )))
    })
}
