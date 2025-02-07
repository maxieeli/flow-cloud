use crate::collab_sync::DEFAULT_SYNC_TIMEOUT;
use std::time::Duration;

pub struct SinkConfig {
    /// `timeout` is the time to wait for the remote to ack the message. If the remote
    /// does not ack the message in time, the message will be sent again.
    pub send_timeout: Duration,
    /// `maximum_payload_size` is the maximum size of the messages to be merged.
    pub maximum_payload_size: usize,
    /// Default is false. If true, the sink will not merge messages.
    pub disable_merge_message: bool,
}

impl SinkConfig {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn send_timeout(mut self, secs: u64) -> Self {
        self.send_timeout = Duration::from_secs(secs);
        self
    }
}

impl Default for SinkConfig {
    fn default() -> Self {
        Self {
            send_timeout: Duration::from_secs(DEFAULT_SYNC_TIMEOUT),
            maximum_payload_size: 1024 * 10,
            disable_merge_message: false,
        }
    }
}
