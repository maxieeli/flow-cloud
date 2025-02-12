use std::collections::BTreeMap;
use std::i64;
use std::str::FromStr;
use crate::error::{internal, StreamError};
use redis::{FromRedisValue, RedisError, RedisResult, Value};

/// The [MessageId] generated by XADD has two parts: a timestamp and a sequence number, separated by
/// a hyphen (-). The timestamp is based on the server's time when the message is added, and the
/// sequence number is used to differentiate messages added at the same millisecond.
/// If multiple messages are added within the same millisecond, Redis increments the sequence number
/// for each subsequent message
/// An example message ID might look like this: 1631020452097-0. In this example, 1631020452097 is
/// the timestamp in milliseconds, and 0 is the sequence number.
#[derive(Debug)]
pub struct MessageId {
    pub timestamp_ms: u64,
    pub sequence_number: u16,
}

impl ToString for MessageId {
    fn to_string(&self) -> String {
        format!("{}-{}", self.timestamp_ms, self.sequence_number)
    }
}

impl MessageId {
    pub fn sub_ms(&mut self, ms: u64) {
        if self.timestamp_ms < ms {
            return;
        }
        self.timestamp_ms -= ms;
    }
}

impl TryFrom<&[u8]> for MessageId {
    type Error = StreamError;
  
    fn try_from(s: &[u8]) -> Result<Self, Self::Error> {
        let s = std::str::from_utf8(s)?;
        Self::try_from(s)
    }
}

impl TryFrom<&str> for MessageId {
    type Error = StreamError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        let parts: Vec<_> = s.splitn(2, '-').collect();
        if parts.len() != 2 {
            return Err(StreamError::InvalidFormat);
        }
        // Directly parse without intermediate assignment.
        let timestamp_ms = u64::from_str(parts[0])?;
        let sequence_number = u16::from_str(parts[1])?;
        Ok(MessageId {
            timestamp_ms,
            sequence_number,
        })
    }
}

impl FromRedisValue for MessageId {
    fn from_redis_value(v: &Value) -> RedisResult<Self> {
        match v {
            Value::Data(stream_key) => MessageId::try_from(stream_key.as_slice()).map_err(|_| {
                RedisError::from((
                    redis::ErrorKind::TypeError,
                    "invalid stream key",
                    format!("{:?}", stream_key),
                ))
            }),
            _ => Err(internal("expecting Value::Data")),
        }
    }
}

#[derive(Debug)]
pub struct StreamMessageByStreamKey(pub BTreeMap<String, Vec<StreamMessage>>);

impl FromRedisValue for StreamMessageByStreamKey {
    fn from_redis_value(v: &Value) -> RedisResult<Self> {
        let mut map: BTreeMap<String, Vec<StreamMessage>> = BTreeMap::new();
        if matches!(v, Value::Nil) {
            return Ok(StreamMessageByStreamKey(map));
        }
        let value_by_id = bulk_from_redis_value(v)?.iter();
        for value in value_by_id {
            let key_values = bulk_from_redis_value(value)?;
            if key_values.len() != 2 {
                return Err(RedisError::from((
                    redis::ErrorKind::TypeError,
                    "Invalid length",
                    "Expected length of 2 for the outer bulk value".to_string(),
                )));
            }
            let stream_key = RedisString::from_redis_value(&key_values[0])?.0;
            let values = bulk_from_redis_value(&key_values[1])?.iter();
            for value in values {
                let value = StreamMessage::from_redis_value(value)?;
                map.entry(stream_key.clone()).or_default().push(value);
            }
        }
        Ok(StreamMessageByStreamKey(map))
    }
}

/// A message in the Redis stream. It's the same as [Message] but with additional metadata.
#[derive(Debug)]
pub struct StreamMessage {
    /// user who did the change
    pub uid: i64,
    pub raw_data: Vec<u8>,
    /// only applicable when reading from redis
    pub message_id: MessageId,
}

impl FromRedisValue for StreamMessage {
    // optimized parsing function
    fn from_redis_value(v: &Value) -> RedisResult<Self> {
        let bulk = bulk_from_redis_value(v)?;
        if bulk.len() != 2 {
            return Err(RedisError::from((
                redis::ErrorKind::TypeError,
                "Invalid length",
                "Expected length of 2 for the outer bulk value".to_string(),
            )));
        }
        let created_time = MessageId::from_redis_value(&bulk[0])?;
        let fields = bulk_from_redis_value(&bulk[1])?;
        if fields.len() != 4 {
            return Err(RedisError::from((
                redis::ErrorKind::TypeError,
                "Invalid length",
                "Expected length of 4 for the inner bulk value".to_string(),
            )));
        }
        verify_field(&fields[0], "uid")?;
        let uid = UserId::from_redis_value(&fields[1])?.0;
        verify_field(&fields[2], "data")?;
        let raw_data = Vec::<u8>::from_redis_value(&fields[3])?;
        Ok(StreamMessage {
            uid,
            raw_data,
            message_id: created_time,
        })
    }
}

#[derive(Debug)]
pub struct Message {
    pub uid: i64,
    pub raw_data: Vec<u8>,
}

impl From<StreamMessage> for Message {
    fn from(m: StreamMessage) -> Self {
        Message {
            uid: m.uid,
            raw_data: m.raw_data,
        }
    }
}

impl Message {
    pub fn into_tuple_array(self) -> [(&'static str, Vec<u8>); 2] {
        static UID: &str = "uid";
        static DATA: &str = "data";
        let mut buf = [0u8; 8];
        buf.copy_from_slice(self.uid.to_be_bytes().as_slice());
        [(UID, buf.to_vec()), (DATA, self.raw_data)]
    }
}

fn verify_field(field: &Value, expected: &str) -> RedisResult<()> {
    let field_str = String::from_redis_value(field)?;
    if field_str != expected {
        return Err(RedisError::from((
            redis::ErrorKind::TypeError,
            "Invalid field",
            format!("Expected '{}', found '{}'", expected, field_str),
        )));
    }
    Ok(())
}

pub struct RedisString(String);
impl FromRedisValue for RedisString {
    fn from_redis_value(v: &Value) -> RedisResult<Self> {
        match v {
            Value::Data(uid_bytes) => Ok(RedisString(String::from_utf8(uid_bytes.to_vec())?)),
            _ => Err(internal("expecting Value::Data")),
        }
    }
}

impl ToString for RedisString {
    fn to_string(&self) -> String {
        self.0.clone()
    }
}

fn bulk_from_redis_value(v: &Value) -> Result<&Vec<Value>, RedisError> {
    match v {
        Value::Bulk(b) => Ok(b),
        _ => Err(internal("expecting Value::Bulk")),
    }
}

struct UserId(i64);

impl FromRedisValue for UserId {
    fn from_redis_value(v: &Value) -> RedisResult<Self> {
        match v {
            Value::Data(uid_bytes) => {
                if uid_bytes.len() == std::mem::size_of::<i64>() {
                    let mut buf = [0u8; 8];
                    buf.copy_from_slice(uid_bytes);
                    let value = i64::from_be_bytes(buf);
                    Ok(Self(value))
                } else {
                    Err(RedisError::from((
                        redis::ErrorKind::TypeError,
                        "Invalid UID length",
                        format!("Expected 8 bytes, got {}", uid_bytes.len()),
                    )))
                }
            },
            _ => Err(RedisError::from((
                redis::ErrorKind::TypeError,
                "Expected Value::Data for UID",
            )))
        }
    }
}
