use crate::biz::workspace::access_control::WorkspaceAccessControl;
use crate::state::GoTrueAdmin;
use anyhow::Context;
use app_error::AppError;
use database::collab::upsert_collab_member_with_txn;
use database::file::bucket_s3_impl::BucketClientS3Impl;
use database::file::BucketStorage;
use database::pg_row::{AFWorkspaceMemberRow, AFWorkspaceRow};
use database::resource_usage::get_all_workspace_blob_metadata;
use database::user::select_uid_from_email;
use database::workspace::{
    change_workspace_icon, delete_from_workspace, delete_workspace_members, get_invitation_by_id,
    insert_user_workspace, insert_workspace_invitation, rename_workspace, select_all_user_workspaces,
    select_user_is_workspace_owner, select_workspace, select_workspace_invitations_for_user,
    select_workspace_member_list, select_workspace_total_collab_bytes,
    update_updated_at_of_workspace, update_workspace_invitation_set_status_accepted,
    upsert_workspace_member, upsert_workspace_member_with_txn,
};
use database_entity::dto::{
    AFAccessLevel, AFRole, AFWorkspace, AFWorkspaceInvitation,
    AFWorkspaceInvitationStatus, WorkspaceUsage,
};
use gotrue::params::InviteUserParams;
use shared_entity::dto::workspace_dto::{
    CreateWorkspaceMember, WorkspaceMemberChangeset, WorkspaceMemberInvitation,
};
use shared_entity::response::AppResponseError;
use sqlx::{types::uuid, PgPool};
use std::collections::HashMap;
use std::ops::DerefMut;
use std::sync::Arc;
use tracing::{info, instrument};
use uuid::Uuid;
use workspace_template::document::get_started::GetStartedDocumentTemplate;
use crate::biz::collab::storage::CollabAccessControlStorage;
use crate::biz::user::initialize_workspace_for_user;

pub async fn delete_workspace_for_user(
    pg_pool: &PgPool,
    workspace_id: &Uuid,
    bucket_storage: &Arc<BucketStorage<BucketClientS3Impl>>,
) -> Result<(), AppResponseError> {
    // remove files from s3
    let blob_metadatas = get_all_workspace_blob_metadata(pg_pool, workspace_id)
        .await
        .context("Get all workspace blob metadata")?;
    for blob_metadata in blob_metadatas {
        bucket_storage
            .delete_blob(workspace_id, blob_metadata.file_id.as_str())
            .await
            .context("Delete blob from s3")?;
    }
    delete_from_workspace(pg_pool, workspace_id).await?;
    Ok(())
}

pub async fn create_workspace_for_user(
    pg_pool: &PgPool,
    workspace_access_control: &impl WorkspaceAccessControl,
    collab_storage: &Arc<CollabAccessControlStorage>,
    user_uuid: &Uuid,
    user_uid: i64,
    workspace_name: &str,
) -> Result<AFWorkspace, AppResponseError> {
    let mut txn = pg_pool.begin().await?;
    let new_workspace_row = insert_user_workspace(&mut txn, user_uuid, workspace_name).await?;
    let new_workspace = AFWorkspace::try_from(new_workspace_row)?;
  
    workspace_access_control
        .insert_role(&user_uid, &new_workspace.workspace_id, AFRole::Owner)
        .await?;
    initialize_workspace_for_user(
        user_uid,
        new_workspace.workspace_id.to_string().as_str(),
        &mut txn,
        vec![GetStartedDocumentTemplate],
        collab_storage,
    )
    .await?;
    txn.commit().await?;
    Ok(new_workspace)
}

pub async fn patch_workspace(
    pg_pool: &PgPool,
    workspace_id: &Uuid,
    workspace_name: Option<&str>,
    workspace_icon: Option<&str>,
) -> Result<(), AppResponseError> {
    let mut tx = pg_pool.begin().await?;
    if let Some(workspace_name) = workspace_name {
        rename_workspace(&mut tx, workspace_id, workspace_name).await?;
    }
    if let Some(workspace_icon) = workspace_icon {
        change_workspace_icon(&mut tx, workspace_id, workspace_icon).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn get_all_user_workspaces(
    pg_pool: &PgPool,
    user_uuid: &Uuid,
) -> Result<Vec<AFWorkspaceRow>, AppResponseError> {
    let workspaces = select_all_user_workspaces(pg_pool, user_uuid).await?;
    Ok(workspaces)
}

pub async fn open_workspace(
    pg_pool: &PgPool,
    user_uuid: &Uuid,
    workspace_id: &Uuid,
) -> Result<AFWorkspace, AppResponseError> {
    let mut txn = pg_pool
        .begin()
        .await
        .context("Begin transaction to open workspace")?;
    let row = select_workspace(txn.deref_mut(), workspace_id).await?;
    update_updated_at_of_workspace(txn.deref_mut(), user_uuid, workspace_id).await?;
    txn
        .commit()
        .await
        .context("Commit transaction to open workspace")?;
    let workspace = AFWorkspace::try_from(row)?;
    Ok(workspace)
}

pub async fn accept_workspace_invite(
    pg_pool: &PgPool,
    workspace_access_control: &impl WorkspaceAccessControl,
    user_uuid: &Uuid,
    invite_id: &Uuid,
) -> Result<(), AppError> {
    let mut txn = pg_pool.begin().await?;
    update_workspace_invitation_set_status_accepted(&mut txn, user_uuid, invite_id).await?;
    let inv = get_invitation_by_id(&mut txn, invite_id).await?;
    let invited_uid = inv
        .invitee_uid
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Invitee uid is missing for {:?}", inv)))?;
    workspace_access_control
        .insert_role(&invited_uid, &inv.workspace_id, inv.role)
        .await?;
    txn.commit().await?;
    Ok(())
}


