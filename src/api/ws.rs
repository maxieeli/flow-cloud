use crate::state::AppState;
use actix::Addr;
use actix_web::web::{Data, Path, Payload};
use actix_web::{get, web, HttpRequest, HttpResponse, Result, Scope};
use actix_web_actors::ws;
use std::collections::HashMap;
use std::sync::Arc;
use crate::biz::collab::storage::CollabAccessControlStorage;
use crate::biz::user::RealtimeUserImpl;
use crate::component::auth::jwt::{authorization_from_token, UserUuid};
use crate::biz::casbin::RealtimeCollabAccessControlImpl;
use actix_http::header::AUTHORIZATION;
use app_error::AppError;
use realtime::client::rt_client::RealtimeClient;
use realtime::server::RealtimeServer;
use semver::Version;
use shared_entity::response::AppResponseError;
use std::time::Duration;
use tracing::{debug, error, instrument, trace};

pub fn ws_scope() -> Scope {
    web::scope("/ws")
        .service(establish_ws_connection)
        .service(web::resource("/v1").route(web::get().to(establish_ws_connection_v1)))
}
const MAX_FRAME_SIZE: usize = 65_536; // 64 KiB

pub type RealtimeServerAddr = Addr<
    RealtimeServer<
        CollabAccessControlStorage,
        Arc<RealtimeUserImpl>,
        RealtimeCollabAccessControlImpl,
    >,
>;

/// This function will not be used after the 0.5.0 of the client.
#[instrument(skip_all, err)]
#[get("/{token}/{device_id}")]
pub async fn establish_ws_connection(
    request: HttpRequest,
    payload: Payload,
    path: Path<(String, String)>,
    state: Data<AppState>,
    server: Data<RealtimeServerAddr>,
) -> Result<HttpResponse> {
    let (access_token, device_id) = path.into_inner();
    let client_version = Version::new(0, 5, 0);
    start_connect(
        &request,
        payload,
        &state,
        server,
        access_token,
        device_id,
        client_version,
    )
    .await
}

#[instrument(skip_all, err)]
pub async fn establish_ws_connection_v1(
    request: HttpRequest,
    payload: Payload,
    state: Data<AppState>,
    server: Data<RealtimeServerAddr>,
    web::Query(query_params): web::Query<HashMap<String, String>>,
) -> Result<HttpResponse> {
    // Try to parse the connect info from the request body
    // If it fails, try to parse it from the query params
    let ConnectInfo {
        access_token,
        client_version,
        device_id,
    } = match ConnectInfo::parse_from(&request) {
        Ok(info) => info,
        Err(_) => {
            trace!("Failed to parse connect info from request body. Trying to parse from query params.");
            ConnectInfo::parse_from(&query_params)?
        },
    };
    start_connect(
        &request,
        payload,
        &state,
        server,
        access_token,
        device_id,
        client_version,
    )
    .await
}

#[inline]
async fn start_connect(
    request: &HttpRequest,
    payload: Payload,
    state: &Data<AppState>,
    server: Data<RealtimeServerAddr>,
    access_token: String,
    device_id: String,
    client_version: Version,
) -> Result<HttpResponse> {
    let auth = authorization_from_token(access_token.as_str(), state)?;
    let user_uuid = UserUuid::from_auth(auth)?;
    let result = state.user_cache.get_user_uid(&user_uuid).await;

    match result {
        Ok(uid) => {
            let user_change_recv = state.pg_listeners.subscribe_user_change(uid);
            debug!(
                "🚀new websocket connect: uid={}, device_id={}, client_version:{}",
                uid, device_id, client_version
            );
            let realtime_user = Arc::new(RealtimeUserImpl::new(uid, device_id));
            let client = RealtimeClient::new(
                realtime_user,
                user_change_recv,
                server.get_ref().clone(),
                Duration::from_secs(state.config.websocket.heartbeat_interval as u64),
                Duration::from_secs(state.config.websocket.client_timeout as u64),
                client_version,
            );
            match ws::WsResponseBuilder::new(client, request, payload)
                .frame_size(MAX_FRAME_SIZE * 2)
                .start()
            {
                Ok(response) => Ok(response),
                Err(e) => {
                    error!("🔴ws connection error: {:?}", e);
                    Err(e)
                },
            }
        },
        Err(err) => {
            if err.is_record_not_found() {
                return Ok(HttpResponse::NotFound().json("user not found"));
            }
            Err(AppResponseError::from(err).into())
        },
    }
}

struct ConnectInfo {
    access_token: String,
    client_version: Version,
    device_id: String,
}
  
const CLIENT_VERSION: &str = "client-version";
const DEVICE_ID: &str = "device-id";

// Trait for parameter extraction
trait ExtractParameter {
    fn extract_param(&self, key: &str) -> Result<String, AppError>;
}

// Implement the trait for HashMap<String, String>
impl ExtractParameter for HashMap<String, String> {
    fn extract_param(&self, key: &str) -> Result<String, AppError> {
        self
            .get(key)
            .ok_or_else(|| {
                AppError::InvalidRequest(format!("Parameter with given key:{} not found", key))
            })
            .map(|s| s.to_string())
    }
}

// Implement the trait for HttpRequest
impl ExtractParameter for HttpRequest {
    fn extract_param(&self, key: &str) -> Result<String, AppError> {
        self
            .headers()
            .get(key)
            .ok_or_else(|| AppError::InvalidRequest(format!("Header with given key:{} not found", key)))
            .and_then(|value| {
                value
                    .to_str()
                    .map_err(|_| {
                        AppError::InvalidRequest(format!("Invalid header value for given key:{}", key))
                    })
                    .map(|s| s.to_string())
            })
    }
}

impl ConnectInfo {
    fn parse_from<T: ExtractParameter>(source: &T) -> Result<Self, AppError> {
        let access_token = source.extract_param(AUTHORIZATION.as_str())?;
        let client_version_str = source.extract_param(CLIENT_VERSION)?;
        let client_version = Version::parse(&client_version_str)
                .map_err(|_| AppError::InvalidRequest(format!("Invalid version:{}", client_version_str)))?;
        let device_id = source.extract_param(DEVICE_ID)?;
    
        Ok(Self {
                access_token,
                client_version,
                device_id,
        })
    }
}
