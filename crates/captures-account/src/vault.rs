use crate::{Vault, VaultError};

// Separate native development identity; never reads shipping Tauri credentials.
const SERVICE: &str = "es.captur.native.account";
const ACCOUNT: &str = "bearer";

/// macOS Keychain / Windows Credential Manager / Linux Secret Service.
/// Available only through `AccountClient::production`, so a fixture or staging
/// origin cannot accidentally receive the production vault's bearer.
///
/// ```compile_fail
/// use captures_account::{AccountClient, OsVault};
/// let client = AccountClient::new("https://another-server.example", OsVault(()));
/// ```
pub struct OsVault(());

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
compile_error!("Captures native account vault requires macOS, Windows, or Linux");

impl OsVault {
    pub(crate) fn new() -> Self {
        Self(())
    }

    fn entry() -> Result<keyring::Entry, VaultError> {
        keyring::Entry::new(SERVICE, ACCOUNT).map_err(map_error)
    }
}

fn map_error(error: keyring::Error) -> VaultError {
    match error {
        keyring::Error::NoStorageAccess(_) => VaultError::Inaccessible,
        _ => VaultError::Unavailable,
    }
}

impl Vault for OsVault {
    fn load(&self) -> Result<Option<String>, VaultError> {
        match Self::entry()?.get_password() {
            Ok(token) => Ok(Some(token)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(map_error(error)),
        }
    }

    fn save(&self, token: &str) -> Result<(), VaultError> {
        Self::entry()?.set_password(token).map_err(map_error)
    }

    fn delete(&self) -> Result<(), VaultError> {
        match Self::entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(map_error(error)),
        }
    }
}
