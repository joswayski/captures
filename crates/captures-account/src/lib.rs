//! Explicit native account operations. Run blocking HTTP and vault calls on a
//! serialized worker, never the UI thread. Construction is side-effect free.
use std::{io::Read, time::Duration};

use reqwest::{
    blocking::{Client, Response},
    header::AUTHORIZATION,
    redirect::Policy,
};
use serde::Deserialize;

pub mod sharing;
mod vault;
pub use vault::OsVault;

pub const DEFAULT_API: &str = "https://captur.es";
const MAX_RESPONSE: u64 = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultError {
    /// Locked, access denied, or an interactive unlock was cancelled.
    Inaccessible,
    /// No usable credential service/session or another backend failure.
    Unavailable,
}

pub trait Vault {
    fn load(&self) -> Result<Option<String>, VaultError>;
    fn save(&self, token: &str) -> Result<(), VaultError>;
    fn delete(&self) -> Result<(), VaultError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidInput,
    InvalidCode { attempts_remaining: Option<u8> },
    InvalidSession,
    Offline,
    Unavailable,
    MalformedResponse,
    Vault(VaultError),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct User {
    pub id: String,
    pub email: String,
}

#[derive(Deserialize)]
struct Challenge {
    #[serde(rename = "challengeId")]
    challenge_id: String,
}
#[derive(Deserialize)]
struct Account {
    user: User,
}
#[derive(Deserialize)]
struct Verified {
    user: User,
    token: String,
}
#[derive(Deserialize)]
struct Rejection {
    error: String,
    #[serde(rename = "attemptsRemaining")]
    attempts_remaining: Option<u8>,
}

// Never derive Debug/Serialize on a type owning a bearer token.
enum Session {
    Empty,
    Active { token: String, user: Option<User> },
    Invalid,
}

/// One worker-owned session. A failed vault write retains a verified token for
/// `retry_save`; an invalidated token is never used for another request.
pub struct AccountClient<V: Vault> {
    http: Client,
    base: reqwest::Url,
    vault: V,
    session: Session,
    saved: bool,
}

impl AccountClient<OsVault> {
    /// Bind the real OS credential vault to the canonical API origin. This
    /// creates neither a network request nor a vault operation/prompt.
    pub fn production() -> Result<Self, Error> {
        Self::new(DEFAULT_API, OsVault::new())
    }
}

impl<V: Vault> AccountClient<V> {
    /// Only HTTPS origins (or loopback HTTP for disposable tests) are accepted.
    /// No HTTP or vault operation occurs until a method is explicitly called.
    pub fn new(origin: &str, vault: V) -> Result<Self, Error> {
        let base = reqwest::Url::parse(origin).map_err(|_| Error::InvalidInput)?;
        let loopback = base.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        if (base.scheme() != "https" && !(base.scheme() == "http" && loopback))
            || base.cannot_be_a_base()
            || base.path() != "/"
            || base.query().is_some()
            || base.fragment().is_some()
            || !base.username().is_empty()
            || base.password().is_some()
        {
            return Err(Error::InvalidInput);
        }
        let http = Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(Policy::none())
            .user_agent(concat!("Captures-Native/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| Error::Unavailable)?;
        Ok(Self {
            http,
            base,
            vault,
            session: Session::Empty,
            saved: false,
        })
    }

    fn url(&self, path: &str) -> reqwest::Url {
        self.base.join(path).expect("fixed API route")
    }

    /// Explicitly query the OS vault. A missing item means signed out; vault
    /// errors do not. Do not call this over an in-memory verified session.
    pub fn load(&mut self) -> Result<bool, Error> {
        if !matches!(self.session, Session::Empty) {
            return Ok(matches!(self.session, Session::Active { .. }));
        }
        if let Some(token) = self.vault.load().map_err(Error::Vault)? {
            if token.is_empty() || !token.is_ascii() || token.contains(char::is_whitespace) {
                return Err(Error::MalformedResponse);
            }
            self.session = Session::Active { token, user: None };
            self.saved = true;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn request_code(&self, email: &str) -> Result<String, Error> {
        if email.trim().is_empty() || email.len() > 254 {
            return Err(Error::InvalidInput);
        }
        let response = self
            .http
            .post(self.url("api/auth/email/request"))
            .json(&serde_json::json!({"email":email}))
            .send()
            .map_err(|_| Error::Offline)?;
        let challenge: Challenge = decode(response, 202)?;
        if challenge.challenge_id.is_empty() {
            return Err(Error::MalformedResponse);
        }
        Ok(challenge.challenge_id)
    }

    /// The OTP is consumed by the server only once. If persistence fails, use
    /// `retry_save` rather than resubmitting this verification request.
    pub fn verify(&mut self, challenge_id: &str, code: &str) -> Result<User, Error> {
        if !matches!(self.session, Session::Empty) {
            return Err(Error::InvalidInput);
        }
        if challenge_id.is_empty()
            || code.len() != 6
            || !code
                .bytes()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        {
            return Err(Error::InvalidInput);
        }
        let response = self
            .http
            .post(self.url("api/auth/email/verify"))
            .json(&serde_json::json!({"challengeId":challenge_id,"code":code,"transport":"bearer"}))
            .send()
            .map_err(|_| Error::Offline)?;
        let verified: Verified = decode(response, 200)?;
        if verified.token.is_empty()
            || !verified.token.is_ascii()
            || verified.token.contains(char::is_whitespace)
            || verified.user.id.is_empty()
            || verified.user.email.is_empty()
        {
            return Err(Error::MalformedResponse);
        }
        self.session = Session::Active {
            token: verified.token,
            user: Some(verified.user.clone()),
        };
        self.saved = false;
        self.retry_save()?;
        Ok(verified.user)
    }

    /// Returns the accepted user after a failed save, without consuming OTP again.
    pub fn retry_save(&mut self) -> Result<Option<User>, Error> {
        if let Session::Active { token, user } = &self.session {
            if !self.saved {
                self.vault.save(token).map_err(Error::Vault)?;
                self.saved = true;
            }
            Ok(user.clone())
        } else {
            Err(Error::InvalidSession)
        }
    }

    pub fn me(&mut self) -> Result<User, Error> {
        let Session::Active { token, .. } = &self.session else {
            return Err(Error::InvalidSession);
        };
        let response = self
            .http
            .get(self.url("api/account/me"))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .send()
            .map_err(|_| Error::Offline)?;
        if response.status().as_u16() == 401 {
            self.session = Session::Invalid;
            self.clear_invalid()?;
            return Err(Error::InvalidSession);
        }
        let account: Account = decode(response, 200)?;
        if account.user.id.is_empty() || account.user.email.is_empty() {
            return Err(Error::MalformedResponse);
        }
        Ok(account.user)
    }

    /// Server revocation first; on offline/503, keep the valid session for
    /// retry. Once revoked, failed vault deletion remains retryable locally.
    pub fn logout(&mut self) -> Result<(), Error> {
        if let Session::Active { token, .. } = &self.session {
            let response = self
                .http
                .post(self.url("api/auth/logout"))
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .send()
                .map_err(|_| Error::Offline)?;
            let status = response.status().as_u16();
            if status != 204 && status != 401 {
                return Err(if status == 503 {
                    Error::Unavailable
                } else {
                    Error::MalformedResponse
                });
            }
            self.session = Session::Invalid;
        }
        self.clear_invalid()
    }

    pub fn clear_invalid(&mut self) -> Result<(), Error> {
        if matches!(self.session, Session::Invalid) {
            if self.saved {
                self.vault.delete().map_err(Error::Vault)?;
            }
            self.session = Session::Empty;
            self.saved = false;
        }
        Ok(())
    }
}

fn decode<T: for<'de> Deserialize<'de>>(response: Response, expected: u16) -> Result<T, Error> {
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::MalformedResponse)?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err(Error::MalformedResponse);
    }
    if status == 503 {
        return Err(Error::Unavailable);
    }
    if status == 401 {
        return Err(Error::InvalidSession);
    }
    if status == 400
        && let Ok(rejection) = serde_json::from_slice::<Rejection>(&bytes)
    {
        if rejection.error == "invalid or expired code" {
            return Err(Error::InvalidCode {
                attempts_remaining: rejection.attempts_remaining,
            });
        }
        if rejection.error == "invalid email" {
            return Err(Error::InvalidInput);
        }
    }
    if status != expected {
        return Err(Error::MalformedResponse);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::MalformedResponse)
}
