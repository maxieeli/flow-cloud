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

// Adds a workspace for user, if success, return the workspace id
#[instrument(skip_all, err)]
async fn patch_workspace_handler(
    _uuid: UserUuid,
    state: Data<AppState>,
    params: Json<PatchWorkspaceParam>,
) -> Result<Json<AppResponse<()>>> {
    let params = params.into_inner();
    workspace::ops::patch_workspace(
        &state.pg_pool,
        &params.workspace_id,
        params.workspace_name.as_deref(),
        params.workspace_icon.as_deref(),
    )
    .await?;
    Ok(AppResponse::Ok().into())
}

async fn delete_workspace_handler(
    _user_id: UserUuid,
    workspace_id: web::Path<Uuid>,
    state: Data<AppState>,
) -> Result<Json<AppResponse<()>>> {
    let bucket_storage = &state.bucket_storage;
    // TODO: add permission for workspace deletion
    workspace::ops::delete_workspace_for_user(&state.pg_pool, &workspace_id, bucket_storage).await?;
    Ok(AppResponse::Ok().into())
}

// TODO: also get shared workspaces
#[instrument(skip_all, err)]
async fn list_workspace_handler(
    uuid: UserUuid,
    state: Data<AppState>,
) -> Result<JsonAppResponse<AFWorkspaces>> {
    let rows = workspace::ops::get_all_user_workspaces(&state.pg_pool, &uuid).await?;
    let workspaces = rows
        .into_iter()
        .flat_map(|row| {
            let result = AFWorkspace::try_from(row);
            if let Err(err) = &result {
                event!(
                    tracing::Level::ERROR,
                    "Failed to convert workspace row to AFWorkspace: {:?}",
                    err
                );
            }
            result
        })
        .collect::<Vec<_>>();
    Ok(AppResponse::Ok().with_data(AFWorkspaces(workspaces)).into())
}

// Deprecated
#[instrument(skip(payload, state), err)]
async fn create_workspace_members_handler(
    user_uuid: UserUuid,
    workspace_id: web::Path<Uuid>,
    payload: Json<CreateWorkspaceMembers>,
    state: Data<AppState>,
) -> Result<JsonAppResponse<()>> {
    let create_members = payload.into_inner();
    workspace::ops::add_workspace_members(
        &state.pg_pool,
        &user_uuid,
        &workspace_id,
        create_members.0,
        &state.workspace_access_control,
    )
    .await?;
    Ok(AppResponse::Ok().into())
}

#[instrument(skip(payload, state), err)]
async fn post_workspace_invite_handler(
    user_uuid: UserUuid,
    workspace_id: web::Path<Uuid>,
    payload: Json<Vec<WorkspaceMemberInvitation>>,
    state: Data<AppState>,
) -> Result<JsonAppResponse<()>> {
    let invited_members = payload.into_inner();
    workspace::ops::invite_workspace_members(
        &state.pg_pool,
        &state.gotrue_admin,
        &state.gotrue_client,
        &user_uuid,
        &workspace_id,
        invited_members,
    )
    .await?;
    Ok(AppResponse::Ok().into())
}

async fn get_workspace_invite_handler(
    user_uuid: UserUuid,
    state: Data<AppState>,
    query: web::Query<WorkspaceInviteQuery>,
) -> Result<JsonAppResponse<Vec<AFWorkspaceInvitation>>> {
    let query = query.into_inner();
    let res =
        workspace::ops::list_workspace_invitations_for_user(&state.pg_pool, &user_uuid, query.status)
            .await?;
    Ok(AppResponse::Ok().with_data(res).into())
}

async fn post_accept_workspace_invite_handler(
    user_uuid: UserUuid,
    invite_id: web::Path<Uuid>,
    state: Data<AppState>,
) -> Result<JsonAppResponse<()>> {
    let invite_id = invite_id.into_inner();
    workspace::ops::accept_workspace_invite(
        &state.pg_pool,
        &state.workspace_access_control,
        &user_uuid,
        &invite_id,
    )
    .await?;
    Ok(AppResponse::Ok().into())
}

