use std::{io, path::PathBuf};

use interprocess::local_socket::{ListenerOptions, Name};

#[cfg(unix)]
pub(super) fn directory() -> io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;

    let uid = rustix::process::geteuid().as_raw();
    let path = PathBuf::from(format!("/tmp/captures-native-{uid}"));
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    verify_unix_directory(&path, uid)?;
    Ok(path)
}

#[cfg(unix)]
fn verify_unix_directory(path: &std::path::Path, uid: u32) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    // symlink_metadata deliberately does not follow a final-component symlink.
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "IPC path is not a real directory",
        ));
    }
    if metadata.uid() != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "IPC directory is owned by another user",
        ));
    }
    if metadata.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "IPC directory is accessible by group or other users",
        ));
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn listener_options(name: Name<'static>) -> io::Result<ListenerOptions<'static>> {
    // The containing 0700 directory is the portability boundary. In particular, the
    // local-socket mode extension is not available on macOS.
    Ok(ListenerOptions::new().name(name))
}

#[cfg(windows)]
pub(super) fn directory() -> io::Result<PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, ptr};
    use windows_sys::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath},
    };

    let mut raw = ptr::null_mut();
    // SAFETY: valid known-folder ID and writable output; successful output is
    // NUL-terminated and allocated by the COM task allocator.
    let result =
        unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, ptr::null_mut(), &mut raw) };
    if result < 0 {
        return Err(io::Error::from_raw_os_error(result));
    }
    // SAFETY: successful SHGetKnownFolderPath output; copy before freeing with
    // its specified allocator (not LocalFree, which is used for the SID below).
    let local_app_data = unsafe {
        let length = (0..).take_while(|&index| *raw.add(index) != 0).count();
        let path = PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(raw, length)));
        CoTaskMemFree(raw.cast());
        path
    };

    let application = local_app_data.join("Captures Native");
    create_and_verify_windows_directory(&application)?;
    let ipc = application.join("ipc");
    create_and_verify_windows_directory(&ipc)?;
    Ok(ipc)
}

#[cfg(windows)]
fn create_and_verify_windows_directory(path: &std::path::Path) -> io::Result<()> {
    match std::fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }

    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, GetFileAttributesW,
        INVALID_FILE_ATTRIBUTES,
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: owned NUL-terminated path remains readable for the call.
    let attributes = unsafe { GetFileAttributesW(wide.as_ptr()) };
    if attributes == INVALID_FILE_ATTRIBUTES {
        return Err(io::Error::last_os_error());
    }
    if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 || attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "IPC path is not a real directory",
        ));
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn listener_options(name: Name<'static>) -> io::Result<ListenerOptions<'static>> {
    use interprocess::os::windows::{
        local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor,
    };

    let sddl = protected_pipe_sddl(&current_user_sid_string()?)?;
    let descriptor = SecurityDescriptor::deserialize(&sddl)?;
    Ok(ListenerOptions::new()
        .name(name)
        .security_descriptor(descriptor))
}

#[cfg(windows)]
fn protected_pipe_sddl(sid: &str) -> io::Result<widestring::U16CString> {
    // D:P makes the DACL protected, and the sole ACE grants the token user full access.
    // This intentionally does not use a null/default DACL.
    widestring::U16CString::from_str(format!("D:P(A;;GA;;;{sid})"))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(windows)]
fn current_user_sid_string() -> io::Result<String> {
    use std::{mem, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, HANDLE, LocalFree},
        Security::{
            Authorization::ConvertSidToStringSidW, GetTokenInformation, TOKEN_QUERY, TOKEN_USER,
            TokenUser,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: process pseudo-handle is valid; output is writable. The successful
    // token is closed below after every path through the fallible closure.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }

    let result = (|| {
        let mut bytes = 0;
        // SAFETY: standard size-query call with an open token and writable size.
        let first =
            unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut bytes) };
        if first != 0
            || io::Error::last_os_error().raw_os_error()
                != Some(ERROR_INSUFFICIENT_BUFFER.cast_signed())
        {
            return Err(io::Error::last_os_error());
        }

        // A usize backing allocation gives TOKEN_USER its required pointer alignment.
        let words = usize::try_from(bytes)
            .ok()
            .and_then(|size| size.checked_add(mem::size_of::<usize>() - 1))
            .map(|size| size / mem::size_of::<usize>())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid token size"))?;
        let mut storage = vec![0_usize; words];
        // SAFETY: aligned allocation contains at least the returned byte count.
        if unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                storage.as_mut_ptr().cast(),
                bytes,
                &mut bytes,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful TokenUser query initialized TOKEN_USER and its SID
        // in the still-live aligned allocation.
        let token_user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };

        let mut raw_sid = ptr::null_mut();
        // SAFETY: valid SID from TokenUser; output is writable.
        if unsafe { ConvertSidToStringSidW(token_user.User.Sid, &mut raw_sid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful conversion returns a NUL-terminated LocalAlloc string.
        let sid = unsafe {
            let length = (0..).take_while(|&index| *raw_sid.add(index) != 0).count();
            let value = String::from_utf16(std::slice::from_raw_parts(raw_sid, length))
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
            LocalFree(raw_sid.cast());
            value
        }?;
        Ok(sid)
    })();
    // SAFETY: successful OpenProcessToken handle, closed exactly once.
    unsafe { CloseHandle(token) };
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::verify_unix_directory;
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("private");
        fs::create_dir(&path).unwrap();
        (directory, path)
    }

    #[test]
    fn accepts_private_owned_directory() {
        use std::os::unix::fs::MetadataExt;
        let (_directory, path) = fixture();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let uid = fs::symlink_metadata(&path).unwrap().uid();
        assert!(verify_unix_directory(&path, uid).is_ok());
        fs::remove_dir(path).unwrap();
    }

    #[test]
    fn rejects_accessible_non_directory_and_symlink_fixtures() {
        use std::os::unix::fs::{MetadataExt, symlink};
        let (_directory, path) = fixture();
        let uid = fs::symlink_metadata(&path).unwrap().uid();

        fs::set_permissions(&path, fs::Permissions::from_mode(0o750)).unwrap();
        assert!(verify_unix_directory(&path, uid).is_err());
        fs::remove_dir(&path).unwrap();

        fs::write(&path, b"not a directory").unwrap();
        assert!(verify_unix_directory(&path, uid).is_err());
        fs::remove_file(&path).unwrap();

        let (_target_directory, target) = fixture();
        symlink(&target, &path).unwrap();
        assert!(verify_unix_directory(&path, uid).is_err());
        fs::remove_file(path).unwrap();
        fs::remove_dir(target).unwrap();
    }

    #[test]
    fn rejects_wrong_owner_identity() {
        use std::os::unix::fs::MetadataExt;
        let (_directory, path) = fixture();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let uid = fs::symlink_metadata(&path).unwrap().uid();
        assert!(verify_unix_directory(&path, uid.wrapping_add(1)).is_err());
        fs::remove_dir(path).unwrap();
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    #[test]
    fn pipe_sddl_has_one_user_ace_and_a_protected_dacl() {
        let sddl = super::protected_pipe_sddl("S-1-5-21-1-2-3-1001").unwrap();
        assert_eq!(sddl.to_string_lossy(), "D:P(A;;GA;;;S-1-5-21-1-2-3-1001)");
    }
}
