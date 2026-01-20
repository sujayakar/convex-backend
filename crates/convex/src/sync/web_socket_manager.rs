use std::{
    collections::BTreeMap,
    convert::Infallible,
    time::Duration,
};

use anyhow::Context;
use async_trait::async_trait;
use convex_sync_types::{
    backoff::Backoff,
    headers::{
        DEPRECATION_MSG_HEADER_NAME,
        DEPRECATION_STATE_HEADER_NAME,
    },
    ClientMessage,
    SessionId,
    Timestamp,
};
use futures::{
    select_biased,
    stream::Fuse,
    FutureExt,
    SinkExt,
    StreamExt,
};
use flexbuffers::{
    FlexBufferType,
    Reader,
};
use serde_json::Value as JsonValue;
use tokio::{
    net::TcpStream,
    sync::{
        mpsc,
        oneshot,
    },
    task::JoinHandle,
    time::{
        Instant,
        Interval,
    },
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        self,
        client::IntoClientRequest,
        http::HeaderMap,
        protocol::Message,
    },
    MaybeTlsStream,
    WebSocketStream,
};
use url::Url;
use uuid::Uuid;

use super::WebSocketState;
use crate::sync::{
    ProtocolResponse,
    ReconnectRequest,
    ServerMessage,
    SyncProtocol,
};

const INITIAL_BACKOFF: Duration = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(15);
const BINARY_FRAME_TYPE_FULL: u8 = 0;
const BINARY_FRAME_TYPE_CHUNK: u8 = 1;
const PACKED_PLACEHOLDER_KEY: &str = "$packed";
type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug)]
enum WebSocketRequest {
    SendMessage(ClientMessage, oneshot::Sender<()>),
    Reconnect(ReconnectRequest),
}

#[derive(Debug)]
struct BinaryChunkFrame {
    message_id: u32,
    part_number: u32,
    total_parts: u32,
    payload: Vec<u8>,
}

#[derive(Debug)]
enum BinaryFrame {
    Full(Vec<u8>),
    Chunk(BinaryChunkFrame),
}

struct WebSocketInternal {
    ws_stream: WsStream,
    last_server_response: Instant,
}
struct BinaryChunkBuffer {
    message_id: u32,
    total_parts: u32,
    chunks: Vec<Vec<u8>>,
}

impl BinaryChunkBuffer {
    fn new(message_id: u32, total_parts: u32) -> Self {
        Self {
            message_id,
            total_parts,
            chunks: Vec::new(),
        }
    }

    fn push(&mut self, frame: BinaryChunkFrame) -> anyhow::Result<Option<Vec<u8>>> {
        anyhow::ensure!(
            self.message_id == frame.message_id && self.total_parts == frame.total_parts,
            "Invalid binary chunk sequence"
        );
        anyhow::ensure!(
            frame.part_number == self.chunks.len() as u32,
            "Out of order binary chunk"
        );
        self.chunks.push(frame.payload);
        if self.chunks.len() == self.total_parts as usize {
            let total_len: usize = self.chunks.iter().map(|chunk| chunk.len()).sum();
            let mut combined = Vec::with_capacity(total_len);
            for chunk in self.chunks.drain(..) {
                combined.extend_from_slice(&chunk);
            }
            Ok(Some(combined))
        } else {
            Ok(None)
        }
    }
}
struct WebSocketWorker {
    ws_url: Url,
    on_response: mpsc::Sender<ProtocolResponse>,
    on_state_change: Option<mpsc::Sender<WebSocketState>>,
    internal_receiver: Fuse<UnboundedReceiverStream<WebSocketRequest>>,
    ping_ticker: Interval,
    connection_count: u32,
    backoff: Backoff,
}

pub struct WebSocketManager {
    internal_sender: mpsc::UnboundedSender<WebSocketRequest>,
    worker_handle: JoinHandle<Infallible>,
}
impl Drop for WebSocketManager {
    fn drop(&mut self) {
        self.worker_handle.abort()
    }
}