#[instrument(skip_all, err)]
async fn get_workspace_members_handler(
    user_uuid: UserUuid,
    state: Data<AppState>,
    workspace_id: web::Path<Uuid>,
) -> Result<JsonAppResponse<Vec<AFWorkspaceMember>>> {
    let members = workspace::ops::get_workspace_members(&state.pg_pool, &user_uuid, &workspace_id)
        .await?
        .into_iter()
        .map(|member| AFWorkspaceMember {
            name: member.name,
            email: member.email,
            role: member.role,
            avatar_url: None,
        })
        .collect();
    Ok(AppResponse::Ok().with_data(members).into())
}

#[instrument(skip_all, err)]
async fn remove_workspace_member_handler(
    _user_uuid: UserUuid,
    payload: Json<WorkspaceMembers>,
    state: Data<AppState>,
    workspace_id: web::Path<Uuid>,
) -> Result<JsonAppResponse<()>> {
    let member_emails = payload
        .into_inner()
        .0
        .into_iter()
        .map(|member| member.0)
        .collect::<Vec<String>>();
    workspace::ops::remove_workspace_members(
        &state.pg_pool,
        &workspace_id,
        &member_emails,
        &state.workspace_access_control,
    )
    .await?;
    Ok(AppResponse::Ok().into())
}

#[instrument(level = "debug", skip_all, err)]
async fn open_workspace_handler(
    user_uuid: UserUuid,
    state: Data<AppState>,
    workspace_id: web::Path<Uuid>,
) -> Result<JsonAppResponse<AFWorkspace>> {
    let workspace_id = workspace_id.into_inner();
    let workspace = workspace::ops::open_workspace(&state.pg_pool, &user_uuid, &workspace_id).await?;
    Ok(AppResponse::Ok().with_data(workspace).into())
}

#[instrument(level = "debug", skip_all, err)]
async fn leave_workspace_handler(
    user_uuid: UserUuid,
    state: Data<AppState>,
    workspace_id: web::Path<Uuid>,
) -> Result<JsonAppResponse<()>> {
    let workspace_id = workspace_id.into_inner();
    workspace::ops::leave_workspace(
        &state.pg_pool,
        &workspace_id,
        &user_uuid,
        &state.workspace_access_control,
    )
    .await?;
    Ok(AppResponse::Ok().into())
}

#[instrument(level = "debug", skip_all, err)]
async fn update_workspace_member_handler(
    payload: Json<WorkspaceMemberChangeset>,
    state: Data<AppState>,
    workspace_id: web::Path<Uuid>,
) -> Result<JsonAppResponse<()>> {
    // TODO: only owner is allowed to update member role
    let workspace_id = workspace_id.into_inner();
    let changeset = payload.into_inner();
  
    if changeset.role.is_some() {
        let uid = select_uid_from_email(&state.pg_pool, &changeset.email)
            .await
            .map_err(AppResponseError::from)?;
        workspace::ops::update_workspace_member(
            &uid,
            &state.pg_pool,
            &workspace_id,
            &changeset,
            &state.workspace_access_control,
        )
        .await?;
    }
    Ok(AppResponse::Ok().into())
}

#[instrument(skip(state, payload), err)]
async fn create_collab_handler(
    user_uuid: UserUuid,
    payload: Bytes,
    state: Data<AppState>,
    req: HttpRequest,
) -> Result<Json<AppResponse<()>>> {
    let uid = state.user_cache.get_user_uid(&user_uuid).await?;
    let params = match req.headers().get(X_COMPRESSION_TYPE) {
        None => serde_json::from_slice::<CreateCollabParams>(&payload).map_err(|err| {
            AppError::InvalidRequest(format!(
                "Failed to parse CreateCollabParams from JSON: {}",
                err
            ))
        })?,
        Some(_) => match compress_type_from_header_value(req.headers())? {
            CompressionType::Brotli { buffer_size } => {
                let decompress_data = decompress(payload.to_vec(), buffer_size).await?;
                CreateCollabParams::from_bytes(&decompress_data).map_err(|err| {
                    AppError::InvalidRequest(format!(
                        "Failed to parse CreateCollabParams with brotli decompression data: {}",
                        err
                    ))
                })?
            },
        },
    };
    let (params, workspace_id) = params.split();
    state
        .collab_access_control_storage
        .insert_or_update_collab(&workspace_id, &uid, params)
        .await?;
    Ok(Json(AppResponse::Ok()))
}


