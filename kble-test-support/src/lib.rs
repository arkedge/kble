//! Test-only helpers for end-to-end testing of kble plug processes.
//!
//! Every kble plug binary (kble-eb90, kble-c2a, kble-tcp, ...) speaks the
//! WebSocket protocol over its stdin/stdout via [`kble_socket::from_stdio`],
//! acting as the WebSocket *server*. The `kble` orchestrator connects to them
//! as the *client* (see `kble/src/plug.rs`).
//!
//! [`Plug`] lets a test play that same client role: it spawns the binary with
//! piped stdio, performs no HTTP handshake (just like the orchestrator —
//! `from_raw_socket` only sets up framing), and hands back a binary
//! [`SocketSink`]/[`SocketStream`] pair to push bytes in and read bytes out.
//!
//! [`WsPlug`] covers the other direction: it stands up an in-process WebSocket
//! *server* on an ephemeral port so a test can exercise the `kble` orchestrator
//! binary itself. The orchestrator connects to `ws://` plugs as a client (a
//! real HTTP handshake via `connect_async`), so a test that wants to inject and
//! observe the bytes crossing a link can register a [`WsPlug`] as the link's
//! endpoint and drive it from the outside.

use std::{io, pin::Pin, process::Stdio, task, time::Duration};

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use kble_socket::{from_tungstenite, SocketSink, SocketStream};
use pin_project_lite::pin_project;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
    process::{Child, ChildStdin, ChildStdout, Command},
};
use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};

/// Default deadline for [`Plug::recv`] and grace period for [`Plug::shutdown`].
///
/// Bounded so a misbehaving plug fails the test instead of hanging it forever:
/// there is no libtest per-test timeout, and this workspace builds `proptest`
/// without its `timeout` feature (see the root `Cargo.toml`).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// A spawned kble plug process together with a client-side socket to talk to it.
pub struct Plug {
    child: Child,
    /// Bytes sent here are delivered to the process's input stream.
    pub sink: SocketSink,
    /// Yields the bytes the process writes to its output stream.
    pub stream: SocketStream,
}

impl Plug {
    /// Spawn `command` as a kble plug and connect to it as the WebSocket client.
    ///
    /// The caller only sets the program and arguments; stdio wiring is handled
    /// here. stderr is inherited so the process's logs surface in test output.
    pub async fn spawn(mut command: Command) -> Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .context("failed to spawn plug process")?;
        let stdin = child.stdin.take().context("plug stdin was not piped")?;
        let stdout = child.stdout.take().context("plug stdout was not piped")?;

        let stdio = ChildStdio { stdin, stdout };
        let wss = WebSocketStream::from_raw_socket(stdio, Role::Client, None).await;
        let (sink, stream) = from_tungstenite(wss);

        Ok(Self {
            child,
            sink,
            stream,
        })
    }

    /// Send one frame to the process's input stream.
    pub async fn send(&mut self, frame: Bytes) -> Result<()> {
        self.sink.send(frame).await
    }

    /// Receive the next frame the process emits, using the default deadline.
    pub async fn recv(&mut self) -> Result<Bytes> {
        self.recv_timeout(DEFAULT_TIMEOUT).await
    }

    /// Receive the next frame the process emits, failing — rather than hanging
    /// forever — if nothing arrives within `timeout` or the process closes its
    /// output stream without sending a frame.
    pub async fn recv_timeout(&mut self, timeout: Duration) -> Result<Bytes> {
        next_frame(&mut self.stream, timeout, "plug").await
    }

    /// Close the connection and wait for the process to exit.
    ///
    /// Closing the sink sends a WebSocket Close frame, which ends the process's
    /// input stream so it can shut down gracefully — the same mechanism the
    /// orchestrator relies on. If the process does not exit within
    /// [`DEFAULT_TIMEOUT`] it is killed and an error is returned, so a plug that
    /// ignores the closing handshake fails the test instead of passing silently.
    pub async fn shutdown(mut self) -> Result<()> {
        // Best-effort: a half-open pipe shouldn't fail the test by itself.
        self.sink.close().await.ok();

        match tokio::time::timeout(DEFAULT_TIMEOUT, self.child.wait()).await {
            Ok(wait_result) => {
                let status = wait_result.context("failed waiting for plug process to exit")?;
                // A plug that emits its frame and then crashes still reaches
                // here with Ok; treat a non-zero exit as a failed shutdown.
                if !status.success() {
                    return Err(anyhow!("plug exited with failure status: {status}"));
                }
                Ok(())
            }
            Err(_) => {
                self.child
                    .kill()
                    .await
                    .context("failed to kill unresponsive plug process")?;
                Err(anyhow!(
                    "plug did not exit within {DEFAULT_TIMEOUT:?} after the closing handshake"
                ))
            }
        }
    }
}

