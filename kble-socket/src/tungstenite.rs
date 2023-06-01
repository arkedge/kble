use bytes::Bytes;
use futures_util::{future, stream, SinkExt, StreamExt, TryStreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

use crate::{SocketSink, SocketStream};

pub fn from_tungstenite<S>(wss: WebSocketStream<S>) -> (SocketSink, SocketStream)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (sink, stream) = wss.split();
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
