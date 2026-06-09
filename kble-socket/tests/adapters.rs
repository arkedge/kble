//! Adapter-level tests for the externally-facing entry points: each converts a
//! transport's WebSocket into the binary [`kble_socket::SocketSink`] /
//! [`SocketStream`] pair, and these tests round-trip a frame through each one in
//! isolation. They exist so that an axum / tokio-tungstenite bump (#174) that
//! breaks an adapter fails *here*, pointing at the boundary, rather than only
//! surfacing as a confusing failure somewhere downstream.
//!
//! Enabled when both `tungstenite` and `axum` are on, as in a workspace build.
//!
//! Every receive/connect await is bounded by [`TIMEOUT`]: a regression that
//! stops an adapter producing frames should fail fast at the offending line, not
//! hang the test (and CI) indefinitely.

#![cfg(all(feature = "tungstenite", feature = "axum"))]

use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};

/// Upper bound on how long any single round-trip step may take.
const TIMEOUT: Duration = Duration::from_secs(5);

/// `from_tungstenite`: bytes round-trip both ways between the adapter and a raw
/// tokio-tungstenite peer connected over an in-memory duplex (no sockets, no
/// HTTP handshake — `from_raw_socket` just sets up framing per role).
#[tokio::test]
async fn from_tungstenite_round_trips_binary_frames() {
    use tokio_tungstenite::tungstenite::protocol::Role;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::WebSocketStream;

    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
    let mut client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
    let (mut sink, mut stream) = kble_socket::from_tungstenite(server);

    // peer -> adapter stream
    client
        .send(Message::Binary(b"to the adapter".to_vec()))
        .await
        .expect("peer sends a binary frame");
    let got = tokio::time::timeout(TIMEOUT, stream.next())
        .await
        .expect("adapter stream timed out")
        .expect("adapter stream yields a frame")
        .expect("frame is not an error");
    assert_eq!(&got[..], b"to the adapter");

    // adapter sink -> peer (a binary frame, per the adapter's encoding)
    sink.send(Bytes::from_static(b"from the adapter"))
        .await
        .expect("adapter sink accepts a frame");
    match tokio::time::timeout(TIMEOUT, client.next())
        .await
        .expect("peer receive timed out")
        .expect("peer receives a frame")
        .expect("frame is not an error")
    {
        Message::Binary(b) => assert_eq!(&b[..], b"from the adapter"),
        other => panic!("expected a binary frame, got: {other:?}"),
    }
}

/// `from_axum`: bytes round-trip through a real (in-process) axum server whose
/// handler echoes via the adapter, driven by a real tokio-tungstenite client
/// that performs the actual HTTP upgrade.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn from_axum_round_trips_binary_frames() {
    use axum::extract::ws::WebSocketUpgrade;
    use axum::response::Response;
    use axum::routing::get;
    use axum::Router;
    use tokio_tungstenite::tungstenite::Message;

    async fn echo(upgrade: WebSocketUpgrade) -> Response {
        upgrade.on_upgrade(|ws| async move {
            let (mut sink, mut stream) = kble_socket::from_axum(ws);
            // Echo every frame back through the adapter's sink.
            while let Some(Ok(frame)) = stream.next().await {
                if sink.send(frame).await.is_err() {
                    break;
                }
            }
        })
    }

    let app = Router::new().route("/", get(echo));
    let server = axum::Server::bind(&"127.0.0.1:0".parse().unwrap()).serve(app.into_make_service());
    let addr = server.local_addr();
    tokio::spawn(async move {
        server.await.expect("axum server runs");
    });

    let (mut client, _resp) = tokio::time::timeout(
        TIMEOUT,
        tokio_tungstenite::connect_async(format!("ws://{addr}/")),
    )
    .await
    .expect("connect timed out")
    .expect("websocket upgrade against the echo server");

    client
        .send(Message::Binary(b"echo me".to_vec()))
        .await
        .expect("client sends a binary frame");
    match tokio::time::timeout(TIMEOUT, client.next())
        .await
        .expect("echo receive timed out")
        .expect("client receives the echo")
        .expect("frame is not an error")
    {
        Message::Binary(b) => assert_eq!(&b[..], b"echo me"),
        other => panic!("expected a binary echo, got: {other:?}"),
    }

    client.close(None).await.ok();
}
