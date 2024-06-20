use crate::api::util::{compress_type_from_header_value, device_id_from_headers};
use crate::api::ws::RealtimeServerAddr;
use crate::biz;
use crate::biz::workspace;
use crate::component::auth::jwt::UserUuid;
use crate::domain::compression::{decompress, CompressionType, X_COMPRESSION_TYPE};
use crate::state::AppState;
use std::time::Duration;
use actix_web::web::{Bytes, Payload};
use actix_web::web::{Data, Json, PayloadConfig};
use actix_web::{web, Scope};
use actix_web::{HttpRequest, Result};
use anyhow::{anyhow, Context};
use app_error::AppError;
use collab::core::collab_plugin::EncodedCollab;
use collab_entity::CollabType;
use database::collab::CollabStorage;
use database::user::select_uid_from_email;
use database_entity::dto::*;
use prost::Message as ProstMessage;
use bytes::BytesMut;
use realtime::entities::{ClientStreamMessage, RealtimeMessage};
use realtime_entity::realtime_proto::HttpRealtimeMessage;
use shared_entity::dto::workspace_dto::*;
use shared_entity::response::AppResponseError;
use shared_entity::response::{AppResponse, JsonAppResponse};
use sqlx::types::uuid;
use tokio::time::{sleep, Instant};
use crate::biz::collab::access_control::CollabAccessControl;
use tokio_stream::StreamExt;
use tokio_tungstenite::tungstenite::Message;
use tracing::{event, instrument};
use uuid::Uuid;
use validator::Validate;

pub const WORKSPACE_ID_PATH: &str = "workspace_id";
pub const COLLAB_OBJECT_ID_PATH: &str = "object_id";
pub const WORKSPACE_PATTERN: &str = "/api/workspace";
pub const WORKSPACE_MEMBER_PATTERN: &str = "/api/workspace/{workspace_id}/member";
pub const WORKSPACE_INVITE_PATTERN: &str = "/api/workspace/{workspace_id}/invite";
pub const COLLAB_PATTERN: &str = "/api/workspace/{workspace_id}/collab/{object_id}";

pub fn workspace_scope() -> Scope {
    web::scope("/api/workspace")
        // deprecated, use the api below instead
        .service(web::resource("/list").route(web::get().to(list_workspace_handler)))
        .service(web::resource("")
            .route(web::get().to(list_workspace_handler))
            .route(web::post().to(create_workspace_handler))
            .route(web::patch().to(patch_workspace_handler))
        )
        .service(
            web::resource("/{workspace_id}/invite")
            .route(web::post().to(post_workspace_invite_handler)) // invite members to workspace
        )
        .service(
        web::resource("/invite")
            .route(web::get().to(get_workspace_invite_handler)) // show invites for user
        )
        .service(
        web::resource("/accept-invite/{invite_id}")
            .route(web::post().to(post_accept_workspace_invite_handler)) // accept invitation to workspace
        )
        .service(web::resource("/{workspace_id}")
            .route(web::delete().to(delete_workspace_handler))
        )
        .service(web::resource("/{workspace_id}/open").route(web::put().to(open_workspace_handler)))
        .service(web::resource("/{workspace_id}/leave").route(web::post().to(leave_workspace_handler)))
        .service(
        web::resource("/{workspace_id}/member")
            .route(web::get().to(get_workspace_members_handler))
            .route(web::post().to(create_workspace_members_handler)) // deprecated, use invite flow instead
            .route(web::put().to(update_workspace_member_handler))
            .route(web::delete().to(remove_workspace_member_handler))
        )
        .service(
        web::resource("/{workspace_id}/collab/{object_id}")
            .app_data(
                PayloadConfig::new(5 * 1024 * 1024), // 5 MB
            )
            .route(web::post().to(create_collab_handler))
            .route(web::get().to(get_collab_handler))
            .route(web::put().to(update_collab_handler))
            .route(web::delete().to(delete_collab_handler)),
        )
        .service(
            web::resource("/{workspace_id}/batch/collab")
            .route(web::post().to(batch_create_collab_handler)),
        )
        // will be deprecated
        .service(
            web::resource("/{workspace_id}/collabs")
                .app_data(PayloadConfig::new(10 * 1024 * 1024))
                .route(web::post().to(create_collab_list_handler)),
        )
        .service(
            web::resource("/{workspace_id}/usage").route(web::get().to(get_workspace_usage_handler)),
        )
        .service(
            web::resource("/{workspace_id}/{object_id}/snapshot")
            .route(web::get().to(get_collab_snapshot_handler))
            .route(web::post().to(create_collab_snapshot_handler)),
        )
        .service(
            web::resource("/{workspace_id}/{object_id}/snapshot/list")
            .route(web::get().to(get_all_collab_snapshot_list_handler)),
        )
        .service(
            web::resource("/{workspace_id}/collab/{object_id}/member")
            .route(web::post().to(add_collab_member_handler))
            .route(web::get().to(get_collab_member_handler))
            .route(web::put().to(update_collab_member_handler))
            .route(web::delete().to(remove_collab_member_handler)),
        )
        .service(
            web::resource("/{workspace_id}/collab/{object_id}/member/list")
            .route(web::get().to(get_collab_member_list_handler)),
        )
        .service(
            web::resource("/{workspace_id}/collab_list").route(web::get().to(batch_get_collab_handler)),
        )
}

pub fn collab_scope() -> Scope {
    web::scope("/api/realtime").service(
        web::resource("post/stream")
            .app_data(
                PayloadConfig::new(10 * 1024 * 1024), // 10 MB
            )
            .route(web::post().to(post_realtime_message_stream_handler)),
    )
}

// Adds a workspace for user, if success, return the workspace id
#[instrument(skip_all, err)]
async fn create_workspace_handler(
    uuid: UserUuid,
    state: Data<AppState>,
    create_workspace_param: Json<CreateWorkspaceParam>,
) -> Result<Json<AppResponse<AFWorkspace>>> {
    let workspace_name = create_workspace_param
        .into_inner()
        .workspace_name
        .unwrap_or_else(|| format!("workspace_{}", chrono::Utc::now().timestamp()));
    let uid = state.user_cache.get_user_uid(&uuid).await?;
    let new_workspace = workspace::ops::create_workspace_for_user(
        &state.pg_pool,
        &state.workspace_access_control,
        &state.collab_access_control_storage,
        &uuid,
        uid,
        &workspace_name,
    )
    .await?;
    Ok(AppResponse::Ok().with_data(new_workspace).into())
}