#[async_trait]
impl SyncProtocol for WebSocketManager {
    async fn open(
        ws_url: Url,
        on_response: mpsc::Sender<ProtocolResponse>,
        on_state_change: Option<mpsc::Sender<WebSocketState>>,
        client_id: &str,
    ) -> anyhow::Result<Self> {
        let (internal_sender, internal_receiver) = mpsc::unbounded_channel();
        let worker_handle = tokio::spawn(WebSocketWorker::run(
            ws_url,
            on_response,
            on_state_change,
            internal_receiver,
            client_id.to_string(),
        ));

        Ok(WebSocketManager {
            internal_sender,
            worker_handle,
        })
    }

    async fn send(&mut self, message: ClientMessage) -> anyhow::Result<()> {
        let (tx, rx) = oneshot::channel();
        self.internal_sender
            .send(WebSocketRequest::SendMessage(message, tx))?;
        rx.await?;
        Ok(())
    }

    async fn reconnect(&mut self, request: ReconnectRequest) {
        let _ = self
            .internal_sender
            .send(WebSocketRequest::Reconnect(request));
    }
}

impl WebSocketWorker {
    /// How often heartbeat pings are sent.
    const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
    /// How long before lack of server response causes a timeout.
    const SERVER_INACTIVITY_THRESHOLD: Duration = Duration::from_secs(30);

    async fn run(
        ws_url: Url,
        on_response: mpsc::Sender<ProtocolResponse>,
        on_state_change: Option<mpsc::Sender<WebSocketState>>,
        internal_receiver: mpsc::UnboundedReceiver<WebSocketRequest>,
        client_id: String,
    ) -> Infallible {
        let ping_ticker = tokio::time::interval(Self::HEARTBEAT_INTERVAL);
        let backoff = Backoff::new(INITIAL_BACKOFF, MAX_BACKOFF);

        let mut worker = Self {
            ws_url,
            on_response,
            on_state_change,
            internal_receiver: UnboundedReceiverStream::new(internal_receiver).fuse(),
            ping_ticker,
            connection_count: 0,
            backoff,
        };

        let mut last_close_reason = "InitialConnect".to_string();
        let mut max_observed_timestamp = None;
        if let Some(state_change_sender) = &worker.on_state_change {
            let _ = state_change_sender.try_send(WebSocketState::Connecting);
        }
        loop {
            let exit_result = worker
                .work(last_close_reason, max_observed_timestamp, &client_id)
                .await;

            if let Some(state_change_sender) = &worker.on_state_change {
                let _ = state_change_sender.try_send(WebSocketState::Connecting);
            }

            let e = match exit_result {
                Ok(reconnect) => {
                    // WS worker exited cleanly because it got a request to reconnect
                    tracing::debug!("Reconnecting websocket due to {}", reconnect.reason);
                    last_close_reason = reconnect.reason;
                    max_observed_timestamp = reconnect.max_observed_timestamp;
                    continue;
                },
                Err(e) => e,
            };
            worker.connection_count += 1;
            last_close_reason = e.to_string();
            let delay = worker.backoff.fail(&mut rand::rng());
            tracing::error!(
                "Convex WebSocketWorker failed: {e:?}. Backing off for {delay:?} and retrying."
            );

            // Tell the worker that we've failed so it can coordinate the reconnect.
            // The worker will send a Reconnect message and the new query set all together.
            // Drain the input request queue until we get that reconnect message - which
            // will be followed by the refreshed query set.
            let _ = worker.on_response.send(ProtocolResponse::Failure).await;
            tracing::debug!("Waiting for base client to acknowledge reconnect");
            loop {
                let request = worker.internal_receiver.next().await;
                // TODO: There is a potential issue where we have multiple queued reconnect
                // requests in which case max_observed_timestamp might be lower than actually
                // observed. This is fine since it will never cause errors. Will can fix this
                // when we restructure the wider protocol to be a single routine.
                if let Some(WebSocketRequest::Reconnect(reconnect)) = request {
                    max_observed_timestamp = reconnect.max_observed_timestamp;
                    break;
                }
            }
            tracing::debug!(
                "Base client acknowledged reconnect. Sleeping {delay:?} and reconnecting"
            );
            tokio::time::sleep(delay).await;
            tracing::debug!("Reconnecting");
        }
    }

