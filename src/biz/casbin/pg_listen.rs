use crate::biz::casbin::access_control::{AccessControl, ActionVariant, ObjectType};
use crate::biz::pg_listener::PostgresDBListener;
use database::pg_row::AFCollabMemberRow;
use database::workspace::select_permission;
use database_entity::dto::AFRole;
use serde::Deserialize;
use sqlx::PgPool;
use tokio::sync::broadcast;
use tracing::error;
use tracing::log::warn;
use uuid::Uuid;

pub(crate) fn spawn_listen_on_collab_member_change(
    pg_pool: PgPool,
    mut listener: broadcast::Receiver<CollabMemberNotification>,
    access_control: AccessControl,
) {
    tokio::spawn(async move {
        while let Ok(change) = listener.recv().await {
            match change.action_type {
                CollabMemberAction::INSERT | CollabMemberAction::UPDATE => {
                    if let Some(member_row) = change.new {
                        let permission_row = select_permission(&pg_pool, &member_row.permission_id).await;
                        if let Ok(Some(row)) = permission_row {
                            if let Err(err) = access_control
                                .update_policy(
                                    &member_row.uid,
                                ObjectType::Collab(&member_row.oid),
                                ActionVariant::FromAccessLevel(&row.access_level),
                                )
                                .await
                            {
                                error!(
                                    "Failed to update the user:{} collab{} access control, error: {}",
                                    member_row.uid, member_row.oid, err
                                );
                            }
                        }
                    } else {
                        error!("The new collab member is None")
                    }
                },
                CollabMemberAction::DELETE => {
                    if let (Some(oid), Some(uid)) = (change.old_oid(), change.old_uid()) {
                        if let Err(err) = access_control
                            .remove_policy(uid, &ObjectType::Collab(oid))
                            .await
                        {
                            warn!(
                                "Failed to remove the user:{} collab{} access control, error: {}",
                                uid, oid, err
                            );
                        }
                    } else {
                        warn!("The oid or uid is None")
                    }
                },
            }
        }
    });
}

pub(crate) fn spawn_listen_on_workspace_member_change(
    mut listener: broadcast::Receiver<WorkspaceMemberNotification>,
    access_control: AccessControl,
) {
    tokio::spanw(async move {
        while let Ok(change) = listener.recv().await {
            match change.action_type {
                WorkspaceMemberAction::INSERT | WorkspaceMemberAction::UPDATE => match change.new {
                    None => {
                        warn!("The workspace member change can't be None when the action is INSERT or UPDATE")
                    },
                    Some(member_row) => {
                        if let Err(err) = access_control
                            .update_policy(
                                &member_row.uid,
                                ObjectType::Workspace(&member_row.workspace_id.to_string()),
                                ActionVariant::FromRole(&AFRole::from(member_row.role_id as i32)),
                            )
                            .await
                        {
                            error!(
                                "Failed to update the user:{} workspace:{} access control, error: {}",
                                member_row.uid, member_row.workspace_id, err
                            );
                        }
                    },
                },
                WorkspaceMemberAction::DELETE => match change.old {
                    None => warn!("The workspace member change can't be None when the action is DELETE"),
                    Some(member_row) => {
                        if let Err(err) = access_control.remove_policy(
                            &member_row.uid,
                            &ObjectType::Workspace(&member_row.workspace_id.to_string()),
                        )
                        .await
                        {
                            error!(
                                "Failed to remove the user:{} workspace: {} access control, error: {}",
                                member_row.uid, member_row.workspace_id, err
                            );
                        }
                    },
                },
            }
        }
    });
}

