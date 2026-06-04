use axum_08::extract::ws::{Message, WebSocket};
use futures_util::{future, stream, SinkExt, StreamExt, TryStreamExt};

use crate::{SocketSink, SocketStream};

pub fn from_axum(ws: WebSocket) -> (SocketSink, SocketStream) {
    let (sink, stream) = ws.split();
    let sink = sink
        .with_flat_map(|b| stream::iter([Ok(Message::Binary(b))]))
        .sink_map_err(Into::into);
    let stream = stream
        .try_filter_map(|msg| match msg {
            Message::Binary(b) => future::ok(Some(b)),
            _ => future::ok(None),
        })
        .map_err(Into::into);
    (Box::pin(sink), Box::pin(stream))
}
