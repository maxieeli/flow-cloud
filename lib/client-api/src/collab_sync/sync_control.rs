use crate::af_spawn;
use crate::collab_sync::sink_config::SinkConfig;
use crate::collab_sync::{
    CollabSink, CollabSinkRunner, SinkSignal,
    SinkState, SyncError, SyncObject,
};
use bytes::Bytes;
use collab::core::awareness::Awareness;
use collab::core::collab::MutexCollab;
use collab::core::collab_state::SyncState;
use collab::core::origin::CollabOrigin;
use collab::preclude::Collab;
use futures_util::{SinkExt, StreamExt};
use realtime_entity::collab_msg::{
    AckCode, BroadcastSync, ClientCollabMessage, InitSync,
    ServerCollabMessage, ServerInit, UpdateSync,
};
use realtime_protocol::{handle_collab_message, ClientSyncProtocol, CollabSyncProtocol};
use realtime_protocol::{Message, MessageReader, SyncMessage};
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use tokio::sync::{watch, Mutex};
use tokio_stream::wrappers::WatchStream;
use tracing::{error, info, trace, warn};
use yrs::encoding::read::Cursor;
use yrs::updates::decoder::DecoderV1;
use yrs::updates::encoder::{Encoder, EncoderV1};

pub const DEFAULT_SYNC_TIMEOUT: u64 = 10;
pub const NUMBER_OF_UPDATE_TRIGGER_INIT_SYNC: u32 = 1;
const DEBOUNCE_DURATION: Duration = Duration::from_secs(10);

pub struct SyncControl<Sink, Stream> {
    object: SyncObject,
    origin: CollabOrigin,
    /// The [CollabSink] is used to send the updates to the remote. It will send the current
    /// update periodically if the timeout is reached or it will send the next update if
    /// it receive previous ack from the remote.
    sink: Arc<CollabSink<Sink, ClientCollabMessage>>,
    /// The [ObserveCollab] will be spawned in a separate task It continuously receive
    /// the updates from the remote.
    #[allow(dead_code)]
    observe_collab: ObserveCollab<Sink, Stream>,
    sync_state: Arc<watch::Sender<SyncState>>,
}

struct LastSyncTime {
    last_sync: Mutex<Instant>,
}

impl LasySyncTime {
    
}