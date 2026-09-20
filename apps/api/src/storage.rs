use crate::config::StorageConfig;
use async_trait::async_trait;
use aws_sdk_s3::{
    Client,
    config::{Credentials, Region, RequestChecksumCalculation, ResponseChecksumValidation},
    error::ProvideErrorMetadata,
    presigning::PresigningConfig,
    types::{CompletedMultipartUpload, CompletedPart},
};
use std::{collections::HashMap, time::Duration};

pub const MIN_PART_SIZE: i64 = 64 * 1024 * 1024;
pub const MAX_PART_SIZE: i64 = 5 * 1024 * 1024 * 1024;
pub const MAX_PARTS: i64 = 10_000;

pub fn multipart_shape(bytes: i64) -> Result<(i64, i32), ()> {
    if !(0..=5_i64 * 1024 * 1024 * 1024 * 1024).contains(&bytes) {
        return Err(());
    }
    let size = MIN_PART_SIZE.max((bytes + MAX_PARTS - 1) / MAX_PARTS);
    if size > MAX_PART_SIZE {
        return Err(());
    }
    let count = if bytes == 0 {
        1
    } else {
        ((bytes + size - 1) / size) as i32
    };
    Ok((size, count))
}

#[async_trait]
pub trait ObjectStore: Send + Sync {
    async fn create_multipart(&self, key: &str, content_type: &str) -> Result<String, ()>;
    async fn sign_part(
        &self,
        key: &str,
        upload_id: &str,
        part: i32,
    ) -> Result<(String, HashMap<String, String>), ()>;
    async fn complete_multipart(
        &self,
        key: &str,
        upload_id: &str,
        parts: Vec<(i32, String)>,
    ) -> Result<(), ()>;
    async fn abort_multipart(&self, key: &str, upload_id: &str) -> Result<(), ()>;
    async fn head(&self, key: &str) -> Result<i64, ()>;
}

pub struct R2Store {
    client: Client,
    bucket: String,
}
impl R2Store {
    pub fn new(config: StorageConfig) -> Self {
        let sdk = aws_sdk_s3::config::Builder::new()
            .behavior_version_latest()
            .region(Region::new("auto"))
            .endpoint_url(format!(
                "https://{}.r2.cloudflarestorage.com",
                config.account_id
            ))
            .credentials_provider(Credentials::new(
                config.access_key,
                config.secret_key,
                None,
                None,
                "captures-r2",
            ))
            .force_path_style(true)
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired)
            .timeout_config(
                aws_sdk_s3::config::timeout::TimeoutConfig::builder()
                    .operation_timeout(Duration::from_secs(60))
                    .build(),
            )
            .build();
        Self {
            client: Client::from_conf(sdk),
            bucket: config.bucket,
        }
    }
}

#[async_trait]
impl ObjectStore for R2Store {
    async fn create_multipart(&self, key: &str, content_type: &str) -> Result<String, ()> {
        self.client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .cache_control("no-store")
            .send()
            .await
            .map_err(|_| ())?
            .upload_id()
            .map(str::to_owned)
            .ok_or(())
    }
    async fn sign_part(
        &self,
        key: &str,
        upload_id: &str,
        part: i32,
    ) -> Result<(String, HashMap<String, String>), ()> {
        let config = PresigningConfig::expires_in(Duration::from_secs(900)).map_err(|_| ())?;
        let request = self
            .client
            .upload_part()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .part_number(part)
            .presigned(config)
            .await
            .map_err(|_| ())?;
        let headers = request
            .headers()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Ok((request.uri().to_string(), headers))
    }
    async fn complete_multipart(
        &self,
        key: &str,
        upload_id: &str,
        parts: Vec<(i32, String)>,
    ) -> Result<(), ()> {
        let parts = parts
            .into_iter()
            .map(|(n, e)| CompletedPart::builder().part_number(n).e_tag(e).build())
            .collect();
        self.client
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .set_parts(Some(parts))
                    .build(),
            )
            .send()
            .await
            .map(|_| ())
            .map_err(|_| ())
    }
    async fn abort_multipart(&self, key: &str, upload_id: &str) -> Result<(), ()> {
        match self
            .client
            .abort_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if e.as_service_error().and_then(|e| e.code()) == Some("NoSuchUpload") => Ok(()),
            Err(_) => Err(()),
        }
    }
    async fn head(&self, key: &str) -> Result<i64, ()> {
        self.client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|_| ())?
            .content_length()
            .ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn presigned_parts_are_scoped_without_an_empty_body_signature() {
        let store = R2Store::new(StorageConfig {
            account_id: "0123456789abcdef0123456789abcdef".into(),
            bucket: "test-captures".into(),
            access_key: "test-key".into(),
            secret_key: "test-secret".into(),
        });
        let (url, headers) = store
            .sign_part("assets/abcdefghijkl", "test-upload", 3)
            .await
            .unwrap();
        let url = reqwest::Url::parse(&url).unwrap();
        assert_eq!(url.path(), "/test-captures/assets/abcdefghijkl");
        let query: HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(query.get("partNumber").unwrap(), "3");
        assert_eq!(query.get("uploadId").unwrap(), "test-upload");
        assert_eq!(query.get("X-Amz-Expires").unwrap(), "900");
        assert!(!headers.contains_key("content-length"));
        assert!(!headers.contains_key("authorization"));
        assert!(!headers.contains_key("x-amz-checksum-crc32"));
    }
}
