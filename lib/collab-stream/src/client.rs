use crate::error::StreamError;
use crate::pubsub::{CollabStreamPub, CollabStreamSub};
use crate::stream::CollabStream;
use crate::stream_group::CollabStreamGroup;
use redis::aio::ConnectionManager;

pub struct CollabStreamClient {
    connection_manager: ConnectionManager,
}

impl CollabStreamClient {
    pub async fn new(redis_client: redis::Client) -> Result<Self, redis::RedisError> {
        let connection_manager = redis_client.get_connection_manager().await?;
        Ok(Self { connection_manager })
    }

    pub async fn stream(&self, workspace_id: &str, oid: &str) -> CollabStream {
        CollabStream::new(workspace_id, oid, self.connection_manager.clone())
    }

    pub async fn group_stream(
        &self,
        workspace_id: &str,
        oid: &str,
        group_name: &str,
    ) -> Result<CollabStreamGroup, StreamError> {
        let mut group = CollabStreamGroup::new(
            workspace_id,
            oid,
            group_name,
            self.connection_manager.clone(),
        );
        group.ensure_consumer_group("0").await?;
        Ok(group)
    }
}

pub struct PubSubClient {
    redis_client: redis::Client,
    connection_manager: ConnectionManager,
}

impl PubSubClient {
    pub async fn new(redis_client: redis::Client) -> Result<Self, redis::RedisError> {
        let connection_manager = redis_client.get_connection_manager().await?;
        Ok(Self {
            redis_client,
            connection_manager,
        })
    }
    
    pub async fn collab_pub(&self) -> CollabStreamPub {
        CollabStreamPub::new(self.connection_manager.clone())
    }

    #[allow(deprecated)]
    pub async fn collab_sub(&self) -> Result<CollabStreamSub, StreamError> {
        let conn = self.redis_client.get_async_connection().await?;
        Ok(CollabStreamSub::new(conn))
    }
}
