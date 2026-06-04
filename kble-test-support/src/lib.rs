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

use std::{io, pin::Pin, process::Stdio, task, time::Duration};

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use kble_socket::{from_tungstenite, SocketSink, SocketStream};
use pin_project_lite::pin_project;
use tokio::{
    io::{AsyncRead, AsyncWrite},
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
        match tokio::time::timeout(timeout, self.stream.next()).await {
            Ok(Some(frame)) => frame,
            Ok(None) => Err(anyhow!(
                "plug closed its output stream without sending a frame"
            )),
            Err(_) => Err(anyhow!("plug produced no frame within {timeout:?}")),
        }
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
}
