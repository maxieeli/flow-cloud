use crate::biz::casbin::access_control::{
    load_group_policies, ActionVariant, ObjectType,
    ToACAction, POLICY_FIELD_INDEX_OBJECT,
    POLICY_FIELD_INDEX_SUBJECT,
};
use crate::biz::casbin::metrics::MetricsCalState;
use crate::biz::casbin::request::{GroupPolicyRequest, PolicyRequest, WorkspacePolicyRequest};
use anyhow::anyhow;
use app_error::AppError;
use async_trait::async_trait;
use casbin::{CoreApi, Enforcer, MgmtApi};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{event, instrument, trace};

pub const ENFORCER_METRICS_TICK_INTERVAL: Duration = Duration::from_secs(120);

#[async_trait]
pub trait EnforcerGroup {
    async fn get_enforce_group_id(&self, uid: &i64) -> Option<String>;
}

pub struct AFEnforcer<T> {
    enforcer: RwLock<Enforcer>,
    pub(crate) metrics_state: MetricsCalState,
    enforce_group: T,
}

impl<T> AFEnforcer<T>
where
    T: EnforcerGroup,
{
    pub async fn new(mut enforcer: Enforcer, enforce_group: T) -> Result<Self, AppError> {
        load_group_policies(&mut enforcer).await?;
        Ok(Self {
            enforcer: RwLock::new(enforcer),
            metrics_state: MetricsCalState::new(),
            enforce_group,
        })
    }

    #[instrument(level = "debug", skip_all, err)]
    pub async fn update_policy(
        &self,
        uid: &i64,
        obj: ObjectType<'_>,
        act: ActionVariant<'_>,
    ) -> Result<(), AppError> {
        validate_obj_action(&obj, &act)?;
        let policy = vec![
            uid.to_string(),
            obj.policy_object(),
            act.to_action().to_string(),
        ];
        // only one policy per user per object. So remove the old policy and add the new one.
        trace!("[access control]: add policy:{}", policy.join(","));
        let mut write_guard = self.enforcer.write().await;
        let _result = write_guard
            .add_policy(policy)
            .await
            .map_err(|e| AppError::Internal(anyhow!("fail to add policy: {e:?}")))?;
        drop(write_guard);
        Ok(())
    }

    /// Returns policies that match the filter.
    pub async fn remove_policy(
        &self,
        uid: &i64,
        object_type: &ObjectType<'_>,
    ) -> Result<(), AppError> {
        let mut enforcer = self.enforcer.write().await;
        self
            .remove_with_enforcer(uid, object_type, &mut enforcer)
            .await
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn enforce_policy(
        &self,
        workspace_id: &str,
        uid: &i64,
        obj: ObjectType<'_>,
        act: ActionVariant<'_>,
    ) -> Result<bool, AppError> {
        self
            .metrics_state
            .total_read_enforce_result
            .fetch_add(1, Ordering::Relaxed);

        // 1. First, check workspace-level permissions.
        let workspace_policy_request = WorkspacePolicyRequest::new(workspace_id, uid, &obj, &act);
        let segments = workspace_policy_request.into_segments();
        let mut result = self
            .enforcer
            .read()
            .await
            .enforce(segments)
            .map_err(|e| AppError::Internal(anyhow!("enforce: {e:?}")))?;
        // 2. Fallback to group policy if workspace-level check fails.
        if !result {
            if let Some(guid) = self.enforce_group.get_enforce_group_id(uid).await {
                let policy_request = GroupPolicyRequest::new(&guid, &obj, &act);
                result = self
                    .enforcer
                    .read()
                    .await
                    .enforce(policy_request.into_segments())
                    .map_err(|e| AppError::Internal(anyhow!("enforce: {e:?}")))?;
            }
        }
        if !result  {
            let policy_request = PolicyRequest::new(*uid, &obj, &act);
            let segments = policy_request.into_segments();
            result = self
                .enforcer
                .read()
                .await
                .enforce(segments)
                .map_err(|e| AppError::Internal(anyhow!("enforce: {e:?}")))?;
        }
        Ok(result)
    }

    #[inline]
    async fn remove_with_enforcer(
        &self,
        uid: &i64,
        object_type: &ObjectType<'_>,
        enforcer: &mut Enforcer,
    ) -> Result<(), AppError> {
        let policies_for_user_on_object = policies_for_subject_with_given_object(uid, object_type, enforcer).await;
        if policies_for_user_on_object.is_empty() {
            return Ok(());
        }
        event!(
            tracing::Level::INFO,
            "[access control]: remove policy:user={}, object={}, policies={:?}",
            uid,
            object_type.policy_object(),
            policies_for_user_on_object
        );
        enforcer
            .remove_policies(policies_for_user_on_object)
            .await
            .map_err(|e| AppError::Internal(anyhow!("error enforce: {e:?}")))?;
        Ok(())
    }
}