    async fn work(
        &mut self,
        last_close_reason: String,
        max_seen_transition: Option<Timestamp>,
        client_id: &str,
    ) -> anyhow::Result<ReconnectRequest> {
        let verb = if self.connection_count == 0 {
            "connect"
        } else {
            "reconnect"
        };
        tracing::debug!("trying to {verb} to {}", self.ws_url);
        let mut internal = WebSocketInternal::new(
            self.ws_url.clone(),
            self.connection_count,
            last_close_reason,
            max_seen_transition,
            client_id,
        )
        .await?;
        tracing::debug!("completed websocket {verb} to {}", self.ws_url);
        if let Some(state_change_sender) = &self.on_state_change {
            let _ = state_change_sender.try_send(WebSocketState::Connected);
        }
        let mut binary_chunk_buffer: Option<BinaryChunkBuffer> = None;

        loop {
            select_biased! {
                _ = self.ping_ticker.tick().fuse() => {
                    let now = Instant::now();
                    if now - internal.last_server_response > Self::SERVER_INACTIVITY_THRESHOLD {
                        anyhow::bail!("InactiveServer");
                    }
                },
                server_msg = internal.ws_stream.select_next_some() => {
                    internal.last_server_response = Instant::now();

                    match server_msg.context("WebsocketConnectionError")? {
                        Message::Close(close_frame) => {
                            let close_frame = close_frame.context("CloseMessageWithoutFrame")?;
                            tracing::debug!("Close frame {close_frame}");
                            anyhow::bail!("{}", close_frame.reason);
                        },
                        Message::Binary(payload) => {
                            let frame = decode_binary_frame(&payload)?;
                            let combined = match frame {
                                BinaryFrame::Full(payload) => {
                                    if binary_chunk_buffer.take().is_some() {
                                        tracing::warn!("Dropping buffered binary chunks due to full frame");
                                    }
                                    Some(payload)
                                },
                                BinaryFrame::Chunk(frame) => {
                                    let buffer = binary_chunk_buffer.get_or_insert_with(|| {
                                        BinaryChunkBuffer::new(
                                            frame.message_id,
                                            frame.total_parts,
                                        )
                                    });
                                    let combined = buffer.push(frame)?;
                                    if combined.is_some() {
                                        binary_chunk_buffer = None;
                                    }
                                    combined
                                },
                            };

                            if let Some(payload) = combined {
                                let server_message = parse_binary_server_message(&payload)?;
                                match server_message {
                                    ServerMessage::Ping => tracing::trace!("received message {server_message:?}"),
                                    _ => tracing::trace!("received message {server_message:?}"),
                                };

                                let resp = ProtocolResponse::ServerMessage(server_message);
                                let _ = self.on_response.send(resp).await;
                                self.backoff.reset();
                            }
                        },
                        Message::Text(t) => {
                            if binary_chunk_buffer.take().is_some() {
                                tracing::warn!(
                                    "Dropping buffered binary chunks due to text message"
                                );
                            }
                            let json: serde_json::Value = serde_json::from_str(&t).context("JsonDeserializeError")?;
                            let server_message = json.try_into()?;
                            match server_message {
                                ServerMessage::Ping => tracing::trace!("received message {server_message:?}"),
                                _ => tracing::trace!("received message {server_message:?}"),
                            };

                            let resp = ProtocolResponse::ServerMessage(server_message);
                            let _ = self.on_response.send(resp).await;

                            // TODO: Similar to JS, we should ideally only reset backoff if we get
                            // the client gets into a correct state, where we have Connected and
                            // received a response to our pending Queries and Mutations.
                            self.backoff.reset();
                        },
                        Message::Ping(_) => {
                            tracing::trace!("received Ping");
                        }
                        server_msg => {
                            tracing::debug!("received unknown message {server_msg:?}");
                        },
                    }
                },
                request = self.internal_receiver.select_next_some() => {
                    match request {
                        WebSocketRequest::SendMessage(message, sender) => {
                            tracing::debug!("Sending {message:?}");
                            let msg = Message::Text(serde_json::Value::try_from(message).context("JsonSerializeError")?.to_string().into());
                            internal.send_worker(msg.clone()).await?;
                            let _ = sender.send(());
                        },
                        WebSocketRequest::Reconnect(reason) => return Ok(reason),
                    };
                }
            };
        }
    }
}

