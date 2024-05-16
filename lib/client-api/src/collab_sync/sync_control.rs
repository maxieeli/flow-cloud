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

impl<Sink, Stream> Drop for SyncControl<Sink, Stream> {
    fn drop(&mut self) {
        trace!("Drop SyncQueue {}", self.object.object_id);
    }
}

impl<E, Sink, Stream> SyncControl<Sink, Stream>
where
    E: Into<anyhow::Error> + Send + Sync + 'static,
    Sink: SinkExt<Vec<ClientCollabMessage>, Error = E> + Send + Sync + Unpin + 'static,
    Stream: StreamExt<Item = Result<ServerCollabMessage, E>> + Send + Sync + Unpin + 'static,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        object: SyncObject,
        origin: CollabOrigin,
        sink: Sink,
        sink_config: SinkConfig,
        stream: Stream,
        collab: Weak<MutexCollab>,
        pause: bool,
    ) -> Self {
        let protocol = ClientSyncProtocol;
        let (notifier, notifier_rx) = watch::channel(SinkSignal::Proceed);
        let sync_state = Arc::new(watch::channel(SyncState::InitSyncBegin).0);
        let (sync_state_tx, sync_state_rx) = watch::channel(SinkState::Init);
        debug_assert!(origin.client_user_id().is_some());
        let sink = Arc::new(CollabSink::new(
            origin.client_user_id().unwrap_or(0),
            object.clone(),
            sink,
            notifier,
            sync_state_tx,
            sink_config,
            pause,
        ));
        af_spawn(CollabSinkRunner::run(Arc::downgrade(&sink), notifier_rx));
        let _cloned_protocol = protocol.clone();
        let _object_id = object.object_id.clone();
        let stream = ObserveCollab::new(
            origin.clone(),
            object.clone(),
            stream,
            collab.clone(),
            Arc::downgrade(&sink),
        );
        let weak_sync_state = Arc::downgrade(&sync_state);
        let mut sink_state_stream = WatchStream::new(sink_state_rx);
        af_spawn(async move {
            while let Some(collab_state) = sink_state_stream.next().await {
                if let Some(sync_state) = weak_sync_state.upgrade() {
                    match collab_state {
                        SinkState::Syncing => {
                            let _ = sync_state.send(SyncState::Syncing);
                        },
                        SinkState::Finished => {
                            let _ = sync_state.send(SyncState::SyncFinished);
                        },
                        SinkState::Init => {
                            let _ = sync_state.send(SyncState::InitSyncBegin);
                        },
                        SinkState::Pause => {},
                    }
                }
            }
        });
        Self {
            object,
            origin,
            sink,
            observe_collab: stream,
            sync_state,        
        }
    }
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
    let awareness = collab.get_awareness();
    if let Some(payload) = doc_init_state(awareness, &ClientSyncProtocol) {
        sink.queue_init_sync(|msg_id| {
            let init_sync = InitSync::new(
                origin,
                sync_object.object_id.clone(),
                sync_object.collab_type.clone(),
                sync_object.workspace_id.clone(),
                msg_id,
                payload,
            );
            ClientCollabMessage::new_init_sync(init_sync)
        })
    } else {
        sink.notify();
    }
}

impl<Sink, Stream> Deref for SyncControl<Sink, Stream> {
    type Target = Arc<CollabSink<Sink, ClientCollabMessage>>;
    fn deref(&self) -> &Self::Target {
        &self.sink
    }
}

struct ObserveCollab<Sink, Stream> {
    object_id: String,
    #[allow(dead_code)]
    weak_collab: Weak<MutexCollab>,
    phantom_sink: PhantomData<Sink>,
    phantom_stream: PhantomData<Stream>,
}

impl<Sink, Stream> Drop for ObserveCollab<Sink, Stream> {
    fn drop(&mut self) {
        trace!("Drop SyncStream {}", self.object_id);
    }
}

impl<E, Sink, Stream> ObserveCollab<Sink, Stream>
where
    E: Into<anyhow::Error> + Send + Sync + 'static,
    Sink: SinkExt<Vec<ClientCollabMessage>, Error = E> + Send + Sync + Unpin + 'static,
    Stream: StreamExt<Item = Result<ServerCollabMessage, E>> + Send + Sync + Unpin + 'static,
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