/// Receive the next frame from `stream`, failing — rather than hanging forever
/// — if nothing arrives within `timeout` or the peer closes the stream without
/// sending a frame. `peer` names the other end for the error message.
async fn next_frame(stream: &mut SocketStream, timeout: Duration, peer: &str) -> Result<Bytes> {
    match tokio::time::timeout(timeout, stream.next()).await {
        Ok(Some(frame)) => frame,
        Ok(None) => Err(anyhow!(
            "{peer} closed its output stream without sending a frame"
        )),
        Err(_) => Err(anyhow!("{peer} produced no frame within {timeout:?}")),
    }
}

/// An in-process WebSocket server that stands in for a `ws://` plug.
///
/// The `kble` orchestrator connects to `ws://` plugs as a client (a real HTTP
/// handshake via `connect_async`). A test that wants to drive the orchestrator
/// binary binds one of these on an ephemeral port, drops its [`url`](Self::url)
/// into the spaghetti `plugs:` map, then [`accept`](Self::accept)s the
/// orchestrator's connection to get a [`WsPlugConn`] it can push bytes into and
/// read forwarded bytes out of.
///
/// Bind every plug a config references *before* spawning the orchestrator, and
/// accept them concurrently (e.g. with `tokio::join!`): the orchestrator dials
/// its plugs one at a time and blocks on each handshake, so accepting them
/// sequentially can deadlock if its connection order differs from the test's.
pub struct WsPlug {
    url: String,
    listener: TcpListener,
}

impl WsPlug {
    /// Bind a server on an ephemeral `127.0.0.1` port.
    pub async fn bind() -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .context("failed to bind ws plug listener")?;
        let port = listener
            .local_addr()
            .context("failed to read ws plug local address")?
            .port();
        Ok(Self {
            url: format!("ws://127.0.0.1:{port}/"),
            listener,
        })
    }

    /// The `ws://` URL the orchestrator should connect to for this plug.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Accept the orchestrator's connection and complete the WebSocket
    /// handshake, yielding a connected endpoint.
    ///
    /// Bounded by [`DEFAULT_TIMEOUT`]: if nothing connects — the orchestrator
    /// failed to start, hit a config parse error, or was never pointed at this
    /// URL — this fails the test rather than hanging it forever (libtest has no
    /// per-test timeout).
    pub async fn accept(self) -> Result<WsPlugConn> {
        let handshake = async {
            let (tcp, _peer) = self
                .listener
                .accept()
                .await
                .context("ws plug never accepted a connection")?;
            tokio_tungstenite::accept_async(tcp)
                .await
                .context("ws plug handshake failed")
        };
        let wss = match tokio::time::timeout(DEFAULT_TIMEOUT, handshake).await {
            Ok(result) => result?,
            Err(_) => {
                return Err(anyhow!(
                    "ws plug: nothing connected within {DEFAULT_TIMEOUT:?} \
                     (did the orchestrator fail to start?)"
                ))
            }
        };
        let (sink, stream) = from_tungstenite(wss);
        Ok(WsPlugConn { sink, stream })
    }
}

