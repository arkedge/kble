//! End-to-end tests that launch the real `kble-serialport` binary.
//!
//! `kble-serialport` runs an axum HTTP server; `GET /open?port=..&baudrate=..`
//! upgrades to a WebSocket and bridges binary WS frames to and from the named
//! serial device. These tests fake the device with a pty pair, so no hardware
//! is needed: kble-serialport opens the pty *slave* named in the query, and the
//! test drives the *master* end. A real WebSocket client (tokio-tungstenite)
//! performs the actual HTTP upgrade, so the whole externally-facing surface —
//! axum's server/router/`WebSocketUpgrade` and `kble_socket::from_axum` — is
//! exercised. That is exactly the surface an axum / tokio-tungstenite bump
//! (#174) would break.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serialport::{SerialPort, TTYPort};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Ask the OS for a free port, then release it so kble-serialport can bind it.
/// The release→rebind gap is a race; [`spawn_server`] serializes it across the
/// suite so two parallel tests can't pick the same port.
fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("bind ephemeral port")
        .local_addr()
        .expect("read ephemeral addr")
        .port()
}

/// A `Command` for the freshly-built `kble-serialport` bound to `127.0.0.1:port`.
fn kble_serialport(port: u16) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kble-serialport"));
    cmd.arg("--addr")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string());
    cmd.kill_on_drop(true); // the server runs forever; never leak it
    cmd
}

/// The server binds asynchronously after spawn; wait until it accepts TCP
/// before connecting (and fail loudly if it never comes up).
async fn wait_until_listening(port: u16) {
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("kble-serialport never started listening on 127.0.0.1:{port}");
}

/// Serializes port selection + bind across the suite's tests. Without it two
/// parallel tests can grab the same just-freed ephemeral port; the loser's
/// server never binds, so its `connect_async` hits ConnectionRefused. Held only
/// for setup — the assertions still run concurrently.
static SETUP: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Spawn kble-serialport on a free port and wait until it is listening, with
/// port selection serialized (see [`SETUP`]).
async fn spawn_server() -> (Child, u16) {
    let _setup = SETUP.lock().await;
    let port = free_port();
    let child = kble_serialport(port)
        .spawn()
        .expect("spawn kble-serialport");
    wait_until_listening(port).await;
    (child, port)
}

/// Spawn kble-serialport, create a pty pair, open a WebSocket to `/open` pointed
/// at the pty slave, and hand back the child, the connected WS client, the pty
/// master (the "device" end the test reads from and writes to), and the pty
/// slave handle, which the caller must keep alive — see below.
async fn spawn_bridge() -> (Child, Ws, TTYPort, TTYPort) {
    let (master, slave) = TTYPort::pair().expect("create pty pair");
    // /dev/pts/N on Linux, /dev/ttysNNN on macOS — kble-serialport just needs a path.
    let slave_name = slave.name().expect("slave pty has a device path");
    // kble-serialport opens the slave by this name. We deliberately keep our own
    // `slave` handle open for the whole test instead of dropping it here: on macOS,
    // closing the last slave fd tears the pts down, so the reopen-by-name lands on
    // a node that is no longer a tty and `tcgetattr` fails with ENOTTY ("Not a
    // typewriter"). Holding it open keeps the pts alive; a tty has a single shared
    // input queue that kble-serialport drains, so our unread handle steals no bytes.

    let (child, port) = spawn_server().await;

    let url = format!("ws://127.0.0.1:{port}/open?port={slave_name}&baudrate=9600");
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("websocket upgrade against the serial bridge");
    (child, ws, master, slave)
}

/// Write bytes to the pty master (the "device"), handing the master back so a
/// later call can reuse it. The master is a blocking handle, so the I/O runs on
/// a blocking thread.
async fn device_write(mut master: TTYPort, bytes: &[u8]) -> TTYPort {
    let bytes = bytes.to_vec();
    tokio::task::spawn_blocking(move || {
        master.write_all(&bytes).expect("write to pty master");
        master.flush().expect("flush pty master");
        master
    })
    .await
    .expect("device write task")
}

/// Read exactly `n` bytes from the pty master, handing the master back.
async fn device_read(mut master: TTYPort, n: usize) -> (TTYPort, Vec<u8>) {
    tokio::task::spawn_blocking(move || {
        master
            .set_timeout(Duration::from_secs(5))
            .expect("set pty master read timeout");
        let mut buf = vec![0u8; n];
        master.read_exact(&mut buf).expect("read from pty master");
        (master, buf)
    })
    .await
    .expect("device read task")
}

/// Receive binary WS frames until `n` bytes have arrived. kble-serialport emits
/// one frame per serial read (up to 4 KiB), so a larger payload spans several
/// frames; this reassembles the byte stream.
async fn ws_recv_n(ws: &mut Ws, n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("ws frame timed out")
            .expect("ws stream ended early")
            .expect("ws frame");
        match msg {
            Message::Binary(b) => out.extend_from_slice(&b),
            other => panic!("expected a binary frame, got: {other:?}"),
        }
    }
    assert_eq!(out.len(), n, "bridge emitted more bytes than were written");
    out
}

/// device -> WS: bytes from the serial device surface as binary WS frames.
#[tokio::test]
async fn forwards_serial_bytes_to_the_websocket() {
    let (mut child, mut ws, master, _slave) = spawn_bridge().await;

    let payload = b"bytes from the serial device";
    let _master = device_write(master, payload).await;

    let got = ws_recv_n(&mut ws, payload.len()).await;
    assert_eq!(got, payload);

    ws.close(None).await.ok();
    child.kill().await.ok();
}

/// WS -> device: a binary WS frame is written to the serial device.
#[tokio::test]
async fn forwards_websocket_frames_to_the_serial_device() {
    let (mut child, mut ws, master, _slave) = spawn_bridge().await;

    let payload = b"bytes for the serial device";
    ws.send(Message::Binary(payload.to_vec()))
        .await
        .expect("ws send");

    let (_master, got) = device_read(master, payload.len()).await;
    assert_eq!(got, payload);

    ws.close(None).await.ok();
    child.kill().await.ok();
}

/// A payload larger than kble-serialport's 4 KiB read buffer arrives over
/// several frames but reassembles to the original bytes.
#[tokio::test]
async fn reassembles_a_large_serial_payload_across_frames() {
    let (mut child, mut ws, master, _slave) = spawn_bridge().await;

    // > 4096 so kble-serialport needs multiple reads, i.e. emits several frames.
    let payload = vec![0x5Au8; 20_000];
    let _master = device_write(master, &payload).await;

    let got = ws_recv_n(&mut ws, payload.len()).await;
    assert_eq!(got, payload);

    ws.close(None).await.ok();
    child.kill().await.ok();
}

/// A serial port that can't be opened fails the upgrade with HTTP 500 rather
/// than crashing the server: `handle_get` opens the port *before* upgrading.
#[tokio::test]
async fn rejects_an_unopenable_serial_port() {
    let (mut child, port) = spawn_server().await;

    let url = format!("ws://127.0.0.1:{port}/open?port=/dev/kble-nonexistent-tty&baudrate=9600");
    let err = tokio_tungstenite::connect_async(url)
        .await
        .expect_err("opening a missing serial port must fail the upgrade");

    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => {
            assert_eq!(resp.status(), 500, "expected an HTTP 500 from handle_get");
        }
        other => panic!("expected an HTTP 500 response, got: {other:?}"),
    }

    child.kill().await.ok();
}
