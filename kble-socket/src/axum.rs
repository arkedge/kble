use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use futures_util::{future, stream, SinkExt, StreamExt, TryStreamExt};

use crate::{SocketSink, SocketStream};

pub fn from_axum(ws: WebSocket) -> (SocketSink, SocketStream) {
    let (sink, stream) = ws.split();
    let sink = sink
        .with_flat_map(|b| stream::iter([Ok(Message::Binary(Bytes::into(b)))]))
        .sink_map_err(Into::into);
    let stream = stream
        .try_filter_map(|msg| match msg {
            Message::Binary(b) => future::ok(Some(b.into())),
            _ => future::ok(None),
        })
        .map_err(Into::into);
    (Box::pin(sink), Box::pin(stream))
}