/// A connected in-process `ws://` plug endpoint (see [`WsPlug::accept`]).
///
/// To signal end-of-stream — the orchestrator's cue that a link's source is
/// done — **drop** the connection. That closes the TCP transport, which the
/// orchestrator reads as EOF. A graceful WebSocket Close frame is not enough:
/// the orchestrator reads each plug through the read half of a *split* stream
/// (see `kble/src/plug.rs`), and that half cannot complete the Close handshake
/// (replying needs the write half), so it never observes the close.
pub struct WsPlugConn {
    /// Bytes sent here are delivered to the orchestrator as binary frames.
    pub sink: SocketSink,
    /// Yields the binary frames the orchestrator forwards to this endpoint.
    pub stream: SocketStream,
}

impl WsPlugConn {
    /// Send one binary frame to the orchestrator.
    pub async fn send(&mut self, frame: Bytes) -> Result<()> {
        self.sink.send(frame).await
    }

    /// Receive the next frame the orchestrator forwards here, using the default
    /// deadline.
    pub async fn recv(&mut self) -> Result<Bytes> {
        self.recv_timeout(DEFAULT_TIMEOUT).await
    }

    /// Receive the next frame, failing rather than hanging if none arrives
    /// within `timeout` or the orchestrator closes this link.
    pub async fn recv_timeout(&mut self, timeout: Duration) -> Result<Bytes> {
        next_frame(&mut self.stream, timeout, "ws plug").await
    }
}

pin_project! {
    /// Adapts a child's piped stdin/stdout into a single duplex byte stream,
    /// mirroring the `ChildStdio` used by the orchestrator in `kble/src/plug.rs`.
    struct ChildStdio {
        #[pin]
        stdin: ChildStdin,
        #[pin]
        stdout: ChildStdout,
    }
}

impl AsyncWrite for ChildStdio {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> task::Poll<io::Result<usize>> {
        self.project().stdin.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<io::Result<()>> {
        self.project().stdin.poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> task::Poll<io::Result<()>> {
        self.project().stdin.poll_shutdown(cx)
    }
}

impl AsyncRead for ChildStdio {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> task::Poll<io::Result<()>> {
        self.project().stdout.poll_read(cx, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plug that holds its pipes open but never writes a frame must make
    /// `recv` fail on its deadline rather than hang the test forever.
    #[tokio::test]
    async fn recv_times_out_on_a_silent_plug() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 5"]);
        let mut plug = Plug::spawn(cmd).await.expect("spawn silent plug");

        let result = plug.recv_timeout(Duration::from_millis(200)).await;

        let err = result.expect_err("silent plug should time out, not yield a frame");
        assert!(
            err.to_string().contains("no frame within"),
            "unexpected error: {err}"
        );
        // `kill_on_drop` reaps the still-running `sleep` when `plug` drops.
    }

    /// `WsPlug` must speak a real WebSocket handshake (the orchestrator dials it
    /// with `connect_async`) and ferry binary frames both ways. Accept and
    /// connect are driven concurrently because each blocks until the other side
    /// is handshaking.
    #[tokio::test]
    async fn ws_plug_ferries_binary_frames_both_ways() {
        use tokio_tungstenite::tungstenite::Message;

        let plug = WsPlug::bind().await.expect("bind ws plug");
        let url = plug.url().to_string();

        let (conn, client) = tokio::join!(plug.accept(), tokio_tungstenite::connect_async(url));
        let mut conn = conn.expect("accept orchestrator-side connection");
        let (mut client, _resp) = client.expect("client connects to ws plug");

        // server -> client (orchestrator forwarding *to* this plug)
        conn.send(Bytes::from_static(b"down")).await.expect("send");
        let down = client.next().await.expect("client recv").expect("ws msg");
        assert_eq!(down.into_data(), b"down");

        // client -> server (this plug feeding the orchestrator)
        client
            .send(Message::Binary(b"up".to_vec()))
            .await
            .expect("client send");
        let up = conn.recv().await.expect("server recv");
        assert_eq!(up.as_ref(), b"up");
    }
}
