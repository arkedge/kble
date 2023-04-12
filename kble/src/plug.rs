use std::{io, pin::Pin, process::Stdio, task};

use anyhow::{anyhow, ensure, Context, Result};
use futures::{future, stream, Sink, SinkExt, Stream, StreamExt, TryStreamExt};
use pin_project::pin_project;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    process::{ChildStdin, ChildStdout},
};
use tokio_tungstenite::{
    tungstenite::{protocol::Role, Message},
    WebSocketStream,
};
use url::Url;

pub type PlugSink = Pin<Box<dyn Sink<Vec<u8>, Error = anyhow::Error> + Send + 'static>>;
pub type PlugStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>>> + Send + 'static>>;

pub async fn connect(url: &Url) -> Result<(PlugSink, PlugStream)> {
    match url.scheme() {
        "exec" => connect_exec(url).await,
        "ws" | "wss" => connect_ws(url).await,
        _ => Err(anyhow!("Unsupported scheme: {}", url.scheme())),
    }
}

async fn connect_exec(url: &Url) -> Result<(PlugSink, PlugStream)> {
    assert_eq!(url.scheme(), "exec");
    ensure!(url.username().is_empty());
    ensure!(url.password().is_none());
    ensure!(url.host().is_none());
    ensure!(url.port().is_none());
    ensure!(url.query().is_none());
    ensure!(url.fragment().is_none());
    let proc = tokio::process::Command::new("sh")
        .args(["-c", url.path()])
        .stderr(Stdio::inherit())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to spawn {}", url))?;
    let stdin = proc.stdin.unwrap();
    let stdout = proc.stdout.unwrap();
    let stdio = ChildStdio { stdin, stdout };
    let wss = WebSocketStream::from_raw_socket(stdio, Role::Client, None).await;
    Ok(wss_to_pair(wss))
}

#[pin_project]
struct ChildStdio {
    #[pin]
    stdin: ChildStdin,
    #[pin]
    stdout: ChildStdout,
}

impl AsyncWrite for ChildStdio {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> task::Poll<Result<usize, io::Error>> {
        let this = self.project();
        this.stdin.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<io::Result<()>> {
        let this = self.project();
        this.stdin.poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> task::Poll<io::Result<()>> {
        let this = self.project();
        this.stdin.poll_shutdown(cx)
    }
}

impl AsyncRead for ChildStdio {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> task::Poll<io::Result<()>> {
        let this = self.project();
        this.stdout.poll_read(cx, buf)
    }
}

async fn connect_ws(url: &Url) -> Result<(PlugSink, PlugStream)> {
    let (wss, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .with_context(|| format!("Failed to connect to {}", url))?;
    Ok(wss_to_pair(wss))
}

fn wss_to_pair<S>(wss: WebSocketStream<S>) -> (PlugSink, PlugStream)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (sink, stream) = wss.split();
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