fn deprecation_message(headers: &HeaderMap) -> Option<String> {
    let dep_state = headers.get(DEPRECATION_STATE_HEADER_NAME)?.to_str().ok()?;
    let msg = headers.get(DEPRECATION_MSG_HEADER_NAME)?.to_str().ok()?;
    Some(format!("{dep_state}: {msg}"))
}

impl WebSocketInternal {
    async fn new(
        ws_url: Url,
        connection_count: u32,
        last_close_reason: String,
        max_observed_timestamp: Option<Timestamp>,
        client_id: &str,
    ) -> anyhow::Result<WebSocketInternal> {
        let mut request = (&ws_url).into_client_request().context("Bad WS Url")?;
        request.headers_mut().insert(
            "Convex-Client",
            client_id.try_into().context("Bad client id")?,
        );
        let (ws_stream, response) = connect_async(request).await.map_err(|e| {
            if let tungstenite::Error::Http(ref response) = e {
                let body = response
                    .body()
                    .as_deref()
                    .map(String::from_utf8_lossy)
                    .unwrap_or_default();
                return anyhow::anyhow!("Connection to {ws_url} failed: {e}: {body}");
            }
            anyhow::anyhow!("Connection to {ws_url} failed: {e}")
        })?;

        if let Some(msg) = deprecation_message(response.headers()) {
            tracing::warn!("{msg}");
        }

        let last_server_response = Instant::now();
        let mut internal = WebSocketInternal {
            ws_stream,
            last_server_response,
        };

        // Send an initial connect message on the new websocket
        let session_id = Uuid::new_v4();
        let message = ClientMessage::Connect {
            session_id: SessionId::new(session_id),
            connection_count,
            last_close_reason,
            max_observed_timestamp,
            client_ts: Some(0),
            supports_binary: true,
        };
        let msg = Message::Text(
            serde_json::Value::try_from(message)
                .context("JSONSerializationErrorOnConnect")?
                .to_string()
                .into(),
        );
        internal.send_worker(msg).await?;

        Ok(internal)
    }

    async fn send_worker(&mut self, message: Message) -> anyhow::Result<()> {
        self.ws_stream
            .send(message)
            .await
            .context("WebsocketClosedOnSend")
    }
}

fn decode_binary_frame(payload: &[u8]) -> anyhow::Result<BinaryFrame> {
    let frame_type = payload.first().context("Empty binary frame")?;
    match *frame_type {
        BINARY_FRAME_TYPE_FULL => Ok(BinaryFrame::Full(payload[1..].to_vec())),
        BINARY_FRAME_TYPE_CHUNK => {
            anyhow::ensure!(payload.len() >= 13, "Binary chunk frame too small");
            let message_id = u32::from_le_bytes(payload[1..5].try_into()?);
            let part_number = u32::from_le_bytes(payload[5..9].try_into()?);
            let total_parts = u32::from_le_bytes(payload[9..13].try_into()?);
            anyhow::ensure!(total_parts > 0, "Binary chunk frame has zero parts");
            anyhow::ensure!(
                part_number < total_parts,
                "Binary chunk frame part out of range"
            );
            Ok(BinaryFrame::Chunk(BinaryChunkFrame {
                message_id,
                part_number,
                total_parts,
                payload: payload[13..].to_vec(),
            }))
        },
        other => anyhow::bail!("Unknown binary frame type {other}"),
    }
}

