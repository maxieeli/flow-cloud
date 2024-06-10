use crate::biz::casbin::{CollabAccessControlImpl, WorkspaceAccessControlImpl};
use crate::biz::collab::access_control::CollabStorageAccessControlImpl;
use anyhow::Context;
use app_error::AppError;
use async_trait::async_trait;
use collab::core::collab_plugin::EncodedCollab;
use collab::core::origin::CollabOrigin;
use collab::preclude::Collab;
use database::collab::{
    is_collab_exists, CollabStorage,
    CollabStorageAccessControl, DatabaseResult,
};
use database_entity::dto::{
    AFAccessLevel, AFSnapshotMeta, AFSnapshotMetas, CollabParams, InsertSnapshotParams,
    QueryCollab, QueryCollabParams, QueryCollabResult, SnapshotData,
};
use itertools::{Either, Itertools};
use sqlx::Transaction;
use std::collections::HashMap;
use std::ops::DerefMut;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::timeout;
use crate::biz::collab::cache::CollabCache;
use crate::biz::snapshot::SnapshotControl;
use realtime::server::{RTCommand, RTCommandSender};
use tracing::{error, instrument};
use validator::Validate;

pub type CollabAccessControlStorage = CollabStorageImpl<
    CollabStorageAccessControlImpl<CollabAccessControlImpl, WorkspaceAccessControlImpl>,
>;

#[derive(Clone)]
pub struct CollabStorageImpl<AC> {
    cache: CollabCache,
    /// access control for collab object. Including read/write
    access_control: AC,
    snapshot_control: SnapshotControl,
    rt_cmd: RTCommandSender,
}

impl<AC> CollabStorageImpl<AC>
where
    AC: CollabStorageAccessControl,
{
    pub fn new(
        cache: CollabCache,
        access_control: AC,
        snapshot_control: SnapshotControl,
        rt_cmd_sender: RTCommandSender,
    ) -> Self {
        Self {
            cache,
            access_control,
            snapshot_control,
            rt_cmd: rt_cmd_sender,
        }
    }

    async fn check_collab_permission(
        &self,
        workspace_id: &str,
        uid: &i64,
        params: &CollabParams,
        is_collab_exist: bool,
    ) -> Result<(), AppError> {
        if is_collab_exist {
            let can_write = self
                .access_control
                .enforce_write_collab(workspace_id, uid, &params.object_id)
                .await?;
            if !can_write {
                return Err(AppError::NotEnoughPermissions {
                    user: uid.to_string(),
                    action: format!("update collab:{}", params.object_id),
                });
            }
        } else {
            let can_write_workspace = self
                .access_control
                .enforce_write_workspace(uid, workspace_id)
                .await?;

            if !can_write_workspace {
                return Err(AppError::NotEnoughPermissions {
                    user: uid.to_string(),
                    action: format!("write workspace:{}", workspace_id),
                });
            }
        }
        Ok(())
    }

    async fn get_encode_collab_from_editing(&self, object_id: &str) -> Option<EncodedCollab> {
        let object_id = object_id.to_string();
        let (ret, rx) = oneshot::channel();
        let timeout_duration = Duration::from_secs(5);

        // Attempt to send the command to the realtime server
        if let Err(err) = self
            .rt_cmd
            .send(RTCommand::GetEncodeCollab { object_id, ret })
            .await
        {
            error!(
                "Failed to send get encode collab command to realtime server: {}",
                err
            );
            return None;
        }

        // Await the response from the realtime server with a timeout
        match timeout(timeout_duration, rx).await {
            Ok(Ok(Some(encode_collab))) => Some(encode_collab),
            Ok(Ok(None)) => None,
            Ok(Err(err)) => {
            error!("Failed to get encode collab from realtime server: {}", err);
                None
            },
            Err(_) => {
                error!("Timeout waiting for encode collab from realtime server");
                None
            },
        }
    }
}

#[async_trait]
impl<AC> CollabStorage for CollabStorageImpl<AC>
where
    AC: CollabStorageAccessControl,
{
    fn encode_collab_mem_hit_rate(&self) -> f64 {
        self.cache.get_hit_rate()
    }

    async fn insert_or_update_collab(
        &self,
        workspace_id: &str,
        uid: &i64,
        params: CollabParams,
    ) -> DatabaseResult<()> {
        params.validate()?;
        let mut transaction = self
            .cache
            .pg_pool()
            .begin()
            .await
            .context("acquire transaction to upsert collab")
            .map_err(AppError::from)?;
        self
            .insert_or_update_collab_with_transaction(workspace_id, uid, params, &mut transaction)
            .await?;
        transaction
            .commit()
            .await
            .context("fail to commit the transaction to upsert collab")
            .map_err(AppError::from)?;
        Ok(())
    }
}


