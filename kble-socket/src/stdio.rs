use std::{io, pin::Pin, task};

use anyhow::Result;
use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, AsyncWrite, Stdin, Stdout};

use crate::{SocketSink, SocketStream};

/// Build a binary socket over this process's stdin/stdout (the WebSocket
/// *server* side; the orchestrator is the client).
///
/// Pipe stdin/stdout (how a plug is launched) are read/written cancellably so
/// the plug shuts down cleanly even when it ends for a reason other than stdin
/// closing; a terminal/file or non-Unix falls back to blocking `tokio::io`. See
/// [`StdinReader`] for the why.
#[cfg(feature = "tungstenite")]
pub async fn from_stdio() -> (SocketSink, SocketStream) {
    use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};
    let stdio = AutoStdio::new();
    let wss = WebSocketStream::from_raw_socket(stdio, Role::Server, None).await;
    crate::from_tungstenite(wss)
}

/// The stdin half of [`from_stdio`]'s socket.
///
/// `tokio::io::stdin()` reads on a blocking thread that can't be cancelled, so a
/// plug that ends for a reason other than stdin closing (a `kble-tcp` peer
/// disconnecting, a `kble-dump replay` file ending) would hang on runtime
/// shutdown. When stdin is a pipe (plugs launched by the orchestrator or test
/// harness) read it via a cancellable `pipe::Receiver`; else fall back to the
/// blocking reader. (Needs an IO runtime; sets the pipe fd non-blocking.)
#[cfg(feature = "tungstenite")]
enum StdinReader {
    #[cfg(unix)]
    Pipe(tokio::net::unix::pipe::Receiver),
    Blocking(Stdin),
}

#[cfg(feature = "tungstenite")]
impl StdinReader {
    fn new() -> Self {
        #[cfg(unix)]
        if let Some(rx) = unix_pipe_stdin() {
            return Self::Pipe(rx);
        }
        Self::Blocking(tokio::io::stdin())
    }
}

/// Wrap a *duplicate* of fd 0 as a cancellable pipe reader (so dropping it never
/// closes the real stdin). `from_owned_fd` accepts only a readable pipe and sets
/// it non-blocking — a non-pipe yields `InvalidInput`, i.e. "fall back". The
/// non-blocking flag is on the shared fd, so the process's stdin is non-blocking
/// afterwards (fine: `from_stdio` owns it).
#[cfg(all(unix, feature = "tungstenite"))]
fn unix_pipe_stdin() -> Option<tokio::net::unix::pipe::Receiver> {
    use std::os::fd::AsFd;
    let dup = std::io::stdin().as_fd().try_clone_to_owned().ok()?;
    tokio::net::unix::pipe::Receiver::from_owned_fd(dup).ok()
}

/// The stdout half, mirroring [`StdinReader`]. `tokio::io::stdout()` writes on
/// the same kind of uncancellable blocking thread, so if a plug ends while a
/// write is parked on a full pipe (the orchestrator stopped reading), runtime
/// shutdown would hang there. When stdout is a pipe, write via a cancellable
/// `pipe::Sender`. (Ordinary backpressure is unaffected — the write just resumes
/// once the reader drains.)
#[cfg(feature = "tungstenite")]
enum StdoutWriter {
    #[cfg(unix)]
    Pipe(tokio::net::unix::pipe::Sender),
    Blocking(Stdout),
}

#[cfg(feature = "tungstenite")]
impl StdoutWriter {
    fn new() -> Self {
        #[cfg(unix)]
        if let Some(tx) = unix_pipe_stdout() {
            return Self::Pipe(tx);
        }
        Self::Blocking(tokio::io::stdout())
    }
}

/// Wrap stdout (fd 1) as a cancellable pipe writer, or `None` to fall back.
/// See [`unix_pipe_stdin`] for the fd-duplication and non-blocking caveats.
#[cfg(all(unix, feature = "tungstenite"))]
fn unix_pipe_stdout() -> Option<tokio::net::unix::pipe::Sender> {
    use std::os::fd::AsFd;
    let dup = std::io::stdout().as_fd().try_clone_to_owned().ok()?;
    tokio::net::unix::pipe::Sender::from_owned_fd(dup).ok()
}

/// stdin + stdout (each cancellable when a pipe) as one duplex stream. Every
/// half is `Unpin`, so the trait impls project via `get_mut`, not `pin_project`.
#[cfg(feature = "tungstenite")]
struct AutoStdio {
    stdin: StdinReader,
    stdout: StdoutWriter,
}

#[cfg(feature = "tungstenite")]
impl AutoStdio {
    fn new() -> Self {
        Self {
            stdin: StdinReader::new(),
            stdout: StdoutWriter::new(),
        }
    }
}

#[cfg(feature = "tungstenite")]
impl AsyncRead for AutoStdio {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> task::Poll<io::Result<()>> {
        match &mut self.get_mut().stdin {
            #[cfg(unix)]
            StdinReader::Pipe(rx) => Pin::new(rx).poll_read(cx, buf),
            StdinReader::Blocking(stdin) => Pin::new(stdin).poll_read(cx, buf),
        }
    }
}

#[cfg(feature = "tungstenite")]
impl AsyncWrite for AutoStdio {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> task::Poll<io::Result<usize>> {
        match &mut self.get_mut().stdout {
            #[cfg(unix)]
            StdoutWriter::Pipe(tx) => Pin::new(tx).poll_write(cx, buf),
            StdoutWriter::Blocking(stdout) => Pin::new(stdout).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<io::Result<()>> {
        match &mut self.get_mut().stdout {
            #[cfg(unix)]
            StdoutWriter::Pipe(tx) => Pin::new(tx).poll_flush(cx),
            StdoutWriter::Blocking(stdout) => Pin::new(stdout).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> task::Poll<io::Result<()>> {
        match &mut self.get_mut().stdout {
            #[cfg(unix)]
            StdoutWriter::Pipe(tx) => Pin::new(tx).poll_shutdown(cx),
            StdoutWriter::Blocking(stdout) => Pin::new(stdout).poll_shutdown(cx),
        }
    }
}

pin_project! {
    pub struct Stdio {
        #[pin]
        stdin: Stdin,
        #[pin]
        stdout: Stdout,
    }
}

impl Stdio {
    pub fn new(stdin: Stdin, stdout: Stdout) -> Self {
        Self { stdin, stdout }
    }
}

impl AsyncRead for Stdio {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> task::Poll<io::Result<()>> {
        let this = self.project();
        this.stdin.poll_read(cx, buf)
    }
}

impl AsyncWrite for Stdio {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> task::Poll<io::Result<usize>> {
        let this = self.project();
        this.stdout.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<io::Result<()>> {
        let this = self.project();
        this.stdout.poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> task::Poll<Result<(), io::Error>> {
        let this = self.project();
        this.stdout.poll_shutdown(cx)
    }
}
