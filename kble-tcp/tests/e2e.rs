//! End-to-end tests that launch the real `kble-tcp` binary.
//!
//! `kble-tcp <addr>` connects to a TCP server at `addr` and bridges it to the
//! WebSocket-over-stdio plug protocol: frames arriving on its WS input are
//! written to the TCP peer, and bytes from the TCP peer are emitted as WS
//! frames. The test plays both ends — it is the WS client (via
//! [`kble_test_support::Plug`]) *and* the TCP server the bridge dials — so it
//! can drive bytes across the bridge in both directions.
//!
//! Unlike the framed plugs (eb90, etc.), TCP is a byte stream: `kble-tcp` does
//! no framing on the WS→TCP side and emits one frame per TCP read (up to 8 KiB)
//! on the TCP→WS side. So the tests treat the bridge as a byte pipe — reading a
//! known number of bytes — rather than asserting on frame boundaries.

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use kble_test_support::Plug;
use proptest::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::runtime::Runtime;

/// Build a `Command` running the freshly-built `kble-tcp` binary pointed at the
/// TCP server `addr`. Cargo exposes the binary path to this crate's own
/// integration tests via the env var.
fn kble_tcp(addr: &SocketAddr) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kble-tcp"));
    cmd.arg(addr.to_string());
    cmd
}

/// Bind a local TCP server, spawn `kble-tcp` pointed at it, and hand back the
/// plug (the WS-over-stdio client side of the bridge) together with the
/// accepted server-side `TcpStream` — the two ends the bridge shuttles bytes
/// between.
async fn spawn_bridge() -> (Plug, TcpStream) {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind tcp server");
    let addr = listener.local_addr().expect("read tcp server addr");

    let plug = Plug::spawn(kble_tcp(&addr)).await.expect("spawn kble-tcp");
    // The bound listener queues the connection, so accepting after the spawn is
    // safe; the deadline turns "the bridge never dialed us" into a failure.
    let (tcp, _peer) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("kble-tcp should connect to the tcp server")
        .expect("accept the bridge's tcp connection");
    (plug, tcp)
}

/// Read WS frames from the bridge until `n` bytes have arrived, concatenated.
/// kble-tcp turns each TCP read (up to 8 KiB) into one frame, so a larger
/// payload spans several frames; this reassembles the byte stream.
async fn recv_bytes(plug: &mut Plug, n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let frame = plug.recv().await.expect("bridge emits a frame");
        out.extend_from_slice(&frame);
    }
    assert_eq!(out.len(), n, "bridge emitted more bytes than were written");
    out
}

/// A small current-thread runtime to drive the async body of `proptest!` cases.
fn runtime() -> Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

/// WS → TCP: a frame written into the bridge's WS input is delivered to the TCP
/// peer as bytes.
#[tokio::test]
async fn forwards_ws_input_to_the_tcp_peer() {
    let (mut plug, mut tcp) = spawn_bridge().await;

    let payload = b"hello, tcp peer";
    plug.send(Bytes::from_static(payload))
        .await
        .expect("ws send");

    let mut got = vec![0u8; payload.len()];
    tcp.read_exact(&mut got)
        .await
        .expect("tcp peer reads bytes");
    assert_eq!(&got, payload);

    plug.shutdown().await.expect("bridge exits cleanly");
}

/// TCP → WS: bytes written by the TCP peer surface on the bridge's WS output.
#[tokio::test]
async fn forwards_tcp_peer_bytes_to_ws() {
    let (mut plug, mut tcp) = spawn_bridge().await;

    let payload = b"hello, ws side";
    tcp.write_all(payload).await.expect("tcp peer writes bytes");

    let got = recv_bytes(&mut plug, payload.len()).await;
    assert_eq!(got, payload);

    plug.shutdown().await.expect("bridge exits cleanly");
}

/// A TCP payload larger than kble-tcp's 8 KiB read buffer arrives over several
/// frames but reassembles to the original bytes.
#[tokio::test]
async fn reassembles_large_tcp_payload_across_frames() {
    let (mut plug, mut tcp) = spawn_bridge().await;

    // > 8192 so kble-tcp needs multiple reads, i.e. emits multiple frames.
    let payload = vec![0xABu8; 20_000];
    tcp.write_all(&payload)
        .await
        .expect("tcp peer writes bytes");

    let got = recv_bytes(&mut plug, payload.len()).await;
    assert_eq!(got, payload);

    plug.shutdown().await.expect("bridge exits cleanly");
}

/// Closing the TCP peer ends the bridge: kble-tcp's TCP read returns EOF, the
/// process exits, and its WS output stream therefore ends.
#[tokio::test]
async fn exits_when_the_tcp_peer_closes() {
    let (mut plug, tcp) = spawn_bridge().await;

    drop(tcp); // close the TCP connection -> kble-tcp's `from_tcp` sees EOF

    let err = plug
        .recv_timeout(Duration::from_secs(5))
        .await
        .expect_err("bridge output should end once the tcp peer closes");
    // The bridge exits and closes stdout abruptly, which the WS client surfaces
    // as a reset / end-of-stream. Assert positively for that: a timeout (the
    // hang this guards against) or any other error must fail the test rather
    // than slip through a mere "not a timeout" check.
    let msg = err.to_string();
    assert!(
        msg.contains("reset") || msg.contains("closed"),
        "expected the bridge's output to end (reset/eof) on tcp close, got: {err}"
    );

    // The bridge has already exited (the assertion above proved the *tcp* close
    // drove it). Reap it and confirm a clean exit status, matching the other
    // tests; closing the now-dead WS sink is a harmless no-op.
    plug.shutdown()
        .await
        .expect("bridge should have exited cleanly after the tcp peer closed");
}

proptest! {
    // Each case spawns a kble-tcp process, so keep the count modest. Integration
    // tests have no crate-root source dir, so disable the on-disk regression file.
    #![proptest_config(ProptestConfig {
        cases: 16,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// WS -> TCP preserves arbitrary payload bytes.
    #[test]
    fn forwards_arbitrary_ws_payload_to_tcp(
        payload in proptest::collection::vec(any::<u8>(), 1..4096)
    ) {
        let got = runtime().block_on(async {
            let (mut plug, mut tcp) = spawn_bridge().await;
            plug.send(Bytes::copy_from_slice(&payload)).await.expect("ws send");
            let mut got = vec![0u8; payload.len()];
            tcp.read_exact(&mut got).await.expect("tcp read");
            // Reap the bridge synchronously: shutdown closes the WS input and
            // waits, so no zombie lingers when this case's runtime is dropped.
            plug.shutdown().await.expect("bridge exits cleanly");
            got
        });
        prop_assert_eq!(got, payload);
    }

    /// TCP -> WS preserves arbitrary payload bytes (reassembled across frames).
    #[test]
    fn forwards_arbitrary_tcp_payload_to_ws(
        payload in proptest::collection::vec(any::<u8>(), 1..4096)
    ) {
        let got = runtime().block_on(async {
            let (mut plug, mut tcp) = spawn_bridge().await;
            tcp.write_all(&payload).await.expect("tcp write");
            let got = recv_bytes(&mut plug, payload.len()).await;
            // Reap the bridge synchronously (see the WS->TCP case above).
            plug.shutdown().await.expect("bridge exits cleanly");
            got
        });
        prop_assert_eq!(got, payload);
    }
}