fn parse_binary_server_message(payload: &[u8]) -> anyhow::Result<ServerMessage> {
    anyhow::ensure!(payload.len() >= 4, "Binary payload missing header length");
    let header_len = u32::from_le_bytes(payload[0..4].try_into()?) as usize;
    anyhow::ensure!(
        payload.len() >= 4 + header_len,
        "Binary payload header length out of range"
    );
    let header_bytes = &payload[4..4 + header_len];
    let mut offset = 4 + header_len;
    let mut packed_values = Vec::new();
    while offset < payload.len() {
        anyhow::ensure!(payload.len() >= offset + 4, "Packed value length missing");
        let packed_len = u32::from_le_bytes(payload[offset..offset + 4].try_into()?) as usize;
        offset += 4;
        anyhow::ensure!(
            payload.len() >= offset + packed_len,
            "Packed value length out of range"
        );
        let packed_bytes = &payload[offset..offset + packed_len];
        offset += packed_len;
        packed_values.push(decode_packed_json(packed_bytes)?);
    }
    let header: JsonValue = serde_json::from_slice(header_bytes)?;
    let hydrated = replace_packed_values(header, &packed_values)?;
    Ok(hydrated.try_into()?)
}

fn decode_packed_json(bytes: &[u8]) -> anyhow::Result<JsonValue> {
    let root = Reader::get_root(bytes)?;
    let value = decode_flex_value(root)?;
    Ok(JsonValue::from(value))
}

fn decode_flex_value(reader: Reader<&[u8]>) -> anyhow::Result<crate::value::Value> {
    let result = match reader.flexbuffer_type() {
        FlexBufferType::Null => crate::value::Value::Null,
        FlexBufferType::Int | FlexBufferType::IndirectInt => {
            crate::value::Value::Int64(reader.get_i64()?)
        },
        FlexBufferType::UInt | FlexBufferType::IndirectUInt => {
            let value = reader.get_u64()?;
            let signed = i64::try_from(value)
                .map_err(|_| anyhow::anyhow!("Unsigned value {value} exceeds i64::MAX"))?;
            crate::value::Value::Int64(signed)
        },
        FlexBufferType::Float | FlexBufferType::IndirectFloat => {
            crate::value::Value::Float64(reader.get_f64()?)
        },
        FlexBufferType::Bool => crate::value::Value::Boolean(reader.get_bool()?),
        FlexBufferType::String => crate::value::Value::String(reader.get_str()?.to_owned()),
        FlexBufferType::Blob => crate::value::Value::Bytes(reader.get_blob()?.0.to_vec()),
        FlexBufferType::Map => {
            let map_reader = reader.get_map()?;
            let mut out = BTreeMap::new();
            for (key, value) in map_reader.iter_keys().zip(map_reader.iter_values()) {
                out.insert(key.to_string(), decode_flex_value(value)?);
            }
            crate::value::Value::Object(out)
        },
        ty if ty.is_vector() => {
            let vector = reader.get_vector()?;
            let mut out = Vec::with_capacity(vector.len());
            for idx in 0..vector.len() {
                out.push(decode_flex_value(vector.index(idx)?)?);
            }
            crate::value::Value::Array(out)
        },
        ty => anyhow::bail!("Unexpected flexbuffer type {ty:?}"),
    };
    Ok(result)
}

