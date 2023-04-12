use std::{io, pin::Pin, task};

use anyhow::Result;
use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, AsyncWrite, Stdin, Stdout};

use crate::{SocketSink, SocketStream};

#[cfg(feature = "tungstenite")]
pub async fn from_stdio() -> (SocketSink, SocketStream) {
    use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};
    let stdio = Stdio::new(tokio::io::stdin(), tokio::io::stdout());
    let wss = WebSocketStream::from_raw_socket(stdio, Role::Server, None).await;
    crate::from_tungstenite(wss)
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
