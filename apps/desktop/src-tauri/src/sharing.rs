use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use keyring::Entry;
use reqwest::{Client, Method, Response, StatusCode, header};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, State};
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;

use crate::{
    models::{ArtifactKind, HistoryEntry},
    state::AppState,
    storage,
};

const DEFAULT_API_ORIGIN: &str = "https://captur.es";
const KEYRING_SERVICE: &str = "dev.valerio.captures";
const KEYRING_ACCOUNT: &str = "sharing-session";
const ONE_GIB: u64 = 1024 * 1024 * 1024;

#[derive(Default)]
pub struct SharingRuntime {
    access_token: Option<String>,
    access_expires_at: Option<std::time::Instant>,
    email: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SharingAuthStatus {
    pub signed_in: bool,
    pub email: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ShareResult {
    pub asset_id: String,
    pub share_url: String,
}

#[derive(Clone, Debug, Serialize)]
struct ShareProgress {
    artifact_id: String,
    stage: &'static str,
    completed_bytes: u64,
    total_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StoredCredential {
    email: String,
    refresh_token: String,
}

#[derive(Debug, Deserialize)]
struct UserResponse {
    email: String,
}

#[derive(Debug, Deserialize)]
struct VerifyResponse {
    user: UserResponse,
    access_token: String,
    refresh_token: String,
    expires_in: u64,
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    user: UserResponse,
    access_token: String,
    expires_in: u64,
}

#[derive(Clone, Debug, Deserialize)]
struct RemoteAsset {
    id: String,
    share_url: String,
}

#[derive(Debug, Deserialize)]
struct AssetResponse {
    asset: RemoteAsset,
}

#[derive(Debug, Deserialize)]
struct ReserveResponse {
    asset: RemoteAsset,
    original_upload: UploadPlan,
    preview_upload: Option<UploadPlan>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum UploadPlan {
    Single {
        url: String,
        headers: HashMap<String, String>,
    },
    Multipart {
        #[allow(dead_code)]
        upload_id: String,
        part_size: usize,
        parts: Vec<UploadPart>,
    },
}

#[derive(Debug, Deserialize)]
struct UploadPart {
    part_number: u32,
    url: String,
}

#[derive(Debug, Deserialize)]
struct PartsResponse {
    parts: Vec<UploadPart>,
}

#[derive(Debug, Serialize)]
struct CompletedPart {
    part_number: u32,
    etag: String,
}

enum LocalContent {
    Bytes(Vec<u8>),
    File { path: PathBuf, bytes: u64 },
}

struct UploadContext<'a> {
    state: &'a Arc<AppState>,
    remote_asset_id: &'a str,
    client: &'a Client,
    app: &'a AppHandle,
    artifact_id: &'a str,
    total_bytes: u64,
}

impl LocalContent {
    fn len(&self) -> u64 {
        match self {
            Self::Bytes(bytes) => u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            Self::File { bytes, .. } => *bytes,
        }
    }

    async fn sha256(&self) -> Result<String, SharingError> {
        let mut hasher = Sha256::new();
        match self {
            Self::Bytes(bytes) => hasher.update(bytes),
            Self::File { path, .. } => {
                let mut file = tokio::fs::File::open(path).await?;
                let mut buffer = vec![0_u8; 1024 * 1024];
                loop {
                    let read = file.read(&mut buffer).await?;
                    if read == 0 {
                        break;
                    }
                    hasher.update(&buffer[..read]);
                }
            }
        }
        Ok(BASE64.encode(hasher.finalize()))
    }

    async fn part(&self, part_number: u32, part_size: usize) -> Result<Vec<u8>, SharingError> {
        let offset = u64::from(part_number.saturating_sub(1))
            .saturating_mul(u64::try_from(part_size).unwrap_or(u64::MAX));
        match self {
            Self::Bytes(bytes) => {
                let start = usize::try_from(offset)
                    .map_err(|_| SharingError::Message("multipart offset is invalid".into()))?;
                let end = start.saturating_add(part_size).min(bytes.len());
                if start >= end {
                    return Err(SharingError::Message("multipart part is empty".into()));
                }
                Ok(bytes[start..end].to_vec())
            }
            Self::File { path, .. } => {
                use tokio::io::AsyncSeekExt;
                let mut file = tokio::fs::File::open(path).await?;
                file.seek(std::io::SeekFrom::Start(offset)).await?;
                let mut bytes = vec![0_u8; part_size];
                let mut filled = 0;
                while filled < bytes.len() {
                    let read = file.read(&mut bytes[filled..]).await?;
                    if read == 0 {
                        break;
                    }
                    filled += read;
                }
                bytes.truncate(filled);
                if bytes.is_empty() {
                    return Err(SharingError::Message("multipart part is empty".into()));
                }
                Ok(bytes)
            }
        }
    }
}

struct LocalAsset {
    history: HistoryEntry,
    original: LocalContent,
    original_mime: String,
    preview: Vec<u8>,
}

#[derive(Debug, Error)]
enum SharingError {
    #[error("{0}")]
    Message(String),
    #[error("request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("could not read the capture: {0}")]
    Io(#[from] std::io::Error),
    #[error("the sharing service returned invalid data: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sharing service returned {status}: {message}")]
    Api { status: StatusCode, message: String },
}

#[tauri::command]
pub async fn sharing_auth_status(
    state: State<'_, Arc<AppState>>,
) -> Result<SharingAuthStatus, String> {
    match ensure_access_token(&state).await {
        Ok((_, email)) => Ok(SharingAuthStatus {
            signed_in: true,
            email: Some(email),
        }),
        Err(_) => Ok(SharingAuthStatus {
            signed_in: false,
            email: None,
        }),
    }
}

#[tauri::command]
pub async fn sharing_start_email(email: String) -> Result<(), String> {
    let response = http_client()
        .post(api_url("/api/auth/email/start"))
        .json(&json!({ "email": email.trim(), "client": "desktop" }))
        .send()
        .await
        .map_err(|error| display_error(error.into()))?;
    ensure_success(response).await.map_err(display_error)?;
    Ok(())
}

#[tauri::command]
pub async fn sharing_verify_email(
    email: String,
    code: String,
    state: State<'_, Arc<AppState>>,
) -> Result<SharingAuthStatus, String> {
    let response = http_client()
        .post(api_url("/api/auth/email/verify"))
        .json(&json!({
            "email": email.trim(),
            "code": code.trim(),
            "client": "desktop",
        }))
        .send()
        .await
        .map_err(|error| display_error(error.into()))?;
    let verified: VerifyResponse = decode_response(response).await.map_err(display_error)?;
    save_credential(StoredCredential {
        email: verified.user.email.clone(),
        refresh_token: verified.refresh_token,
    })
    .await
    .map_err(display_error)?;
    update_runtime(
        &state,
        verified.user.email.clone(),
        verified.access_token,
        verified.expires_in,
    );
    Ok(SharingAuthStatus {
        signed_in: true,
        email: Some(verified.user.email),
    })
}

#[tauri::command]
pub async fn sharing_sign_out(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let token = ensure_access_token(&state)
        .await
        .ok()
        .map(|(token, _)| token);
    if let Some(token) = token {
        let _ = http_client()
            .post(api_url("/api/auth/logout"))
            .bearer_auth(token)
            .send()
            .await;
    }
    *state.sharing.lock() = SharingRuntime::default();
    delete_credential().await.map_err(display_error)
}

#[tauri::command]
pub async fn share_history_artifact(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    artifact_id: String,
    expires_in_seconds: Option<u64>,
) -> Result<ShareResult, String> {
    share_history_artifact_inner(&app, &state, &artifact_id, expires_in_seconds)
        .await
        .map_err(display_error)
}

#[tauri::command]
pub async fn make_history_artifact_private(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    artifact_id: String,
) -> Result<(), String> {
    make_history_artifact_private_inner(&app, &state, &artifact_id)
        .await
        .map_err(display_error)
}

#[tauri::command]
pub async fn delete_remote_history_artifact(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    artifact_id: String,
) -> Result<(), String> {
    delete_remote_history_artifact_inner(&app, &state, &artifact_id)
        .await
        .map_err(display_error)
}

async fn share_history_artifact_inner(
    app: &AppHandle,
    state: &Arc<AppState>,
    artifact_id: &str,
    expires_in_seconds: Option<u64>,
) -> Result<ShareResult, SharingError> {
    let history = state
        .history
        .lock()
        .iter()
        .find(|entry| entry.id == artifact_id)
        .cloned()
        .ok_or_else(|| SharingError::Message("capture history entry is unavailable".into()))?;

    if let Some(remote_id) = history.remote_asset_id.as_deref() {
        match authorized_json::<AssetResponse>(
            state,
            Method::GET,
            &format!("/api/assets/{remote_id}"),
            None,
        )
        .await
        {
            Ok(_) => {
                let shared = enable_share(state, remote_id, expires_in_seconds).await?;
                persist_remote_share(app, state, artifact_id, &shared.asset)?;
                return Ok(ShareResult {
                    asset_id: shared.asset.id,
                    share_url: shared.asset.share_url,
                });
            }
            Err(SharingError::Api {
                status: StatusCode::NOT_FOUND,
                ..
            }) => {}
            Err(error) => return Err(error),
        }
    }

    emit_progress(app, artifact_id, "preparing", 0, history.size_bytes);
    let local = load_local_asset(history).await?;
    let total_bytes = local
        .original
        .len()
        .saturating_add(u64::try_from(local.preview.len()).unwrap_or(u64::MAX));
    if total_bytes > ONE_GIB {
        return Err(SharingError::Message(
            "this capture and its preview exceed the 1 GiB upload limit".into(),
        ));
    }
    let original_sha256 = local.original.sha256().await?;
    let preview_sha256 = BASE64.encode(Sha256::digest(&local.preview));
    emit_progress(app, artifact_id, "reserving", 0, total_bytes);

    let reserve_body = json!({
        "kind": artifact_kind(&local.history),
        "original": {
            "bytes": local.original.len(),
            "mime_type": local.original_mime,
            "sha256": original_sha256,
        },
        "preview": {
            "bytes": local.preview.len(),
            "mime_type": "image/png",
            "sha256": preview_sha256,
        },
        "width": local.history.width,
        "height": local.history.height,
        "duration_ms": local.history.duration_ms,
    });
    let reservation: ReserveResponse =
        authorized_json(state, Method::POST, "/api/assets", Some(reserve_body)).await?;

    let remote_asset_id = reservation.asset.id.clone();
    let preview_bytes = u64::try_from(local.preview.len()).unwrap_or(u64::MAX);
    let result = async {
        let client = http_client();
        let upload = UploadContext {
            state,
            remote_asset_id: &remote_asset_id,
            client: &client,
            app,
            artifact_id,
            total_bytes,
        };
        if let Some(preview_plan) = reservation.preview_upload {
            upload_content(
                &upload,
                &LocalContent::Bytes(local.preview),
                preview_plan,
                0,
            )
            .await?;
        }
        let completed_parts = upload_content(
            &upload,
            &local.original,
            reservation.original_upload,
            preview_bytes,
        )
        .await?;
        emit_progress(app, artifact_id, "finalizing", total_bytes, total_bytes);
        let completion: AssetResponse = authorized_json(
            state,
            Method::POST,
            &format!("/api/assets/{remote_asset_id}/complete"),
            Some(json!({ "parts": completed_parts })),
        )
        .await?;
        enable_share(state, &completion.asset.id, expires_in_seconds).await
    }
    .await;
    let shared = match result {
        Ok(shared) => shared,
        Err(error) => {
            let _ = authorized_empty(
                state,
                Method::DELETE,
                &format!("/api/assets/{remote_asset_id}"),
                None,
            )
            .await;
            return Err(error);
        }
    };
    persist_remote_share(app, state, artifact_id, &shared.asset)?;
    emit_progress(app, artifact_id, "complete", total_bytes, total_bytes);
    Ok(ShareResult {
        asset_id: shared.asset.id,
        share_url: shared.asset.share_url,
    })
}

async fn make_history_artifact_private_inner(
    app: &AppHandle,
    state: &Arc<AppState>,
    artifact_id: &str,
) -> Result<(), SharingError> {
    let remote_asset_id = remote_asset_id(state, artifact_id)?;
    let response: AssetResponse = authorized_json(
        state,
        Method::PATCH,
        &format!("/api/assets/{remote_asset_id}/share"),
        Some(json!({ "access": "private" })),
    )
    .await?;
    persist_remote_metadata(app, state, artifact_id, Some(&response.asset.id), None)
}

async fn delete_remote_history_artifact_inner(
    app: &AppHandle,
    state: &Arc<AppState>,
    artifact_id: &str,
) -> Result<(), SharingError> {
    let remote_asset_id = remote_asset_id(state, artifact_id)?;
    authorized_empty(
        state,
        Method::DELETE,
        &format!("/api/assets/{remote_asset_id}"),
        None,
    )
    .await?;
    persist_remote_metadata(app, state, artifact_id, None, None)
}

fn remote_asset_id(state: &Arc<AppState>, artifact_id: &str) -> Result<String, SharingError> {
    state
        .history
        .lock()
        .iter()
        .find(|entry| entry.id == artifact_id)
        .and_then(|entry| entry.remote_asset_id.clone())
        .ok_or_else(|| SharingError::Message("this capture has no remote asset".into()))
}

async fn load_local_asset(history: HistoryEntry) -> Result<LocalAsset, SharingError> {
    let (original, original_mime, preview) = match history.kind {
        ArtifactKind::Screenshot => {
            let id = history.id.clone();
            let (original, preview) = tauri::async_runtime::spawn_blocking(move || {
                storage::load_history_images(&id).map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| SharingError::Message(error.to_string()))?
            .map_err(SharingError::Message)?;
            (
                LocalContent::Bytes(original),
                "image/png".to_owned(),
                preview,
            )
        }
        ArtifactKind::Video | ArtifactKind::Gif => {
            let path = history
                .recording_media_path()
                .map(PathBuf::from)
                .filter(|path| path.is_file())
                .ok_or_else(|| SharingError::Message("recording file is unavailable".into()))?;
            let bytes = tokio::fs::metadata(&path).await?.len();
            let preview_id = history.id.clone();
            let preview = tauri::async_runtime::spawn_blocking(move || {
                storage::load_history_image(&preview_id, true).map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| SharingError::Message(error.to_string()))?
            .map_err(SharingError::Message)?;
            let mime = history.mime_type.clone().unwrap_or_else(|| {
                if history.kind == ArtifactKind::Gif {
                    "image/gif".to_owned()
                } else {
                    "video/mp4".to_owned()
                }
            });
            (LocalContent::File { path, bytes }, mime, preview)
        }
    };
    Ok(LocalAsset {
        history,
        original,
        original_mime,
        preview,
    })
}

async fn upload_content(
    upload: &UploadContext<'_>,
    content: &LocalContent,
    plan: UploadPlan,
    completed_before: u64,
) -> Result<Option<Vec<CompletedPart>>, SharingError> {
    match plan {
        UploadPlan::Single { url, headers } => {
            let mut request = upload
                .client
                .put(url)
                .header(header::CONTENT_LENGTH, content.len());
            for (name, value) in headers {
                request = request.header(&name, value);
            }
            request = match content {
                LocalContent::Bytes(bytes) => request.body(bytes.clone()),
                LocalContent::File { path, .. } => {
                    let file = tokio::fs::File::open(path).await?;
                    request.body(reqwest::Body::wrap_stream(ReaderStream::new(file)))
                }
            };
            ensure_success(request.send().await?).await?;
            emit_progress(
                upload.app,
                upload.artifact_id,
                "uploading",
                completed_before.saturating_add(content.len()),
                upload.total_bytes,
            );
            Ok(None)
        }
        UploadPlan::Multipart {
            part_size, parts, ..
        } => {
            let mut completed = Vec::with_capacity(parts.len());
            let mut uploaded = 0_u64;
            for part in parts {
                let bytes = content.part(part.part_number, part_size).await?;
                let mut response = upload
                    .client
                    .put(part.url)
                    .header(header::CONTENT_LENGTH, bytes.len())
                    .body(bytes.clone())
                    .send()
                    .await?;
                if response.status() == StatusCode::FORBIDDEN {
                    let refreshed: PartsResponse = authorized_json(
                        upload.state,
                        Method::POST,
                        &format!("/api/assets/{}/parts", upload.remote_asset_id),
                        Some(json!({ "part_numbers": [part.part_number] })),
                    )
                    .await?;
                    let Some(refreshed_part) = refreshed.parts.into_iter().next() else {
                        return Err(SharingError::Message(
                            "sharing service omitted the refreshed upload URL".into(),
                        ));
                    };
                    response = upload
                        .client
                        .put(refreshed_part.url)
                        .header(header::CONTENT_LENGTH, bytes.len())
                        .body(bytes.clone())
                        .send()
                        .await?;
                }
                if !response.status().is_success() {
                    return Err(api_error(response).await);
                }
                let etag = response
                    .headers()
                    .get(header::ETAG)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        SharingError::Message("upload response omitted its ETag".into())
                    })?;
                uploaded = uploaded.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
                completed.push(CompletedPart {
                    part_number: part.part_number,
                    etag,
                });
                emit_progress(
                    upload.app,
                    upload.artifact_id,
                    "uploading",
                    completed_before.saturating_add(uploaded),
                    upload.total_bytes,
                );
            }
            Ok(Some(completed))
        }
    }
}

async fn enable_share(
    state: &Arc<AppState>,
    asset_id: &str,
    expires_in_seconds: Option<u64>,
) -> Result<AssetResponse, SharingError> {
    authorized_json(
        state,
        Method::PATCH,
        &format!("/api/assets/{asset_id}/share"),
        Some(json!({
            "access": "shared",
            "expires_in_seconds": expires_in_seconds,
        })),
    )
    .await
}

fn persist_remote_share(
    app: &AppHandle,
    state: &Arc<AppState>,
    artifact_id: &str,
    remote: &RemoteAsset,
) -> Result<(), SharingError> {
    persist_remote_metadata(
        app,
        state,
        artifact_id,
        Some(&remote.id),
        Some(&remote.share_url),
    )
}

fn persist_remote_metadata(
    app: &AppHandle,
    state: &Arc<AppState>,
    artifact_id: &str,
    remote_asset_id: Option<&str>,
    remote_share_url: Option<&str>,
) -> Result<(), SharingError> {
    let updated = {
        let mut history = state.history.lock();
        let entry = history
            .iter_mut()
            .find(|entry| entry.id == artifact_id)
            .ok_or_else(|| SharingError::Message("capture history entry is unavailable".into()))?;
        entry.remote_asset_id = remote_asset_id.map(str::to_owned);
        entry.remote_share_url = remote_share_url.map(str::to_owned);
        entry.clone()
    };
    if let Err(error) = storage::update_history_entry_metadata(&updated) {
        eprintln!("failed to persist remote share metadata: {error}");
    }
    let _ = app.emit("capture-history-changed", ());
    Ok(())
}

async fn authorized_json<T: DeserializeOwned>(
    state: &Arc<AppState>,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<T, SharingError> {
    decode_response(authorized_response(state, method, path, body).await?).await
}

async fn authorized_empty(
    state: &Arc<AppState>,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<(), SharingError> {
    ensure_success(authorized_response(state, method, path, body).await?).await?;
    Ok(())
}

async fn authorized_response(
    state: &Arc<AppState>,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<Response, SharingError> {
    let (token, _) = ensure_access_token(state).await?;
    let response = send_authorized(&token, method.clone(), path, body.clone()).await?;
    if response.status() != StatusCode::UNAUTHORIZED {
        return Ok(response);
    }
    clear_runtime(state);
    let (token, _) = ensure_access_token(state).await?;
    send_authorized(&token, method, path, body).await
}

async fn send_authorized(
    token: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<Response, SharingError> {
    let mut request = http_client()
        .request(method, api_url(path))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    Ok(request.send().await?)
}

async fn ensure_access_token(state: &Arc<AppState>) -> Result<(String, String), SharingError> {
    {
        let runtime = state.sharing.lock();
        if runtime
            .access_expires_at
            .is_some_and(|expiry| expiry > std::time::Instant::now() + Duration::from_secs(30))
            && let (Some(token), Some(email)) = (&runtime.access_token, &runtime.email)
        {
            return Ok((token.clone(), email.clone()));
        }
    }
    let credential = load_credential().await?;
    let response = http_client()
        .post(api_url("/api/auth/refresh"))
        .json(&json!({ "refresh_token": credential.refresh_token }))
        .send()
        .await?;
    if response.status() == StatusCode::UNAUTHORIZED {
        let _ = delete_credential().await;
        clear_runtime(state);
        return Err(SharingError::Message(
            "sign in to share this capture".into(),
        ));
    }
    let refreshed: RefreshResponse = decode_response(response).await?;
    update_runtime(
        state,
        refreshed.user.email.clone(),
        refreshed.access_token.clone(),
        refreshed.expires_in,
    );
    Ok((refreshed.access_token, refreshed.user.email))
}

fn update_runtime(state: &Arc<AppState>, email: String, token: String, expires_in: u64) {
    *state.sharing.lock() = SharingRuntime {
        access_token: Some(token),
        access_expires_at: Some(
            std::time::Instant::now() + Duration::from_secs(expires_in.saturating_sub(5)),
        ),
        email: Some(email),
    };
}

fn clear_runtime(state: &Arc<AppState>) {
    *state.sharing.lock() = SharingRuntime::default();
}

async fn save_credential(credential: StoredCredential) -> Result<(), SharingError> {
    let value = serde_json::to_string(&credential)?;
    tauri::async_runtime::spawn_blocking(move || {
        keyring_entry()?.set_password(&value).map_err(keyring_error)
    })
    .await
    .map_err(|error| SharingError::Message(error.to_string()))??;
    Ok(())
}

async fn load_credential() -> Result<StoredCredential, SharingError> {
    let value = tauri::async_runtime::spawn_blocking(move || {
        keyring_entry()?.get_password().map_err(keyring_error)
    })
    .await
    .map_err(|error| SharingError::Message(error.to_string()))??;
    Ok(serde_json::from_str(&value)?)
}

async fn delete_credential() -> Result<(), SharingError> {
    tauri::async_runtime::spawn_blocking(move || {
        let entry = keyring_entry()?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(keyring_error(error)),
        }
    })
    .await
    .map_err(|error| SharingError::Message(error.to_string()))??;
    Ok(())
}

fn keyring_entry() -> Result<Entry, SharingError> {
    Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).map_err(keyring_error)
}

fn keyring_error(error: keyring::Error) -> SharingError {
    SharingError::Message(format!(
        "could not access the operating system keychain: {error}"
    ))
}

async fn ensure_success(response: Response) -> Result<Response, SharingError> {
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(api_error(response).await)
    }
}

async fn decode_response<T: DeserializeOwned>(response: Response) -> Result<T, SharingError> {
    let response = ensure_success(response).await?;
    Ok(response.json().await?)
}

async fn api_error(response: Response) -> SharingError {
    let status = response.status();
    let body = response.json::<Value>().await.unwrap_or(Value::Null);
    let message = body
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("request failed")
        .to_owned();
    SharingError::Api { status, message }
}

fn artifact_kind(entry: &HistoryEntry) -> &'static str {
    match entry.kind {
        ArtifactKind::Screenshot => "screenshot",
        ArtifactKind::Video => "video",
        ArtifactKind::Gif => "gif",
    }
}

fn api_url(path: &str) -> String {
    let origin = option_env!("CAPTURES_API_BASE_URL")
        .unwrap_or(DEFAULT_API_ORIGIN)
        .trim_end_matches('/');
    format!("{origin}{path}")
}

fn http_client() -> Client {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30 * 60))
        .build()
        .unwrap_or_else(|_| Client::new())
}

fn emit_progress(
    app: &AppHandle,
    artifact_id: &str,
    stage: &'static str,
    completed_bytes: u64,
    total_bytes: u64,
) {
    let _ = app.emit(
        "share-upload-progress",
        ShareProgress {
            artifact_id: artifact_id.to_owned(),
            stage,
            completed_bytes,
            total_bytes,
        },
    );
}

fn display_error(error: SharingError) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_api_is_the_default() {
        if option_env!("CAPTURES_API_BASE_URL").is_none() {
            assert_eq!(api_url("/api/assets"), "https://captur.es/api/assets");
        }
    }

    #[test]
    fn byte_parts_are_bounded_and_ordered() {
        tauri::async_runtime::block_on(async {
            let content = LocalContent::Bytes((0_u8..10).collect());
            assert_eq!(content.part(1, 4).await.unwrap(), vec![0, 1, 2, 3]);
            assert_eq!(content.part(2, 4).await.unwrap(), vec![4, 5, 6, 7]);
            assert_eq!(content.part(3, 4).await.unwrap(), vec![8, 9]);
        });
    }
}
