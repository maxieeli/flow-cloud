use anyhow::{Context, Result};
use crate::biz::workspace::access_control::WorkspaceAccessControl;
use crate::state::AppState;
use app_error::AppError;
use database::collab::CollabStorage;
use database::pg_row::AFUserNotification;
use database::user::{create_user, is_user_exist};
use database::workspace::{select_user_profile, select_user_workspace, select_workspace};
use database_entity::dto::{AFRole, AFUserProfile, AFUserWorkspaceInfo, AFWorkspace, CollabParams};
use realtime::entities::RealtimeUser;
use serde_json::json;
use shared_entity::dto::auth_dto::UpdateUserParams;
use shared_entity::response::AppResponseError;
use sqlx::{types::uuid, PgPool, Transaction};
use std::fmt::{Display, Formatter};
use std::ops::DerefMut;
use std::sync::Arc;
use tracing::{debug, event, instrument};
use uuid::Uuid;
use workspace_template::document::get_started::GetStartedDocumentTemplate;
use workspace_template::{WorkspaceTemplate, WorkspaceTemplateBuilder};
use super::collab::storage::CollabAccessControlStorage;

#[instrument(skip_all, err)]
pub async fn verify_token(access_token: &str, state: &AppState) -> Result<bool, AppError> {
    let user = state.gotrue_client.user_info(access_token).await?;
    let user_uuid = uuid::Uuid::parse_str(&user.id)?;
    let name = name_from_user_metadata(&user.user_metadata);
    let mut txn = state
        .pg_pool
        .begin()
        .await
        .context("acquire transaction to verify token")?;

    let lock_key = user_uuid.as_u128() as i64;
    sqlx::query!("SELECT pg_advisory_xact_lock($1)", lock_key)
        .execute(txn.deref_mut())
        .await?;
    
    let is_new = !is_user_exist(txn.deref_mut(), &user_uuid).await?;
    if is_new {
        let new_uid = state.id_gen.write().await.next_id();
        event!(tracing::Level::INFO, "create new user:{}", new_uid);
        let workspace_id = create_user(txn.deref_mut(), new_uid, &user_uuid, &user.email, &name).await?;
        state
            .workspace_access_control
            .insert_role(
                &new_uid,
                &Uuid::parse_str(&workspace_id).unwrap(),
                AFRole::Owner,
            )
            .await?;
        // Create a workspace with the GetStarted template
        initialize_workspace_for_user(
            new_uid,
            &workspace_id,
            &mut txn,
            vec![GetStartedDocumentTemplate],
            &state.collab_access_control_storage,
        )
        .await?;
    }
    txn
        .commit()
        .await
        .context("fail to commit transaction to verify token")?;
    Ok(is_new)
}

#[instrument(level = "debug", skip_all, err)]
pub async fn initialize_workspace_for_user<T>(
    uid: i64,
    workspace_id: &str,
    txn: &mut Transaction<'_, sqlx::Postgres>,
    templates: Vec<T>,
    collab_storage: &Arc<CollabAccessControlStorage>,
) -> Result<(), AppError>
where
    T: WorkspaceTemplate + Send + Sync + 'static,
{
    let templates = WorkspaceTemplateBuilder::new(uid, workspace_id)
        .with_templates(templates)
        .build()
        .await?;
    debug!("create {} templates for user:{}", templates.len(), uid);

    for template in templates {
        let object_id = template.object_id;
        let encoded_collab_v1 = template
            .object_data
            .encode_to_bytes()
            .map_err(|err| AppError::Internal(anyhow::Error::from(err)))?;
        collab_storage
            .insert_or_update_collab_with_transaction(
                workspace_id,
                &uid,
                CollabParams {
                    object_id,
                    encoded_collab_v1,
                    collab_type: template.object_type,
                    override_if_exist: false,
                },
                txn,
            )
            .await?;
    }
    Ok(())
}