fn replace_packed_values(
    value: JsonValue,
    packed_values: &[JsonValue],
) -> anyhow::Result<JsonValue> {
    match value {
        JsonValue::Array(values) => {
            let replaced = values
                .into_iter()
                .map(|entry| replace_packed_values(entry, packed_values))
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok(JsonValue::Array(replaced))
        },
        JsonValue::Object(mut map) => {
            if map.len() == 1 {
                if let Some(index_value) = map.remove(PACKED_PLACEHOLDER_KEY) {
                    let index = index_value
                        .as_u64()
                        .context("Packed placeholder index must be a number")?;
                    return packed_values
                        .get(index as usize)
                        .cloned()
                        .context("Packed placeholder index out of range");
                }
            }
            let mut out = serde_json::Map::with_capacity(map.len());
            for (key, value) in map {
                out.insert(key, replace_packed_values(value, packed_values)?);
            }
            Ok(JsonValue::Object(out))
        },
        other => Ok(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_binary_server_message_roundtrip() -> anyhow::Result<()> {
        let packed = flexbuffers::singleton(42i64);
        let header = serde_json::json!({
            "type": "MutationResponse",
            "requestId": 1,
            "success": true,
            "result": { "$packed": 0 },
            "logLines": [],
        });
        let header_bytes = serde_json::to_vec(&header)?;
        let mut payload = Vec::new();
        payload.extend_from_slice(&(header_bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(&header_bytes);
        payload.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        payload.extend_from_slice(&packed);

        let message = parse_binary_server_message(&payload)?;
        match message {
            ServerMessage::MutationResponse { result: Ok(value), .. } => {
                assert_eq!(value, crate::value::Value::Int64(42));
            },
            other => anyhow::bail!("Unexpected server message {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn binary_chunk_buffer_reassembles_payload() -> anyhow::Result<()> {
        let mut buffer = BinaryChunkBuffer::new(3, 3);
        let chunk0 = BinaryChunkFrame {
            message_id: 3,
            part_number: 0,
            total_parts: 3,
            payload: vec![1, 2],
        };
        let chunk1 = BinaryChunkFrame {
            message_id: 3,
            part_number: 1,
            total_parts: 3,
            payload: vec![3, 4],
        };
        let chunk2 = BinaryChunkFrame {
            message_id: 3,
            part_number: 2,
            total_parts: 3,
            payload: vec![5],
        };

        assert!(buffer.push(chunk0)?.is_none());
        assert!(buffer.push(chunk1)?.is_none());
        let combined = buffer.push(chunk2)?.expect("Expected full payload");
        assert_eq!(combined, vec![1, 2, 3, 4, 5]);
        Ok(())
    }

    #[test]
    fn parse_binary_server_message_accepts_uints() -> anyhow::Result<()> {
        let packed = flexbuffers::singleton(7u64);
        let header = serde_json::json!({
            "type": "MutationResponse",
            "requestId": 1,
            "success": true,
            "result": { "$packed": 0 },
            "logLines": [],
        });
        let header_bytes = serde_json::to_vec(&header)?;
        let mut payload = Vec::new();
        payload.extend_from_slice(&(header_bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(&header_bytes);
        payload.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        payload.extend_from_slice(&packed);

        let message = parse_binary_server_message(&payload)?;
        match message {
            ServerMessage::MutationResponse { result: Ok(value), .. } => {
                assert_eq!(value, crate::value::Value::Int64(7));
            },
            other => anyhow::bail!("Unexpected server message {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn decode_binary_frame_rejects_zero_parts() {
        let mut frame = vec![BINARY_FRAME_TYPE_CHUNK];
        frame.extend_from_slice(&1u32.to_le_bytes());
        frame.extend_from_slice(&0u32.to_le_bytes());
        frame.extend_from_slice(&0u32.to_le_bytes());
        frame.extend_from_slice(&[1, 2, 3]);
        let err = decode_binary_frame(&frame).expect_err("expected error");
        assert!(err.to_string().contains("zero parts"));
    }

    #[test]
    fn decode_binary_frame_rejects_out_of_range_part() {
        let mut frame = vec![BINARY_FRAME_TYPE_CHUNK];
        frame.extend_from_slice(&2u32.to_le_bytes());
        frame.extend_from_slice(&5u32.to_le_bytes());
        frame.extend_from_slice(&3u32.to_le_bytes());
        frame.extend_from_slice(&[1, 2, 3]);
        let err = decode_binary_frame(&frame).expect_err("expected error");
        assert!(err.to_string().contains("part out of range"));
    }
}
