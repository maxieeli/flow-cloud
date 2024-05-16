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

fn doc_init_state<P: CollabSyncProtocol>(awareness: &Awareness, protocol: &P) -> Option<Vec<u8>> {
    let payload = {
        let mut encoder = EncoderV1::new();
        protocol.start(awareness, &mut encoder).ok()?;
        encoder.to_vec()
    };
    if payload.is_empty() {
        None
    } else {
        Some(payload)
    }
}

pub fn _init_sync<E, Sink>(
    origin: CollabOrigin,
    sync_object: &SyncObject,
    collab: &Collab,
    sink: &Arc<CollabSink<Sink, ClientCollabMessage>>,
) where
    E: Into<anyhow::Error> + Send + Sync + 'static,
    Sink: SinkExt<Vec<ClientCollabMessage>, Error = E> + Send + Sync + Unpin + 'static,
{
    
}

struct LastSyncTime {
    last_sync: Mutex<Instant>,
}

impl LastSyncTime {
    fn new() -> Self {
        let now = Instant::now();
        let one_hour = Duration::from_secs(3600);
        // Use checked_sub to safely attempt subtraction, falling back to 'now' if underflow would occur
        let one_hour_ago = now.checked_sub(one_hour).unwrap_or(now);

        LastSyncTime {
            last_sync: Mutex::new(one_hour_ago),
        }
    }
    async fn should_sync(&self, debounce_duration: Duration) -> bool {
        let now = Instant::now();
        let mut last_sync_locked = self.last_sync.lock().await;
        if now.duration_since(*last_sync_locked) > debounce_duration {
            *last_sync_locked = now;
            true
        } else {
            false
        }
    }
}