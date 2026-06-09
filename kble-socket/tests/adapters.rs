//! Adapter-level tests for the externally-facing entry points: each converts a
//! transport's WebSocket into the binary [`kble_socket::SocketSink`] /
//! [`SocketStream`] pair, and these tests round-trip a frame through each one in
//! isolation. They exist so that an axum / tokio-tungstenite bump (#174) that
//! breaks an adapter fails *here*, pointing at the boundary, rather than only
//! surfacing as a confusing failure somewhere downstream.
//!
//! Each test is gated on just the feature its adapter needs, so `tungstenite`
//! and `axum` can be exercised independently (e.g. `--features tungstenite`).
//!
//! Every await is bounded by [`TIMEOUT`]: a regression that stalls an adapter
//! should fail fast at the offending line, not hang the test (and CI).

/// Upper bound on how long any single round-trip step may take.
#[cfg(any(feature = "tungstenite", feature = "axum"))]
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// `from_tungstenite`: bytes round-trip both ways between the adapter and a raw
/// tokio-tungstenite peer connected over an in-memory duplex (no sockets, no
/// HTTP handshake — `from_raw_socket` just sets up framing per role).
#[cfg(feature = "tungstenite")]
#[tokio::test]
async fn from_tungstenite_round_trips_binary_frames() {
    use bytes::Bytes;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::protocol::Role;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::WebSocketStream;

    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
    let mut client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
    let (mut sink, mut stream) = kble_socket::from_tungstenite(server);

    // peer -> adapter stream
    tokio::time::timeout(
        TIMEOUT,
        client.send(Message::Binary(b"to the adapter".to_vec())),
    )
    .await
    .expect("peer send timed out")
    .expect("peer sends a binary frame");
    let got = tokio::time::timeout(TIMEOUT, stream.next())
        .await
        .expect("adapter stream timed out")
        .expect("adapter stream yields a frame")
        .expect("frame is not an error");
    assert_eq!(&got[..], b"to the adapter");

    // adapter sink -> peer (a binary frame, per the adapter's encoding)
    tokio::time::timeout(TIMEOUT, sink.send(Bytes::from_static(b"from the adapter")))
        .await
        .expect("adapter sink send timed out")
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
#[cfg(feature = "axum")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn from_axum_round_trips_binary_frames() {
    use axum::extract::ws::WebSocketUpgrade;
    use axum::response::Response;
    use axum::routing::get;
    use axum::Router;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    async fn echo(upgrade: WebSocketUpgrade) -> Response {
        upgrade.on_upgrade(|ws| async move {
            let (mut sink, mut stream) = kble_socket::from_axum(ws);
            // Echo every frame back through the adapter's sink. A stall here
            // surfaces as the client's bounded receive timing out below.
            while let Some(Ok(frame)) = stream.next().await {
                if sink.send(frame).await.is_err() {
                    break;
                }
            }
        })
    }

    let app = Router::new().route("/", get(echo));
    // axum 0.6 server setup. At the axum 0.8 bump (#174) this migrates to
    // `tokio::net::TcpListener::bind` + `axum::serve` — the same change the
    // binary needs; the WebSocketUpgrade -> `from_axum` path under test is
    // unchanged, so the adapter stays covered across the bump.
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

    tokio::time::timeout(TIMEOUT, client.send(Message::Binary(b"echo me".to_vec())))
        .await
        .expect("client send timed out")
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

    // Bounded like the rest: a hung close shouldn't stall the suite.
    let _ = tokio::time::timeout(TIMEOUT, client.close(None)).await;
}
