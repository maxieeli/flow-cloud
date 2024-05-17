use collab::core::awareness::{AwarenessUpdate, Event};
use std::sync::{Arc, Weak};

use collab::core::collab::MutexCollab;
use collab::core::collab_state::SyncState;
use collab::core::origin::CollabOrigin;
use collab::preclude::{Collab, CollabPlugin};
use collab_entity::{CollabObject, CollabType};
use futures_util::SinkExt;
use realtime_entity::collab_msg::{ClientCollabMessage, ServerCollabMessage, UpdateSync};
use realtime_protocol::{Message, SyncMessage};
use tokio_stream::StreamExt;

use crate::collab_sync::SyncControl;
use tokio_stream::wrappers::WatchStream;
use tracing::trace;

use crate::af_spawn;
use crate::collab_sync::sink_config::SinkConfig;
use crate::ws::{ConnectState, WSConnectStateReceiver};
use yrs::updates::encoder::Encode;


#[derive(Clone, Debug)]
pub struct SyncObject {
    pub object_id: String,
    pub workspace_id: String,
    pub collab_type: CollabType,
    pub device_id: String,
}

impl SyncObject {
    pub fn new(
        object_id: &str,
        workspace_id: &str,
        collab_type: CollabType,
        device_id: &str,
    ) -> Self {
        Self {
            object_id: object_id.to_string(),
            workspace_id: workspace_id.to_string(),
            collab_type,
            device_id: device_id.to_string(),
        }
    }
}

impl From<CollabObject> for SyncObject {
    fn from(collab_object: CollabObject) -> Self {
        Self {
            object_id: collab_object.object_id,
            workspace_id: collab_object.workspace_id,
            collab_type: collab_object.collab_type,
            device_id: collab_object.device_id,
        }
    }
}
