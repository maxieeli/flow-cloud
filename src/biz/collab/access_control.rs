use crate::api::workspace::COLLAB_PATTERN;
use crate::biz::casbin::access_control::Action;
use crate::biz::workspace::access_control::WorkspaceAccessControl;
use crate::middleware::access_control_mw::{AccessResource, MiddlewareAccessControl};
use actix_router::{Path, ResourceDef, Url};
use actix_web::http::Method;
use app_error::AppError;
use async_trait::async_trait;
use database::collab::CollabStorageAccessControl;
use database_entity::dto::{AFAccessLevel, AFRole};
use crate::biz::collab::cache::CollabCache;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{instrument, trace};

#[async_trait]
pub trait CollabAccessControl: Sync + Send + 'static {
    async fn enforce_action(
        &self,
        workspace_id: &str,
        uid: &i64,
        oid: &str,
        action: Action,
    ) -> Result<bool, AppError>;

    async fn enforce_access_level(
        &self,
        workspace_id: &str,
        uid: &i64,
        oid: &str,
        access_level: AFAccessLevel,
    ) -> Result<bool, AppError>;

    /// Return the access level of the user in the collab
    async fn update_access_level_policy(
        &self,
        uid: &i64,
        oid: &str,
        level: AFAccessLevel,
    ) -> Result<(), AppError>;

    async fn remove_access_level(&self, uid: &i64, oid: &str) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct CollabMiddlewareAccessControl<AC: CollabAccessControl> {
    pub access_control: Arc<AC>,
    collab_cache: CollabCache,
    skip_resources: Vec<(Method, ResourceDef)>,
    require_access_levels: Vec<(ResourceDef, HashMap<Method, AFAccessLevel>)>,
}

impl<AC> CollabMiddlewareAccessControl<AC>
where
    AC: CollabAccessControl,
{
    pub fn new(access_control: Arc<AC>, collab_cache: CollabCache) -> Self {
        Self {
            skip_resources: vec![
                // Skip access control when trying to create a collab
                (Method::POST, ResourceDef::new(COLLAB_PATTERN)),
            ],
            require_access_levels: vec![(
                ResourceDef::new(COLLAB_PATTERN),
                [
                    // Only the user with FullAccess can delete the collab
                    (Method::DELETE, AFAccessLevel::FullAccess),
                ]
                .into(),
            )],
            access_control,
            collab_cache,
        }
    }

    fn should_skip(&self, method: &Method, path: &Path<Url>) -> bool {
        self.skip_resources.iter().any(|(m, r)| {
            if m != method {
                return false;
            }
            r.is_match(path.as_str())
        })
    }

    fn require_access_level(&self, method: &Method, path: &Path<Url>) -> Option<AFAccessLevel> {
        self.require_access_levels.iter().find_map(|(r, roles)| {
            if r.is_match(path.as_str()) {
                roles.get(method).cloned()
            } else {
                None
            }
        })
    }
}

#[async_trait]
impl<AC> MiddlewareAccessControl for CollabMiddlewareAccessControl<AC>
where
    AC: CollabAccessControl,
{
    fn resource(&self) -> AccessResource {
        AccessResource::Collab
    }

    #[instrument(name = "check_collab_permission", level = "trace", skip_all)]
    async fn check_resource_permission(
        &self,
        workspace_id: &str,
        uid: &i64,
        oid: &str,
        method: Method,
        path: &Path<Url>,
    ) -> Result<(), AppError> {
        if self.should_skip(&method, path) {
            trace!("Skip access control for the request");
            return Ok(());
        }
        let collab_exists = self.collab_cache.is_exist(oid).await?;
        if !collab_exists {
            return Err(AppError::RecordNotFound(format!(
                "Collab not exist in db. {}",
                oid
            )));
        }

        let access_level = self.require_access_level(&method, path);
        let result = match access_level {
            None => {
                self
                    .access_control
                    .enforce_action(workspace_id, uid, oid, Action::from(&method))
                    .await?
            },
            Some(access_level) => {
                self
                    .access_control
                    .enforce_access_level(workspace_id, uid, oid, access_level)
                    .await?
            },
        };

        if result {
            Ok(())
        } else {
            Err(AppError::NotEnoughPermissions {
                user: uid.to_string(),
                action: format!(
                    "access collab:{} with url:{}, method:{}",
                    oid,
                    path.as_str(),
                    method
                ),
            })
        }
    }
}

#[derive(Clone)]
pub struct CollabStorageAccessControlImpl<CollabAC, WorkspaceAC> {
    pub(crate) collab_access_control: Arc<CollabAC>,
    pub(crate) workspace_access_control: Arc<WorkspaceAC>,
    pub(crate) cache: CollabCache,
}

#[async_trait]
impl<CollabAC, WorkspaceAC> CollabStorageAccessControl
    for CollabStorageAccessControlImpl<CollabAC, WorkspaceAC>
where
    CollabAC: CollabAccessControl,
    WorkspaceAC: WorkspaceAccessControl,
{
    async fn update_policy(
        &self,
        uid: &i64,
        oid: &str,
        level: AFAccessLevel,
    ) -> Result<(), AppError> {
        self
            .collab_access_control
            .update_access_level_policy(uid, oid, level)
            .await
    }

    async fn enforce_read_collab(
        &self,
        workspace_id: &str,
        uid: &i64,
        oid: &str,
    ) -> Result<bool, AppError> {
        let collab_exists = self.cache.is_exist(oid).await?;
        if !collab_exists {
            return Err(AppError::RecordNotFound(format!(
                "Collab not exist in db. {}",
                oid
            )));
        }
        self
            .collab_access_control
            .enforce_action(workspace_id, uid, oid, Action::Read)
            .await
    }

    async fn enforce_write_collab(
        &self,
        workspace_id: &str,
        uid: &i64,
        oid: &str,
    ) -> Result<bool, AppError> {
        let collab_exists = self.cache.is_exist(oid).await?;
        if !collab_exists {
            return Err(AppError::RecordNotFound(format!(
                "Collab not exist in db. {}",
                oid
            )));
        }
        self
            .collab_access_control
            .enforce_action(workspace_id, uid, oid, Action::Write)
            .await
    }

    async fn enforce_delete(
        &self,
        workspace_id: &str,
        uid: &i64,
        oid: &str,
    ) -> Result<bool, AppError> {
        self
            .collab_access_control
            .enforce_access_level(workspace_id, uid, oid, AFAccessLevel::FullAccess)
            .await
    }

    async fn enforce_write_workspace(&self, uid: &i64, workspace_id: &str) -> Result<bool, AppError> {
        self
            .workspace_access_control
            .enforce_role(uid, workspace_id, AFRole::Owner)
            .await
    }
}
