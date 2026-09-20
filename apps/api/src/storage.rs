use crate::config::StorageConfig;
use async_trait::async_trait;
use aws_sdk_s3::{
    Client,
    config::{Credentials, Region, RequestChecksumCalculation, ResponseChecksumValidation},
    primitives::ByteStream,
};
use std::time::Duration;

pub const MAX_BYTES: usize = 20 * 1024 * 1024;

#[async_trait]
pub trait ObjectStore: Send + Sync {
    async fn put(&self, key: &str, content_type: &str, bytes: Vec<u8>) -> Result<(), ()>;
    async fn get(&self, key: &str) -> Result<Vec<u8>, ()>;
    async fn delete(&self, key: &str) -> Result<(), ()>;
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
            .build();
        Self {
            client: Client::from_conf(sdk),
            bucket: config.bucket,
        }
    }
}

#[async_trait]
impl ObjectStore for R2Store {
    async fn put(&self, key: &str, content_type: &str, bytes: Vec<u8>) -> Result<(), ()> {
        tokio::time::timeout(
            Duration::from_secs(60),
            self.client
                .put_object()
                .bucket(&self.bucket)
                .key(key)
                .content_type(content_type)
                .cache_control("no-store")
                .body(ByteStream::from(bytes))
                .send(),
        )
        .await
        .map_err(|_| ())?
        .map(|_| ())
        .map_err(|_| ())
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, ()> {
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut object = self
                .client
                .get_object()
                .bucket(&self.bucket)
                .key(key)
                .send()
                .await
                .map_err(|_| ())?;
            let mut bytes = Vec::new();
            while let Some(chunk) = object.body.next().await {
                let chunk = chunk.map_err(|_| ())?;
                if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
                    return Err(());
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        })
        .await
        .map_err(|_| ())?
    }

    async fn delete(&self, key: &str) -> Result<(), ()> {
        tokio::time::timeout(
            Duration::from_secs(30),
            self.client
                .delete_object()
                .bucket(&self.bucket)
                .key(key)
                .send(),
        )
        .await
        .map_err(|_| ())?
        .map(|_| ())
        .map_err(|_| ())
    }
}
