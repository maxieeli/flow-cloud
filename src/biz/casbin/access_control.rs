use crate::biz::casbin::collab_ac::CollabAccessControlImpl;
use crate::biz::casbin::enforcer::{AFEnforcer, NoEnforceGroup};
use crate::biz::casbin::workspace_ac::WorkspaceAccessControlImpl;
use std::cmp::Ordering;
use app_error::AppError;
use casbin::{CoreApi, DefaultModel, Enforcer, MgmtApi};
use database_entity::dto::{AFAccessLevel, AFRole};
use crate::biz::casbin::adapter::PgAdapter;
use crate::biz::casbin::metrics::{tick_metric, AccessControlMetrics};
use actix_http::Method;
use anyhow::anyhow;
use sqlx::PgPool;
use lazy_static::lazy_static;
use redis::{ErrorKind, FromRedisValue, RedisError, RedisResult, RedisWrite, ToRedisArgs, Value};
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub enum AccessControlChange {
    UpdatePolicy { uid: i64, oid: String },
    RemovePolicy { uid: i64, oid: String },
}

/// Manages access control.
/// Stores access control policies in the form `subject, object, role`
/// where `subject` is `uid`, `object` is `oid`, and `role` is [AFAccessLevel] or [AFRole].
/// Roles are mapped to the corresponding actions that they are allowed to perform.
/// `FullAccess` has write
/// `FullAccess` has read
/// Access control requests are made in the form `subject, object, action`
/// and will be evaluated against the policies and mappings stored,
/// according to the model defined.
#[derive(Clone)]
pub struct  AccessControl {
    enforcer: Arc<AFEnforcer<NoEnforceGroup>>,
    #[allow(dead_code)]
    access_control_metrics: Arc<AccessControlMetrics>,
    change_tx: broadcast::Sender<AccessControlChange>,
}

impl AccessControl {
    pub async fn new(
        pg_pool: PgPool,
        access_control_metrics: Arc<AccessControlMetrics>,
    ) -> Result<Self, AppError> {
        let model = casbin_model().await?;
        let adapter = PgAdapter::new(pg_pool.clone(), access_control_metrics.clone());
        let enforcer = casbin::Enforcer::new(model, adapter).await.map_err(|e| {
            AppError::Internal(anyhow!("Failed to create access control enforcer: {}", e))
        })?;
        let enforcer = Arc::new(AFEnforcer::new(enforcer, NoEnforceGroup).await?);
        tick_metric(
            enforcer.metrics_state.clone(),
            access_control_metrics.clone(),
        );
        let (change_tx, _) = broadcast::channel(1000);
        Ok(Self {
            enforcer,
            access_control_metrics,
            change_tx,
        })
    }

    pub fn subscribe_change(&self) -> broadcast::Receiver<AccessControlChange> {
        self.change_tx.subscribe()
    }
    
    pub fn new_collab_access_control(&self) -> CollabAccessControlImpl {
        CollabAccessControlImpl::new(self.clone())
    }

    pub fn new_workspace_access_control(&self) -> WorkspaceAccessControlImpl {
        WorkspaceAccessControlImpl::new(self.clone())
    }

    pub async fn update_policy(
        &self,
        uid: &i64,
        obj: ObjectType<'_>,
        act: ActionVariant<'_>,
    ) -> Result<(), AppError> {
        if enable_access_control() {
            let change = AccessControlChange::UpdatePolicy {
                uid: *uid,
                oid: obj.object_id().to_string(),
            };
            let result = self.enforcer.update_policy(uid, obj, act).await;
            let _ = self.change_tx.send(change);
            result
        } else {
            Ok(())
        }
    }

    pub async fn remove_policy(&self, uid: &i64, obj: &ObjectType<'_>) -> Result<(), AppError> {
        if enable_access_control() {
            self.enforcer.remove_policy(uid, obj).await?;
            let _ = self.change_tx.send(AccessControlChange::RemovePolicy {
                uid: *uid,
                oid: obj.object_id().to_string(),
            });
            Ok(())
        } else {
            Ok(())
        }
    }

    pub async fn enforce(
        &self,
        workspace_id: &str,
        uid: &i64,
        obj: ObjectType<'_>,
        act: ActionVariant<'_>,
    ) -> Result<bool, AppError> {
        if enable_access_control() {
            self
                .enforcer
                .enforce_policy(workspace_id, uid, obj, act)
                .await
        } else {
            Ok(true)
        }
    }

}



