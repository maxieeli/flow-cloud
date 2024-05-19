use crate::notify::{ClientToken, TokenStateReceiver};
use brotli::CompressorReader;
use gotrue_entity::dto::AuthProvider;
use shared_entity::dto::workspace_dto::{
    CreateWorkspaceParam, PatchWorkspaceParam, WorkspaceMemberInvitation,
};
use std::fmt::{Display, Formatter};
use std::io::Read;

use app_error::AppError;
use bytes::Bytes;
use database_entity::dto::{
    AFCollabMember, AFCollabMembers, AFSnapshotMeta,
    AFSnapshotMetas, AFUserProfile, AFUserWorkspaceInfo,
    AFWorkspace, AFWorkspaceInvitation, AFWorkspaceInvitationStatus,
    AFWorkspaceMember, AFWorkspaces, BatchQueryCollabParams,
    BatchQueryCollabResult, CollabMemberIdentify, CreateCollabParams,
    DeleteCollabParams, InsertCollabMemberParams, QueryCollab,
    QueryCollabMembers, QueryCollabParams, QuerySnapshotParams,
    SnapshotData, UpdateCollabMemberParams,
};
use futures_util::StreamExt;
use gotrue::grant::PasswordGrant;
use gotrue::grant::{Grant, RefreshTokenGrant};
use gotrue::params::MagicLinkParams;
use gotrue::params::{AdminUserParams, GenerateLinkParams};
use mime::Mime;
use parking_lot::RwLock;
use realtime_entity::EncodedCollab;
use reqwest::{header, StatusCode};
use collab_entity::CollabType;
use reqwest::header::HeaderValue;
use reqwest::Method;
use reqwest::RequestBuilder;
use semver::Version;
use shared_entity::dto::auth_dto::SignInTokenResponse;
use shared_entity::dto::auth_dto::UpdateUserParams;
use shared_entity::dto::workspace_dto::{
    BlobMetadata, CreateWorkspaceMembers, RepeatedBlobMetaData,
    WorkspaceMemberChangeset, WorkspaceMembers, WorkspaceSpaceUsage,
};
use shared_entity::response::{AppResponse, AppResponseError};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, event, info, instrument, trace, warn};
use url::Url;
use crate::ws::ConnectInfo;
use gotrue_entity::dto::SignUpResponse::{Authenticated, NotAuthenticated};
use gotrue_entity::dto::{GotrueTokenResponse, UpdateGotrueUserParams, User};

#[derive(Clone)]
pub struct ClientConfiguration {
    /// Lower Levels (0-4): Faster compression and decompression speeds but lower compression ratios. Suitable for scenarios where speed is more critical than reducing data size.
    /// Medium Levels (5-9): A balance between compression ratio and speed. These levels are generally good for a mix of performance and efficiency.
    /// Higher Levels (10-11): The highest compression ratios, but significantly slower and more resource-intensive. These are typically used in scenarios where reducing data size is paramount and resource usage is a secondary concern, such as for static content compression in web servers.
    pub(crate) compression_quality: u32,
    /// A larger buffer size means more data is compressed in a single operation, which can lead to better compression ratios
    /// since Brotli has more data to analyze for patterns and repetitions.
    pub(crate) compression_buffer_size: usize,
}

impl ClientConfiguration {
    pub fn with_compression_buffer_size(mut self, compression_buffer_size: usize) -> Self {
        self.compression_buffer_size = compression_buffer_size;
        self
    }
    pub fn with_compression_quality(mut self, compression_quality: u32) -> Self {
        self.compression_quality = if compression_quality > 11 {
            warn!("compression_quality is larger than 11, set it to 11");
            11
        } else {
            compression_quality
        };
        self
    }
}

impl Default for ClientConfiguration {
    fn default() -> Self {
        Self {
            compression_quality: 8,
            compression_buffer_size: 10240,
        }
    }
}


#[derive(Clone)]
pub struct Client {
    pub(crate) cloud_client: reqwest::Client,
    pub(crate) gotrue_client: gotrue::api::Client,
    pub base_url: String,
    ws_addr: String,
    pub device_id: String,
    pub client_version: Version,
    pub(crate) token: Arc<RwLock<ClientToken>>,
    pub(crate) is_refreshing_token: Arc<AtomicBool>,
    pub(crate) refresh_ret_txs: Arc<RwLock<Vec<RefreshTokenSender>>>,
    pub(crate) config: ClientConfiguration,
}

pub(crate) type RefreshTokenSender = tokio::sync::oneshot::Sender<Result<(), AppResponseError>>;

/// Hardcoded schema in the frontend application. Do not change this value.
const DESKTOP_CALLBACK_URL: &str = "appflow-flutter://login-callback";

impl Client {}

impl Display for Client {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "Client {{ base_url: {}, ws_addr: {}, gotrue_url: {} }}",
            self.base_url, self.ws_addr, self.gotrue_client.base_url
        ))
    }
}

fn url_missing_param(param: &str) -> AppResponseError {
    AppError::InvalidRequest(format!("Url Missing Parameter:{}", param)).into()
}

pub(crate) fn log_request_id(resp: &reqwest::Response) {
    if let Some(request_id) = resp.headers().get("x-request-id") {
        event!(tracing::Level::INFO, "request_id: {:?}", request_id);
    } else {
        event!(tracing::Level::DEBUG, "request_id: not found");
    }
}

pub async fn spawn_blocking_brotli_compress(
    data: Vec<u8>,
    quality: u32,
    buffer_size: usize,
) -> Result<Vec<u8>, AppError> {
    tokio::task::spawn_blocking(move || {
        let mut compressor = CompressorReader::new(&*data, buffer_size, quality, 22);
        let mut compressed_data = Vec::new();
        compressor
            .read_to_end(&mut compressed_data)
            .map_err(|err| AppError::InvalidRequest(format!("Failed to compress data: {}", err)))?;
        Ok(compressed_data)
    })
    .await
    .map_err(AppError::from)?
}
