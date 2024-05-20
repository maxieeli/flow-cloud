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

impl Client {
    /// Constructs a new `Client` instance.
    pub fn new(
        base_url: &str,
        ws_addr: &str,
        gotrue_url: &str,
        device_id: &str,
        config: ClientConfiguration,
        client_id: &str,
    ) -> Self {
        let reqwest_client = reqwest::Client::new();
        let client_version = Version::parse(client_id).unwrap_or_else(|_| Version::new(0, 5, 0));
        Self {
            base_url: base_url.to_string(),
            ws_addr: ws_addr.to_string(),
            cloud_client: reqwest_client.clone(),
            gotrue_client: gotrue::api::Client::new(reqwest_client, gotrue_url),
            token: Arc::new(RwLock::new(ClientToken::new())),
            is_refreshing_token: Default::default(),
            refresh_ret_txs: Default::default(),
            config,
            device_id: device_id.to_string(),
            client_version,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }
    pub fn ws_addr(&self) -> &str {
        &self.ws_addr
    }
    pub fn gotrue_url(&self) -> &str {
        &self.gotrue_client.base_url
    }

    #[instrument(level = "debug", skip_all, err)]
    pub fn restore_token(&self, token: &str) -> Result<(), AppResponseError> {
        match serde_json::from_str::<GotrueTokenResponse>(token) {
            Ok(token) => {
                self.token.write().set(token);
                Ok(())
            },
            Err(err) => {
                error!("fail to deserialize token:{}, error:{}", token, err);
                Err(err.into())
            },
        }
    }

    /// Retrieves the string representation of the [GotrueTokenResponse]. The returned value can be
    /// saved to the client application's local storage and used to restore the client's authentication
    /// This function attempts to acquire a read lock on `self.token` and retrieves the
    /// string representation of the access token. If the lock cannot be acquired or
    /// the token is not present, an error is returned.
    #[instrument(level = "debug", skip_all, err)]
    pub fn get_token(&self) -> Result<String, AppResponseError> {
        let token_str = self
            .token
            .read()
            .try_get()
            .map_err(|err| AppResponseError::from(AppError::OAuthError(err.to_string())))?;
        Ok(token_str)
    }

    pub fn subscribe_token_state(&self) -> TokenStateReceiver {
        self.token.read().subscribe()
    }

    /// Attempts to sign in using a URL, extracting refresh_token from the URL.
    /// It looks like, e.g., `appflow-flutter://#access_token=...&expires_in=3600&provider_token=...&refresh_token=...&token_type=bearer`.
    /// return a bool indicating if the user is new
    #[instrument(level = "debug", skip_all, err)]
    pub async fn sign_in_with_url(&self, url: &str) -> Result<bool, AppResponseError> {
        let parsed = Url::parse(url)?;
        let key_value_pairs = parsed
            .fragment()
            .ok_or(url_missing_param("fragment"))?
            .split('&');
        let mut refresh_token: Option<&str> = None;
        for param in key_value_pairs {
            match param.split_once('=') {
                Some(pair) => {
                    let (k, v) = pair;
                    if k == "refresh_token" {
                        refresh_token = Some(v);
                        break;
                    }
                },
                None => warn!("param is not in key=value format: {}", param),
            }
        }
        let refresh_token = refresh_token.ok_or(url_missing_param("refresh_token"))?;
        let new_token = self
            .gotrue_client
            .token(&Grant::RefreshToken(RefreshTokenGrant {
                refresh_token: refresh_token.to_owned(),
            }))
            .await?;
        let (_user, new) = self.verify_token(&new_token.access_token).await?;
        self.token.write().set(new_token);
        Ok(new)
    }

    /// Returns an OAuth URL by constructing the authorization URL for the specified provider.
    /// This asynchronous function communicates with the GoTrue client to retrieve settings and
    /// validate the availability of the specified OAuth provider. If the provider is available,
    /// it constructs and returns the OAuth URL. When the user opens the OAuth URL, it redirects to
    /// the corresponding provider's OAuth web page. After the user is authenticated, the browser will open
    /// a deep link to the AppFlow app (iOS, macOS, etc.), which will call [Client::sign_in_with_url] to sign in.
    #[instrument(level = "debug", skip_all, err)]
    pub async fn generate_oauth_url_with_provider(
        &self,
        provider: &AuthProvider,
    ) -> Result<String, AppResponseError> {
        let settings = self.gotrue_client.settings().await?;
        if !settings.external.has_provider(provider) {
            return Err(AppError::InvalidOAuthProvider(provider.as_str().to_owned()).into());
        }
        let url = format!("{}/authorize", self.gotrue_client.base_url,);
        let mut url = Url::parse(&url)?;
        url
            .query_pairs_mut()
            .append_pair("provider", provider.as_str())
            .append_pair("redirect_to", DESKTOP_CALLBACK_URL);
        if let AuthProvider::Google = provider {
            url
                .query_pairs_mut()
                .append_pair("access_type", "offline")
                .append_pair("prompt", "consent");
        }
        Ok(url.to_string())
    }

    /// Generates a sign action link for the specified email address.
    /// This is only applicable if user token is with admin privilege.
    /// This action link is used on web browser to sign in. When user then click the action link in the browser,
    /// which calls gotrue authentication server, which then redirects to the appflowy-flutter:// with the authentication token.
    #[instrument(level = "debug", skip_all, err)]
    pub async fn generate_sign_in_action_link(
        &self,
        email: &str,
    ) -> Result<String, AppResponseError> {
        let admin_user_params: GenerateLinkParams = GenerateLinkParams {
            email: email.to_string(),
            ..Default::default()
        };
        let link_resp = self
            .gotrue_client
            .admin_generate_link(&self.access_token()?, &admin_user_params)
            .await?;
        assert_eq!(link_resp.email, email);
        Ok(link_resp.action_link)
    }

    #[cfg(feature = "test_util")]
    /// Used to extract the sign in url from the action link
    /// Only expose this method for testing
    pub async fn extract_sign_in_url(&self, action_link: &str) -> Result<String, AppResponseError> {
        let resp = reqwest::Client::new().get(action_link).send().await?;
        let html = resp.text().await.unwrap();
        trace!("action_link:{}, html: {}", action_link, html);
        let fragment = scraper::Html::parse_fragment(&html);
        let selector = scraper::Selector::parse("a").unwrap();
        let sign_in_url = fragment
            .select(&selector)
            .next()
            .unwrap()
            .value()
            .attr("href")
            .unwrap()
            .to_string();
        Ok(sign_in_url)
    }

    #[inline]
    #[instrument(level = "debug", skip_all, err)]
    async fn verify_token(&self, access_token: &str) -> Result<(User, bool), AppResponseError> {
        let user = self.gotrue_client.user_info(access_token).await?;
        let is_new = self.verify_token_cloud(access_token).await?;
        Ok((user, is_new))
    }

    #[instrument(level = "debug", skip_all, err)]
    #[inline]
    async fn verify_token_cloud(&self, access_token: &str) -> Result<bool, AppResponseError> {
        let url = format!("{}/api/user/verify/{}", self.base_url, access_token);
        let resp = self.cloud_client.get(&url).send().await?;
        let sign_in_resp: SignInTokenResponse = AppResponse::from_response(resp).await?.into_data()?;
        Ok(sign_in_resp.is_new)
    }

    // Invites another user by sending a magic link to the user's email address.
    #[instrument(level = "debug", skip_all, err)]
    pub async fn invite(&self, email: &str) -> Result<(), AppResponseError> {
        self
            .gotrue_client
            .magic_link(
                &MagicLinkParams {
                    email: email.to_owned(),
                    ..Default::default()
                },
                None,
            )
            .await?;
        Ok(())
    }

    #[instrument(level = "debug", skip_all, err)]
    pub async fn create_user(&self, email: &str, password: &str) -> Result<User, AppResponseError> {
        Ok(
            self
                .gotrue_client
                .admin_add_user(
                    &self.access_token()?,
                    &AdminUserParams {
                        email: email.to_owned(),
                        password: Some(password.to_owned()),
                        email_confirm: true,
                        ..Default::default()
                    },
                )
                .await?,
        )
    }

    #[instrument(level = "debug", skip_all, err)]
    pub async fn create_email_verified_user(
        &self,
        email: &str,
        password: &str,
    ) -> Result<User, AppResponseError> {
        Ok(
            self
                .gotrue_client
                .admin_add_user(
                    &self.access_token()?,
                    &AdminUserParams {
                        email: email.to_owned(),
                        password: Some(password.to_owned()),
                        email_confirm: true,
                        ..Default::default()
                    },
                )
                .await?,
        )
    }

    // filter is postgre sql like filter
    #[instrument(level = "debug", skip_all, err)]
    pub async fn admin_list_users(
        &self,
        filter: Option<&str>,
    ) -> Result<Vec<User>, AppResponseError> {
        let user = self
            .gotrue_client
            .admin_list_user(&self.access_token()?, filter)
            .await?;
        Ok(user.users)
    }

    /// Only expose this method for testing
    #[cfg(debug_assertions)]
    pub fn token(&self) -> Arc<RwLock<ClientToken>> {
        self.token.clone()
    }

    /// Retrieves the expiration timestamp of the current token.
    /// This function attempts to read the current token and, if successful, returns the expiration timestamp.
    /// # Returns
    /// - `Ok(i64)`: An `i64` representing the expiration timestamp of the token in seconds.
    /// - `Err(AppError)`: An `AppError` indicating either an inability to read the token or that the user is not logged in.
    #[inline]
    pub fn token_expires_at(&self) -> Result<i64, AppResponseError> {
        match &self.token.try_read() {
            None => Err(AppError::Unhandled("Failed to read token".to_string()).into()),
            Some(token) => Ok(
                token
                .as_ref()
                .ok_or(AppResponseError::from(AppError::NotLoggedIn(
                    "token is empty".to_string(),
                )))?
                .expires_at,
            ),
        }
    }

    /// Retrieves the access token string.
    /// This function attempts to read the current token and, if successful, returns the access token string.
    /// # Returns
    /// - `Ok(String)`: A `String` containing the access token.
    /// - `Err(AppResponseError)`: An `AppResponseError` indicating either an inability to read the token or that the user is not logged in.
    pub fn access_token(&self) -> Result<String, AppResponseError> {
        match &self.token.try_read_for(Duration::from_secs(2)) {
            None => Err(AppError::Unhandled("Failed to read token".to_string()).into()),
            Some(token) => {
                let access_token = token
                    .as_ref()
                    .ok_or(AppResponseError::from(AppError::NotLoggedIn(
                        "fail to get access token. Token is empty".to_string(),
                    )))?
                    .access_token
                    .clone();
    
                if access_token.is_empty() {
                    error!("Unexpected empty access token");
                }
                Ok(access_token)
            },
        }
    }

    #[instrument(level = "debug", skip_all, err)]
    pub async fn get_profile(&self) -> Result<AFUserProfile, AppResponseError> {
        let url = format!("{}/api/user/profile", self.base_url);
        let resp = self
            .http_client_with_auth(Method::GET, &url)
            .await?
            .send()
            .await?;
        log_request_id(&resp);
        AppResponse::<AFUserProfile>::from_response(resp)
            .await?
            .into_data()
    }

    #[instrument(level = "debug", skip_all, err)]
    pub async fn get_user_workspace_info(&self) -> Result<AFUserWorkspaceInfo, AppResponseError> {
        let url = format!("{}/api/user/workspace", self.base_url);
        let resp = self
            .http_client_with_auth(Method::GET, &url)
            .await?
            .send()
            .await?;
        log_request_id(&resp);
        AppResponse::<AFUserWorkspaceInfo>::from_response(resp)
            .await?
            .into_data()
    }

    #[instrument(level = "debug", skip_all, err)]
    pub async fn delete_workspace(&self, workspace_id: &str) -> Result<(), AppResponseError> {
        let url = format!("{}/api/workspace/{}", self.base_url, workspace_id);
        let resp = self
            .http_client_with_auth(Method::DELETE, &url)
            .await?
            .send()
            .await?;
        log_request_id(&resp);
        AppResponse::<()>::from_response(resp).await?.into_error()?;
        Ok(())
    }

}

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
