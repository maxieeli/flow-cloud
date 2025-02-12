use app_error::AppError;
use async_trait::async_trait;
use collab::core::collab_plugin::EncodedCollab;
use database_entity::dto::{
    AFAccessLevel, AFSnapshotMeta, AFSnapshotMetas, CollabParams,
    InsertSnapshotParams, QueryCollab, QueryCollabParams,
    QueryCollabResult, SnapshotData,
};
use sqlx::Transaction;
use std::collections::HashMap;
use std::sync::Arc;
pub const COLLAB_SNAPSHOT_LIMIT: i64 = 30;
pub const SNAPSHOT_PER_HOUR: i64 = 6;
pub type DatabaseResult<T, E = AppError> = core::result::Result<T, E>;

/// [CollabStorageAccessControl] is a trait that provides access control when accessing the storage
/// of the Collab object.
#[async_trait]
pub trait CollabStorageAccessControl: Send + Sync + 'static {
    /// Updates the cache of the access level of the user for given collab object.
    async fn update_policy(&self, uid: &i64, oid: &str, level: AFAccessLevel) -> Result<(), AppError>;

    async fn enforce_read_collab(
        &self,
        workspace_id: &str,
        uid: &i64,
        oid: &str,
    ) -> Result<bool, AppError>;

    async fn enforce_write_collab(
        &self,
        worksapce_id: &str,
        uid: &i64,
        oid: &str,
    ) -> Result<bool, AppError>;

    async fn enforce_delete(
        &self,
        workspace_id: &str,
        uid: &i64,
        oid: &str,
    ) -> Result<bool, AppError>;

    /// Returns the role of the user in the workspace.
    async fn enforce_write_workspace(&self, uid: &i64, workspace_id: &str) -> Result<bool, AppError>;
}

/// Represents a storage mechanism for collaborations.
///
/// This trait provides asynchronous methods for CRUD operations related to collaborations.
/// Implementors of this trait should provide the actual storage logic, be it in-memory, file-based, database-backed, etc.
#[async_trait]
pub trait CollabStorage: Send + Sync + 'static {
    fn encode_collab_mem_hit_rate(&self) -> f64;

    async fn insert_or_update_collab(
        &self,
        workspace_id: &str,
        uid: &i64,
        params: CollabParams,
    ) -> DatabaseResult<()>;

    /// Insert/update a new collaboration in the storage.
    ///
    /// # Arguments
    ///
    /// * `params` - The parameters required to create a new collaboration.
    ///
    /// # Returns
    ///
    /// * `Result<()>` - Returns `Ok(())` if the collaboration was created successfully, `Err` otherwise.
    async fn insert_or_update_collab_with_transaction(
        &self,
        workspace_id: &str,
        uid: &i64,
        params: CollabParams,
        transaction: &mut Transaction<'_, sqlx::Postgres>,
    ) -> DatabaseResult<()>;

    /// Retrieves a collaboration from the storage.
    ///
    /// # Arguments
    ///
    /// * `params` - The parameters required to query a collab object.
    ///
    /// # Returns
    ///
    /// * `Result<RawData>` - Returns the data of the collaboration if found, `Err` otherwise.
    async fn get_collab_encoded(
        &self,
        uid: &i64,
        params: QueryCollabParams,
        is_collab_init: bool,
    ) -> DatabaseResult<EncodedCollab>;

    async fn batch_get_collab(
        &self,
        uid: &i64,
        queries: Vec<QueryCollab>,
    ) -> HashMap<String, QueryCollabResult>;

    /// Deletes a collaboration from the storage.
    ///
    /// # Arguments
    ///
    /// * `object_id` - A string slice that holds the ID of the collaboration to delete.
    ///
    /// # Returns
    ///
    /// * `Result<()>` - Returns `Ok(())` if the collaboration was deleted successfully, `Err` otherwise.
    async fn delete_collab(
        &self,
        workspace_id: &str,
        uid: &i64,
        object_id: &str,
    ) -> DatabaseResult<()>;
    async fn should_create_snapshot(&self, oid: &str) -> bool;

    async fn create_snapshot(&self, params: InsertSnapshotParams) -> DatabaseResult<AFSnapshotMeta>;
    async fn queue_snapshot(&self, params: InsertSnapshotParams) -> DatabaseResult<()>;

    async fn get_collab_snapshot(
        &self,
        workspace_id: &str,
        object_id: &str,
        snapshot_id: &i64,
    ) -> DatabaseResult<SnapshotData>;

    /// Returns list of snapshots for given object_id in descending order of creation time.
    async fn get_collab_snapshot_list(&self, oid: &str) -> DatabaseResult<AFSnapshotMetas>;
}

#[async_trait]
impl<T> CollabStorage for Arc<T>
where
    T: CollabStorage,
{
    fn encode_collab_mem_hit_rate(&self) -> f64 {
        self.as_ref().encode_collab_mem_hit_rate()
    }

    async fn insert_or_update_collab(
        &self,
        workspace_id: &str,
        uid: &i64,
        params: CollabParams,
    ) -> DatabaseResult<()> {
        self
        .as_ref()
        .insert_or_update_collab(workspace_id, uid, params)
        .await
    }

    async fn insert_or_update_collab_with_transaction(
        &self,
        workspace_id: &str,
        uid: &i64,
        params: CollabParams,
        transaction: &mut Transaction<'_, sqlx::Postgres>,
    ) -> DatabaseResult<()> {
        self
        .as_ref()
        .insert_or_update_collab_with_transaction(workspace_id, uid, params, transaction)
        .await
    }

    async fn get_collab_encoded(
        &self,
        uid: &i64,
        params: QueryCollabParams,
        is_collab_init: bool,
    ) -> DatabaseResult<EncodedCollab> {
        self
        .as_ref()
        .get_collab_encoded(uid, params, is_collab_init)
        .await
    }

    async fn batch_get_collab(
        &self,
        uid: &i64,
        queries: Vec<QueryCollab>,
    ) -> HashMap<String, QueryCollabResult> {
        self.as_ref().batch_get_collab(uid, queries).await
    }

    async fn delete_collab(
        &self,
        workspace_id: &str,
        uid: &i64,
        object_id: &str,
    ) -> DatabaseResult<()> {
        self
        .as_ref()
        .delete_collab(workspace_id, uid, object_id)
        .await
    }

    async fn should_create_snapshot(&self, oid: &str) -> bool {
        self.as_ref().should_create_snapshot(oid).await
    }

    async fn create_snapshot(&self, params: InsertSnapshotParams) -> DatabaseResult<AFSnapshotMeta> {
        self.as_ref().create_snapshot(params).await
    }

    async fn queue_snapshot(&self, params: InsertSnapshotParams) -> DatabaseResult<()> {
        self.as_ref().queue_snapshot(params).await
    }

    async fn get_collab_snapshot(
        &self,
        workspace_id: &str,
        object_id: &str,
        snapshot_id: &i64,
    ) -> DatabaseResult<SnapshotData> {
        self
        .as_ref()
        .get_collab_snapshot(workspace_id, object_id, snapshot_id)
        .await
    }

    async fn get_collab_snapshot_list(&self, oid: &str) -> DatabaseResult<AFSnapshotMetas> {
        self.as_ref().get_collab_snapshot_list(oid).await
    }
}
